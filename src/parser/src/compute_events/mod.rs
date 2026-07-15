// ADR-007 §VI.2 (cs2-analytics) "events" domain port -- orchestrator. Mirrors the "events"
// section of computeMatchData() in packages/parse-core/src/compute.ts (L79-105): filter
// all_events by event_name (byEvent), call each build_* in the SAME order, encode replay-event
// chunks from [...weaponFireBatch, ...hurtBatch] only. "stats"/"aim" (computeWeaponAccuracyStats,
// computePlayerStats, computeTickAggregates, computeDamageStats, computeAimStats) are OUT OF
// SCOPE for this phase -- they stay in TS, see plan/ADR-007 §VI update.
//
// Public surface: ONLY `compute_events` is exported -- mirrors packages/parse-core only exporting
// computeMatchData despite many internal helper functions.

mod bomb;
mod burn_blind;
mod constants;
mod grenades;
mod helpers;
mod kills;
mod replay_event_chunks;
mod rounds;
mod types;
mod weapon_fire_hurt;

pub use types::{RawEvent, RawGrenadeSample};
pub use types::{ParsedRound, ReplayEventChunkOut};
// Re-exported for compute_stats/compute_aim (ADR-007 §VI.2, "stats"/"aim" phase): killsBatch/
// weaponFireBatch/hurtBatch in compute.ts are literally the output of buildKills/buildWeaponFire/
// buildHurt (KillData/WeaponFireData/HurtData below) -- reusing the same types instead of
// hand-duplicating their fields keeps the two phases from silently drifting apart.
pub use types::{HurtData, KillData, WeaponFireData};
// Small stateless helpers reused as-is by compute_stats (norm_weapon_name/hitgroup_to_int/
// clean_name/side_or_null) -- see helpers.rs.
pub use helpers::{clean_name, hitgroup_to_int, norm_weapon_name, side_2_else_ct, side_or_null};

use replay_event_chunks::ReplayEventItem;
use serde_json::Value;
use types::ParsedEvent;

pub struct ComputeEventsResult {
  pub rounds: Vec<ParsedRound>,
  pub events: Vec<Value>,
  pub replay_event_chunks: Vec<ReplayEventChunkOut>,
}

// Closure không generic được ở Rust stable (mỗi closure chỉ có 1 chữ ký cụ thể) -- dùng hàm
// thường thay vì closure để gọi được với nhiều D khác nhau (KillData, BombPlantData, ...).
fn push_all<D: serde::Serialize>(events: &mut Vec<Value>, batch: &[ParsedEvent<D>]) {
  for e in batch {
    events.push(serde_json::to_value(e).expect("ParsedEvent serialize không bao giờ lỗi (không có key non-string/NaN)"));
  }
}

