// Ghép đường bay + gốc ném cho smoke/fire/HE/flash (buildGrenadeEffects / buildInstantNade).
//
// Đường bay ghép theo ENTITY projectile, không theo "cùng người + cùng loại + cửa sổ tick": heuristic
// cũ (port từ helpers.ts) cắt mất đầu smoke bay > 6s (smoke qua mái bay tới ~9.5s), trộn sample của
// nhiều quả cùng loại (flash ném liên tiếp, smoke cũ còn nằm đất vẫn được sample) và lấy nhầm
// weapon_fire của quả ném sau làm gốc ném. Sample projectile liên tục từng tick -- không có "gap qua mái".

use super::constants::{
  DETONATE_END_SLACK, GRENADE_DOWNSAMPLE, INFERNO_MATCH_TICKS, PROJECTILE_SPLIT_GAP, THROW_BEFORE_SPAWN_MAX,
  THROW_MATCH_MAX_TICKS,
};
use super::helpers::side_2_else_ct;
use super::types::{GrenadeEffectData, InstantNadeData, ParsedEvent, RawEvent, RawGrenadeSample, TrajPoint};
use std::collections::{HashMap, HashSet};

// JS Math.round(): làm tròn nửa về phía +∞ (khác Rust f64::round(), làm tròn nửa RA XA 0) --
// floor(x+0.5) tái tạo đúng hành vi JS cho mọi x thực tế gặp ở toạ độ demo.
fn js_round(v: f64) -> f64 {
  (v + 0.5).floor()
}

pub struct ThrowOrigin {
  pub tick: i64,
  pub x: f64,
  pub y: f64,
  pub z: f64,
}

// Ghép mỗi event start (smoke/fire) với expire gần nhất cùng entityid → khoảng thời gian tồn tại.
struct PairedGrenadeEvent<'a> {
  ev: &'a RawEvent,
  end_tick: i64,
}

fn pair_grenade_effects<'a>(
  start_events: &'a [RawEvent],
  end_events: &[RawEvent],
  fallback_ticks: i64,
) -> Vec<PairedGrenadeEvent<'a>> {
  let mut ends_by_entity: HashMap<Option<i64>, Vec<i64>> = HashMap::new();
  for e in end_events {
    ends_by_entity.entry(e.entityid).or_default().push(e.tick.unwrap_or(0));
  }
  for arr in ends_by_entity.values_mut() {
    arr.sort();
  }

  start_events
    .iter()
    .map(|s| {
      let s_tick = s.tick.unwrap_or(0);
      let end_tick = ends_by_entity
        .get(&s.entityid)
        .and_then(|candidates| candidates.iter().copied().find(|t| *t > s_tick))
        .unwrap_or(s_tick + fallback_ticks);
      PairedGrenadeEvent { ev: s, end_tick }
    })
    .collect()
}

// 1 chuyến bay = các sample liên tục của 1 entity projectile (sort tăng theo tick).
pub struct ProjectileFlight {
  steamid: Option<String>,
  grenade_type: String,
  rows: Vec<RawGrenadeSample>,
}

impl ProjectileFlight {
  fn first_tick(&self) -> i64 {
    self.rows[0].tick.unwrap_or(0)
  }
  fn last_tick(&self) -> i64 {
    self.rows[self.rows.len() - 1].tick.unwrap_or(0)
  }
}

pub struct ProjectileFlights {
  flights: Vec<ProjectileFlight>,
  by_entity: HashMap<i64, Vec<usize>>,
}

// Rows đầu vào ĐÃ được lọc x/y/z != null bởi caller. Row thiếu entity id/tick/type bị bỏ (không ghép được).
pub fn build_projectile_flights(rows: &[RawGrenadeSample]) -> ProjectileFlights {
  let mut by_eid: HashMap<i64, Vec<&RawGrenadeSample>> = HashMap::new();
  for r in rows {
    if let (Some(eid), Some(_), Some(_)) = (r.grenade_entity_id, r.tick, r.grenade_type.as_ref()) {
      by_eid.entry(eid).or_default().push(r);
    }
  }

  let mut flights: Vec<ProjectileFlight> = Vec::new();
  let mut by_entity: HashMap<i64, Vec<usize>> = HashMap::new();
  for (eid, mut samples) in by_eid {
    samples.sort_by_key(|r| r.tick.unwrap_or(0));
    let mut cur: Option<ProjectileFlight> = None;
    for r in samples {
      let continues = cur.as_ref().is_some_and(|f| {
        r.tick.unwrap_or(0) - f.last_tick() <= PROJECTILE_SPLIT_GAP
          && r.grenade_type.as_deref() == Some(f.grenade_type.as_str())
          && r.steamid == f.steamid
      });
      if !continues {
        if let Some(f) = cur.take() {
          by_entity.entry(eid).or_default().push(flights.len());
          flights.push(f);
        }
        cur = Some(ProjectileFlight {
          steamid: r.steamid.clone(),
          grenade_type: r.grenade_type.clone().unwrap_or_default(),
          rows: Vec::new(),
        });
      }
      cur.as_mut().unwrap().rows.push(r.clone());
    }
    if let Some(f) = cur {
      by_entity.entry(eid).or_default().push(flights.len());
      flights.push(f);
    }
  }
  ProjectileFlights { flights, by_entity }
}

