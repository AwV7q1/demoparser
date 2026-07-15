// ADR-007 §VI.2 (cs2-analytics) "aim" domain port -- orchestrator. Mirrors computeAimStats
// (compute.ts L589-686). See types.rs header for why this module takes pre-fetched
// AIM_TICK_FIELDS rows rather than calling a parser itself.

mod constants;
mod types;

pub use types::{PlayerAimStat, RawAimKillRow, RawAimTickRow};

use crate::compute_events::norm_weapon_name;
use crate::compute_stats::{OrderedMap, WeaponFireBatchItem};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
struct Acc {
  preaim_sum: f64,
  preaim_n: i64,
  ttk_sum: f64,
  ttk_n: i64,
  rifle_shots: i64,
  moving_shots: i64,
  speed_sum: f64,
  has_vel: bool,
}

fn finalize(acc: OrderedMap<String, Acc>) -> Vec<PlayerAimStat> {
  acc
    .into_entries()
    .into_iter()
    .map(|(steam_id, a)| PlayerAimStat {
      steam_id,
      engagements: a.preaim_n,
      preaim_deg: if a.preaim_n > 0 { Some((a.preaim_sum / a.preaim_n as f64 * 100.0).round() / 100.0) } else { None },
      time_to_kill_ms: if a.ttk_n > 0 { Some((a.ttk_sum / a.ttk_n as f64).round() as i64) } else { None },
      reaction_samples: a.ttk_n,
      rifle_shots: a.rifle_shots,
      moving_shots: a.moving_shots,
      avg_speed: if a.rifle_shots > 0 { Some((a.speed_sum / a.rifle_shots as f64).round() as i64) } else { None },
      has_velocity_data: a.has_vel,
    })
    .collect()
}

/// Pure helper mirroring the `wanted`/`wantedTicks` computation inside computeAimStats -- a
/// future production caller (ADR-007 roadmap step 4/5) can use this to know which ticks to fetch
/// without depending on TS. NOT required by the harness (which hands in rows already fetched by
/// the real TS run for parity purposes) but ported since it's small, pure, and testable.
pub fn compute_aim_wanted_ticks(kills: &[RawAimKillRow]) -> Vec<i64> {
  let mut wanted: BTreeSet<i64> = BTreeSet::new();
  for k in kills {
    if !is_engagement(k) {
      continue;
    }
    let end = k.tick.unwrap();
    let start = (end - constants::AIM_PREAIM_WINDOW).max(0);
    for t in start..=end {
      wanted.insert(t);
    }
  }
  wanted.into_iter().collect()
}

// `attacker_steamid && user_steamid && attacker_steamid !== user_steamid && k.tick` -- note the
// bare `k.tick` truthy check: a kill at tick 0 is excluded (0 is falsy in JS), not just a missing
// tick.
fn is_engagement(k: &RawAimKillRow) -> bool {
  let attacker = match &k.attacker_steamid { Some(s) if !s.is_empty() => s, _ => return false };
  let victim = match &k.user_steamid { Some(s) if !s.is_empty() => s, _ => return false };
  if attacker == victim {
    return false;
  }
  matches!(k.tick, Some(t) if t != 0)
}

fn coerce_steamid(v: &Option<serde_json::Value>) -> String {
  match v {
    Some(serde_json::Value::String(s)) if !s.is_empty() => s.clone(),
    Some(serde_json::Value::Number(n)) => n.to_string(),
    _ => String::new(),
  }
}

