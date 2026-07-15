// ADR-007 §VI.2 (cs2-analytics) "events" domain port. Input/output shapes for the whole
// compute_events module. Mirrors packages/parse-core/src/compute.ts's "events" section +
// packages/parse-core/src/helpers.ts + packages/replay-codec-core/src/replay-event-codec-core.ts.
//
// RawEvent is intentionally ONE flat struct with Option<T> for every field read by ANY of the
// events-domain functions, mirroring how compute.ts treats each raw demoparser2 row as `any` --
// a field simply reads as `undefined`/None when the row's event_name doesn't carry it. Field
// names/case match the literal JSON keys demoparser2 emits (NOT Rust snake_case convention) --
// see per-field `#[serde(rename)]` where the JSON key uses a capital letter demoparser2 itself
// uses (bare `X`/`Y`/`Z`, `attacker_X`, `user_X`, ...). Do NOT "clean up" these names --
// compute.ts relies on the exact-cased field existing or not (e.g. bare lowercase `x`/`y`/`z` on
// grenade detonate events vs uppercase `X`/`Y`/`Z`/`user_X` on kill/bomb events are DIFFERENT
// fields, not a casing inconsistency to fix).

use serde::{Deserialize, Serialize};
use serde_json::Value;

// NOTE (ADR-007 §VI.2, "stats"/"aim" phase): ParsedRound/KillData/WeaponFireData/HurtData below
// gained a `Deserialize` derive so compute_stats/compute_aim can accept them as INPUT (they are
// literally the output of compute_events -- killsBatch/weaponFireBatch/hurtBatch in compute.ts).
// Purely additive: does not change any field or the Serialize output used by the "events" phase.

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawEvent {
  pub event_name: String,
  #[serde(default)]
  pub tick: Option<i64>,
  #[serde(default)]
  pub total_rounds_played: Option<i64>,
  #[serde(default)]
  pub winner: Option<String>,
  #[serde(default)]
  pub reason: Option<String>,

  #[serde(default)]
  pub attacker_steamid: Option<String>,
  #[serde(default)]
  pub attacker_name: Option<Value>, // có thể là object -- xem clean_name()
  #[serde(default)]
  pub attacker_team_num: Option<i64>,
  #[serde(rename = "attacker_X", default)]
  pub attacker_x: Option<f64>,
  #[serde(rename = "attacker_Y", default)]
  pub attacker_y: Option<f64>,
  #[serde(rename = "attacker_Z", default)]
  pub attacker_z: Option<f64>,

  #[serde(default)]
  pub user_steamid: Option<String>,
  #[serde(default)]
  pub user_name: Option<Value>,
  #[serde(default)]
  pub user_team_num: Option<i64>,
  #[serde(rename = "user_X", default)]
  pub user_x: Option<f64>,
  #[serde(rename = "user_Y", default)]
  pub user_y: Option<f64>,
  #[serde(rename = "user_Z", default)]
  pub user_z: Option<f64>,
  #[serde(default)]
  pub user_yaw: Option<f64>,
  #[serde(rename = "user_velocity_X", default)]
  pub user_velocity_x: Option<f64>,
  #[serde(rename = "user_velocity_Y", default)]
  pub user_velocity_y: Option<f64>,

  #[serde(default)]
  pub assister_steamid: Option<String>,
  #[serde(default)]
  pub assister_name: Option<Value>,

  #[serde(default)]
  pub weapon: Option<String>,
  #[serde(default)]
  pub headshot: Option<bool>,
  #[serde(default)]
  pub hitgroup: Option<Value>, // passthrough thô -- KHÔNG convert ở tầng events (xem gotcha helpers.rs)
  #[serde(default)]
  pub assistedflash: Option<bool>,
  #[serde(default)]
  pub penetrated: Option<i64>,
  #[serde(default)]
  pub noscope: Option<bool>,
  #[serde(default)]
  pub thrusmoke: Option<bool>,
  #[serde(default)]
  pub attackerblind: Option<bool>,
  #[serde(default)]
  pub distance: Option<f64>,

  // Bare uppercase (kill/bomb-plant fallback fields) -- KHÁC bare lowercase (grenade detonate).
  #[serde(rename = "X", default)]
  pub bare_x: Option<f64>,
  #[serde(rename = "Y", default)]
  pub bare_y: Option<f64>,
  #[serde(rename = "Z", default)]
  pub bare_z: Option<f64>,
  // Bare lowercase -- vị trí detonate (smoke/fire/he/flash), field riêng của event, không qua
  // wanted-player-props.
  #[serde(default)]
  pub x: Option<f64>,
  #[serde(default)]
  pub y: Option<f64>,
  #[serde(default)]
  pub z: Option<f64>,

  #[serde(default)]
  pub entityid: Option<i64>,
  // GOTCHA: `site` là SỐ NGUYÊN thô từ demoparser2 (vd 96/97), KHÔNG phải chuỗi "A"/"B" -- verify
  // qua parity harness trên demo thật (§VI.2 events-half). compute.ts KHÔNG convert field này
  // (`site: ev.site` truyền nguyên), nên Rust cũng giữ nguyên kiểu số, không tự ý format.
  #[serde(default)]
  pub site: Option<i64>,
  #[serde(default)]
  pub dmg_health: Option<f64>,
  #[serde(default)]
  pub blind_duration: Option<f64>,
  #[serde(default)]
  pub item: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawGrenadeSample {
  #[serde(default)]
  pub steamid: Option<String>,
  #[serde(default)]
  pub tick: Option<i64>,
  #[serde(default)]
  pub x: Option<f64>,
  #[serde(default)]
  pub y: Option<f64>,
  #[serde(default)]
  pub z: Option<f64>,
  #[serde(default)]
  pub grenade_type: Option<String>,
}

// ── Rounds ────────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedRound {
  pub round_number: i64,
  pub winner_side: String,
  pub reason: String,
  pub t_score: i64,
  pub ct_score: i64,
  pub start_tick: i64,
  pub end_tick: i64,
}

