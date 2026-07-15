// ADR-007 §VI.2 streaming prototype (cs2-analytics): Rust port of
// packages/replay-codec-core/src/replay-codec-core.ts's encodeReplayChunkBody, so a round's
// RoundFlushChunk can be encoded to the SAME pre-zstd byte layout directly in Rust instead of
// materializing it as JSON. zstd compression itself is NOT done here (no C toolchain available
// on this dev machine to build zstd-sys) -- the raw bytes this produces are handed to Node's
// existing `zstdCompress()` (packages/shared/src/replay-codec.ts, built on node:zlib, already
// designed to accept pre-built raw bytes for exactly this "encode elsewhere, compress here"
// case) so the final on-disk blob format is completely unchanged from production.
//
// Byte layout MUST match replay-codec-core.ts exactly (see its own header comment): this is
// what makes the parity test meaningful. Field-by-field mapping mirrors compute.ts's
// toTickRow(); dtype/order mirrors replay-codec-core.ts's COLUMNS table.

use crate::first_pass::prop_controller::PropInfo;
use crate::second_pass::parser_settings::RoundFlushChunk;
use crate::second_pass::variants::{PropColumn, Variant, VarVec};
use crate::zstd_codec::compress;
use ahash::HashMap;

const NULL_U8: u8 = 255;
const NULL_U16: u16 = 65535;
const NULL_I32: i32 = i32::MIN;
const ANGLE_SCALE: f64 = 182.0;

struct Dict {
  list: Vec<String>,
  map: HashMap<String, u32>,
}
impl Dict {
  fn new() -> Self {
    Dict { list: vec![], map: HashMap::default() }
  }
  // null-aware (None -> id 0), matches buildDict() in replay-codec-core.ts: "" is a real value,
  // gets its own id, only None (missing prop) maps to the null sentinel.
  fn id_of(&mut self, s: Option<&str>) -> u32 {
    match s {
      None => 0,
      Some(s) => {
        if let Some(&id) = self.map.get(s) {
          return id;
        }
        self.list.push(s.to_string());
        let id = self.list.len() as u32; // 1-based
        self.map.insert(s.to_string(), id);
        id
      }
    }
  }
}

fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
  if v < lo { lo } else if v > hi { hi } else { v }
}
// JS Math.round semantics (round half toward +∞) -- Rust f64::round() rounds half AWAY from zero,
// which diverges from replay-codec-core.ts at exact .5 values (esp. negative velocities). ALL
// quantization must use this to stay byte-identical with the TS encoder.
fn js_round(x: f64) -> f64 {
  (x + 0.5).floor()
}
fn norm_angle(a: f64) -> f64 {
  (((a % 360.0) + 540.0) % 360.0) - 180.0
}
fn enc_angle(a: f64) -> u16 {
  clamp(js_round((norm_angle(a) + 180.0) * ANGLE_SCALE), 0.0, 360.0 * ANGLE_SCALE) as u16
}

pub fn build_name_to_id(prop_infos: &[PropInfo]) -> HashMap<String, u32> {
  let mut m = HashMap::default();
  for p in prop_infos {
    m.insert(p.prop_friendly_name.clone(), p.id);
  }
  m
}

