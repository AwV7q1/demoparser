// ADR-007 roadmap bước 4, Giai đoạn 2 (cs2-analytics): N-API async thật cho TOÀN BỘ pipeline đã
// port ở bước 3 (parse + compute_events + compute_stats + compute_aim_stats), dùng napi::Task
// (env.spawn trên libuv threadpool riêng của napi -- KHÔNG cần tokio, xem Cargo.toml đã có sẵn
// napi 2.12.2 hỗ trợ Task từ trước, chỉ chưa ai dùng). Mục đích: hàm hiện có (parse_events/
// parse_ticks/parse_grenades/parse_player_info/compute_*) đều ĐỒNG BỘ -- gọi từ 1 tiến trình Node
// duy nhất với N job đồng thời sẽ KHÔNG có song song CPU thật (event loop bị chặn tuần tự), đây là
// đúng lý do `apps/parser-worker` hôm nay phải `cluster.fork()` N tiến trình OS (main.ts). Hàm mới
// này chạy TRÊN THREAD NỀN của napi, trả Promise -- gọi N lần đồng thời từ 1 tiến trình Node sẽ
// overlap CPU thật, không cần fork tiến trình.
//
// KHÔNG sửa parse_events/parse_ticks/parse_grenades/parse_player_info hiện có (giữ nguyên hành vi
// đã ship + verify parity) -- các hàm run_parse_* dưới đây COPY lại phần dựng ParserInputs cần
// thiết (không Env, không phụ thuộc gì từ lib.rs ngoài các type dùng chung của crate `parser`), để
// compute() (chạy trên thread nền, KHÔNG có Env) tự làm hết từ đọc file tới compute domain logic,
// không phải gọi ngược lại các hàm #[napi] hiện có (vốn nhận tham số kiểu napi Either/Buffer chỉ
// hợp lệ trên main thread lúc entry).
//
// Scope: giống bench.mjs ở Giai đoạn 1 -- gồm raw parse (events/grenades/playerInfo/ticks) +
// computeEvents + computeStats + computeAimStats + ReplayChunk/ReplayEventChunk (Giai đoạn 3,
// wiring production thật) -- ReplayChunk giờ ăn theo CÙNG lượt tick gộp B1 bên dưới (slice cột
// thô từ MergedTickPass), không phải một lượt ForceSingleThreaded riêng.
//
// plan-128tick-tick-decode-optimization.md B1+B3: 2 lượt quét tick riêng (sampled + aim) đã gộp
// thành 1 (`run_parse_ticks_pass` + `extract_tick_view`), và huffman lookup table build 1 lần
// (`compute()`) thay vì rebuild ở mỗi run_parse_* -- xem doc comment từng hàm.

use ahash::AHashMap;
use ahash::AHashSet;
use memmap2::MmapOptions;
use napi::bindgen_prelude::*;
use napi::Error;
use napi::JsObject;
use napi::Status;
use std::time::Instant;
use parser::first_pass::parser_settings::{rm_user_friendly_names, FirstPassParser, ParserInputs};
use parser::first_pass::prop_controller::{PropInfo, TICK_ID};
use parser::parse_demo::{Parser, ParsingMode};
use parser::second_pass::parser_settings::create_huffman_lookup_table;
use parser::second_pass::variants::{soa_to_aos, OutputSerdeHelperStruct, PropColumn, VarVec};
use parser::tick_codec::{build_replay_chunks, ReplayChunkParsed};
use serde_json::Value;
use std::fs::File;

fn io_err(msg: impl std::fmt::Display) -> Error {
  Error::new(Status::InvalidArg, format!("{msg}"))
}

/// mmap `path` -- mở file MỚI mỗi lần gọi (đúng hành vi thật hôm nay: mỗi raw-fetch tự decode lại
/// toàn bộ demo, xem ghi chú "5 lần decode độc lập/job" ở Giai đoạn 1 ADR log).
fn mmap_path(path: &str) -> napi::Result<memmap2::Mmap> {
  let file = File::open(path).map_err(io_err)?;
  unsafe { MmapOptions::new().map(&file) }.map_err(io_err)
}

// events + playerInfo fusion (hướng 1, SAFE subset -- xem
// .claude/note/adr-007-header-fusion-and-resolve-cost-followup.md): `player_md` (output.player_md)
// đến từ `other_netmessages.rs` (net-message end-of-match/scoreboard, message type KHÁC hẳn
// CsvcMsgPacketEntities), hoàn toàn độc lập với wanted_events/parse_projectiles/parse_ents -- xác
// nhận bằng cách đọc `second_pass/entities.rs::parse_packet_ents` (parse_ents chỉ gate message đó,
// không đụng player_md). Settings của player_info-alone (wanted_player_props/other_props/
// wanted_events RỖNG, parse_ents:false) là tập con YẾU HƠN settings của events -- events tự nó đã
// cần parse_ents:true, nên chạy CHUNG 1 lượt parse với settings của events và lấy CẢ HAI field ra
// từ CÙNG 1 `Output` là byte-identical với chạy 2 lượt riêng, không cần lọc/tách gì thêm.
//
// KHÔNG gộp grenades vào đây: `parse_projectiles` tương tác với `collect_entities()`'s per-tick
// dispatch theo cách KHÔNG cộng dồn an toàn được với `wanted_events` (xem note trên) -- gộp cả 3 sẽ
// làm mất/sai `velocity_X/Y` của mọi event. Grenades giữ nguyên 1 lượt riêng (`run_parse_grenades`).
fn run_parse_events_and_player_info(
  path: &str,
  huf: &Vec<(u8, u8)>,
  event_names: Vec<String>,
  player_props: Vec<String>,
  other_props: Vec<String>,
) -> napi::Result<(Value, Value)> {
  let real_names_player = rm_user_friendly_names(&player_props).map_err(io_err)?;
  let real_other_props = rm_user_friendly_names(&other_props).map_err(io_err)?;

  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, og) in real_names_player.iter().zip(&player_props) {
    real_name_to_og_name.insert(real_name.clone(), og.clone());
  }
  for (real_name, og) in real_other_props.iter().zip(&other_props) {
    real_name_to_og_name.insert(real_name.clone(), og.clone());
  }

  let mmap = mmap_path(path)?;
  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names_player,
    wanted_other_props: real_other_props,
    wanted_prop_states: AHashMap::default(),
    wanted_events: event_names,
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let output = parser.parse_demo(&mmap).map_err(io_err)?;
  let events_val = serde_json::to_value(&output.game_events).map_err(io_err)?;
  let player_info_val = serde_json::to_value(&output.player_md).map_err(io_err)?;
  Ok((events_val, player_info_val))
}

