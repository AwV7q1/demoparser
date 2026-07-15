// ADR-007 §VI.2 (cs2-analytics) "stats" domain port. Input/output shapes.
//
// Inputs deliberately reuse compute_events::{KillData, WeaponFireData, HurtData, ParsedRound} --
// killsBatch/weaponFireBatch/hurtBatch/rounds in compute.ts are literally the output of
// buildKills/buildWeaponFire/buildHurt/computeRounds (see compute_events/mod.rs re-exports).
// Only round_number/tick/data are needed here (no caller reads `.type` on any of these batches
// in computeWeaponAccuracyStats/computePlayerStats/computeTickAggregates/computeDamageStats), so
// the wrapper below skips the `type` field entirely rather than depending on ParsedEvent<D>
// (whose `r#type: &'static str` can't round-trip through Deserialize).

use crate::compute_events::{HurtData, KillData, WeaponFireData};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KillsBatchItem {
  pub round_number: i64,
  pub tick: i64,
  pub data: KillData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeaponFireBatchItem {
  pub round_number: i64,
  pub tick: i64,
  pub data: WeaponFireData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HurtBatchItem {
  pub round_number: i64,
  pub tick: i64,
  pub data: HurtData,
}

// ── Raw inputs (parser.parsePlayerInfo() / parseEvents() rows used directly, not via builders) ─
// computePlayerStats(kills, playerInfo, hurtEvents) reads RAW event rows -- NOT killsBatch/
// hurtBatch -- and does NOT filter by validRounds (gotcha, see player_stats.rs header comment).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawKillRow {
  #[serde(default)]
  pub attacker_steamid: Option<String>,
  #[serde(default)]
  pub attacker_name: Option<Value>,
  #[serde(default)]
  pub attacker_team_num: Option<i64>,
  #[serde(default)]
  pub user_steamid: Option<String>,
  #[serde(default)]
  pub user_name: Option<Value>,
  #[serde(default)]
  pub user_team_num: Option<i64>,
  #[serde(default)]
  pub assister_steamid: Option<String>,
  #[serde(default)]
  pub assister_name: Option<Value>,
  #[serde(default)]
  pub headshot: Option<bool>,
  #[serde(default)]
  pub assistedflash: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawHurtRow {
  #[serde(default)]
  pub attacker_steamid: Option<String>,
  #[serde(default)]
  pub user_steamid: Option<String>,
  #[serde(default)]
  pub dmg_health: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawPlayerInfo {
  #[serde(default)]
  pub steamid: Option<String>,
  #[serde(default)]
  pub name: Option<Value>,
  #[serde(default)]
  pub team_number: Option<i64>,
}

// ── Outputs ─────────────────────────────────────────────────────────────────
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchWeaponStat {
  pub weapon: String,
  pub kills: i64,
  pub hs_kills: i64,
  pub shots: i64,
  pub hits: i64,
  pub damage: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct HitDetail {
  pub hitgroup: i64,
  pub tick: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerAccuracyStat {
  pub steam_id: String,
  pub shots: i64,
  pub hits: i64,
  pub hitgroups: std::collections::BTreeMap<i64, i64>,
  pub hits_detail: Vec<HitDetail>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerMatchStat {
  pub steam_id: String,
  pub player_name: String,
  // Option, NOT String: assister-only creation path uses side_or_null (kill.attacker_team_num
  // === 2 ? 'T' : === 3 ? 'CT' : null) -- unlike attacker/victim/playerInfo creation (always
  // 'T'/'CT'), a player who ONLY ever assists (never kills/dies) can end up with side: null.
  pub side: Option<String>,
  pub kills: i64,
  pub deaths: i64,
  pub assists: i64,
  pub headshot_kills: i64,
  pub damage: f64,
  pub flash_assists: i64,
  pub slot_order: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundSurvivorStat {
  pub round_number: i64,
  pub steam_id: String,
  pub alive: bool,
  pub side: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerZoneStat {
  pub steam_id: String,
  pub side: String,
  pub place: String,
  pub alive_count: i64,
  pub all_count: i64,
  pub sum_x: f64,
  pub sum_y: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundEconomyStat {
  pub round_number: i64,
  pub steam_id: String,
  pub side: String,
  pub equip_buy: f64,
  pub money_end: f64,
  pub equip_at_death: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundDamageStat {
  pub round_number: i64,
  pub steam_id: String,
  pub damage: f64,
  pub he_damage: f64,
}

// ── Tick row (normalizeTicks output, internal-only -- never crosses N-API) ────
// toTickRow() in compute.ts (L692-726) produces ~24 fields, but computeTickAggregates (the ONLY
// consumer in the "stats" domain scope) only ever reads steamId/tick/x/y/isAlive/side/equipValue/
// money/lastPlace -- the rest (z, yaw, pitch, armor, weapon, ammo, hasHelmet/Defuser, inventory,
// flashDuration, isDefusing/isScoped, velX/Y/Z, duckAmount, isWalking) are computed by toTickRow
// but never read afterwards anywhere in this phase's scope, so they are intentionally NOT ported
// here (nothing to verify parity against -- they'd be untested dead weight). `health` is used
// only transiently to derive `is_alive` during normalization, so it isn't kept on the row either.
#[derive(Debug, Clone)]
pub struct TickRow {
  pub steam_id: String,
  pub tick: i64,
  pub x: f64,
  pub y: f64,
  pub is_alive: bool,
  pub side: Option<String>,
  pub money: Option<f64>,
  pub equip_value: Option<f64>,
  pub last_place: Option<String>,
}
