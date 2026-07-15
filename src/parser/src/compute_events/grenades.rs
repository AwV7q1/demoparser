// Port 1:1 của helpers.ts (pairGrenadeEffects, buildGrenadeTrajectoryIndex, flightFor,
// throwOriginFor, prependThrowOrigin) + compute.ts's buildGrenadeEffects (L323-346) /
// buildInstantNade (L349-369). Đây là phần rủi ro nhất của "events" domain (heuristic ghép
// trajectory) -- giữ nguyên từng bước, không "tối ưu" logic khi port.

use super::constants::{GRENADE_MAX_FLIGHT_TICKS, GRENADE_RUN_GAP, GRENADE_SOLO_MAX_GAP};
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
// Key HashMap<Option<i64>, _> mirror đúng `Map<number, ...>` của JS khi entityid có thể undefined
// (JS Map cho phép key undefined, JSON demoparser2 gần như luôn có entityid nên nhánh None hiếm
// khi trúng, nhưng giữ đúng type để không âm thầm gộp sai nhóm).
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

// Index vị trí grenade theo steamid người ném (sort tăng theo tick). Rows đầu vào ĐÃ được lọc
// x/y/z != null bởi caller (tương đương `parser.parseGrenades().filter(...)` ở helpers.ts).
pub fn build_grenade_trajectory_index(rows: &[RawGrenadeSample]) -> HashMap<String, Vec<RawGrenadeSample>> {
  let mut by_player: HashMap<String, Vec<RawGrenadeSample>> = HashMap::new();
  for r in rows {
    // `String(r.steamid)` ở JS cho ra chuỗi "undefined" khi thiếu steamid (không phải rỗng) --
    // giữ đúng quirk này dù thực tế gần như không bao giờ trúng trên demo thật.
    let sid = r.steamid.clone().unwrap_or_else(|| "undefined".to_string());
    by_player.entry(sid).or_default().push(r.clone());
  }
  for arr in by_player.values_mut() {
    arr.sort_by_key(|r| r.tick.unwrap_or(0));
  }
  by_player
}

// Đường bay 1 quả nade ứng với 1 detonate: cùng người ném + đúng loại + tick <= detTick trong
// cửa sổ. `max_gap` KHÔNG có default ở Rust (khác TS `= GRENADE_RUN_GAP`) -- caller phải truyền
// tường minh (buildInstantNade truyền GRENADE_RUN_GAP, buildGrenadeEffects truyền
// GRENADE_SOLO_MAX_GAP -- xem 2 call site dưới).
fn flight_for(
  index: &HashMap<String, Vec<RawGrenadeSample>>,
  steamid: &Option<String>,
  types: &[&str],
  det_tick: i64,
  max_gap: i64,
) -> Option<Vec<TrajPoint>> {
  let sid = steamid.as_ref()?;
  let empty: Vec<RawGrenadeSample> = Vec::new();
  let all_rows = index.get(sid).unwrap_or(&empty);
  let rows: Vec<&RawGrenadeSample> = all_rows
    .iter()
    .filter(|r| {
      let t = r.tick.unwrap_or(0);
      let type_ok = r.grenade_type.as_deref().map(|gt| types.contains(&gt)).unwrap_or(false);
      type_ok && t <= det_tick && t >= det_tick - GRENADE_MAX_FLIGHT_TICKS
    })
    .collect();
  if rows.len() < 2 {
    return None;
  }

  let mut start = rows.len() - 1;
  while start > 0 && rows[start].tick.unwrap_or(0) - rows[start - 1].tick.unwrap_or(0) <= max_gap {
    start -= 1;
  }
  let run = &rows[start..];
  if run.len() < 2 {
    return None;
  }

  let downsample = super::constants::GRENADE_DOWNSAMPLE;
  let out: Vec<TrajPoint> = run
    .iter()
    .enumerate()
    .filter(|(i, _)| i % downsample == 0 || *i == run.len() - 1)
    .map(|(_, r)| TrajPoint {
      tick: r.tick.unwrap_or(0),
      x: js_round(r.x.unwrap_or(0.0)),
      y: js_round(r.y.unwrap_or(0.0)),
      z: js_round(r.z.unwrap_or(0.0)),
    })
    .collect();
  Some(out)
}