fn run_parse_grenades(path: &str, huf: &Vec<(u8, u8)>) -> napi::Result<Value> {
  let mmap = mmap_path(path)?;
  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: true,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false, // KHÔNG parse grenade LOGIC (throw/detonate) -- chỉ tick sample vị trí, đúng dp.parseGrenades(demoPath, null, false) phía Node dùng ở bench.mjs.
  };
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let output = parser.parse_demo(&mmap).map_err(io_err)?;
  let prop_infos = output.prop_controller.prop_infos.clone();
  let helper = OutputSerdeHelperStruct { prop_infos, inner: output.df.clone().into() };
  let result = soa_to_aos(helper);
  serde_json::to_value(&result).map_err(io_err)
}

// ADR-007 tick-pass fusion (B1): raw columnar output of ONE merged second-pass walk requesting
// the UNION of two different tick-cadence field lists (e.g. sampled-every-8 ∪ dense-aim-window).
// Kept un-serialized so `extract_tick_view` can slice it into each view's own rows/fields after
// the fact, instead of walking the demo twice (the expensive, walk-bound part -- see B1 note in
// .claude/note/plan-128tick-tick-decode-optimization.md).
struct MergedTickPass {
  df: AHashMap<u32, PropColumn>,
  prop_infos: Vec<PropInfo>,
  tickrate: u32,
}

fn run_parse_ticks_pass(path: &str, huf: &Vec<(u8, u8)>, wanted_props: Vec<String>, wanted_ticks: Vec<i32>, velocity_tick_filter: AHashSet<i32>) -> napi::Result<MergedTickPass> {
  let real_names = rm_user_friendly_names(&wanted_props).map_err(io_err)?;
  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, og) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), og.clone());
  }

  let mmap = mmap_path(path)?;
  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: real_names.clone(),
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks,
    parse_projectiles: false,
    only_header: false,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  // ForceSingleThreaded: ADR-007 gotcha #1 (§VI.2f) -- ParsingMode::Normal picks multi-threaded for
  // this prop set, splitting the demo into segments each with its own history buffer -> velocity
  // props go null/0 at segment boundaries, diverging from the ST baseline all parity was verified
  // against. ST keeps continuous history, so replay/aim ticks stay byte-identical with parity.
  //
  // velocity_tick_filter: see SecondPassParser::velocity_tick_filter -- `wanted_ticks` above is a
  // UNION of two cadences sharing one internal `self.output`; without this, velocity's "previous
  // collected tick" search could land on a row from the OTHER cadence and corrupt the delta. This
  // is a DIFFERENT failure mode than the segment-boundary one above (same-buffer cross-cadence
  // contamination vs cross-buffer segment split) -- ForceSingleThreaded does not make this filter
  // redundant, nor vice versa; both guards are needed together on the merged (B1) pass.
  let mut parser = Parser::new(settings, ParsingMode::ForceSingleThreaded).with_velocity_tick_filter(velocity_tick_filter);
  let output = parser.parse_demo(&mmap).map_err(io_err)?;

  let mut prop_infos = output.prop_controller.prop_infos.clone();
  prop_infos.sort_by_key(|x| x.prop_name.clone());
  Ok(MergedTickPass { df: output.df, prop_infos, tickrate: output.tickrate })
}

// Slices a `MergedTickPass` down to just `view_fields` (+ the always-present tick/steamid/name
// baseline -- see prop_controller.rs set_custom_propinfos, pushed unconditionally) and rows whose
// TICK_ID is in `view_ticks`, returning the RAW columnar df + prop_infos -- byte-for-byte what a
// standalone `run_parse_ticks(view_fields, view_ticks.collect())` call would have produced,
// without a second demo walk. Kept un-serialized so a caller can feed it straight into
// `build_replay_chunks` (needs columns, not JSON) as well as into `extract_tick_view` (JSON).
fn slice_tick_columns(merged: &MergedTickPass, view_fields: &[String], view_ticks: &AHashSet<i32>) -> (AHashMap<u32, PropColumn>, Vec<PropInfo>) {
  let tick_col = merged.df.get(&TICK_ID).and_then(|c| c.data.as_ref());
  let indices: Vec<usize> = match tick_col {
    Some(VarVec::I32(v)) => v
      .iter()
      .enumerate()
      .filter(|(_, t)| t.map_or(false, |t| view_ticks.contains(&t)))
      .map(|(i, _)| i)
      .collect(),
    _ => vec![],
  };

  let mut keep_names: AHashSet<&str> = view_fields.iter().map(|s| s.as_str()).collect();
  keep_names.insert("tick");
  keep_names.insert("steamid");
  keep_names.insert("name");

  let prop_infos: Vec<PropInfo> = merged.prop_infos.iter().filter(|p| keep_names.contains(p.prop_friendly_name.as_str())).cloned().collect();

  let mut inner: AHashMap<u32, PropColumn> = AHashMap::default();
  for p in &prop_infos {
    if let Some(col) = merged.df.get(&p.id) {
      if let Some(sliced) = col.slice_to_new(&indices) {
        inner.insert(p.id, sliced);
      }
    }
  }

  (inner, prop_infos)
}

// Same slice as `slice_tick_columns`, serialized to SoA (struct_of_arrays) or AoS JSON.
fn extract_tick_view(merged: &MergedTickPass, view_fields: &[String], view_ticks: &AHashSet<i32>, struct_of_arrays: bool) -> napi::Result<Value> {
  let (inner, prop_infos) = slice_tick_columns(merged, view_fields, view_ticks);
  let helper = OutputSerdeHelperStruct { prop_infos, inner: inner.into() };
  if struct_of_arrays {
    serde_json::to_value(&helper).map_err(io_err)
  } else {
    let result = soa_to_aos(helper);
    serde_json::to_value(&result).map_err(io_err)
  }
}