pub fn compute_events(all_events: &[RawEvent], grenade_rows: &[RawGrenadeSample], zstd_level: i32) -> ComputeEventsResult {
  let by_event = |name: &str| -> Vec<RawEvent> { all_events.iter().filter(|e| e.event_name == name).cloned().collect() };

  let kill_events = by_event("player_death");
  let round_end_events = by_event("round_end");
  let round_start_events = by_event("round_start");
  let bomb_plant_events = by_event("bomb_planted");
  let bomb_defuse_events = by_event("bomb_defused");
  let bomb_dropped_events = by_event("bomb_dropped");
  let bomb_pickup_events = by_event("bomb_pickup");
  let smoke_start_events = by_event("smokegrenade_detonate");
  let smoke_end_events = by_event("smokegrenade_expired");
  let fire_start_events = by_event("inferno_startburn");
  let fire_end_events = by_event("inferno_expire");
  let he_events = by_event("hegrenade_detonate");
  let flash_events = by_event("flashbang_detonate");
  let hurt_events = by_event("player_hurt");
  let fire_shots = by_event("weapon_fire");
  let blinded_events = by_event("player_blind");
  let item_pickup_events = by_event("item_pickup");
  let buytime_ended_events = by_event("buytime_ended");

  // buildGrenadeTrajectoryIndex: filter x/y/z != null trước khi group (helpers.ts).
  let grenade_traj_rows: Vec<RawGrenadeSample> =
    grenade_rows.iter().filter(|r| r.x.is_some() && r.y.is_some() && r.z.is_some()).cloned().collect();
  let grenade_traj = grenades::build_grenade_trajectory_index(&grenade_traj_rows);

  let rr = rounds::compute_rounds(&round_start_events, &round_end_events);

  let mut events: Vec<Value> = Vec::new();

  let kills = kills::build_kills(&kill_events, &rr.valid_rounds);
  push_all(&mut events, &kills);

  let (plants, defuses) =
    bomb::build_bomb_plant_defuse(&bomb_plant_events, &bomb_defuse_events, &rr.valid_rounds, &rr.round_start_tick_by_num);
  push_all(&mut events, &plants);
  push_all(&mut events, &defuses);

  let (drops, pickups) = bomb::build_bomb_drop_pickup(&bomb_dropped_events, &bomb_pickup_events, &rr.valid_rounds);
  push_all(&mut events, &drops);
  push_all(&mut events, &pickups);

  let weapon_pickups = bomb::build_weapon_pickup(&item_pickup_events, &buytime_ended_events, &rr.rounds);
  push_all(&mut events, &weapon_pickups);

  let smokes = grenades::build_grenade_effects(
    "SMOKE",
    &smoke_start_events,
    &smoke_end_events,
    constants::SMOKE_FALLBACK_TICKS,
    &fire_shots,
    &grenade_traj,
    constants::smoke_fire_weapon(),
    constants::smoke_proj(),
    &rr.valid_rounds,
  );
  push_all(&mut events, &smokes);

  let fires = grenades::build_grenade_effects(
    "FIRE",
    &fire_start_events,
    &fire_end_events,
    constants::FIRE_FALLBACK_TICKS,
    &fire_shots,
    &grenade_traj,
    constants::fire_fire_weapon(),
    constants::fire_proj(),
    &rr.valid_rounds,
  );
  push_all(&mut events, &fires);

  let hes = grenades::build_instant_nade(
    "HE_EXPLODE",
    &he_events,
    &fire_shots,
    &grenade_traj,
    constants::he_fire_weapon(),
    constants::he_proj(),
    &rr.valid_rounds,
  );
  push_all(&mut events, &hes);

  let flashes = grenades::build_instant_nade(
    "FLASH",
    &flash_events,
    &fire_shots,
    &grenade_traj,
    constants::flash_fire_weapon(),
    constants::flash_proj(),
    &rr.valid_rounds,
  );
  push_all(&mut events, &flashes);

  let burns = burn_blind::build_burn(&hurt_events, &rr.valid_rounds);
  push_all(&mut events, &burns);

  let blinds = burn_blind::build_blind(&blinded_events, &rr.valid_rounds);
  push_all(&mut events, &blinds);

  // weaponFireBatch/hurtBatch KHÔNG vào `events` vĩnh viễn -- chỉ nguồn cho replay event chunks
  // (compute.ts L101-103: `buildReplayEventChunks([...weaponFireBatch, ...hurtBatch], compress)`).
  let weapon_fire_batch = weapon_fire_hurt::build_weapon_fire(&fire_shots, &rr.valid_rounds);
  let hurt_batch = weapon_fire_hurt::build_hurt(&hurt_events, &rr.valid_rounds);

  let mut replay_items: Vec<ReplayEventItem> = Vec::new();
  for e in &weapon_fire_batch {
    replay_items.push(ReplayEventItem {
      round_number: e.round_number,
      tick: e.tick,
      r#type: e.r#type.to_string(),
      data: serde_json::to_value(&e.data).expect("WeaponFireData serialize"),
    });
  }
  for e in &hurt_batch {
    replay_items.push(ReplayEventItem {
      round_number: e.round_number,
      tick: e.tick,
      r#type: e.r#type.to_string(),
      data: serde_json::to_value(&e.data).expect("HurtData serialize"),
    });
  }
  let replay_event_chunks = replay_event_chunks::build_replay_event_chunks(replay_items, zstd_level);

  ComputeEventsResult { rounds: rr.rounds, events, replay_event_chunks }
}