// ── Events (generic envelope, `data` filled by each builder) ───────────────────
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedEvent<D: Serialize> {
  pub round_number: i64,
  pub tick: i64,
  pub r#type: &'static str,
  pub data: D,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KillData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub attacker_name: Option<Value>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub attacker_steam_id: Option<String>,
  pub attacker_side: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub victim_name: Option<Value>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub victim_steam_id: Option<String>,
  pub victim_side: Option<String>,
  pub assister_name: Option<Value>,
  pub assister_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub weapon: Option<String>,
  pub headshot: bool,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub hitgroup: Option<Value>,
  pub assisted_flash: bool,
  pub penetrated: bool,
  pub noscope: bool,
  pub thrusmoke: bool,
  pub attacker_blind: bool,
  pub suicide: bool,
  pub distance: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub x: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub y: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub attacker_x: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub attacker_y: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BombPlantData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub x: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub y: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub z: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub site: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub planted_by: Option<Value>,
  pub team: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BombDefuseData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub x: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub y: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub z: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub site: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub defused_by: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BombDroppedData {
  pub x: f64,
  pub y: f64,
  pub z: f64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub dropped_by_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub dropped_by_name: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BombPickupData {
  pub x: f64,
  pub y: f64,
  pub z: f64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub picked_up_by_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub picked_up_by_name: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponPickupData {
  pub x: f64,
  pub y: f64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub z: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub weapon: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub picked_up_by_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub picked_up_by_name: Option<Value>,
}

// SMOKE/FIRE (buildGrenadeEffects) -- luôn withEnd=true ở 2 call site thật (L87-94), nên
// startTick/endTick luôn có mặt, không port cờ withEnd (nhánh false chết, không có call site nào
// dùng false -- xem plan §"compute_events/grenades.rs").
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrenadeEffectData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub x: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub y: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub z: Option<f64>,
  pub start_tick: i64,
  pub end_tick: i64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub thrown_by: Option<Value>,
  pub team: String,
  pub thrower_x: Option<f64>,
  pub thrower_y: Option<f64>,
  pub throw_tick: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub trajectory: Option<Vec<TrajPoint>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstantNadeData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub x: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub y: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub z: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub thrown_by: Option<Value>,
  pub team: String,
  pub thrower_x: Option<f64>,
  pub thrower_y: Option<f64>,
  pub throw_tick: Option<i64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub trajectory: Option<Vec<TrajPoint>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrajPoint {
  pub tick: i64,
  pub x: f64,
  pub y: f64,
  pub z: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BurnData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub victim_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub attacker_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub dmg: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlindData {
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub flasher_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub flasher_name: Option<Value>,
  pub flasher_side: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub victim_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub victim_name: Option<Value>,
  pub victim_side: Option<String>,
  pub blind_duration: f64,
  pub is_enemy_flash: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponFireData {
  pub x: f64,
  pub y: f64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub yaw: Option<f64>,
  pub vel_x: Option<f64>,
  pub vel_y: Option<f64>,
  pub steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub weapon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HurtData {
  pub attacker_x: f64,
  pub attacker_y: f64,
  pub x: f64,
  pub y: f64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub z: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub weapon: Option<String>,
  pub dmg_health: f64,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub hitgroup: Option<Value>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub attacker_steam_id: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none", default)]
  pub victim_steam_id: Option<String>,
}

// ── Replay event chunk (JSON+zstd, port replay-event-codec-core.ts) ────────────
// (slim {tick,type,data} shape sống trong replay_event_chunks.rs, không lặp lại ở đây)
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayEventChunkOut {
  pub round_number: i64,
  pub format: i32,
  pub event_count: i64,
  pub data: Vec<u8>, // pre-zstd raw hoặc đã nén tuỳ nơi gọi -- xem replay_event_chunks.rs
}
