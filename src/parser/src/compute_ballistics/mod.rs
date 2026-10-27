// Port của packages/parse-core/src/ballistics.ts -- ĐẠN ĐẠO theo từng viên + tổng hợp theo (round,
// player) cho tab "Đạn đạo". Khác bản TS (tự gọi parseEvents/parseTicks riêng), bản Rust ăn theo các
// lượt parse SẴN CÓ của full_pipeline: weapon_fire/player_hurt/fire_bullets nằm trong lượt event gộp
// (player prop đạn đạo thêm vào all_event_player_fields), vị trí địch tại tick bắn nằm trong lượt tick
// gộp (B1) -- caller gọi `shot_ticks` TRƯỚC lượt tick để gộp tick bắn vào union_ticks.
//
// Giới hạn (giữ như TS): không có hình học map → "mục tiêu" của viên trượt = địch còn sống gần tâm
// nhất, KHÔNG kiểm tra tường che. Góc lệch là xấp xỉ "tâm cách địch bao xa".
//
// Làm tròn: Math.round của JS ≠ f64::round ở số âm .5 → dùng js_round (floor(x + 0.5)).

use crate::compute_events::{hitgroup_to_int, is_gun_weapon, norm_weapon_name};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

// v2: "viên đầu"/"spray" chỉ tính súng tự động (is_auto_weapon) -- v1 gộp cả AWP/súng lục/shotgun.
pub const BALLISTICS_VERSION: i64 = 2;
const HIT_MAX_GAP_TICKS: i64 = 2;
const BURST_GAP_SEC: f64 = 0.5;
const TARGET_MAX_DEG: f64 = 20.0;
const SPRAY_FROM_IDX: i64 = 3;
const HEAD_Z: f64 = 64.0;
const CHEST_Z: f64 = 50.0;
const DUCK_DROP: f64 = 18.0;
const ACCURACY_SPEED_FACTOR: f64 = 0.34;
const DEFAULT_GUN_MAX_SPEED: f64 = 240.0;

/// Player prop cần thêm vào lượt event gộp (ngoài các prop đã có: X/Y/Z/pitch/yaw/team_num/velocity).
pub fn extra_event_player_fields() -> &'static [&'static str] {
  &["shots_fired", "duck_amount", "is_scoped", "is_airborne"]
}
/// Field tick tại đúng tick bắn -- đều đã nằm trong sampled_tick_fields nên không thêm cột nào.
pub fn tick_fields() -> &'static [&'static str] {
  &["X", "Y", "Z", "is_alive", "team_num", "duck_amount"]
}

// Tốc độ tối đa khi cầm súng (u/s) -- bắn ở > ACCURACY_SPEED_FACTOR × giá trị này = "bắn khi chạy".
fn weapon_max_speed(w: &str) -> f64 {
  match w {
    "ak47" | "galilar" | "galil" | "xm1014" => 215.0,
    "m4a4" | "m4a1" | "m4a1s" | "m4a1silencer" | "mag7" => 225.0,
    "famas" | "aug" | "mp7" | "nova" | "revolver" => 220.0,
    "sg553" | "sg556" | "sawedoff" => 210.0,
    "deagle" | "ump45" | "p90" | "ssg08" => 230.0,
    "mp5sd" => 235.0,
    "negev" => 150.0,
    "m249" => 195.0,
    "awp" => 200.0,
    "scar20" | "g3sg1" => 215.0,
    _ => DEFAULT_GUN_MAX_SPEED,
  }
}
fn scoped_max_speed(w: &str) -> Option<f64> {
  match w {
    "awp" => Some(100.0),
    "ssg08" => Some(230.0),
    "scar20" | "g3sg1" => Some(120.0),
    _ => None,
  }
}
fn max_speed_of(w: &str, scoped: bool) -> f64 {
  if scoped {
    if let Some(s) = scoped_max_speed(w) {
      return s;
    }
  }
  weapon_max_speed(w)
}

// Súng xả loạt khi giữ cò (rifle, SMG, máy, auto-sniper, CZ75) -- chỉ nhóm này có nghĩa "viên đầu" vs "spray".
// AWP/SSG/súng lục/shotgun bắn từng phát nên KHÔNG cộng vào first_*/spray_*. Tên đã qua norm_weapon_name.
// Đồng bộ với weaponClass() ở apps/web/src/shared/utils/weaponIcon.ts.
fn is_auto_weapon(w: &str) -> bool {
  matches!(
    w,
    "ak47" | "aug" | "famas" | "galilar" | "galil" | "m4a4" | "m4a1" | "m4a1s" | "m4a1silencer" | "sg553" | "sg556"
      | "g3sg1" | "scar20" | "mac10" | "mp9" | "mp7" | "mp5sd" | "ump45" | "ump" | "p90" | "bizon" | "ppbizon"
      | "m249" | "negev" | "cz75a" | "cz75auto"
  )
}