pub fn compute_aim_stats(kill_events: &[RawAimKillRow], weapon_fire_batch: &[WeaponFireBatchItem], aim_tick_rows: &[RawAimTickRow]) -> Vec<PlayerAimStat> {
  let kills: Vec<&RawAimKillRow> = kill_events.iter().filter(|k| is_engagement(k)).collect();

  let mut acc: OrderedMap<String, Acc> = OrderedMap::new();

  for f in weapon_fire_batch {
    let d = &f.data;
    let sid = match &d.steam_id { Some(s) if !s.is_empty() => s.clone(), _ => continue };
    let max_speed = match constants::rifle_max_speed(&norm_weapon_name(&d.weapon)) { Some(v) => v, None => continue };
    let (vel_x, vel_y) = match (d.vel_x, d.vel_y) { (Some(x), Some(y)) => (x, y), _ => continue };
    let a = acc.entry_or_insert_with(sid, Acc::default);
    a.has_vel = true;
    let speed = vel_x.hypot(vel_y);
    a.rifle_shots += 1;
    a.speed_sum += speed;
    if speed > max_speed * constants::ACCURACY_SPEED_FACTOR {
      a.moving_shots += 1;
    }
  }

  if kills.is_empty() {
    return finalize(acc);
  }

  let mut by_tick: BTreeMap<i64, BTreeMap<String, &RawAimTickRow>> = BTreeMap::new();
  for r in aim_tick_rows {
    let sid = coerce_steamid(&r.steamid);
    if sid.is_empty() {
      continue;
    }
    let tick = match r.tick { Some(t) => t, None => continue };
    by_tick.entry(tick).or_default().insert(sid, r);
  }

  for k in &kills {
    let attacker = k.attacker_steamid.clone().unwrap();
    let victim = k.user_steamid.clone().unwrap();
    let kill_tick = k.tick.unwrap();
    let start = (kill_tick - constants::AIM_PREAIM_WINDOW).max(0);

    let mut t0: i64 = -1;
    for t in start..=kill_tick {
      if let Some(vrow) = by_tick.get(&t).and_then(|m| m.get(&victim)) {
        if vrow.spotted == Some(true) {
          t0 = t;
          break;
        }
      }
    }
    if t0 < 0 {
      continue;
    }

    let a_row = by_tick.get(&t0).and_then(|m| m.get(&attacker));
    let v_row = by_tick.get(&t0).and_then(|m| m.get(&victim));
    let (a_row, v_row) = match (a_row, v_row) {
      (Some(a), Some(v)) => (a, v),
      _ => continue,
    };
    if a_row.x.is_none() || v_row.x.is_none() {
      continue;
    }

    let acc_entry = acc.entry_or_insert_with(attacker.clone(), Acc::default);
    acc_entry.ttk_sum += ((kill_tick - t0) as f64 / constants::AIM_TICK_RATE) * 1000.0;
    acc_entry.ttk_n += 1;

    let yaw = a_row.yaw.unwrap_or(0.0).to_radians();
    let pitch = a_row.pitch.unwrap_or(0.0).to_radians();
    let aim_x = pitch.cos() * yaw.cos();
    let aim_y = pitch.cos() * yaw.sin();
    let aim_z = -pitch.sin();
    // NOTE: only X is null-checked above (mirrors compute.ts exactly -- `aRow.X == null ||
    // vRow.X == null`, NOT Y/Z). If Y or Z were ever absent while X is present, JS would compute
    // `undefined - number` => NaN here, which then makes `len` NaN and the `len < 1` guard below
    // silently false (NaN comparisons are always false) rather than skipping the sample -- an
    // existing quirk of compute.ts, not something to "fix" while porting. `.unwrap_or(f64::NAN)`
    // reproduces that NaN-propagation exactly; in practice demoparser2 always emits X/Y/Z
    // together so this never actually triggers on real data.
    let dx = v_row.x.unwrap() - a_row.x.unwrap();
    let dy = v_row.y.unwrap_or(f64::NAN) - a_row.y.unwrap_or(f64::NAN);
    let dz = (v_row.z.unwrap_or(f64::NAN) + constants::AIM_TARGET_Z) - (a_row.z.unwrap_or(f64::NAN) + constants::AIM_EYE_Z);
    let len = (dx * dx + dy * dy + dz * dz).sqrt();
    if len < 1.0 {
      continue;
    }
    let dot = (aim_x * dx + aim_y * dy + aim_z * dz) / len;
    let angle = dot.max(-1.0).min(1.0).acos().to_degrees();
    acc_entry.preaim_sum += angle;
    acc_entry.preaim_n += 1;
  }

  finalize(acc)
}