// Cách tìm chuyến bay cho 1 event nổ/cháy.
#[derive(Clone, Copy)]
pub enum FlightMatch {
  // smoke/HE/flash: `entityid` của event chính là entity projectile.
  Entity,
  // molotov/incendiary: `inferno_startburn.entityid` là entity của inferno → ghép theo cùng người +
  // đúng loại + tick projectile biến mất sát tick bắt đầu cháy.
  OwnerEnd,
}

fn find_flight(
  flights: &ProjectileFlights,
  ev: &RawEvent,
  types: &[&str],
  det_tick: i64,
  mode: FlightMatch,
  used: &mut HashSet<usize>,
) -> Option<usize> {
  let type_ok = |f: &ProjectileFlight| types.contains(&f.grenade_type.as_str());
  match mode {
    FlightMatch::Entity => {
      let candidates = flights.by_entity.get(&ev.entityid?)?;
      candidates
        .iter()
        .copied()
        .filter(|&i| {
          let f = &flights.flights[i];
          type_ok(f) && f.first_tick() <= det_tick && f.last_tick() >= det_tick - DETONATE_END_SLACK
        })
        .max_by_key(|&i| flights.flights[i].first_tick())
    }
    FlightMatch::OwnerEnd => {
      let sid = ev.user_steamid.as_ref()?;
      let best = flights
        .flights
        .iter()
        .enumerate()
        .filter(|(i, f)| {
          !used.contains(i)
            && f.steamid.as_ref() == Some(sid)
            && type_ok(f)
            && (f.last_tick() - det_tick).abs() <= INFERNO_MATCH_TICKS
        })
        .min_by_key(|(_, f)| (f.last_tick() - det_tick).abs())
        .map(|(i, _)| i)?;
      used.insert(best);
      Some(best)
    }
  }
}

// Đường bay từ lúc spawn tới tick nổ (downsample, luôn giữ điểm cuối).
fn trajectory_of(flight: &ProjectileFlight, det_tick: i64) -> Option<Vec<TrajPoint>> {
  let run: Vec<&RawGrenadeSample> = flight.rows.iter().filter(|r| r.tick.unwrap_or(0) <= det_tick).collect();
  if run.len() < 2 {
    return None;
  }
  let out: Vec<TrajPoint> = run
    .iter()
    .enumerate()
    .filter(|(i, _)| i % GRENADE_DOWNSAMPLE == 0 || *i == run.len() - 1)
    .map(|(_, r)| TrajPoint {
      tick: r.tick.unwrap_or(0),
      x: js_round(r.x.unwrap_or(0.0)),
      y: js_round(r.y.unwrap_or(0.0)),
      z: js_round(r.z.unwrap_or(0.0)),
    })
    .collect();
  Some(out)
}

// GỐC NÉM (vị trí + tick lúc rời tay): weapon_fire đúng loại nade của người ném, muộn nhất trong [lo, hi].
fn throw_origin_for(fire_shots: &[RawEvent], steamid: &Option<String>, weapon_keys: &[&str], lo: i64, hi: i64) -> Option<ThrowOrigin> {
  let sid = steamid.as_ref()?;
  let mut best: Option<ThrowOrigin> = None;
  for ev in fire_shots {
    if ev.user_steamid.as_ref() != Some(sid) || ev.user_x.is_none() {
      continue;
    }
    let w = ev.weapon.clone().unwrap_or_default().to_lowercase();
    if !weapon_keys.iter().any(|k| w.contains(k)) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    if tick < lo || tick > hi {
      continue;
    }
    if best.as_ref().map(|b| tick > b.tick).unwrap_or(true) {
      best = Some(ThrowOrigin { tick, x: ev.user_x.unwrap(), y: ev.user_y.unwrap_or(0.0), z: ev.user_z.unwrap_or(0.0) });
    }
  }
  best
}

