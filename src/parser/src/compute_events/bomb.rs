// Port 1:1 của buildBombPlantDefuse (compute.ts L200-226), buildBombDropPickup (L229-265),
// buildWeaponPickup (L277-320).

use super::helpers::side_2_else_ct;
use super::types::{BombDefuseData, BombDroppedData, BombPickupData, BombPlantData, ParsedEvent, ParsedRound, RawEvent, WeaponPickupData};
use std::collections::{HashMap, HashSet};

pub fn build_bomb_plant_defuse(
  bomb_plant_events: &[RawEvent],
  bomb_defuse_events: &[RawEvent],
  valid_rounds: &HashSet<i64>,
  round_start_tick_by_num: &HashMap<i64, i64>,
) -> (Vec<ParsedEvent<BombPlantData>>, Vec<ParsedEvent<BombDefuseData>>) {
  let mut plants = Vec::new();
  for ev in bomb_plant_events {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    if let Some(start_tick) = round_start_tick_by_num.get(&r_num) {
      if tick < *start_tick {
        continue;
      }
    }
    let data = BombPlantData {
      // `ev.X ?? ev.user_X` -- bare X trước, KHÁC thứ tự buildKills (user_X trước). Đừng đảo.
      x: ev.bare_x.or(ev.user_x),
      y: ev.bare_y.or(ev.user_y),
      z: ev.bare_z.or(ev.user_z),
      site: ev.site.clone(),
      planted_by: ev.user_name.clone(),
      team: side_2_else_ct(ev.user_team_num),
    };
    plants.push(ParsedEvent { round_number: r_num, tick, r#type: "BOMB_PLANT", data });
  }

  let mut defuses = Vec::new();
  for ev in bomb_defuse_events {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    if let Some(start_tick) = round_start_tick_by_num.get(&r_num) {
      if tick < *start_tick {
        continue;
      }
    }
    let data = BombDefuseData {
      x: ev.bare_x.or(ev.user_x),
      y: ev.bare_y.or(ev.user_y),
      z: ev.bare_z.or(ev.user_z),
      site: ev.site.clone(),
      defused_by: ev.user_name.clone(),
    };
    defuses.push(ParsedEvent { round_number: r_num, tick, r#type: "BOMB_DEFUSE", data });
  }

  (plants, defuses)
}

struct SettledPos {
  x: f64,
  y: Option<f64>,
  z: Option<f64>,
}

// `Map<any, ...>` keyed theo object reference ở TS -- thay bằng index vào `bomb_dropped_events`
// (mỗi drop event chỉ xuất hiện 1 lần trong mảng gốc, tương đương identity).
//
// GOTCHA: CS2 đôi khi KHÔNG bắn `bomb_pickup` khi nhặt lại C4 đồng đội vứt trong freeze time --
// chỉ có `item_pickup` (item="c4") cùng lúc. Thiếu pickup thì drop bị ghép nhầm với pickup đầu
// round SAU (toạ độ còn stale trước teleport, vd đứng ở site vừa gài) → replay vẽ C4 nằm sẵn ở
// site suốt round. Nên: (1) bù pickup từ `item_pickup` c4 không có `bomb_pickup` cùng tick+người,
// (2) chỉ ghép drop↔pickup khi cùng `total_rounds_played`.
pub fn build_bomb_drop_pickup(
  bomb_dropped_events: &[RawEvent],
  bomb_pickup_events: &[RawEvent],
  item_pickup_events: &[RawEvent],
  valid_rounds: &HashSet<i64>,
) -> (Vec<ParsedEvent<BombDroppedData>>, Vec<ParsedEvent<BombPickupData>>) {
  // ----- START: gộp bomb_pickup + item_pickup c4 bị thiếu bomb_pickup -----
  let have: HashSet<(i64, Option<&str>)> =
    bomb_pickup_events.iter().map(|e| (e.tick.unwrap_or(0), e.user_steamid.as_deref())).collect();
  let mut pickups_all: Vec<RawEvent> = bomb_pickup_events.to_vec();
  for ev in item_pickup_events {
    let is_c4 = ev.item.as_deref().map(|s| s.eq_ignore_ascii_case("c4")).unwrap_or(false);
    if is_c4 && !have.contains(&(ev.tick.unwrap_or(0), ev.user_steamid.as_deref())) {
      pickups_all.push(ev.clone());
    }
  }
  pickups_all.sort_by_key(|e| e.tick.unwrap_or(0));
  let bomb_pickup_events = &pickups_all[..];
  // ----- END: gộp pickup -----

  enum Kind {
    Drop(usize),
    Pickup(usize),
  }
  let mut chrono: Vec<(i64, Kind)> = Vec::new();
  for (i, ev) in bomb_dropped_events.iter().enumerate() {
    chrono.push((ev.tick.unwrap_or(0), Kind::Drop(i)));
  }
  for (i, ev) in bomb_pickup_events.iter().enumerate() {
    chrono.push((ev.tick.unwrap_or(0), Kind::Pickup(i)));
  }
  // Rust's sort_by_key is stable (như Array.prototype.sort từ ES2019) -- tie giữ đúng thứ tự
  // push ban đầu (mọi drop trước mọi pickup, giống spread [...drop.map, ...pickup.map] của TS).
  chrono.sort_by_key(|(t, _)| *t);

  let mut settled: HashMap<usize, SettledPos> = HashMap::new();
  for i in 0..chrono.len() {
    if let Kind::Drop(drop_idx) = chrono[i].1 {
      if let Some((_, Kind::Pickup(pickup_idx))) = chrono.get(i + 1) {
        let next = &bomb_pickup_events[*pickup_idx];
        // Khác round → pickup đầu round sau (toạ độ stale), không phải chỗ bom rơi.
        let same_round = next.total_rounds_played == bomb_dropped_events[drop_idx].total_rounds_played;
        if let (true, Some(ux)) = (same_round, next.user_x) {
          settled.insert(drop_idx, SettledPos { x: ux, y: next.user_y, z: next.user_z });
        }
      }
    }
  }

  let mut dropped_out = Vec::new();
  for (i, ev) in bomb_dropped_events.iter().enumerate() {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) || ev.user_x.is_none() {
      continue;
    }
    let s = settled.get(&i);
    let data = BombDroppedData {
      x: s.map(|s| s.x).unwrap_or_else(|| ev.user_x.unwrap()),
      y: s.and_then(|s| s.y).or(ev.user_y).unwrap_or(0.0),
      z: s.and_then(|s| s.z).or(ev.user_z).unwrap_or(0.0),
      dropped_by_steam_id: ev.user_steamid.clone(),
      dropped_by_name: ev.user_name.clone(),
    };
    dropped_out.push(ParsedEvent { round_number: r_num, tick: ev.tick.unwrap_or(0), r#type: "BOMB_DROPPED", data });
  }

  let mut pickup_out = Vec::new();
  for ev in bomb_pickup_events {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) || ev.user_x.is_none() {
      continue;
    }
    let data = BombPickupData {
      x: ev.user_x.unwrap(),
      y: ev.user_y.unwrap_or(0.0),
      z: ev.user_z.unwrap_or(0.0),
      picked_up_by_steam_id: ev.user_steamid.clone(),
      picked_up_by_name: ev.user_name.clone(),
    };
    pickup_out.push(ParsedEvent { round_number: r_num, tick: ev.tick.unwrap_or(0), r#type: "BOMB_PICKUP", data });
  }

  (dropped_out, pickup_out)
}

fn find_round_idx(ranges: &[&ParsedRound], tick: i64) -> Option<usize> {
  let mut lo: i64 = 0;
  let mut hi: i64 = ranges.len() as i64 - 1;
  let mut found: i64 = -1;
  while lo <= hi {
    let mid = (lo + hi) >> 1;
    if ranges[mid as usize].start_tick <= tick {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  if found < 0 {
    return None;
  }
  let r = ranges[found as usize];
  if tick <= r.end_tick {
    Some(found as usize)
  } else {
    None
  }
}

// GOTCHA (đã ghi ở compute.ts L268-276, giữ nguyên): bucket theo `total_rounds_played + 1` KHÔNG
// đủ tin cậy ở đây -- warmup/knife-round có thể lặp lại total_rounds_played=0 trước round 1 thật,
// làm buytime_ended của warmup lẫn vào round 1. Dùng binary-search theo TICK RANGE thật của từng
// round instance (giống buildReplayChunks) thay vì số round. `buytime_ended` cũng có thể bắn
// nhiều lần trong CÙNG 1 round -- lấy mốc CUỐI CÙNG (không phải đầu tiên) làm cutoff.
//
// `kill_ticks`: tick mọi KILL hợp lệ. Lọc thứ 2 (chính): bỏ mọi pickup TRƯỚC kill đầu tiên của
// round. Đo trên 10 trận thật, lọc buytime_ended gần như không ăn (~80% pickup còn lại là đồ
// mua/spawn: nade, glock, hkp2000 trong 20s đầu round). Consumer duy nhất (FE useRoundEvents)
// chỉ ghép pickup với vũ khí RƠI TỪ KILL (`p.tick >= dropTick`) → pickup trước kill đầu không bao
// giờ khớp được gì, bỏ không mất thông tin.
pub fn build_weapon_pickup(
  item_pickup_events: &[RawEvent],
  buytime_ended_events: &[RawEvent],
  kill_ticks: &[i64],
  rounds: &[ParsedRound],
) -> Vec<ParsedEvent<WeaponPickupData>> {
  if rounds.is_empty() {
    return Vec::new();
  }
  let mut ranges: Vec<&ParsedRound> = rounds.iter().collect();
  ranges.sort_by_key(|r| r.start_tick);

  let mut buy_end_by_round: HashMap<usize, i64> = HashMap::new();
  for ev in buytime_ended_events {
    let tick = ev.tick.unwrap_or(0);
    let Some(idx) = find_round_idx(&ranges, tick) else { continue };
    let existing = buy_end_by_round.get(&idx).copied();
    if existing.is_none() || tick > existing.unwrap() {
      buy_end_by_round.insert(idx, tick);
    }
  }

  // ----- START: tick KILL đầu tiên theo từng round instance (cùng cách bucket theo tick range) -----
  let mut first_kill_by_round: HashMap<usize, i64> = HashMap::new();
  for &tick in kill_ticks {
    let Some(idx) = find_round_idx(&ranges, tick) else { continue };
    let e = first_kill_by_round.entry(idx).or_insert(tick);
    if tick < *e {
      *e = tick;
    }
  }
  // ----- END: tick KILL đầu tiên -----

  let pickup_exclude = super::constants::pickup_exclude();
  let mut out = Vec::new();
  for ev in item_pickup_events {
    if ev.user_x.is_none() {
      continue;
    }
    let tick = ev.tick.unwrap_or(0);
    let Some(idx) = find_round_idx(&ranges, tick) else { continue };
    let item = ev.item.clone().unwrap_or_default().to_lowercase();
    if item.is_empty() || pickup_exclude.iter().any(|k| item.contains(k)) {
      continue;
    }
    if let Some(buy_end_tick) = buy_end_by_round.get(&idx) {
      if tick <= *buy_end_tick {
        continue; // trong giờ mua → không phải nhặt thật
      }
    }
    // Round chưa có kill (hoặc pickup trước kill đầu) → chưa có vũ khí nào rơi từ KILL để nhặt.
    match first_kill_by_round.get(&idx) {
      Some(first_kill) if tick >= *first_kill => {}
      _ => continue,
    }
    let r = ranges[idx];
    let data = WeaponPickupData {
      x: ev.user_x.unwrap(),
      y: ev.user_y.unwrap_or(0.0),
      z: ev.user_z,
      weapon: ev.item.clone(),
      picked_up_by_steam_id: ev.user_steamid.clone(),
      picked_up_by_name: ev.user_name.clone(),
    };
    out.push(ParsedEvent { round_number: r.round_number, tick, r#type: "WEAPON_PICKUP", data });
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  fn round(n: i64, start: i64, end: i64) -> ParsedRound {
    ParsedRound { round_number: n, winner_side: "CT".into(), reason: String::new(), t_score: 0, ct_score: 0, start_tick: start, end_tick: end }
  }

  fn pickup(tick: i64, item: &str) -> RawEvent {
    RawEvent {
      event_name: "item_pickup".to_string(),
      tick: Some(tick),
      item: Some(item.to_string()),
      user_x: Some(0.0),
      user_y: Some(0.0),
      user_steamid: Some("765".to_string()),
      ..Default::default()
    }
  }

  // Pickup trước kill đầu tiên của round (đồ mua/spawn) bị bỏ; sau kill thì giữ; round không có
  // kill thì bỏ hết. Bucket theo tick range nên kill round 1 không mở khoá pickup round 2.
  #[test]
  fn weapon_pickup_only_after_first_kill_of_round() {
    let rounds = vec![round(1, 0, 1000), round(2, 1001, 2000), round(3, 2001, 3000)];
    let pickups = vec![
      pickup(100, "flashbang"), // round 1, trước kill đầu (500)
      pickup(500, "ak47"),      // round 1, cùng tick kill → giữ
      pickup(800, "awp"),       // round 1, sau kill → giữ
      pickup(1200, "glock"),    // round 2, trước kill đầu (1500)
      pickup(1600, "m4a1"),     // round 2, sau kill → giữ
      pickup(2500, "deagle"),   // round 3 không có kill → bỏ
    ];
    let out = build_weapon_pickup(&pickups, &[], &[700, 500, 1500], &rounds);
    let got: Vec<(i64, i64)> = out.iter().map(|e| (e.round_number, e.tick)).collect();
    assert_eq!(got, vec![(1, 500), (1, 800), (2, 1600)]);
  }
}
