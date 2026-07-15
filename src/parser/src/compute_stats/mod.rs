// ADR-007 §VI.2 (cs2-analytics) "stats" domain port -- orchestrator. Mirrors compute.ts's
// computeWeaponAccuracyStats/computePlayerStats/computeTickAggregates/computeDamageStats (the
// "stats" section referenced in the plan; "aim" -- computeAimStats -- is its own sibling module,
// compute_aim/, since it needs a different, dynamically-computed tick window rather than the
// fixed SAMPLED_TICK_FIELDS sample this module consumes).
//
// Inputs are independent of compute_events at the N-API boundary (each phase is dumped/verified
// against the real TS baseline separately, per ADR-007's "port dần, verify từng phần" mandate) --
// but the Rust TYPES are shared (see types.rs header comment) since killsBatch/weaponFireBatch/
// hurtBatch/rounds ARE literally compute_events' own output shapes in compute.ts.

mod constants;
mod damage_stats;
mod ordered_map;
mod player_stats;
mod tick_aggregates;
mod tick_normalize;
mod types;
mod weapon_accuracy;

pub use types::{
  HurtBatchItem, KillsBatchItem, MatchWeaponStat, PlayerAccuracyStat, PlayerMatchStat, PlayerZoneStat, RawHurtRow, RawKillRow, RawPlayerInfo,
  RoundDamageStat, RoundEconomyStat, RoundSurvivorStat, TickRow, WeaponFireBatchItem,
};
// Reused by compute_aim (its `acc` map has the exact same "JS Map insertion order matters"
// requirement -- see ordered_map.rs header comment).
pub use ordered_map::OrderedMap;
// ADR-007 §VI.2u lever ②: full_pipeline normalize sớm rồi drop Value+helper trước compute nặng.
pub use tick_normalize::normalize_ticks;

use crate::compute_events::ParsedRound;
use serde_json::Value;

pub struct ComputeStatsResult {
  pub match_weapon_stats: Vec<MatchWeaponStat>,
  pub player_accuracy_stats: Vec<PlayerAccuracyStat>,
  pub player_match_stats: Vec<PlayerMatchStat>,
  pub round_survivor_stats: Vec<RoundSurvivorStat>,
  pub player_zone_stats: Vec<PlayerZoneStat>,
  pub round_economy_stats: Vec<RoundEconomyStat>,
  pub round_player_damage_stats: Vec<RoundDamageStat>,
}

// Wrapper nhận SoA/AoS Value (giữ cho N-API standalone `computeStats` + parity harness TS). Chỉ
// normalize rồi uỷ cho `compute_stats_rows`. ADR-007 §VI.2u lever ②: full_pipeline KHÔNG dùng
// wrapper này — nó normalize sớm rồi DROP Value+helper trước compute nặng (bớt 1-2 bản tick trong
// RAM), gọi thẳng `compute_stats_rows`. Output byte-identical (cùng normalize_ticks).
#[allow(clippy::too_many_arguments)]
pub fn compute_stats(
  kills_batch: &[KillsBatchItem],
  weapon_fire_batch: &[WeaponFireBatchItem],
  hurt_batch: &[HurtBatchItem],
  raw_kills: &[RawKillRow],
  raw_hurt: &[RawHurtRow],
  player_info: &[RawPlayerInfo],
  tick_data: &Value,
  rounds: &[ParsedRound],
) -> ComputeStatsResult {
  let tick_rows = tick_normalize::normalize_ticks(tick_data);
  compute_stats_rows(kills_batch, weapon_fire_batch, hurt_batch, raw_kills, raw_hurt, player_info, &tick_rows, rounds)
}

#[allow(clippy::too_many_arguments)]
pub fn compute_stats_rows(
  kills_batch: &[KillsBatchItem],
  weapon_fire_batch: &[WeaponFireBatchItem],
  hurt_batch: &[HurtBatchItem],
  raw_kills: &[RawKillRow],
  raw_hurt: &[RawHurtRow],
  player_info: &[RawPlayerInfo],
  tick_rows: &[TickRow],
  rounds: &[ParsedRound],
) -> ComputeStatsResult {
  let (match_weapon_stats, player_accuracy_stats) = weapon_accuracy::compute_weapon_accuracy_stats(kills_batch, weapon_fire_batch, hurt_batch);
  let player_match_stats = player_stats::compute_player_stats(raw_kills, player_info, raw_hurt);
  let (round_survivor_stats, player_zone_stats, round_economy_stats) = tick_aggregates::compute_tick_aggregates(tick_rows, kills_batch, rounds);
  let round_player_damage_stats = damage_stats::compute_damage_stats(hurt_batch);

  ComputeStatsResult {
    match_weapon_stats,
    player_accuracy_stats,
    player_match_stats,
    round_survivor_stats,
    player_zone_stats,
    round_economy_stats,
    round_player_damage_stats,
  }
}