// Nối GỐC NÉM vào đầu đường bay khi lúc rời tay sớm hơn sample projectile đầu tiên.
fn prepend_throw_origin(traj: Option<Vec<TrajPoint>>, origin: &Option<ThrowOrigin>) -> Option<Vec<TrajPoint>> {
  let Some(origin) = origin else { return traj };
  let mut traj = match traj {
    Some(t) if !t.is_empty() => t,
    other => return other,
  };
  if origin.tick >= traj[0].tick {
    return Some(traj);
  }
  traj.insert(0, TrajPoint { tick: origin.tick, x: js_round(origin.x), y: js_round(origin.y), z: js_round(origin.z) });
  Some(traj)
}

// Đường bay + gốc ném của 1 event. Có chuyến bay → gốc ném neo theo tick spawn projectile. Không có
// (dữ liệu thiếu entity id) → không có trajectory, gốc ném lấy weapon_fire gần nhất trước lúc nổ để
// frontend vẫn vẽ được vệt fallback tay → điểm rơi.
#[allow(clippy::too_many_arguments)]
fn flight_and_origin(
  flights: &ProjectileFlights,
  ev: &RawEvent,
  det_tick: i64,
  fire_shots: &[RawEvent],
  fire_weapon: &[&str],
  proj: &[&str],
  mode: FlightMatch,
  used: &mut HashSet<usize>,
) -> (Option<Vec<TrajPoint>>, Option<ThrowOrigin>) {
  match find_flight(flights, ev, proj, det_tick, mode, used) {
    Some(i) => {
      let flight = &flights.flights[i];
      let spawn = flight.first_tick();
      let origin = throw_origin_for(fire_shots, &ev.user_steamid, fire_weapon, spawn - THROW_BEFORE_SPAWN_MAX, spawn);
      (prepend_throw_origin(trajectory_of(flight, det_tick), &origin), origin)
    }
    None => (None, throw_origin_for(fire_shots, &ev.user_steamid, fire_weapon, det_tick - THROW_MATCH_MAX_TICKS, det_tick)),
  }
}

