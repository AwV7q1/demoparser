// Port of computeTickAggregates (compute.ts L749-836).
//
// GOTCHA: the survivor/economy loop walks a COPY of tick_rows sorted by tick (`rows` in the JS),
// but the zone loop right after walks the ORIGINAL tick_rows in the order the caller passed them
// in -- two different iteration orders in the same JS function, both of which matter here because
// output array order (OrderedMap insertion order) is part of what the parity harness checks.

use super::ordered_map::OrderedMap;
use super::types::{KillsBatchItem, PlayerZoneStat, RoundEconomyStat, RoundSurvivorStat, TickRow};
use super::constants::{BUY_WINDOW, ROSTER_WINDOW, SURVIVOR_WINDOW};
use crate::compute_events::ParsedRound;
use std::collections::HashMap;

#[derive(Default)]
struct DeathEquip {
  dist: i64,
  equip: f64,
}

#[derive(Default)]
struct PlayerRoundAcc {
  roster_side: String,
  any_side: String,
  has_buy_sample: bool,
  equip_buy: f64,
  money_end: f64,
  last_alive: Option<bool>,
  death_equip: Option<DeathEquip>,
}

fn lower_bound(rows: &[TickRow], v: i64) -> usize {
  let (mut lo, mut hi) = (0usize, rows.len());
  while lo < hi {
    let mid = (lo + hi) / 2;
    if rows[mid].tick < v {
      lo = mid + 1;
    } else {
      hi = mid;
    }
  }
  lo
}

pub fn compute_tick_aggregates(
  tick_rows: &[TickRow],
  kills_batch: &[KillsBatchItem],
  rounds: &[ParsedRound],
) -> (Vec<RoundSurvivorStat>, Vec<PlayerZoneStat>, Vec<RoundEconomyStat>) {
  if tick_rows.is_empty() {
    return (vec![], vec![], vec![]);
  }

  // Survivor/economy walk a COPY sorted by tick; the zone loop below walks `tick_rows` itself,
  // in the caller's original order (see header gotcha comment).
  let mut rows_owned: Vec<TickRow> = tick_rows.to_vec();
  rows_owned.sort_by_key(|r| r.tick);

  let mut death_tick_of: HashMap<String, i64> = HashMap::new();
  for k in kills_batch {
    if let Some(victim) = &k.data.victim_steam_id {
      let key = format!("{}|{}", k.round_number, victim);
      death_tick_of.entry(key).or_insert(k.tick);
    }
  }

  let mut survivor_rows: Vec<RoundSurvivorStat> = Vec::new();
  let mut economy_rows: Vec<RoundEconomyStat> = Vec::new();

  for r in rounds {
    let buy_hi_candidate = if r.end_tick > r.start_tick { r.end_tick } else { r.start_tick + BUY_WINDOW };
    let buy_hi = (r.start_tick + BUY_WINDOW).min(buy_hi_candidate);
    let surv_lo = r.end_tick - SURVIVOR_WINDOW;

    let mut acc: OrderedMap<String, PlayerRoundAcc> = OrderedMap::new();
    let mut i = lower_bound(&rows_owned, r.start_tick);
    while i < rows_owned.len() && rows_owned[i].tick <= r.end_tick {
      let t = &rows_owned[i];
      i += 1;
      if t.steam_id.is_empty() {
        continue;
      }
      let p = acc.entry_or_insert_with(t.steam_id.clone(), PlayerRoundAcc::default);
      if let Some(side) = &t.side {
        if p.any_side.is_empty() {
          p.any_side = side.clone();
        }
        if p.roster_side.is_empty() && t.is_alive && t.tick <= r.start_tick + ROSTER_WINDOW {
          p.roster_side = side.clone();
        }
      }
      if t.side.is_some() && t.tick <= buy_hi {
        let equip = t.equip_value.unwrap_or(0.0);
        let money = t.money.unwrap_or(0.0);
        if !p.has_buy_sample {
          p.has_buy_sample = true;
          p.equip_buy = equip;
          p.money_end = money;
        } else {
          if equip > p.equip_buy {
            p.equip_buy = equip;
          }
          if money < p.money_end {
            p.money_end = money;
          }
        }
      }
      if t.tick >= surv_lo {
        p.last_alive = Some(t.is_alive);
      }
      if let Some(dt) = death_tick_of.get(&format!("{}|{}", r.round_number, t.steam_id)) {
        if let Some(equip_value) = t.equip_value {
          let dist = (t.tick - dt).abs();
          let better = match &p.death_equip {
            None => true,
            Some(d) => dist < d.dist,
          };
          if better {
            p.death_equip = Some(DeathEquip { dist, equip: equip_value });
          }
        }
      }
    }

    for (steam_id, p) in acc.into_entries() {
      if p.last_alive.is_some() || !p.roster_side.is_empty() {
        survivor_rows.push(RoundSurvivorStat {
          round_number: r.round_number,
          steam_id: steam_id.clone(),
          alive: p.last_alive.unwrap_or(false),
          side: p.roster_side.clone(),
        });
      }
      let side = if !p.roster_side.is_empty() { p.roster_side.clone() } else { p.any_side.clone() };
      if !side.is_empty() && p.has_buy_sample {
        economy_rows.push(RoundEconomyStat {
          round_number: r.round_number,
          steam_id,
          side,
          equip_buy: p.equip_buy,
          money_end: p.money_end,
          equip_at_death: p.death_equip.map(|d| d.equip),
        });
      }
    }
  }

  #[derive(Default)]
  struct ZoneAcc {
    steam_id: String,
    side: String,
    place: String,
    alive_count: i64,
    all_count: i64,
    sum_x: f64,
    sum_y: f64,
  }
  let mut zone_acc: OrderedMap<String, ZoneAcc> = OrderedMap::new();
  for t in tick_rows {
    if t.steam_id.is_empty() || t.last_place.is_none() {
      continue;
    }
    let side = t.side.clone().unwrap_or_default();
    let place = t.last_place.clone().unwrap();
    let key = format!("{}|{}|{}", t.steam_id, side, place);
    let z = zone_acc.entry_or_insert_with(key, || ZoneAcc {
      steam_id: t.steam_id.clone(),
      side: side.clone(),
      place: place.clone(),
      alive_count: 0,
      all_count: 0,
      sum_x: 0.0,
      sum_y: 0.0,
    });
    z.all_count += 1;
    z.sum_x += t.x;
    z.sum_y += t.y;
    if t.is_alive {
      z.alive_count += 1;
    }
  }
  let zone_rows: Vec<PlayerZoneStat> = zone_acc
    .into_values()
    .into_iter()
    .map(|z| PlayerZoneStat { steam_id: z.steam_id, side: z.side, place: z.place, alive_count: z.alive_count, all_count: z.all_count, sum_x: z.sum_x, sum_y: z.sum_y })
    .collect();

  (survivor_rows, zone_rows, economy_rows)
}