// GỐC NÉM (vị trí + tick lúc rời tay) từ weapon_fire của đúng loại nade -- lần bắn gần nhất
// TRƯỚC mốc nổ.
fn throw_origin_for(
  fire_shots: &[RawEvent],
  steamid: &Option<String>,
  det_tick: i64,
  weapon_keys: &[&str],
) -> Option<ThrowOrigin> {
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
    let back = det_tick - tick;
    if back < 0 || back > super::constants::THROW_MATCH_MAX_TICKS {
      continue;
    }
    if best.as_ref().map(|b| tick > b.tick).unwrap_or(true) {
      best = Some(ThrowOrigin { tick, x: ev.user_x.unwrap(), y: ev.user_y.unwrap_or(0.0), z: ev.user_z.unwrap_or(0.0) });
    }
  }
  best
}

// Nối GỐC NÉM vào đầu đường bay. GOTCHA (đã ghi ở helpers.ts, giữ nguyên): nối bất cứ khi nào
// `origin.tick < first.tick` (tức CÓ trễ), BẤT KỂ gap khoảng cách bao nhiêu -- KHÔNG chỉ khi gap
// "lớn" (bug cũ từng chỉ nối khi gap >= 64, bỏ sót case bay bình thường gap nhỏ).
fn prepend_throw_origin(traj: Option<Vec<TrajPoint>>, origin: &Option<ThrowOrigin>) -> Option<Vec<TrajPoint>> {
  let Some(origin) = origin else { return traj };
  let is_nonempty = matches!(&traj, Some(t) if !t.is_empty());
  if !is_nonempty {
    return traj;
  }
  let mut traj = traj.unwrap();
  let first_tick = traj[0].tick;
  if origin.tick >= first_tick {
    return Some(traj);
  }
  let mut out = Vec::with_capacity(traj.len() + 1);
  out.push(TrajPoint { tick: origin.tick, x: js_round(origin.x), y: js_round(origin.y), z: js_round(origin.z) });
  out.append(&mut traj);
  Some(out)
}

// ── Smoke / fire (paired start+expire, with trajectory) ────────────────────────
#[allow(clippy::too_many_arguments)]
pub fn build_grenade_effects(
  event_type: &'static str,
  start_events: &[RawEvent],
  end_events: &[RawEvent],
  fallback_ticks: i64,
  fire_shots: &[RawEvent],
  grenade_traj: &HashMap<String, Vec<RawGrenadeSample>>,
  fire_weapon: &[&str],
  proj: &[&str],
  valid_rounds: &HashSet<i64>,
) -> Vec<ParsedEvent<GrenadeEffectData>> {
  let mut out = Vec::new();
  for paired in pair_grenade_effects(start_events, end_events, fallback_ticks) {
    let ev = paired.ev;
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    let origin = throw_origin_for(fire_shots, &ev.user_steamid, tick, fire_weapon);
    let trajectory = prepend_throw_origin(
      flight_for(grenade_traj, &ev.user_steamid, proj, tick, GRENADE_SOLO_MAX_GAP),
      &origin,
    );
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

// ── HE / flash (instant, trajectory maxGap mặc định GRENADE_RUN_GAP) ───────────
pub fn build_instant_nade(
  event_type: &'static str,
  det_events: &[RawEvent],
  fire_shots: &[RawEvent],
  grenade_traj: &HashMap<String, Vec<RawGrenadeSample>>,
  fire_weapon: &[&str],
  proj: &[&str],
  valid_rounds: &HashSet<i64>,
) -> Vec<ParsedEvent<InstantNadeData>> {
  let mut out = Vec::new();
  for ev in det_events {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    let origin = throw_origin_for(fire_shots, &ev.user_steamid, tick, fire_weapon);
    let trajectory = prepend_throw_origin(
      flight_for(grenade_traj, &ev.user_steamid, proj, tick, GRENADE_RUN_GAP),
      &origin,
    );
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