fn get_variant(output: &ahash::AHashMap<u32, PropColumn>, name_to_id: &HashMap<String, u32>, name: &str, row: usize) -> Option<Variant> {
  let id = *name_to_id.get(name)?;
  let col = output.get(&id)?;
  match &col.data {
    None => None,
    Some(VarVec::F32(v)) => v.get(row).copied().flatten().map(Variant::F32),
    Some(VarVec::I32(v)) => v.get(row).copied().flatten().map(Variant::I32),
    Some(VarVec::U32(v)) => v.get(row).copied().flatten().map(Variant::U32),
    Some(VarVec::U64(v)) => v.get(row).copied().flatten().map(Variant::U64),
    Some(VarVec::Bool(v)) => v.get(row).copied().flatten().map(Variant::Bool),
    Some(VarVec::String(v)) => v.get(row).and_then(|x| x.clone()).map(Variant::String),
    Some(VarVec::StringVec(v)) => v.get(row).map(|x| Variant::StringVec(x.clone())),
    _ => None,
  }
}
fn as_f64(v: &Option<Variant>) -> Option<f64> {
  match v {
    Some(Variant::F32(x)) => Some(*x as f64),
    Some(Variant::I32(x)) => Some(*x as f64),
    Some(Variant::U32(x)) => Some(*x as f64),
    Some(Variant::U64(x)) => Some(*x as f64),
    Some(Variant::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
    _ => None,
  }
}
fn as_bool(v: &Option<Variant>) -> bool {
  match v {
    Some(Variant::Bool(b)) => *b,
    Some(Variant::F32(x)) => *x != 0.0,
    Some(Variant::I32(x)) => *x != 0,
    Some(Variant::U32(x)) => *x != 0,
    _ => false,
  }
}
fn as_str(v: &Option<Variant>) -> Option<String> {
  match v {
    Some(Variant::String(s)) => Some(s.clone()),
    _ => None,
  }
}
fn as_u64(v: &Option<Variant>) -> Option<u64> {
  match v {
    Some(Variant::U64(x)) => Some(*x),
    _ => None,
  }
}
fn as_str_vec(v: &Option<Variant>) -> Vec<String> {
  match v {
    Some(Variant::StringVec(s)) => s.clone(),
    _ => vec![],
  }
}

struct Row {
  steamid_idx: u32,
  tick: i32,
  x: f64,
  y: f64,
  z: f64,
  yaw: f64,
  pitch: Option<f64>,
  is_alive: bool,
  side: u8,
  health: Option<f64>,
  armor: Option<f64>,
  weapon_id: u32,
  ammo: Option<f64>,
  money: Option<f64>,
  equip: Option<f64>,
  has_helmet: bool,
  has_defuser: bool,
  place_id: u32,
  flash: Option<f64>,
  is_defusing: bool,
  is_scoped: bool,
  vel_x: Option<f64>,
  vel_y: Option<f64>,
  vel_z: Option<f64>,
  duck: Option<f64>,
  is_walking: bool,
  inv: Vec<String>,
}

fn push_u8(out: &mut Vec<u8>, v: u8) {
  out.push(v);
}
fn push_u16(out: &mut Vec<u8>, v: u16) {
  out.extend_from_slice(&v.to_le_bytes());
}
fn push_i32(out: &mut Vec<u8>, v: i32) {
  out.extend_from_slice(&v.to_le_bytes());
}

/// Encode one round's decoded ticks -> raw bytes (pre-zstd), byte-layout-identical to
/// replay-codec-core.ts's encodeReplayChunkBody. Empty chunk -> empty Vec (matches TS).
pub fn encode_round_tick_body(chunk: &RoundFlushChunk, name_to_id: &HashMap<String, u32>) -> Vec<u8> {
  let tick_id = match name_to_id.get("tick") {
    Some(id) => *id,
    None => return vec![],
  };
  let r = match chunk.output.get(&tick_id) {
    Some(c) => c.len(),
    None => 0,
  };
  if r == 0 {
    return vec![];
  }
  let indices: Vec<usize> = (0..r).collect();
  encode_rows(&chunk.output, name_to_id, &indices).0
}

/// Encode a SUBSET of rows (given by `indices`) from a columnar tick `output` -> (raw bytes
/// pre-zstd, distinct player count). Byte-layout-identical to replay-codec-core.ts's
/// encodeReplayChunkBody; offsets/dicts computed from ONLY the given rows (per-chunk, matches TS).
/// Shared core behind both the streaming per-round path (encode_round_tick_body) and the bulk
/// sampled path (build_replay_chunks).
pub fn encode_rows(
  output: &ahash::AHashMap<u32, PropColumn>,
  name_to_id: &HashMap<String, u32>,
  indices: &[usize],
) -> (Vec<u8>, usize) {
  let r = indices.len();
  if r == 0 {
    return (vec![], 0);
  }

  let mut steam_list: Vec<String> = vec![];
  let mut steam_index: HashMap<String, u32> = HashMap::default();
  let mut weapon = Dict::new();
  let mut place = Dict::new();
  let mut item = Dict::new();

  let mut rows: Vec<Row> = Vec::with_capacity(r);
  let mut min_x = f64::INFINITY;
  let mut min_y = f64::INFINITY;
  let mut min_z = f64::INFINITY;
  let mut tick_start = i32::MAX;
  let mut tick_end = i32::MIN;

  for &i in indices {
    let g = |name: &str| get_variant(output, name_to_id, name, i);

    // steamId: matches `g('steamid') || ''` -- U64 serializes as a JS STRING (not number), so
    // only true absence (None) falls back to '' -- a legit steamid of 0 stays "0", not "".
    let steamid_str = match as_u64(&g("steamid")) {
      None => String::new(),
      Some(u) => u.to_string(),
    };
    let steamid_idx = match steam_index.get(&steamid_str) {
      Some(&id) => id,
      None => {
        let id = steam_list.len() as u32;
        steam_list.push(steamid_str.clone());
        steam_index.insert(steamid_str, id);
        id
      }
    };

    let tick = as_f64(&g("tick")).unwrap_or(0.0) as i32;
    if tick < tick_start { tick_start = tick; }
    if tick > tick_end { tick_end = tick; }

    let x = as_f64(&g("X")).unwrap_or(0.0);
    let y = as_f64(&g("Y")).unwrap_or(0.0);
    let z = as_f64(&g("Z")).unwrap_or(0.0);
    if x < min_x { min_x = x; }
    if y < min_y { min_y = y; }
    if z < min_z { min_z = z; }

    let yaw = as_f64(&g("yaw")).unwrap_or(0.0);
    let pitch = as_f64(&g("pitch"));
    let health = as_f64(&g("health"));
    let is_alive = match g("is_alive") {
      Some(Variant::Bool(false)) => false,
      _ => health.unwrap_or(1.0) > 0.0,
    };
    let team_num = as_f64(&g("team_num"));
    let side = match team_num {
      Some(t) if t == 2.0 => 1u8,
      Some(t) if t == 3.0 => 2u8,
      _ => 0u8,
    };
    let armor = as_f64(&g("armor_value"));
    let weapon_name = as_str(&g("active_weapon_name"));
    let weapon_id = weapon.id_of(weapon_name.as_deref());
    let ammo = as_f64(&g("active_weapon_ammo"));
    let money = as_f64(&g("balance"));
    let equip = as_f64(&g("current_equip_value"));
    let has_helmet = as_bool(&g("has_helmet"));
    let has_defuser = as_bool(&g("has_defuser"));
    let place_name = as_str(&g("last_place_name"));
    let place_id = place.id_of(place_name.as_deref());
    let flash = as_f64(&g("flash_duration"));
    let is_defusing = as_bool(&g("is_defusing"));
    let is_scoped = as_bool(&g("is_scoped"));
    let vel_x = as_f64(&g("velocity_X"));
    let vel_y = as_f64(&g("velocity_Y"));
    let vel_z = as_f64(&g("velocity_Z"));
    let duck = as_f64(&g("duck_amount"));
    let is_walking = as_bool(&g("is_walking"));
    let inv_names = as_str_vec(&g("inventory"));
    let inv: Vec<String> = inv_names.into_iter().take(255).map(|n| {
      item.id_of(Some(&n));
      n
    }).collect();

    rows.push(Row {
      steamid_idx, tick, x, y, z, yaw, pitch, is_alive, side, health, armor, weapon_id,
      ammo, money, equip, has_helmet, has_defuser, place_id, flash, is_defusing, is_scoped,
      vel_x, vel_y, vel_z, duck, is_walking, inv,
    });
  }

  let off_x = min_x.floor();
  let off_y = min_y.floor();
  let off_z = min_z.floor();

  // ---- pack columns, exact COLUMNS order/dtype from replay-codec-core.ts ----
  let mut steam_id_idx_col = Vec::with_capacity(r);
  let mut tick_col = Vec::with_capacity(r * 4);
  let mut x_col = Vec::with_capacity(r * 2);
  let mut y_col = Vec::with_capacity(r * 2);
  let mut z_col = Vec::with_capacity(r * 2);
  let mut yaw_col = Vec::with_capacity(r * 2);
  let mut pitch_col = Vec::with_capacity(r * 2);
  let mut flags_col = Vec::with_capacity(r);
  let mut side_col = Vec::with_capacity(r);
  let mut health_col = Vec::with_capacity(r);
  let mut armor_col = Vec::with_capacity(r);
  let mut weapon_id_col = Vec::with_capacity(r * 2);
  let mut ammo_col = Vec::with_capacity(r * 2);
  let mut money_col = Vec::with_capacity(r * 2);
  let mut equip_col = Vec::with_capacity(r * 2);
  let mut place_id_col = Vec::with_capacity(r * 2);
  let mut flash_col = Vec::with_capacity(r * 2);
  let mut vel_x_col = Vec::with_capacity(r * 4);
  let mut vel_y_col = Vec::with_capacity(r * 4);
  let mut vel_z_col = Vec::with_capacity(r * 4);
  let mut duck_col = Vec::with_capacity(r * 2);
  let mut inv_count_col = Vec::with_capacity(r);
  let mut inv_items: Vec<u16> = vec![];

  for row in &rows {
    push_u8(&mut steam_id_idx_col, row.steamid_idx as u8);
    push_i32(&mut tick_col, row.tick);

    let qx = js_round(row.x - off_x);
    let qy = js_round(row.y - off_y);
    let qz = js_round(row.z - off_z);
    push_u16(&mut x_col, qx as u16);
    push_u16(&mut y_col, qy as u16);
    push_u16(&mut z_col, qz as u16);

    push_u16(&mut yaw_col, enc_angle(row.yaw));
    push_u16(&mut pitch_col, match row.pitch { None => NULL_U16, Some(p) => enc_angle(p) });

    let flags = (row.is_alive as u8)
      | ((row.has_helmet as u8) << 1)
      | ((row.has_defuser as u8) << 2)
      | ((row.is_defusing as u8) << 3)
      | ((row.is_scoped as u8) << 4)
      | ((row.is_walking as u8) << 5);
    push_u8(&mut flags_col, flags);
    push_u8(&mut side_col, row.side);
    push_u8(&mut health_col, match row.health { None => NULL_U8, Some(h) => clamp(js_round(h), 0.0, 254.0) as u8 });
    push_u8(&mut armor_col, match row.armor { None => NULL_U8, Some(a) => clamp(js_round(a), 0.0, 254.0) as u8 });
    push_u16(&mut weapon_id_col, row.weapon_id as u16);
    push_u16(&mut ammo_col, match row.ammo { None => NULL_U16, Some(a) => clamp(js_round(a), 0.0, 65534.0) as u16 });
    push_u16(&mut money_col, match row.money { None => NULL_U16, Some(m) => clamp(js_round(m), 0.0, 65534.0) as u16 });
    push_u16(&mut equip_col, match row.equip { None => NULL_U16, Some(e) => clamp(js_round(e), 0.0, 65534.0) as u16 });
    push_u16(&mut place_id_col, row.place_id as u16);
    push_u16(&mut flash_col, match row.flash { None => NULL_U16, Some(f) => clamp(js_round(f * 1000.0), 0.0, 65534.0) as u16 });
    push_i32(&mut vel_x_col, match row.vel_x { None => NULL_I32, Some(v) => clamp(js_round(v), -2147483647.0, 2147483647.0) as i32 });
    push_i32(&mut vel_y_col, match row.vel_y { None => NULL_I32, Some(v) => clamp(js_round(v), -2147483647.0, 2147483647.0) as i32 });
    push_i32(&mut vel_z_col, match row.vel_z { None => NULL_I32, Some(v) => clamp(js_round(v), -2147483647.0, 2147483647.0) as i32 });
    push_u16(&mut duck_col, match row.duck { None => NULL_U16, Some(d) => clamp(js_round(d * 1000.0), 0.0, 65534.0) as u16 });

    let inv_n = row.inv.len().min(255);
    push_u8(&mut inv_count_col, inv_n as u8);
    for k in 0..inv_n {
      inv_items.push(item.id_of(Some(&row.inv[k])) as u16);
    }
  }

  // ---- header JSON. Field ORDER must match replay-codec-core.ts's `header` object literal exactly
  // for byte-identity (serde_json::json! uses a BTreeMap → alphabetical, which would diverge). A
  // derived struct serializes fields in DECLARATION order. off* emitted as integers (floor()'d, so
  // whole) to match JS JSON.stringify(-640) vs serde's f64 "-640.0". ----
  #[derive(serde::Serialize)]
  struct ChunkHeader {
    r: usize,
    #[serde(rename = "sampleStep")]
    sample_step: u32,
    #[serde(rename = "tickStart")]
    tick_start: i32,
    #[serde(rename = "tickEnd")]
    tick_end: i32,
    #[serde(rename = "offX")]
    off_x: i64,
    #[serde(rename = "offY")]
    off_y: i64,
    #[serde(rename = "offZ")]
    off_z: i64,
    #[serde(rename = "steamIds")]
    steam_ids: Vec<String>,
    weapons: Vec<String>,
    places: Vec<String>,
    items: Vec<String>,
  }
  let player_count = steam_list.len();
  let header = ChunkHeader {
    r,
    sample_step: 8,
    tick_start: if tick_start == i32::MAX { 0 } else { tick_start },
    tick_end: if tick_end == i32::MIN { 0 } else { tick_end },
    off_x: off_x as i64,
    off_y: off_y as i64,
    off_z: off_z as i64,
    steam_ids: steam_list,
    weapons: weapon.list,
    places: place.list,
    items: item.list,
  };
  let header_json = serde_json::to_vec(&header).unwrap_or_default();

  let mut out = Vec::with_capacity(4 + 1 + 4 + header_json.len() + r * 30);
  out.extend_from_slice(b"RPTK");
  out.push(1u8); // REPLAY_FORMAT_ZSTD_V1
  out.extend_from_slice(&(header_json.len() as u32).to_le_bytes());
  out.extend_from_slice(&header_json);

  // COLUMNS order MUST match replay-codec-core.ts exactly.
  out.extend_from_slice(&steam_id_idx_col);
  out.extend_from_slice(&tick_col);
  out.extend_from_slice(&x_col);
  out.extend_from_slice(&y_col);
  out.extend_from_slice(&z_col);
  out.extend_from_slice(&yaw_col);
  out.extend_from_slice(&pitch_col);
  out.extend_from_slice(&flags_col);
  out.extend_from_slice(&side_col);
  out.extend_from_slice(&health_col);
  out.extend_from_slice(&armor_col);
  out.extend_from_slice(&weapon_id_col);
  out.extend_from_slice(&ammo_col);
  out.extend_from_slice(&money_col);
  out.extend_from_slice(&equip_col);
  out.extend_from_slice(&place_id_col);
  out.extend_from_slice(&flash_col);
  out.extend_from_slice(&vel_x_col);
  out.extend_from_slice(&vel_y_col);
  out.extend_from_slice(&vel_z_col);
  out.extend_from_slice(&duck_col);
  out.extend_from_slice(&inv_count_col);
  for v in &inv_items {
    out.extend_from_slice(&v.to_le_bytes());
  }

  (out, player_count)
}

// ── Bulk sampled replay chunks (ADR-007 Giai đoạn 3) ───────────────────────────
/// One round's encoded + zstd-compressed replay tick blob + metadata, matching
/// packages/parse-core buildReplayChunks() output (ParsedReplayChunk).
pub struct ReplayChunkParsed {
  pub round_number: i64,
  pub tick_start: i64,
  pub tick_end: i64,
  pub sample_step: i64,
  pub player_count: i64,
  pub data: Vec<u8>,
}

/// Port of compute.ts buildReplayChunks(tickRows, rounds): group the bulk SAMPLED tick columns by
/// round (binary search on sorted [startTick,endTick]) then encode + zstd each round. `rounds` is
/// (round_number, start_tick, end_tick) in the ORIGINAL rounds order (output preserves that order).
/// Empty rounds are skipped (matches TS). Per-round dicts/offsets fall out of encode_rows.
pub fn build_replay_chunks(
  df: &ahash::AHashMap<u32, PropColumn>,
  prop_infos: &[PropInfo],
  rounds: &[(i64, i64, i64)],
  sample_step: i64,
  zstd_level: i32,
) -> std::io::Result<Vec<ReplayChunkParsed>> {
  let name_to_id = build_name_to_id(prop_infos);
  let tick_id = match name_to_id.get("tick") {
    Some(id) => *id,
    None => return Ok(vec![]),
  };
  let r = df.get(&tick_id).map(|c| c.len()).unwrap_or(0);
  if r == 0 || rounds.is_empty() {
    return Ok(vec![]);
  }

  // sorted ranges (by startTick) for binary search, same as TS buildReplayChunks.
  let mut ranges: Vec<(i64, i64, i64)> = rounds.to_vec();
  ranges.sort_by_key(|x| x.1);

  let mut by_round: HashMap<i64, Vec<usize>> = HashMap::default();
  for &(n, _, _) in &ranges {
    by_round.insert(n, Vec::new());
  }

  for i in 0..r {
    let t = as_f64(&get_variant(df, &name_to_id, "tick", i)).unwrap_or(0.0) as i64;
    // largest idx with ranges[idx].start <= t
    let (mut lo, mut hi, mut found): (isize, isize, isize) = (0, ranges.len() as isize - 1, -1);
    while lo <= hi {
      let mid = ((lo + hi) / 2) as usize;
      if ranges[mid].1 <= t {
        found = mid as isize;
        lo = mid as isize + 1;
      } else {
        hi = mid as isize - 1;
      }
    }
    if found >= 0 && t <= ranges[found as usize].2 {
      if let Some(v) = by_round.get_mut(&ranges[found as usize].0) {
        v.push(i);
      }
    }
  }

  let mut out = Vec::new();
  for &(n, s, e) in rounds {
    let indices = match by_round.get(&n) {
      Some(v) if !v.is_empty() => v,
      _ => continue,
    };
    let (bytes, player_count) = encode_rows(df, &name_to_id, indices);
    let compressed = compress(&bytes, zstd_level)?;
    out.push(ReplayChunkParsed {
      round_number: n,
      tick_start: s,
      tick_end: e,
      sample_step,
      player_count: player_count as i64,
      data: compressed,
    });
  }
  Ok(out)
}
