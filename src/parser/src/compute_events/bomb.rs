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
pub fn build_weapon_pickup(
  item_pickup_events: &[RawEvent],
  buytime_ended_events: &[RawEvent],
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