// ── Smoke / fire (paired start+expire, with trajectory) ────────────────────────
#[allow(clippy::too_many_arguments)]
pub fn build_grenade_effects(
  event_type: &'static str,
  start_events: &[RawEvent],
  end_events: &[RawEvent],
  fallback_ticks: i64,
  fire_shots: &[RawEvent],
  flights: &ProjectileFlights,
  fire_weapon: &[&str],
  proj: &[&str],
  mode: FlightMatch,
  valid_rounds: &HashSet<i64>,
) -> Vec<ParsedEvent<GrenadeEffectData>> {
  let mut out = Vec::new();
  let mut used = HashSet::new();
  for paired in pair_grenade_effects(start_events, end_events, fallback_ticks) {
    let ev = paired.ev;
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    let (trajectory, origin) = flight_and_origin(flights, ev, tick, fire_shots, fire_weapon, proj, mode, &mut used);
    let data = GrenadeEffectData {
      x: ev.x,
      y: ev.y,
      z: ev.z,
      start_tick: tick,
      end_tick: paired.end_tick,
      thrown_by: ev.user_name.clone(),
      team: side_2_else_ct(ev.user_team_num),
      thrower_x: origin.as_ref().map(|o| o.x).or(ev.user_x),
      thrower_y: origin.as_ref().map(|o| o.y).or(ev.user_y),
      throw_tick: origin.as_ref().map(|o| o.tick),
      trajectory,
    };
    out.push(ParsedEvent { round_number: r_num, tick, r#type: event_type, data });
  }
  out
}

// ── HE / flash (instant) ───────────────────────────────────────────────────────
pub fn build_instant_nade(
  event_type: &'static str,
  det_events: &[RawEvent],
  fire_shots: &[RawEvent],
  flights: &ProjectileFlights,
  fire_weapon: &[&str],
  proj: &[&str],
  valid_rounds: &HashSet<i64>,
) -> Vec<ParsedEvent<InstantNadeData>> {
  let mut out = Vec::new();
  let mut used = HashSet::new();
  for ev in det_events {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    let (trajectory, origin) =
      flight_and_origin(flights, ev, tick, fire_shots, fire_weapon, proj, FlightMatch::Entity, &mut used);
    let data = InstantNadeData {
      x: ev.x,
      y: ev.y,
      z: ev.z,
      thrown_by: ev.user_name.clone(),
      team: side_2_else_ct(ev.user_team_num),
      thrower_x: origin.as_ref().map(|o| o.x).or(ev.user_x),
      thrower_y: origin.as_ref().map(|o| o.y).or(ev.user_y),
      throw_tick: origin.as_ref().map(|o| o.tick),
      trajectory,
    };
    out.push(ParsedEvent { round_number: r_num, tick, r#type: event_type, data });
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  const SID: &str = "765";

  // Chuyến bay giả: sample mỗi tick trong [from, to], x tăng theo tick để phân biệt được từng quả.
  fn flight_rows(eid: i64, ty: &str, from: i64, to: i64, x0: f64) -> Vec<RawGrenadeSample> {
    (from..=to)
      .map(|t| RawGrenadeSample {
        steamid: Some(SID.to_string()),
        tick: Some(t),
        x: Some(x0 + (t - from) as f64),
        y: Some(0.0),
        z: Some(0.0),
        grenade_type: Some(ty.to_string()),
        grenade_entity_id: Some(eid),
      })
      .collect()
  }

  fn det(name: &str, tick: i64, eid: i64) -> RawEvent {
    RawEvent {
      event_name: name.to_string(),
      tick: Some(tick),
      total_rounds_played: Some(0),
      entityid: Some(eid),
      user_steamid: Some(SID.to_string()),
      x: Some(1.0),
      y: Some(0.0),
      z: Some(0.0),
      ..Default::default()
    }
  }

  fn throw(tick: i64, weapon: &str, x: f64) -> RawEvent {
    RawEvent {
      event_name: "weapon_fire".to_string(),
      tick: Some(tick),
      user_steamid: Some(SID.to_string()),
      weapon: Some(weapon.to_string()),
      user_x: Some(x),
      user_y: Some(0.0),
      user_z: Some(0.0),
      ..Default::default()
    }
  }

  fn rounds() -> HashSet<i64> {
    HashSet::from([1])
  }

  #[test]
  fn overlapping_flashes_do_not_mix_and_keep_own_throw() {
    // Flash 1 (entity 10) ném tick 93, bay 100..204; flash 2 (entity 11) ném tick 143, bay 150..254.
    let mut rows = flight_rows(10, "CFlashbangProjectile", 100, 204, 0.0);
    rows.extend(flight_rows(11, "CFlashbangProjectile", 150, 254, 5000.0));
    let flights = build_projectile_flights(&rows);
    let shots = vec![throw(93, "weapon_flashbang", -1.0), throw(143, "weapon_flashbang", -2.0)];
    let dets = vec![det("flashbang_detonate", 204, 10), det("flashbang_detonate", 254, 11)];
    let out = build_instant_nade("FLASH", &dets, &shots, &flights, &["flashbang"], &["CFlashbangProjectile"], &rounds());

    let t1 = out[0].data.trajectory.as_ref().unwrap();
    assert_eq!(out[0].data.throw_tick, Some(93));
    assert_eq!(t1[0].tick, 93);
    assert!(t1[1..].iter().all(|p| p.x < 5000.0), "flash 1 không được lẫn sample của flash 2");

    let t2 = out[1].data.trajectory.as_ref().unwrap();
    assert_eq!(out[1].data.throw_tick, Some(143));
    assert!(t2[1..].iter().all(|p| p.x >= 5000.0));
  }

  #[test]
  fn long_smoke_flight_is_not_truncated() {
    // Smoke qua mái: bay 500 tick (> cửa sổ 384 tick của heuristic cũ).
    let rows = flight_rows(20, "CSmokeGrenadeProjectile", 1000, 1500, 0.0);
    let flights = build_projectile_flights(&rows);
    let shots = vec![throw(993, "weapon_smokegrenade", -1.0)];
    let starts = vec![det("smokegrenade_detonate", 1500, 20)];
    let out = build_grenade_effects(
      "SMOKE", &starts, &[], 1152, &shots, &flights, &["smokegrenade"], &["CSmokeGrenadeProjectile"], FlightMatch::Entity, &rounds(),
    );
    let t = out[0].data.trajectory.as_ref().unwrap();
    assert_eq!(t[0].tick, 993);
    assert_eq!(t[1].tick, 1000, "đường bay phải bắt đầu từ lúc spawn");
    assert_eq!(t.last().unwrap().tick, 1500);
  }

  #[test]
  fn resting_old_smoke_is_not_mixed_into_new_flight() {
    // Smoke cũ (entity 30) nổ tick 300 rồi nằm yên tới 1400; smoke mới (entity 31) bay 500..800.
    let mut rows = flight_rows(30, "CSmokeGrenadeProjectile", 100, 1400, 0.0);
    rows.extend(flight_rows(31, "CSmokeGrenadeProjectile", 500, 800, 9000.0));
    let flights = build_projectile_flights(&rows);
    let shots = vec![throw(93, "weapon_smokegrenade", -1.0), throw(493, "weapon_smokegrenade", -2.0)];
    let starts = vec![det("smokegrenade_detonate", 300, 30), det("smokegrenade_detonate", 800, 31)];
    let out = build_grenade_effects(
      "SMOKE", &starts, &[], 1152, &shots, &flights, &["smokegrenade"], &["CSmokeGrenadeProjectile"], FlightMatch::Entity, &rounds(),
    );
    assert_eq!(out[0].data.throw_tick, Some(93));
    assert_eq!(out[0].data.trajectory.as_ref().unwrap().last().unwrap().tick, 300);
    let t2 = out[1].data.trajectory.as_ref().unwrap();
    assert_eq!(out[1].data.throw_tick, Some(493));
    assert!(t2[1..].iter().all(|p| p.x >= 9000.0));
  }

  #[test]
  fn reused_entity_id_is_split_into_separate_flights() {
    let mut rows = flight_rows(40, "CHEGrenadeProjectile", 100, 200, 0.0);
    rows.extend(flight_rows(40, "CHEGrenadeProjectile", 5000, 5100, 7000.0));
    let flights = build_projectile_flights(&rows);
    assert_eq!(flights.flights.len(), 2);
    let shots = vec![throw(4990, "weapon_hegrenade", -1.0)];
    let dets = vec![det("hegrenade_detonate", 5100, 40)];
    let out = build_instant_nade("HE_EXPLODE", &dets, &shots, &flights, &["hegrenade"], &["CHEGrenadeProjectile"], &rounds());
    let t = out[0].data.trajectory.as_ref().unwrap();
    assert_eq!(t[1].tick, 5000);
    assert!(t[1..].iter().all(|p| p.x >= 7000.0));
  }

  #[test]
  fn inferno_matches_projectile_by_disappear_tick() {
    // inferno_startburn mang entity của inferno (99), không phải projectile (50/51).
    let mut rows = flight_rows(50, "CMolotovProjectile", 100, 220, 0.0);
    rows.extend(flight_rows(51, "CMolotovProjectile", 300, 420, 8000.0));
    let flights = build_projectile_flights(&rows);
    let shots = vec![throw(92, "weapon_molotov", -1.0), throw(292, "weapon_molotov", -2.0)];
    let starts = vec![det("inferno_startburn", 221, 99), det("inferno_startburn", 421, 98)];
    let out = build_grenade_effects(
      "FIRE", &starts, &[], 448, &shots, &flights, &["molotov"], &["CMolotovProjectile"], FlightMatch::OwnerEnd, &rounds(),
    );
    assert_eq!(out[0].data.throw_tick, Some(92));
    assert!(out[0].data.trajectory.as_ref().unwrap()[1..].iter().all(|p| p.x < 8000.0));
    assert_eq!(out[1].data.throw_tick, Some(292));
    assert!(out[1].data.trajectory.as_ref().unwrap()[1..].iter().all(|p| p.x >= 8000.0));
  }

  #[test]
  fn missing_flight_falls_back_to_latest_throw_without_trajectory() {
    let flights = build_projectile_flights(&[]);
    let shots = vec![throw(100, "weapon_flashbang", 42.0)];
    let dets = vec![det("flashbang_detonate", 204, 10)];
    let out = build_instant_nade("FLASH", &dets, &shots, &flights, &["flashbang"], &["CFlashbangProjectile"], &rounds());
    assert!(out[0].data.trajectory.is_none());
    assert_eq!(out[0].data.throw_tick, Some(100));
    assert_eq!(out[0].data.thrower_x, Some(42.0));
  }
}
