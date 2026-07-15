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
// Scope: giống bench.mjs ở Giai đoạn 1 -- gồm raw parse (events/grenades/playerInfo/ticks×2) +
// computeEvents + computeStats + computeAimStats. CHƯA gồm ReplayChunk (tick-codec streaming) --
// để dành Giai đoạn 3 khi wiring production thật, 2 luồng sẽ ghép chung.

use ahash::AHashMap;
use memmap2::MmapOptions;
use napi::bindgen_prelude::*;
use napi::Error;
use napi::JsUnknown;
use napi::Status;
use parser::first_pass::parser_settings::{rm_user_friendly_names, ParserInputs};
use parser::parse_demo::{Parser, ParsingMode};
use parser::second_pass::parser_settings::create_huffman_lookup_table;
use parser::second_pass::variants::{soa_to_aos, OutputSerdeHelperStruct};
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

fn run_parse_events(path: &str, event_names: Vec<String>, player_props: Vec<String>, other_props: Vec<String>) -> napi::Result<Value> {
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
  let huf = create_huffman_lookup_table();
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
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let output = parser.parse_demo(&mmap).map_err(io_err)?;
  serde_json::to_value(&output.game_events).map_err(io_err)
}

fn run_parse_grenades(path: &str) -> napi::Result<Value> {
  let mmap = mmap_path(path)?;
  let huf = create_huffman_lookup_table();
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
    huffman_lookup_table: &huf,
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

fn run_parse_player_info(path: &str) -> napi::Result<Value> {
  let mmap = mmap_path(path)?;
  let huf = create_huffman_lookup_table();
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
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let output = parser.parse_demo(&mmap).map_err(io_err)?;
  serde_json::to_value(&output.player_md).map_err(io_err)
}

fn run_parse_ticks(path: &str, wanted_props: Vec<String>, wanted_ticks: Vec<i32>, struct_of_arrays: bool) -> napi::Result<Value> {
  let real_names = rm_user_friendly_names(&wanted_props).map_err(io_err)?;
  let mut real_name_to_og_name = AHashMap::default();
  for (real_name, og) in real_names.iter().zip(&wanted_props) {
    real_name_to_og_name.insert(real_name.clone(), og.clone());
  }

  let mmap = mmap_path(path)?;
  let huf = create_huffman_lookup_table();
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
    huffman_lookup_table: &huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let output = parser.parse_demo(&mmap).map_err(io_err)?;

  let mut prop_infos = output.prop_controller.prop_infos.clone();
  prop_infos.sort_by_key(|x| x.prop_name.clone());
  let helper = OutputSerdeHelperStruct { prop_infos, inner: output.df.clone().into() };

  if struct_of_arrays {
    serde_json::to_value(&helper).map_err(io_err)
  } else {
    let result = soa_to_aos(helper);
    serde_json::to_value(&result).map_err(io_err)
  }
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
}

impl FullPipelineTask {
  pub fn new(path: String, zstd_level: i32) -> Self {
    Self { path, zstd_level }
  }
}

impl Task for FullPipelineTask {
  type Output = Value;
  type JsValue = JsUnknown;

  // Chạy trên thread nền của napi (libuv threadpool) -- KHÔNG có Env, không đụng JS/V8. Đây là
  // toàn bộ lý do hàm này không chặn main thread: mọi việc nặng (đọc/giải mã demo + tính domain
  // logic) đều nằm ở đây.
  fn compute(&mut self) -> napi::Result<Self::Output> {
    // 1) raw parse (4 lần decode demo -- events/grenades/playerInfo/ticks-sampled -- + 1 lần nữa
    //    cho cửa sổ aim bên dưới, đúng 5 lần như đã đo ở Giai đoạn 1).
    let raw_events_val = run_parse_events(&self.path, all_event_names(), all_event_player_fields(), all_event_other_fields())?;
    let grenade_rows_val = run_parse_grenades(&self.path)?;
    let player_info_val = run_parse_player_info(&self.path)?;

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
    let events_result = parser::compute_events::compute_events(&events_in, &grenade_in, self.zstd_level);

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

    // 3) tick data cho stats (SAMPLED_TICK_FIELDS, SoA) -- replicate wantedTicks compute.ts tự tính.
    let last_tick = round_end_ticks.iter().copied().max().unwrap_or(0).max(0);
    let mut wanted_ticks_i32: Vec<i32> = Vec::new();
    let mut t = 0i64;
    while t <= last_tick {
      wanted_ticks_i32.push(t as i32);
      t += PLAYER_TICK_SAMPLE_STEP;
    }
    let tick_data_val = run_parse_ticks(&self.path, sampled_tick_fields(), wanted_ticks_i32, true)?;

    // 4) cửa sổ aim (AIM_TICK_FIELDS, AoS) -- tick quanh mỗi kill "engagement thật".
    let raw_kills_for_aim: Vec<parser::compute_aim::RawAimKillRow> =
      serde_json::from_value(Value::Array(raw_kills_arr.clone())).map_err(io_err)?;
    let aim_wanted_ticks = parser::compute_aim::compute_aim_wanted_ticks(&raw_kills_for_aim);
    let aim_tick_rows_val = if aim_wanted_ticks.is_empty() {
      Value::Array(vec![])
    } else {
      run_parse_ticks(&self.path, aim_tick_fields(), aim_wanted_ticks.iter().map(|t| *t as i32).collect(), false)?
    };
    let aim_tick_rows: Vec<parser::compute_aim::RawAimTickRow> = serde_json::from_value(aim_tick_rows_val).map_err(io_err)?;

    // 5) computeStats + computeAimStats.
    let raw_kills: Vec<parser::compute_stats::RawKillRow> = serde_json::from_value(Value::Array(raw_kills_arr)).map_err(io_err)?;
    let raw_hurt: Vec<parser::compute_stats::RawHurtRow> = serde_json::from_value(Value::Array(raw_hurt_arr)).map_err(io_err)?;
    let player_info: Vec<parser::compute_stats::RawPlayerInfo> = serde_json::from_value(player_info_val).map_err(io_err)?;

    let stats_result = parser::compute_stats::compute_stats(
      &kills_batch, &weapon_fire_batch, &hurt_batch, &raw_kills, &raw_hurt, &player_info, &tick_data_val, &events_result.rounds,
    );
    let aim_result = parser::compute_aim::compute_aim_stats(&raw_kills_for_aim, &weapon_fire_batch, &aim_tick_rows);

    Ok(serde_json::json!({
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
    }))
  }

  // Chạy lại trên main thread (có Env) -- `Task::JsValue` bắt buộc `ToNapiValue + TypeName`, mà
  // `serde_json::Value` không impl `TypeName` (chỉ ToNapiValue, đủ cho return type thường nhưng
  // không đủ cho Task) -- dùng `env.to_js_value` (napi serde-json feature) chuyển thẳng sang
  // JsUnknown, tương đương serde_json::Value nhưng thoả được bound của Task.
  fn resolve(&mut self, env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
    env.to_js_value(&output)
  }
}

#[napi]
pub fn compute_full_pipeline_async(path: String, zstd_level: Option<i32>) -> AsyncTask<FullPipelineTask> {
  AsyncTask::new(FullPipelineTask::new(path, zstd_level.unwrap_or(3)))
}
