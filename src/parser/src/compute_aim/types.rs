// ADR-007 §VI.2 (cs2-analytics) "aim" domain port. Mirrors computeAimStats (compute.ts L589-686).
//
// computeAimStats(parser, killEvents, weaponFireBatch) calls `parser.parseTicks(AIM_TICK_FIELDS,
// wantedTicks)` ITSELF, with `wantedTicks` computed from a window around each kill -- unlike
// every other function ported so far, this one drives its own parser call rather than consuming
// an already-parsed batch. This Rust port stays a pure function like its siblings: the caller
// (harness today; production wiring later, ADR-007 roadmap step 4/5) is expected to already have
// fetched AIM_TICK_FIELDS rows for the SAME wanted-tick window (see `compute_aim_wanted_ticks`,
// which IS ported here so a real caller can compute that window without needing TS at all) and
// hand them in as `aim_tick_rows`.

use serde::Deserialize;
use serde_json::Value;

// Only kills.tick/attacker_steamid/user_steamid are read by computeAimStats -- a separate,
// minimal row shape rather than reusing compute_stats::RawKillRow (that one carries fields this
// domain never touches, e.g. team_num/headshot/assist -- keeping this self-contained avoids
// coupling two independently-verified phases through a shared "kitchen sink" row type).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawAimKillRow {
  #[serde(default)]
  pub attacker_steamid: Option<String>,
  #[serde(default)]
  pub user_steamid: Option<String>,
  #[serde(default)]
  pub tick: Option<i64>,
}

// AIM_TICK_FIELDS = ['X','Y','Z','pitch','yaw','spotted','is_alive'] (constants.ts) -- `is_alive`
// is fetched by the real pipeline but never actually read anywhere in computeAimStats, so it is
// intentionally not modeled here (nothing to verify parity against).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct RawAimTickRow {
  #[serde(default)]
  pub tick: Option<i64>,
  #[serde(default)]
  pub steamid: Option<Value>, // coerced via String(r.steamid||'') -- see mod.rs
  #[serde(rename = "X", default)]
  pub x: Option<f64>,
  #[serde(rename = "Y", default)]
  pub y: Option<f64>,
  #[serde(rename = "Z", default)]
  pub z: Option<f64>,
  #[serde(default)]
  pub pitch: Option<f64>,
  #[serde(default)]
  pub yaw: Option<f64>,
  #[serde(default)]
  pub spotted: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerAimStat {
  pub steam_id: String,
  pub engagements: i64,
  pub preaim_deg: Option<f64>,
  pub time_to_kill_ms: Option<i64>,
  pub reaction_samples: i64,
  pub rifle_shots: i64,
  pub moving_shots: i64,
  pub avg_speed: Option<i64>,
  pub has_velocity_data: bool,
}
