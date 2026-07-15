// Port of computeWeaponAccuracyStats (compute.ts L455-511). Uses OrderedMap (not HashMap/BTreeMap)
// because matchWeaponStats/playerAccuracyStats are output ARRAYS built by iterating a JS `Map` in
// insertion order -- see ordered_map.rs header comment.

use super::ordered_map::OrderedMap;
use super::types::{HitDetail, HurtBatchItem, KillsBatchItem, MatchWeaponStat, PlayerAccuracyStat, WeaponFireBatchItem};
use crate::compute_events::{hitgroup_to_int, norm_weapon_name};
use std::collections::BTreeMap;

struct WeaponAgg {
  weapon: String,
  kills: i64,
  hs_kills: i64,
  shots: i64,
  hits: i64,
  damage: f64,
}

struct PlayerAcc {
  shots: i64,
  hits: i64,
  hitgroups: BTreeMap<i64, i64>,
  hits_detail: Vec<HitDetail>,
}

pub fn compute_weapon_accuracy_stats(
  kills_batch: &[KillsBatchItem],
  weapon_fire_batch: &[WeaponFireBatchItem],
  hurt_batch: &[HurtBatchItem],
) -> (Vec<MatchWeaponStat>, Vec<PlayerAccuracyStat>) {
  let mut weapon_map: OrderedMap<String, WeaponAgg> = OrderedMap::new();
  let mut acc_map: OrderedMap<String, PlayerAcc> = OrderedMap::new();

  for k in kills_batch {
    let d = &k.data;
    if d.suicide {
      continue;
    }
    let w = norm_weapon_name(&d.weapon);
    if w.is_empty() {
      continue;
    }
    let s = weapon_map.entry_or_insert_with(w.clone(), || WeaponAgg { weapon: w, kills: 0, hs_kills: 0, shots: 0, hits: 0, damage: 0.0 });
    s.kills += 1;
    if d.headshot {
      s.hs_kills += 1;
    }
  }

  for f in weapon_fire_batch {
    let d = &f.data;
    let w = norm_weapon_name(&d.weapon);
    if !w.is_empty() {
      let s = weapon_map.entry_or_insert_with(w.clone(), || WeaponAgg { weapon: w, kills: 0, hs_kills: 0, shots: 0, hits: 0, damage: 0.0 });
      s.shots += 1;
    }
    if let Some(sid) = &d.steam_id {
      let p = acc_map.entry_or_insert_with(sid.clone(), || PlayerAcc { shots: 0, hits: 0, hitgroups: BTreeMap::new(), hits_detail: vec![] });
      p.shots += 1;
    }
  }

  for h in hurt_batch {
    let d = &h.data;
    let w = norm_weapon_name(&d.weapon);
    if !w.is_empty() {
      let s = weapon_map.entry_or_insert_with(w.clone(), || WeaponAgg { weapon: w, kills: 0, hs_kills: 0, shots: 0, hits: 0, damage: 0.0 });
      s.hits += 1;
      s.damage += d.dmg_health;
    }
    if let Some(sid) = &d.attacker_steam_id {
      if Some(sid) != d.victim_steam_id.as_ref() {
        let p = acc_map.entry_or_insert_with(sid.clone(), || PlayerAcc { shots: 0, hits: 0, hitgroups: BTreeMap::new(), hits_detail: vec![] });
        p.hits += 1;
        let hg = hitgroup_to_int(&d.hitgroup);
        if hg != 0 {
          *p.hitgroups.entry(hg).or_insert(0) += 1;
        }
        p.hits_detail.push(HitDetail { hitgroup: hg, tick: h.tick });
      }
    }
  }

  let match_weapon_stats: Vec<MatchWeaponStat> = weapon_map
    .into_values()
    .into_iter()
    .filter(|s| s.kills > 0 || s.shots > 0 || s.damage > 0.0)
    .map(|s| MatchWeaponStat { weapon: s.weapon, kills: s.kills, hs_kills: s.hs_kills, shots: s.shots, hits: s.hits, damage: s.damage })
    .collect();

  let player_accuracy_stats: Vec<PlayerAccuracyStat> = acc_map
    .into_entries()
    .into_iter()
    .map(|(steam_id, mut p)| {
      p.hits_detail.sort_by_key(|h| h.tick);
      PlayerAccuracyStat { steam_id, shots: p.shots, hits: p.hits, hitgroups: p.hitgroups, hits_detail: p.hits_detail }
    })
    .collect();

  (match_weapon_stats, player_accuracy_stats)
}