fn js_round(x: f64) -> f64 {
  (x + 0.5).floor()
}
fn r2(v: f64) -> f64 {
  js_round(v * 100.0) / 100.0
}
fn r4(v: f64) -> f64 {
  js_round(v * 10000.0) / 10000.0
}
fn side_of(team: Option<i64>) -> Option<String> {
  match team {
    Some(2) => Some("T".into()),
    Some(3) => Some("CT".into()),
    _ => None,
  }
}
fn num(v: &Value, k: &str) -> Option<f64> {
  v.get(k).and_then(|x| x.as_f64())
}
fn int(v: &Value, k: &str) -> Option<i64> {
  v.get(k).and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}
// steamid có thể là chuỗi hoặc số tuỳ đường serialize -- JS dùng String(x).
fn sid(v: &Value, k: &str) -> Option<String> {
  match v.get(k) {
    Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
    Some(Value::Number(n)) => Some(n.to_string()),
    _ => None,
  }
}

// Quy ước Source: forward = (cos p·cos y, cos p·sin y, −sin p), góc theo độ. None nếu điểm trùng mắt.
fn angle_to(pitch_deg: f64, yaw_deg: f64, from: [f64; 3], to: [f64; 3]) -> Option<f64> {
  let p = pitch_deg.to_radians();
  let y = yaw_deg.to_radians();
  let (fx, fy, fz) = (p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
  let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
  let len = (dx * dx + dy * dy + dz * dz).sqrt();
  if len < 1.0 {
    return None;
  }
  let dot = (fx * dx + fy * dy + fz * dz) / len;
  Some(dot.clamp(-1.0, 1.0).acos().to_degrees())
}

#[derive(Debug, Clone, Serialize)]
pub struct ShotHit {
  pub victim: String,
  pub hg: i64,
  pub dmg: f64,
}

/// 1 viên -- cùng shape ShotRow (types.ts) để web giải nén blob như cũ.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotRow {
  pub round: i64,
  pub steam_id: String,
  pub side: Option<String>,
  pub weapon: String,
  pub idx: i64,
  pub burst: i64,
  pub eye: [f64; 3],
  pub ang: [f64; 2],
  pub view: [f64; 2],
  pub inacc: Option<f64>,
  pub speed: Option<i64>,
  pub moving: bool,
  pub duck: bool,
  pub scoped: bool,
  pub air: bool,
  pub hits: Vec<ShotHit>,
  pub target: Option<String>,
  pub err: Option<f64>,
  pub dist: Option<i64>,
  pub kill: bool,
}