/// Header-only parse for `meta` (map/tickrate) -- mirrors lib.rs parse_header (FirstPassParser::
/// parse_header_only), runnable on the Task background thread (no Env).
fn run_parse_header(path: &str) -> napi::Result<AHashMap<String, String>> {
  let mmap = mmap_path(path)?;
  let huf = create_huffman_lookup_table();
  let settings = ParserInputs {
    real_name_to_og_name: AHashMap::default(),
    wanted_players: vec![],
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec![],
    parse_ents: false,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = FirstPassParser::new(&settings);
  parser.parse_header_only(&mmap).map_err(io_err)
}

// -- constants (subset, y hệt packages/parse-core/src/constants.ts / prototypes bench.mjs) --
fn all_event_names() -> Vec<String> {
  [
    "player_death", "round_end", "round_start",
    "bomb_planted", "bomb_defused", "bomb_dropped", "bomb_pickup",
    "smokegrenade_detonate", "smokegrenade_expired",
    "inferno_startburn", "inferno_expire",
    "hegrenade_detonate", "flashbang_detonate",
    "player_hurt", "weapon_fire", "player_blind",
    "item_pickup", "buytime_ended",
  ].iter().map(|s| s.to_string()).collect()
}
fn all_event_player_fields() -> Vec<String> {
  [
    "X", "Y", "Z", "attacker_X", "attacker_Y", "attacker_Z",
    "team_num", "attacker_team_num", "user_team_num",
    "site", "user_X", "user_Y", "user_Z", "yaw",
    "pitch", "velocity_X", "velocity_Y",
  ].iter().map(|s| s.to_string()).collect()
}
fn all_event_other_fields() -> Vec<String> {
  ["total_rounds_played", "round_start_time", "winner", "reason"].iter().map(|s| s.to_string()).collect()
}
fn sampled_tick_fields() -> Vec<String> {
  [
    "X", "Y", "Z", "pitch", "yaw", "is_alive", "team_num",
    "health", "armor_value", "active_weapon_name", "active_weapon_ammo",
    "balance", "current_equip_value", "has_helmet", "has_defuser",
    "inventory", "flash_duration", "last_place_name", "is_defusing", "is_scoped",
    "velocity_X", "velocity_Y", "velocity_Z", "duck_amount", "is_walking",
  ].iter().map(|s| s.to_string()).collect()
}
fn aim_tick_fields() -> Vec<String> {
  ["X", "Y", "Z", "pitch", "yaw", "spotted", "is_alive"].iter().map(|s| s.to_string()).collect()
}
const PLAYER_TICK_SAMPLE_STEP: i64 = 8;
const AIM_PREAIM_WINDOW: i64 = 64;

pub struct FullPipelineTask {
  path: String,
  zstd_level: i32,
  max_demo_ticks: i64,
}

impl FullPipelineTask {
  pub fn new(path: String, zstd_level: i32, max_demo_ticks: i64) -> Self {
    Self { path, zstd_level, max_demo_ticks }
  }
}

/// Task::Output -- everything the ParsedMatch needs. Plain fields go through `json` (serde ->
/// JsUnknown on the main thread); the two chunk families carry raw compressed bytes that must
/// cross N-API as `Buffer` (not a serde array of numbers), so they're kept out of `json` and
/// assembled into the object in resolve() where an `Env` is available.
pub struct FullPipelineOutput {
  json: Value,
  replay_chunks: Vec<ReplayChunkParsed>,
  replay_event_chunks: Vec<parser::compute_events::ReplayEventChunkOut>,
}

// Extracted from FullPipelineTask::compute() so the resolve()-cost A/B bench below
// (FullPipelineBufTask) can run the EXACT SAME background-thread work and only vary what happens
// on the main thread afterwards -- see plan-128tick-tick-decode-optimization.md "hướng 2".
fn run_full_pipeline_core(path: &str, zstd_level: i32, max_demo_ticks: i64) -> napi::Result<FullPipelineOutput> {
  // B3: huffman lookup table built ONCE (used to be rebuilt per raw-parse call, 5x/job).
  let huf = create_huffman_lookup_table();

  // 1) raw parse -- events+playerInfo GỘP 1 lượt (hướng 1 an toàn, xem
  //    .claude/note/adr-007-header-fusion-and-resolve-cost-followup.md) + grenades riêng (không gộp
  //    được an toàn với wanted_events -- xem note) + 1 lần quét tick GỘP (B1) -- 3 lần tổng, thay vì 5.
  let (raw_events_val, player_info_val) =
    run_parse_events_and_player_info(path, &huf, all_event_names(), all_event_player_fields(), all_event_other_fields())?;
  let grenade_rows_val = run_parse_grenades(path, &huf)?;

    let raw_events_arr = raw_events_val.as_array().cloned().unwrap_or_default();
    let raw_kills_arr: Vec<Value> = raw_events_arr
      .iter().filter(|e| e.get("event_name").and_then(|v| v.as_str()) == Some("player_death")).cloned().collect();
    let raw_hurt_arr: Vec<Value> = raw_events_arr
      .iter().filter(|e| e.get("event_name").and_then(|v| v.as_str()) == Some("player_hurt")).cloned().collect();
    let round_end_ticks: Vec<i64> = raw_events_arr
      .iter()
      .filter(|e| e.get("event_name").and_then(|v| v.as_str()) == Some("round_end"))
      .filter_map(|e| e.get("tick").and_then(|v| v.as_i64()))
      .collect();

    // 2) computeEvents (pure Rust, không qua N-API JSON round-trip -- deserialize thẳng Value đã có).
    let events_in: Vec<parser::compute_events::RawEvent> = serde_json::from_value(raw_events_val).map_err(io_err)?;
    let grenade_in: Vec<parser::compute_events::RawGrenadeSample> = serde_json::from_value(grenade_rows_val).map_err(io_err)?;
    let mut events_result = parser::compute_events::compute_events(&events_in, &grenade_in, zstd_level);

    let kills_batch: Vec<parser::compute_stats::KillsBatchItem> = events_result
      .events.iter()
      .filter(|e| e.get("type").and_then(|v| v.as_str()) == Some("KILL"))
      .filter_map(|e| {
        let round_number = e.get("roundNumber")?.as_i64()?;
        let tick = e.get("tick")?.as_i64()?;
        let data = serde_json::from_value(e.get("data")?.clone()).ok()?;
        Some(parser::compute_stats::KillsBatchItem { round_number, tick, data })
      })
      .collect();

    // weaponFireBatch/hurtBatch: giải nén + JSON.parse replayEventChunks (encode_replay_events_body
    // Rust y hệt encodeReplayEventsBody.ts -- JSON thô {tick,type,data}[], xem replay_event_chunks.rs).
    #[derive(serde::Deserialize)]
    struct SlimEvent { tick: i64, r#type: String, data: Value }
    let mut weapon_fire_batch: Vec<parser::compute_stats::WeaponFireBatchItem> = Vec::new();
    let mut hurt_batch: Vec<parser::compute_stats::HurtBatchItem> = Vec::new();
    for c in &events_result.replay_event_chunks {
      if c.data.is_empty() {
        continue;
      }
      let decompressed = parser::zstd_codec::decompress(&c.data).map_err(io_err)?;
      let evs: Vec<SlimEvent> = serde_json::from_slice(&decompressed).map_err(io_err)?;
      for ev in evs {
        match ev.r#type.as_str() {
          "WEAPON_FIRE" => {
            if let Ok(data) = serde_json::from_value(ev.data) {
              weapon_fire_batch.push(parser::compute_stats::WeaponFireBatchItem { round_number: c.round_number, tick: ev.tick, data });
            }
          }
          "HURT" => {
            if let Ok(data) = serde_json::from_value(ev.data) {
              hurt_batch.push(parser::compute_stats::HurtBatchItem { round_number: c.round_number, tick: ev.tick, data });
            }
          }
          _ => {}
        }
      }
    }

    // 3+4) B1: MỘT lượt quét tick gộp field ∪ field + tick ∪ tick, thay cho 2 lượt riêng
    // (sampled_tick_fields SoA cho stats + aim_tick_fields AoS quanh mỗi kill). ReplayChunk (Giai
    // đoạn 3) ăn theo CÙNG lượt gộp này (slice cột thô, không phải 1 lượt ForceSingleThreaded
    // riêng nữa) -- xem MergedTickPass/run_parse_ticks_pass/extract_tick_view/slice_tick_columns
    // ở trên. Tổng số lượt quét tick: 1 (thay vì 2 riêng sampled+aim trước B1, hoặc 2 nếu
    // ReplayChunk tự đi quét riêng như bản gốc chưa fuse với B1).
    let last_tick = round_end_ticks.iter().copied().max().unwrap_or(0).max(0);
    let mut sampled_ticks_i32: Vec<i32> = Vec::new();
    let mut t = 0i64;
    while t <= last_tick {
      sampled_ticks_i32.push(t as i32);
      t += PLAYER_TICK_SAMPLE_STEP;
    }
    // DemoTooLargeError guard (compute.ts:63) -- cap sampled tick count.
    if (sampled_ticks_i32.len() as i64) > max_demo_ticks {
      return Err(Error::new(
        Status::GenericFailure,
        format!("DemoTooLarge: {} sampled ticks > max {}", sampled_ticks_i32.len(), max_demo_ticks),
      ));
    }
    let sampled_ticks_set: AHashSet<i32> = sampled_ticks_i32.iter().copied().collect();

    let raw_kills_for_aim: Vec<parser::compute_aim::RawAimKillRow> =
      serde_json::from_value(Value::Array(raw_kills_arr.clone())).map_err(io_err)?;
    let aim_wanted_ticks = parser::compute_aim::compute_aim_wanted_ticks(&raw_kills_for_aim);
    let aim_ticks_set: AHashSet<i32> = aim_wanted_ticks.iter().map(|t| *t as i32).collect();

    let sampled_fields = sampled_tick_fields();
    let aim_fields = aim_tick_fields();
    let mut union_fields = sampled_fields.clone();
    for f in &aim_fields {
      if !union_fields.contains(f) {
        union_fields.push(f.clone());
      }
    }
    let mut union_ticks_set: AHashSet<i32> = sampled_ticks_set.clone();
    union_ticks_set.extend(aim_ticks_set.iter().copied());
    let union_ticks: Vec<i32> = union_ticks_set.into_iter().collect();

    // velocity_tick_filter = sampled cadence only: aim_tick_fields() never requests velocity, so
    // only the sampled view's velocity_X/Y/Z deltas need protecting from the aim window's denser
    // interleaved ticks (see velocity_tick_filter doc comment on SecondPassParser).
    let merged = run_parse_ticks_pass(path, &huf, union_fields, union_ticks, sampled_ticks_set.clone())?;
    let tickrate = merged.tickrate as i64;

    // ReplayChunk: slice RAW columns for just the sampled cadence straight from `merged` (no extra
    // demo walk). round tuples in ORIGINAL rounds order (build_replay_chunks output preserves it,
    // matching compute.ts buildReplayChunks iterating `rounds`).
    let (tick_df, tick_prop_infos) = slice_tick_columns(&merged, &sampled_fields, &sampled_ticks_set);
    let round_tuples: Vec<(i64, i64, i64)> =
      events_result.rounds.iter().map(|r| (r.round_number, r.start_tick, r.end_tick)).collect();
    let replay_chunks =
      build_replay_chunks(&tick_df, &tick_prop_infos, &round_tuples, PLAYER_TICK_SAMPLE_STEP, zstd_level).map_err(io_err)?;

    // SoA JSON for compute_stats (prop_infos sorted by prop_name, matching run_parse_ticks output
    // shape parity was verified against). Moves tick_df (build_replay_chunks already done borrowing).
    // ADR-007 §VI.2u lever ②: normalize NGAY trong scope này rồi để `helper` (bản df đã move) +
    // `tick_data_val` (bản Value) DROP trước compute nặng — chỉ giữ lại `tick_rows` (9 field, nhỏ).
    // Trước đây cả 2 bản to sống song song suốt compute_stats. Output byte-identical (cùng
    // normalize_ticks mà wrapper compute_stats vẫn gọi).
    let tick_rows = {
      let mut sorted_prop_infos = tick_prop_infos.clone();
      sorted_prop_infos.sort_by_key(|x| x.prop_name.clone());
      let helper = OutputSerdeHelperStruct { prop_infos: sorted_prop_infos, inner: tick_df.into() };
      let tick_data_val = serde_json::to_value(&helper).map_err(io_err)?;
      parser::compute_stats::normalize_ticks(&tick_data_val)
    };

    let aim_tick_rows_val = if aim_ticks_set.is_empty() {
      Value::Array(vec![])
    } else {
      extract_tick_view(&merged, &aim_fields, &aim_ticks_set, false)?
    };
    let aim_tick_rows: Vec<parser::compute_aim::RawAimTickRow> = serde_json::from_value(aim_tick_rows_val).map_err(io_err)?;

    // 5) computeStats + computeAimStats.
    let raw_kills: Vec<parser::compute_stats::RawKillRow> = serde_json::from_value(Value::Array(raw_kills_arr)).map_err(io_err)?;
    let raw_hurt: Vec<parser::compute_stats::RawHurtRow> = serde_json::from_value(Value::Array(raw_hurt_arr)).map_err(io_err)?;
    let player_info: Vec<parser::compute_stats::RawPlayerInfo> = serde_json::from_value(player_info_val).map_err(io_err)?;

    let stats_result = parser::compute_stats::compute_stats_rows(
      &kills_batch, &weapon_fire_batch, &hurt_batch, &raw_kills, &raw_hurt, &player_info, &tick_rows, &events_result.rounds,
    );
    let aim_result = parser::compute_aim::compute_aim_stats(&raw_kills_for_aim, &weapon_fire_batch, &aim_tick_rows);

    // 6) meta (map/duration) -- header parse for map name only; tickrate comes from `merged`
    // (A0, detected during the tick pass itself -- see plan-128tick-tick-decode-optimization.md
    // "Một nguồn sự thật") rather than re-parsing it out of the header string map. matchDate is NOT
    // known to Rust (upload-time user choice) -> Node injects it into meta after the call. Mirrors
    // compute.ts:70-72.
    let header = run_parse_header(path)?;
    let map_name = match header.get("map_name") {
      Some(m) if !m.is_empty() => m.clone(),
      _ => "unknown".to_string(),
    };
    let duration: Option<f64> = if last_tick > 0 {
      format!("{:.1}", last_tick as f64 / tickrate as f64).parse::<f64>().ok()
    } else {
      None
    };

    let replay_event_chunks = std::mem::take(&mut events_result.replay_event_chunks);
    let json = serde_json::json!({
      "meta": { "map": map_name, "tickrate": tickrate, "duration": duration },
      "rounds": events_result.rounds,
      "events": events_result.events,
      "matchWeaponStats": stats_result.match_weapon_stats,
      "playerAccuracyStats": stats_result.player_accuracy_stats,
      "playerMatchStats": stats_result.player_match_stats,
      "roundSurvivorStats": stats_result.round_survivor_stats,
      "playerZoneStats": stats_result.player_zone_stats,
      "roundEconomyStats": stats_result.round_economy_stats,
      "roundPlayerDamageStats": stats_result.round_player_damage_stats,
      "playerAimStats": aim_result,
    });
    Ok(FullPipelineOutput { json, replay_chunks, replay_event_chunks })
}

// Builds the 2 chunk arrays shared by both resolve() implementations below. Iterates BY VALUE
// (`into_iter`, not `.iter()+.clone()`) -- the resolve()-cost bench (hướng 2, fix (1)) found the
// previous `.iter()` version had to `.clone()` each chunk's compressed bytes on the main thread
// because it only had a borrow, even though the doc comment claimed "zero-copy". `output` is owned
// in both resolve() calls, so `into_iter()` moves `Vec<u8>` straight into `Buffer::from` for real.
fn build_chunk_arrays(
  env: Env,
  replay_chunks: Vec<ReplayChunkParsed>,
  replay_event_chunks: Vec<parser::compute_events::ReplayEventChunkOut>,
) -> napi::Result<(JsObject, JsObject)> {
  let mut rc = env.create_array_with_length(replay_chunks.len())?;
  for (i, c) in replay_chunks.into_iter().enumerate() {
    let mut o = env.create_object()?;
    o.set("roundNumber", c.round_number)?;
    o.set("format", 1i64)?;
    o.set("tickStart", c.tick_start)?;
    o.set("tickEnd", c.tick_end)?;
    o.set("sampleStep", c.sample_step)?;
    o.set("playerCount", c.player_count)?;
    o.set("data", Buffer::from(c.data))?;
    rc.set_element(i as u32, o)?;
  }

  let mut ec = env.create_array_with_length(replay_event_chunks.len())?;
  for (i, c) in replay_event_chunks.into_iter().enumerate() {
    let mut o = env.create_object()?;
    o.set("roundNumber", c.round_number)?;
    o.set("format", c.format)?;
    o.set("eventCount", c.event_count)?;
    o.set("data", Buffer::from(c.data))?;
    ec.set_element(i as u32, o)?;
  }

  Ok((rc, ec))
}

impl Task for FullPipelineTask {
  type Output = FullPipelineOutput;
  type JsValue = JsObject;

  // Chạy trên thread nền của napi (libuv threadpool) -- KHÔNG có Env, không đụng JS/V8. Đây là
  // toàn bộ lý do hàm này không chặn main thread: mọi việc nặng (đọc/giải mã demo + tính domain
  // logic) đều nằm ở đây.
  fn compute(&mut self) -> napi::Result<Self::Output> {
    run_full_pipeline_core(&self.path, self.zstd_level, self.max_demo_ticks)
  }

  // Main thread (has Env). Plain fields come across via serde (`to_js_value` walks the whole
  // `serde_json::Value` tree field-by-field through N-API); the two chunk arrays are built via
  // `build_chunk_arrays` above (real move, not clone). `__resolveMs` times ONLY this
  // object-construction work (not the wait for the task to be scheduled) -- read directly by
  // resolve-cost-bench.mjs to A/B against FullPipelineBufTask below, instead of inferring the cost
  // from parallel-speedup deltas.
  fn resolve(&mut self, env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
    let t0 = Instant::now();
    let mut obj: JsObject = env.to_js_value(&output.json)?.coerce_to_object()?;
    let (rc, ec) = build_chunk_arrays(env, output.replay_chunks, output.replay_event_chunks)?;
    obj.set("replayChunks", rc)?;
    obj.set("replayEventChunks", ec)?;
    obj.set("__resolveMs", t0.elapsed().as_secs_f64() * 1000.0)?;
    Ok(obj)
  }
}

#[napi]
pub fn compute_full_pipeline_async(
  path: String,
  zstd_level: Option<i32>,
  max_demo_ticks: Option<i64>,
) -> AsyncTask<FullPipelineTask> {
  AsyncTask::new(FullPipelineTask::new(
    path,
    zstd_level.unwrap_or(3),
    max_demo_ticks.unwrap_or(i64::MAX),
  ))
}

// ---- ADR-007 "hướng 2" resolve()-cost A/B bench (plan-128tick-tick-decode-optimization.md) ----
// EXPERIMENTAL, not wired into parser-worker. Runs the identical `run_full_pipeline_core` work on
// the background thread, but instead of leaving `json` as a `serde_json::Value` for `resolve()` to
// walk via `to_js_value`, `compute()` itself serializes it to bytes (`serde_json::to_vec`, still on
// the background thread, no Env needed) -- `resolve()` then only wraps 1 `Buffer` + the chunk
// arrays, no per-field FFI walk. JS side does `JSON.parse(buf.toString())` to get the object back
// (V8's native JSON.parse, not hand-rolled `to_js_value`). Exposed as `compute_full_pipeline_async_buf`
// purely so resolve-cost-bench.mjs can measure whether this actually beats the production path
// before anyone rewires nativeDemoEngine.ts onto it.
pub struct FullPipelineBufOutput {
  json_bytes: Vec<u8>,
  replay_chunks: Vec<ReplayChunkParsed>,
  replay_event_chunks: Vec<parser::compute_events::ReplayEventChunkOut>,
}

pub struct FullPipelineBufTask {
  path: String,
  zstd_level: i32,
  max_demo_ticks: i64,
}

impl Task for FullPipelineBufTask {
  type Output = FullPipelineBufOutput;
  type JsValue = JsObject;

  fn compute(&mut self) -> napi::Result<Self::Output> {
    let out = run_full_pipeline_core(&self.path, self.zstd_level, self.max_demo_ticks)?;
    // Serialization happens HERE (background thread) instead of via `to_js_value` on the main
    // thread in resolve() -- this is the entire point of the A/B: move the "walk the whole value
    // tree" cost off the thread that gates parallelism.
    let json_bytes = serde_json::to_vec(&out.json).map_err(io_err)?;
    Ok(FullPipelineBufOutput { json_bytes, replay_chunks: out.replay_chunks, replay_event_chunks: out.replay_event_chunks })
  }

  // Main thread: wrap 1 Buffer (json bytes) + the chunk arrays -- no per-field FFI walk. Same
  // `__resolveMs` convention as FullPipelineTask::resolve so the bench compares like for like
  // (object-construction time only; JSON.parse happens back in JS and is timed separately there).
  fn resolve(&mut self, env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
    let t0 = Instant::now();
    let mut obj = env.create_object()?;
    obj.set("jsonBuf", Buffer::from(output.json_bytes))?;
    let (rc, ec) = build_chunk_arrays(env, output.replay_chunks, output.replay_event_chunks)?;
    obj.set("replayChunks", rc)?;
    obj.set("replayEventChunks", ec)?;
    obj.set("__resolveMs", t0.elapsed().as_secs_f64() * 1000.0)?;
    Ok(obj)
  }
}

#[napi]
pub fn compute_full_pipeline_async_buf(
  path: String,
  zstd_level: Option<i32>,
  max_demo_ticks: Option<i64>,
) -> AsyncTask<FullPipelineBufTask> {
  AsyncTask::new(FullPipelineBufTask {
    path,
    zstd_level: zstd_level.unwrap_or(3),
    max_demo_ticks: max_demo_ticks.unwrap_or(i64::MAX),
  })
}

// plan-128tick-tick-decode-optimization.md B1 gate: "parity byte-identical cả sampled-output lẫn
// aim-output" -- `legacy_run_parse_ticks` below is a byte-for-byte copy of the PRE-fusion
// `run_parse_ticks` (two fully independent second-pass walks), kept ONLY as the oracle this test
// diffs the new merged-pass output against. Not called from production code.
#[cfg(test)]
mod b1_tick_fusion_parity {
  use super::*;

  const TEST_DEMO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../parser/test_demo.dem");

  fn legacy_run_parse_ticks(path: &str, huf: &Vec<(u8, u8)>, wanted_props: Vec<String>, wanted_ticks: Vec<i32>, struct_of_arrays: bool) -> Value {
    let real_names = rm_user_friendly_names(&wanted_props).unwrap();
    let mut real_name_to_og_name = AHashMap::default();
    for (real_name, og) in real_names.iter().zip(&wanted_props) {
      real_name_to_og_name.insert(real_name.clone(), og.clone());
    }
    let mmap = mmap_path(path).unwrap();
    let settings = ParserInputs {
      real_name_to_og_name,
      wanted_players: vec![],
      wanted_player_props: real_names.clone(),
      wanted_other_props: vec![],
      wanted_events: vec![],
      wanted_prop_states: AHashMap::default(),
      parse_ents: true,
      wanted_ticks,
      parse_projectiles: false,
      only_header: false,
      list_props: false,
      only_convars: false,
      huffman_lookup_table: huf,
      order_by_steamid: false,
      fallback_bytes: None,
      parse_grenades: false,
    };
    let mut parser = Parser::new(settings, ParsingMode::Normal);
    let output = parser.parse_demo(&mmap).unwrap();
    let mut prop_infos = output.prop_controller.prop_infos.clone();
    prop_infos.sort_by_key(|x| x.prop_name.clone());
    let helper = OutputSerdeHelperStruct { prop_infos, inner: output.df.clone().into() };
    if struct_of_arrays {
      serde_json::to_value(&helper).unwrap()
    } else {
      serde_json::to_value(&soa_to_aos(helper)).unwrap()
    }
  }

  #[test]
  fn merged_tick_pass_matches_two_separate_passes() {
    let huf = create_huffman_lookup_table();

    let sampled_fields = sampled_tick_fields();
    let aim_fields = aim_tick_fields();

    // Two synthetic dense windows, deliberately NOT 8-aligned so they interleave with the sampled
    // cadence at odd offsets -- this is exactly the boundary a naive merge would corrupt (sampled
    // velocity_X/Y/Z picking up an aim-only row as its "previous tick").
    let aim_ticks: Vec<i32> = (503..=567).chain(3003..=3067).collect();
    let aim_ticks_set: AHashSet<i32> = aim_ticks.iter().copied().collect();

    let sampled_ticks: Vec<i32> = (0..=4000).step_by(8).collect();
    let sampled_ticks_set: AHashSet<i32> = sampled_ticks.iter().copied().collect();

    // --- oracle: two fully separate second-pass walks (pre-B1 behaviour) ---
    let legacy_sampled = legacy_run_parse_ticks(TEST_DEMO, &huf, sampled_fields.clone(), sampled_ticks.clone(), true);
    let legacy_aim = legacy_run_parse_ticks(TEST_DEMO, &huf, aim_fields.clone(), aim_ticks.clone(), false);

    // --- new: one merged walk, split via extract_tick_view ---
    let mut union_fields = sampled_fields.clone();
    for f in &aim_fields {
      if !union_fields.contains(f) {
        union_fields.push(f.clone());
      }
    }
    let mut union_ticks_set = sampled_ticks_set.clone();
    union_ticks_set.extend(aim_ticks_set.iter().copied());
    let union_ticks: Vec<i32> = union_ticks_set.into_iter().collect();

    let merged = run_parse_ticks_pass(TEST_DEMO, &huf, union_fields, union_ticks, sampled_ticks_set.clone()).unwrap();
    let new_sampled = extract_tick_view(&merged, &sampled_fields, &sampled_ticks_set, true).unwrap();
    let new_aim = extract_tick_view(&merged, &aim_fields, &aim_ticks_set, false).unwrap();

    assert_eq!(legacy_sampled, new_sampled, "sampled view diverged after tick-pass fusion");
    assert_eq!(legacy_aim, new_aim, "aim view diverged after tick-pass fusion");
  }

  // Sanity companion: with NO aim window at all (union == sampled), the merge must still match --
  // covers the common real-world case of a demo with zero valid engagements.
  #[test]
  fn merged_tick_pass_matches_when_aim_window_empty() {
    let huf = create_huffman_lookup_table();
    let sampled_fields = sampled_tick_fields();
    let sampled_ticks: Vec<i32> = (0..=4000).step_by(8).collect();
    let sampled_ticks_set: AHashSet<i32> = sampled_ticks.iter().copied().collect();

    let legacy_sampled = legacy_run_parse_ticks(TEST_DEMO, &huf, sampled_fields.clone(), sampled_ticks.clone(), true);

    let merged = run_parse_ticks_pass(TEST_DEMO, &huf, sampled_fields.clone(), sampled_ticks.clone(), sampled_ticks_set.clone()).unwrap();
    let new_sampled = extract_tick_view(&merged, &sampled_fields, &sampled_ticks_set, true).unwrap();

    assert_eq!(legacy_sampled, new_sampled, "sampled view diverged with an empty aim window");
  }

  // A0 sanity: test_demo.dem is a 64-tick demo -- detected tickrate must come out at 64, not the
  // hard-coded fallback silently masking a broken CsvcMsgServerInfo.tick_interval read.
  #[test]
  fn tickrate_detected_as_64_on_64tick_fixture() {
    let huf = create_huffman_lookup_table();
    let merged = run_parse_ticks_pass(TEST_DEMO, &huf, sampled_tick_fields(), vec![0, 8, 16], AHashSet::from_iter([0, 8, 16])).unwrap();
    assert_eq!(merged.tickrate, 64);
  }

  // Real 128-tick fixture (pro POV, not checked into git -- huge). Not run by a plain
  // `cargo test`; opt in with:
  //   CS2_128TICK_DEMO=/path/to/demo.dem cargo test --lib -- --ignored 128tick
  #[test]
  #[ignore]
  fn tickrate_and_c4_timer_sanity_on_real_128tick_demo() {
    let Ok(path) = std::env::var("CS2_128TICK_DEMO") else {
      panic!("set CS2_128TICK_DEMO=/path/to/demo.dem to run this test");
    };
    let huf = create_huffman_lookup_table();
    let mmap = mmap_path(&path).unwrap();
    let settings = ParserInputs {
      real_name_to_og_name: AHashMap::default(),
      wanted_players: vec![],
      wanted_player_props: vec![],
      wanted_other_props: vec![],
      wanted_prop_states: AHashMap::default(),
      wanted_events: vec!["bomb_planted".to_string(), "bomb_exploded".to_string()],
      parse_ents: true,
      wanted_ticks: vec![],
      parse_projectiles: false,
      only_header: true,
      list_props: false,
      only_convars: false,
      huffman_lookup_table: &huf,
      order_by_steamid: false,
      fallback_bytes: None,
      parse_grenades: false,
    };
    let mut parser = Parser::new(settings, ParsingMode::Normal);
    let output = parser.parse_demo(&mmap).unwrap();

    eprintln!("detected tickrate={}", output.tickrate);

    // C4 timer sanity (plan gate §6): (bomb_exploded.tick - bomb_planted.tick) / tickrate ≈ 40s.
    // NOTE: on the real "128tick" G2 vs Spirit fixtures this ended up empirically confirming the
    // plan's own open question (§7) -- CsvcMsgServerInfo.tick_interval on these GOTV demos reads
    // as 1/64 (tickrate=64), and every plant->explode delta is a constant 2624 ticks, which is
    // 41.0s at the 64 interpretation vs a nonsensical 20.5s at 128. So despite the folder name,
    // these particular demo files are NOT a genuine 128-tick sample by the metric that actually
    // matters (the recorded tick encoding) -- GOTV recording rate can differ from the server's
    // matchmaking-advertised tickrate. Assert against whatever tickrate was ACTUALLY detected,
    // not a hard-coded 128, so this test still catches a real off-by-2x bug on a genuine 128-tick
    // sample later without special-casing this fixture.
    let planted: Vec<i32> = output.game_events.iter().filter(|e| e.name == "bomb_planted").map(|e| e.tick).collect();
    let exploded: Vec<i32> = output.game_events.iter().filter(|e| e.name == "bomb_exploded").map(|e| e.tick).collect();
    assert!(!planted.is_empty(), "no bomb_planted events found -- wrong fixture?");
    assert!(!exploded.is_empty(), "no bomb_exploded events found -- pick a round where the bomb actually detonated");

    for &exp_tick in &exploded {
      let plant_tick = planted.iter().filter(|&&p| p <= exp_tick).max().copied().expect("no bomb_planted preceding this explosion");
      let delta_ticks = exp_tick - plant_tick;
      let secs_at_detected = delta_ticks as f64 / output.tickrate as f64;
      let secs_at_half = delta_ticks as f64 / (output.tickrate as f64 / 2.0);
      eprintln!(
        "plant={plant_tick} explode={exp_tick} delta_ticks={delta_ticks} -> {secs_at_detected:.3}s@detected({}) {secs_at_half:.3}s@half",
        output.tickrate,
      );
      // Loose absolute bound (some leagues/offsets push this a little past 40) PLUS a relative
      // check that the detected tickrate is unambiguously the better fit than half its value --
      // catches a genuine 2x tickrate misdetection even if the absolute C4 constant isn't 40.0.
      assert!((secs_at_detected - 40.0).abs() < 2.0, "C4 timer implausible at detected tickrate: {secs_at_detected:.2}s");
      assert!(
        (secs_at_detected - 40.0).abs() < (secs_at_half - 40.0).abs(),
        "detected tickrate({}) fits the C4 timer WORSE than half its value -- likely a 2x tickrate misdetection",
        output.tickrate
      );
    }
  }
}

// ADR-007 "hướng 1" events+playerInfo fusion parity (plan-128tick-tick-decode-optimization.md /
// adr-007-header-fusion-and-resolve-cost-followup.md). `legacy_run_parse_events`/
// `legacy_run_parse_player_info` below are byte-for-byte copies of the PRE-fusion standalone
// functions (each its own `parse_demo()` walk) -- kept ONLY as the oracle this test diffs the new
// `run_parse_events_and_player_info` merged-pass output against. Not called from production code.
//
// NOTE: grenades is intentionally NOT part of this fusion/test -- `parse_projectiles` is not safely
// additive with `wanted_events` in `collect_entities()` (would corrupt event velocity_X/Y), so it
// stays a fully separate pass (`run_parse_grenades`). See the note file for the full trace.
#[cfg(test)]
mod events_playerinfo_fusion_parity {
  use super::*;

  const TEST_DEMO: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../parser/test_demo.dem");

  fn legacy_run_parse_events(path: &str, huf: &Vec<(u8, u8)>, event_names: Vec<String>, player_props: Vec<String>, other_props: Vec<String>) -> Value {
    let real_names_player = rm_user_friendly_names(&player_props).unwrap();
    let real_other_props = rm_user_friendly_names(&other_props).unwrap();
    let mut real_name_to_og_name = AHashMap::default();
    for (real_name, og) in real_names_player.iter().zip(&player_props) {
      real_name_to_og_name.insert(real_name.clone(), og.clone());
    }
    for (real_name, og) in real_other_props.iter().zip(&other_props) {
      real_name_to_og_name.insert(real_name.clone(), og.clone());
    }
    let mmap = mmap_path(path).unwrap();
    let settings = ParserInputs {
      real_name_to_og_name,
      wanted_players: vec![],
      wanted_player_props: real_names_player,
      wanted_other_props: real_other_props,
      wanted_prop_states: AHashMap::default(),
      wanted_events: event_names,
      parse_ents: true,
      wanted_ticks: vec![],
      parse_projectiles: false,
      only_header: true,
      list_props: false,
      only_convars: false,
      huffman_lookup_table: huf,
      order_by_steamid: false,
      fallback_bytes: None,
      parse_grenades: false,
    };
    let mut parser = Parser::new(settings, ParsingMode::Normal);
    let output = parser.parse_demo(&mmap).unwrap();
    serde_json::to_value(&output.game_events).unwrap()
  }

  fn legacy_run_parse_player_info(path: &str, huf: &Vec<(u8, u8)>) -> Value {
    let settings = ParserInputs {
      wanted_players: vec![],
      real_name_to_og_name: AHashMap::default(),
      wanted_player_props: vec![],
      wanted_other_props: vec![],
      wanted_prop_states: AHashMap::default(),
      wanted_events: vec![],
      parse_ents: false,
      wanted_ticks: vec![],
      parse_projectiles: false,
      only_header: true,
      list_props: false,
      only_convars: false,
      huffman_lookup_table: huf,
      order_by_steamid: false,
      fallback_bytes: None,
      parse_grenades: false,
    };
    let mmap = mmap_path(path).unwrap();
    let mut parser = Parser::new(settings, ParsingMode::Normal);
    let output = parser.parse_demo(&mmap).unwrap();
    serde_json::to_value(&output.player_md).unwrap()
  }

  #[test]
  fn merged_events_and_player_info_match_two_separate_passes() {
    let huf = create_huffman_lookup_table();
    let event_names = all_event_names();
    let player_fields = all_event_player_fields();
    let other_fields = all_event_other_fields();

    let legacy_events = legacy_run_parse_events(TEST_DEMO, &huf, event_names.clone(), player_fields.clone(), other_fields.clone());
    let legacy_player_info = legacy_run_parse_player_info(TEST_DEMO, &huf);

    let (merged_events, merged_player_info) =
      run_parse_events_and_player_info(TEST_DEMO, &huf, event_names, player_fields, other_fields).unwrap();

    assert_eq!(legacy_events, merged_events, "game_events diverged after events+playerInfo fusion");
    assert_eq!(legacy_player_info, merged_player_info, "player_md diverged after events+playerInfo fusion");
  }
}