struct Draft {
  tick: i64,
  team: Option<i64>,
  idx_raw: Option<i64>,
  row: ShotRow,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShotWeapon {
  pub w: String,
  pub shots: i64,
  pub hits: i64,
  pub hs: i64,
  pub dmg: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundPlayerBallistics {
  pub round_number: i64,
  pub steam_id: String,
  pub side: Option<String>,
  pub shots: i64,
  pub hits: i64,
  pub head_hits: i64,
  pub first_shots: i64,
  pub first_hits: i64,
  pub spray_shots: i64,
  pub spray_hits: i64,
  pub moving_shots: i64,
  pub air_shots: i64,
  pub kill_shots: i64,
  pub err_sum: f64,
  pub err_n: i64,
  pub inacc_sum: f64,
  pub inacc_n: i64,
  pub shot_weapons: Vec<ShotWeapon>,
  pub shot_hitgroups: BTreeMap<i64, i64>,
}

pub struct BallisticsOut {
  pub version: i64,
  pub shot_count: i64,
  /// JSON body (codec replay-event, `{tick,type:'SHOT',data}[]`) đã nén zstd.
  pub data: Vec<u8>,
  pub round_player: Vec<RoundPlayerBallistics>,
}

fn is_gun_event(ev: &Value) -> bool {
  ev.get("weapon").and_then(|w| w.as_str()).map_or(false, is_gun_weapon)
}
fn round_of(ev: &Value) -> i64 {
  int(ev, "total_rounds_played").unwrap_or(0) + 1
}

/// Tick của mọi viên hợp lệ -- caller gộp vào union_ticks của lượt tick để có vị trí địch.
pub fn shot_ticks(raw_events: &[Value], valid_rounds: &HashSet<i64>) -> HashSet<i32> {
  raw_events
    .iter()
    .filter(|ev| ev.get("event_name").and_then(|v| v.as_str()) == Some("weapon_fire"))
    .filter(|ev| is_gun_event(ev) && sid(ev, "user_steamid").is_some() && valid_rounds.contains(&round_of(ev)))
    .filter_map(|ev| int(ev, "tick").map(|t| t as i32))
    .collect()
}

/// `raw_events`: output lượt event gộp (gồm weapon_fire/player_hurt/fire_bullets). `tick_rows`: AoS
/// các field `tick_fields()` tại đúng tick bắn.
pub fn compute_ballistics(raw_events: &[Value], valid_rounds: &HashSet<i64>, tickrate: i64, tick_rows: &[Value], zstd_level: i32) -> BallisticsOut {
  // ── fire_bullets theo (tick, steamid) → hàng đợi (1 weapon_fire ↔ 1 fire_bullets) ──
  let mut fb_by_key: HashMap<(i64, String), VecDeque<&Value>> = HashMap::new();
  for b in raw_events.iter().filter(|e| e.get("event_name").and_then(|v| v.as_str()) == Some("fire_bullets")) {
    if let (Some(t), Some(s)) = (int(b, "tick"), sid(b, "user_steamid")) {
      fb_by_key.entry((t, s)).or_default().push_back(b);
    }
  }

  // ── dựng danh sách viên ──
  let mut shots: Vec<Draft> = Vec::new();
  for ev in raw_events.iter().filter(|e| e.get("event_name").and_then(|v| v.as_str()) == Some("weapon_fire")) {
    let steam_id = match sid(ev, "user_steamid") {
      Some(s) if is_gun_event(ev) => s,
      _ => continue,
    };
    let round = round_of(ev);
    if !valid_rounds.contains(&round) {
      continue;
    }
    let tick = int(ev, "tick").unwrap_or(0);
    let weapon = norm_weapon_name(&ev.get("weapon").and_then(|w| w.as_str()).map(String::from));
    let duck_amt = num(ev, "user_duck_amount").unwrap_or(0.0);
    let fb = fb_by_key.get_mut(&(tick, steam_id.clone())).and_then(|q| q.pop_front());
    let view = [r2(num(ev, "user_pitch").unwrap_or(0.0)), r2(num(ev, "user_yaw").unwrap_or(0.0))];
    let ang = match fb.and_then(|b| Some([num(b, "angles_x")?, num(b, "angles_y")?])) {
      Some([p, y]) => [r2(p), r2(y)],
      None => view,
    };
    let eye = match fb.and_then(|b| Some([num(b, "origin_x")?, num(b, "origin_y")?, num(b, "origin_z")?])) {
      Some([x, y, z]) => [r2(x), r2(y), r2(z)],
      None => [
        r2(num(ev, "user_X").unwrap_or(0.0)),
        r2(num(ev, "user_Y").unwrap_or(0.0)),
        r2(num(ev, "user_Z").unwrap_or(0.0) + HEAD_Z - duck_amt * DUCK_DROP),
      ],
    };
    let scoped = ev.get("user_is_scoped").and_then(|v| v.as_bool()) == Some(true);
    let speed = match (num(ev, "user_velocity_X"), num(ev, "user_velocity_Y")) {
      (Some(x), Some(y)) => Some((x * x + y * y).sqrt()),
      _ => None,
    };
    let team = int(ev, "user_team_num");
    shots.push(Draft {
      tick,
      team,
      idx_raw: int(ev, "user_shots_fired"),
      row: ShotRow {
        round,
        side: side_of(team),
        idx: 0,
        burst: 0,
        eye,
        ang,
        view,
        inacc: fb.and_then(|b| num(b, "inaccuracy")).map(r4),
        speed: speed.map(|s| js_round(s) as i64),
        moving: speed.map_or(false, |s| s > max_speed_of(&weapon, scoped) * ACCURACY_SPEED_FACTOR),
        duck: duck_amt > 0.5,
        scoped,
        air: ev.get("user_is_airborne").and_then(|v| v.as_bool()) == Some(true),
        hits: vec![],
        target: None,
        err: None,
        dist: None,
        kill: false,
        steam_id,
        weapon,
      },
    });
  }
  shots.sort_by_key(|s| s.tick);

  // ── số thứ tự viên + loạt: loạt mới khi idx về 0, đổi súng, sang round khác, hoặc nghỉ > BURST_GAP ──
  let burst_gap = js_round(BURST_GAP_SEC * tickrate as f64) as i64;
  let mut last_by_player: HashMap<String, usize> = HashMap::new();
  let mut burst_seq: HashMap<String, i64> = HashMap::new();
  for i in 0..shots.len() {
    let prev = last_by_player.get(&shots[i].row.steam_id).map(|&j| (shots[j].row.weapon.clone(), shots[j].row.round, shots[j].tick, shots[j].row.idx));
    let s = &mut shots[i];
    let same_run = prev.as_ref().map_or(false, |(w, r, t, _)| *w == s.row.weapon && *r == s.row.round && s.tick - t <= burst_gap);
    s.row.idx = s.idx_raw.unwrap_or(match (&prev, same_run) {
      (Some((_, _, _, pidx)), true) => pidx + 1,
      _ => 0,
    });
    let new_burst = !same_run || s.row.idx == 0;
    let seq = burst_seq.get(&s.row.steam_id).copied().unwrap_or(0) + i64::from(new_burst);
    burst_seq.insert(s.row.steam_id.clone(), seq);
    s.row.burst = seq;
    last_by_player.insert(s.row.steam_id.clone(), i);
  }

  // ── ghép HURT → viên gần nhất TRƯỚC/CÙNG tick của attacker, lệch ≤ HIT_MAX_GAP_TICKS ──
  // GOTCHA: KHÔNG so tên súng -- player_hurt ghi tên GỐC (usp_silencer → "hkp2000") khác weapon_fire.
  let mut idx_by_player: HashMap<String, Vec<usize>> = HashMap::new();
  for (i, s) in shots.iter().enumerate() {
    idx_by_player.entry(s.row.steam_id.clone()).or_default().push(i);
  }
  for ev in raw_events.iter().filter(|e| e.get("event_name").and_then(|v| v.as_str()) == Some("player_hurt")) {
    if !is_gun_event(ev) {
      continue;
    }
    let (Some(attacker), Some(victim)) = (sid(ev, "attacker_steamid"), sid(ev, "user_steamid")) else { continue };
    if attacker == victim {
      continue;
    }
    let att_team = int(ev, "attacker_team_num");
    if att_team.is_some() && att_team == int(ev, "user_team_num") {
      continue;
    }
    let Some(list) = idx_by_player.get(&attacker) else { continue };
    let tick = int(ev, "tick").unwrap_or(0);
    let pos = list.partition_point(|&i| shots[i].tick <= tick);
    if pos == 0 {
      continue;
    }
    let found = list[pos - 1];
    if tick - shots[found].tick > HIT_MAX_GAP_TICKS {
      continue;
    }
    shots[found].row.hits.push(ShotHit { victim, hg: hitgroup_to_int(&ev.get("hitgroup").cloned()), dmg: num(ev, "dmg_health").unwrap_or(0.0) });
    if int(ev, "health") == Some(0) {
      shots[found].row.kill = true;
    }
  }

  // ── vị trí địch tại tick bắn → mục tiêu + góc lệch ──
  struct Pos {
    steam_id: String,
    team: Option<i64>,
    alive: bool,
    p: [f64; 3],
    duck: f64,
  }
  let mut pos_by_tick: HashMap<i64, Vec<Pos>> = HashMap::new();
  for r in tick_rows {
    let (Some(x), Some(steam_id), Some(tick)) = (num(r, "X"), sid(r, "steamid"), int(r, "tick")) else { continue };
    pos_by_tick.entry(tick).or_default().push(Pos {
      steam_id,
      team: int(r, "team_num"),
      alive: r.get("is_alive").and_then(|v| v.as_bool()) != Some(false),
      p: [x, num(r, "Y").unwrap_or(0.0), num(r, "Z").unwrap_or(0.0)],
      duck: num(r, "duck_amount").unwrap_or(0.0),
    });
  }
  let err_to = |s: &ShotRow, p: &Pos| -> Option<f64> {
    let drop = p.duck * DUCK_DROP;
    let head = angle_to(s.ang[0], s.ang[1], s.eye, [p.p[0], p.p[1], p.p[2] + HEAD_Z - drop]);
    let chest = angle_to(s.ang[0], s.ang[1], s.eye, [p.p[0], p.p[1], p.p[2] + CHEST_Z - drop]);
    match (head, chest) {
      (None, None) => None,
      (h, c) => Some(h.unwrap_or(f64::INFINITY).min(c.unwrap_or(f64::INFINITY))),
    }
  };
  let empty: Vec<Pos> = vec![];
  for s in shots.iter_mut() {
    let players = pos_by_tick.get(&s.tick).unwrap_or(&empty);
    let mut best: Option<(&Pos, f64)> = None;
    if let Some(victim) = s.row.hits.first().map(|h| h.victim.clone()) {
      if let Some(p) = players.iter().find(|q| q.steam_id == victim) {
        if let Some(err) = err_to(&s.row, p) {
          best = Some((p, err));
        }
      }
      s.row.target = Some(victim);
    } else {
      for p in players {
        if !p.alive || p.steam_id == s.row.steam_id || (s.team.is_some() && p.team == s.team) {
          continue;
        }
        if let Some(err) = err_to(&s.row, p) {
          if err <= TARGET_MAX_DEG && best.map_or(true, |(_, b)| err < b) {
            best = Some((p, err));
          }
        }
      }
      s.row.target = best.map(|(p, _)| p.steam_id.clone());
    }
    if let Some((p, err)) = best {
      s.row.err = Some(r2(err));
      let (dx, dy, dz) = (p.p[0] - s.row.eye[0], p.p[1] - s.row.eye[1], p.p[2] + CHEST_Z - s.row.eye[2]);
      s.row.dist = Some(js_round((dx * dx + dy * dy + dz * dz).sqrt()) as i64);
    }
  }

  // ── tổng hợp theo (round, player), giữ thứ tự chèn như JS Map ──
  let mut order: Vec<(i64, String)> = Vec::new();
  let mut agg: HashMap<(i64, String), RoundPlayerBallistics> = HashMap::new();
  for s in &shots {
    let r = &s.row;
    let key = (r.round, r.steam_id.clone());
    let a = agg.entry(key.clone()).or_insert_with(|| {
      order.push(key.clone());
      RoundPlayerBallistics {
        round_number: r.round,
        steam_id: r.steam_id.clone(),
        side: r.side.clone(),
        shots: 0,
        hits: 0,
        head_hits: 0,
        first_shots: 0,
        first_hits: 0,
        spray_shots: 0,
        spray_hits: 0,
        moving_shots: 0,
        air_shots: 0,
        kill_shots: 0,
        err_sum: 0.0,
        err_n: 0,
        inacc_sum: 0.0,
        inacc_n: 0,
        shot_weapons: vec![],
        shot_hitgroups: BTreeMap::new(),
      }
    });
    let hit = !r.hits.is_empty();
    let head = r.hits.iter().any(|h| h.hg == 1);
    a.shots += 1;
    a.hits += i64::from(hit);
    a.head_hits += i64::from(head);
    let auto = is_auto_weapon(&r.weapon);
    if auto && r.idx == 0 {
      a.first_shots += 1;
      a.first_hits += i64::from(hit);
    }
    if auto && r.idx >= SPRAY_FROM_IDX {
      a.spray_shots += 1;
      a.spray_hits += i64::from(hit);
    }
    a.moving_shots += i64::from(r.moving);
    a.air_shots += i64::from(r.air);
    a.kill_shots += i64::from(r.kill);
    if let Some(e) = r.err {
      a.err_sum += e;
      a.err_n += 1;
    }
    if let Some(i) = r.inacc {
      a.inacc_sum += i;
      a.inacc_n += 1;
    }
    for h in &r.hits {
      if h.hg != 0 {
        *a.shot_hitgroups.entry(h.hg).or_insert(0) += 1;
      }
    }
    let wi = match a.shot_weapons.iter().position(|x| x.w == r.weapon) {
      Some(i) => i,
      None => {
        a.shot_weapons.push(ShotWeapon { w: r.weapon.clone(), shots: 0, hits: 0, hs: 0, dmg: 0.0 });
        a.shot_weapons.len() - 1
      }
    };
    let w = &mut a.shot_weapons[wi];
    w.shots += 1;
    w.hits += i64::from(hit);
    w.hs += i64::from(head);
    w.dmg += r.hits.iter().map(|h| h.dmg).sum::<f64>();
  }
  let round_player: Vec<RoundPlayerBallistics> = order
    .into_iter()
    .filter_map(|k| agg.remove(&k))
    .map(|mut a| {
      a.err_sum = r2(a.err_sum);
      a.inacc_sum = r4(a.inacc_sum);
      a
    })
    .collect();

  // ── blob chi tiết: codec replay-event, 1 body cho cả trận ──
  #[derive(Serialize)]
  struct SlimShot<'a> {
    tick: i64,
    r#type: &'static str,
    data: &'a ShotRow,
  }
  let shot_count = shots.len() as i64;
  let body: Vec<u8> = if shots.is_empty() {
    Vec::new()
  } else {
    let slim: Vec<SlimShot> = shots.iter().map(|s| SlimShot { tick: s.tick, r#type: "SHOT", data: &s.row }).collect();
    serde_json::to_vec(&slim).unwrap_or_default()
  };
  let data = if body.is_empty() { body } else { crate::zstd_codec::compress(&body, zstd_level).unwrap_or(body) };

  BallisticsOut { version: BALLISTICS_VERSION, shot_count, data, round_player }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  #[test]
  fn pairs_hit_by_tick_only_and_builds_bursts() {
    let valid: HashSet<i64> = [1].into_iter().collect();
    let fire = |tick: i64, idx: i64| {
      json!({ "event_name": "weapon_fire", "tick": tick, "total_rounds_played": 0, "weapon": "weapon_usp_silencer",
              "user_steamid": "A", "user_team_num": 3, "user_pitch": 0.0, "user_yaw": 0.0, "user_X": 0.0, "user_Y": 0.0, "user_Z": 0.0,
              "user_shots_fired": idx, "user_velocity_X": 0.0, "user_velocity_Y": 0.0 })
    };
    let events = vec![
      fire(100, 0),
      fire(108, 1),
      json!({ "event_name": "weapon_fire", "tick": 105, "total_rounds_played": 0, "weapon": "weapon_flashbang", "user_steamid": "A" }),
      // Tên súng HURT khác weapon_fire (hkp2000) -- vẫn phải ghép theo tick.
      json!({ "event_name": "player_hurt", "tick": 108, "weapon": "hkp2000", "attacker_steamid": "A", "user_steamid": "B",
              "attacker_team_num": 3, "user_team_num": 2, "hitgroup": "head", "dmg_health": 100, "health": 0 }),
      fire(400, 0),
    ];
    let rows = vec![json!({ "tick": 100, "steamid": "B", "X": 1000.0, "Y": 0.0, "Z": 0.0, "is_alive": true, "team_num": 2, "duck_amount": 0.0 })];
    let out = compute_ballistics(&events, &valid, 64, &rows, 3);

    assert_eq!(out.shot_count, 3, "flashbang không phải viên");
    let a = &out.round_player[0];
    // USP là súng lục → không vào "viên đầu" (chỉ súng tự động).
    assert_eq!((a.shots, a.hits, a.head_hits, a.kill_shots, a.first_shots), (3, 1, 1, 1, 0));
    assert_eq!(a.err_n, 1, "viên tick 100 nhắm thẳng đầu B (lệch ~0°)");
    assert!(a.err_sum < 0.5);
    let body = crate::zstd_codec::decompress(&out.data).unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v[1]["data"]["burst"], 1, "viên 2 cùng loạt");
    assert_eq!(v[2]["data"]["burst"], 2, "nghỉ > 0.5s = loạt mới");
    assert_eq!(v[1]["data"]["hits"][0]["victim"], "B");
    assert_eq!(v[0]["data"]["steamId"], "A");
  }

  #[test]
  fn first_and_spray_only_count_auto_weapons() {
    let valid: HashSet<i64> = [1].into_iter().collect();
    let fire = |tick: i64, weapon: &str, idx: i64| {
      json!({ "event_name": "weapon_fire", "tick": tick, "total_rounds_played": 0, "weapon": weapon,
              "user_steamid": "A", "user_team_num": 3, "user_pitch": 0.0, "user_yaw": 0.0, "user_X": 0.0, "user_Y": 0.0, "user_Z": 0.0,
              "user_shots_fired": idx, "user_velocity_X": 0.0, "user_velocity_Y": 0.0 })
    };
    let events = vec![
      fire(100, "weapon_awp", 0),
      fire(200, "weapon_ak47", 0),
      fire(300, "weapon_negev", 5),
      fire(400, "weapon_deagle", 4),
    ];
    let out = compute_ballistics(&events, &valid, 64, &[], 3);
    let a = &out.round_player[0];
    assert_eq!(a.shots, 4);
    assert_eq!(a.first_shots, 1, "chỉ AK idx 0, AWP không tính");
    assert_eq!(a.spray_shots, 1, "chỉ Negev idx 5, Deagle không tính");
    assert_eq!(out.version, 2);
  }
}
