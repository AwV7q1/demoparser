// Port of computeWeaponAccuracyStats (compute.ts). Uses OrderedMap (not HashMap/BTreeMap) because
// matchWeaponStats/playerWeaponStats/playerAccuracyStats are output ARRAYS built by iterating a JS
// `Map` in insertion order -- see ordered_map.rs header comment.
//
// Accuracy người chơi chỉ tính SÚNG ở cả 2 vế: shots đã lọc non_gun lúc build WEAPON_FIRE → hits/
// hitgroups/hitsDetail cũng bỏ HE/dao (is_gun_weapon). Dòng per-weapon (match + player) vẫn giữ
// hits/damage của HE/dao vì damage theo vũ khí vẫn có nghĩa.

use super::ordered_map::OrderedMap;
use super::types::{HitDetail, HurtBatchItem, KillsBatchItem, MatchWeaponStat, PlayerAccuracyStat, PlayerWeaponStat, WeaponFireBatchItem};
use crate::compute_events::{hitgroup_to_int, is_gun_weapon, norm_weapon_name};
use std::collections::BTreeMap;

struct WeaponAgg {
  weapon: String,
  kills: i64,
  hs_kills: i64,
  shots: i64,
  hits: i64,
  damage: f64,
}

// Như WeaponAgg nhưng theo (steamId, weapon) -- nuôi PlayerWeaponStat (main weapon per-player).
struct PlayerWeaponAgg {
  steam_id: String,
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

fn ensure_weapon<'a>(m: &'a mut OrderedMap<String, WeaponAgg>, w: &str) -> &'a mut WeaponAgg {
  m.entry_or_insert_with(w.to_string(), || WeaponAgg { weapon: w.to_string(), kills: 0, hs_kills: 0, shots: 0, hits: 0, damage: 0.0 })
}

fn ensure_player_weapon<'a>(m: &'a mut OrderedMap<String, PlayerWeaponAgg>, sid: &str, w: &str) -> &'a mut PlayerWeaponAgg {
  m.entry_or_insert_with(format!("{sid}|{w}"), || PlayerWeaponAgg {
    steam_id: sid.to_string(),
    weapon: w.to_string(),
    kills: 0,
    hs_kills: 0,
    shots: 0,
    hits: 0,
    damage: 0.0,
  })
}

fn ensure_acc<'a>(m: &'a mut OrderedMap<String, PlayerAcc>, sid: &str) -> &'a mut PlayerAcc {
  m.entry_or_insert_with(sid.to_string(), || PlayerAcc { shots: 0, hits: 0, hitgroups: BTreeMap::new(), hits_detail: vec![] })
}

// JS truthiness của steamId: undefined/null/"" đều bỏ.
fn non_empty(s: &Option<String>) -> Option<&str> {
  s.as_deref().filter(|s| !s.is_empty())
}

pub fn compute_weapon_accuracy_stats(
  kills_batch: &[KillsBatchItem],
  weapon_fire_batch: &[WeaponFireBatchItem],
  hurt_batch: &[HurtBatchItem],
) -> (Vec<MatchWeaponStat>, Vec<PlayerWeaponStat>, Vec<PlayerAccuracyStat>) {
  let mut weapon_map: OrderedMap<String, WeaponAgg> = OrderedMap::new();
  let mut player_weapon_map: OrderedMap<String, PlayerWeaponAgg> = OrderedMap::new();
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
    let s = ensure_weapon(&mut weapon_map, &w);
    s.kills += 1;
    if d.headshot {
      s.hs_kills += 1;
    }
    if let Some(sid) = non_empty(&d.attacker_steam_id) {
      let p = ensure_player_weapon(&mut player_weapon_map, sid, &w);
      p.kills += 1;
      if d.headshot {
        p.hs_kills += 1;
      }
    }
  }

  for f in weapon_fire_batch {
    let d = &f.data;
    let w = norm_weapon_name(&d.weapon);
    if !w.is_empty() {
      ensure_weapon(&mut weapon_map, &w).shots += 1;
    }
    if let Some(sid) = non_empty(&d.steam_id) {
      ensure_acc(&mut acc_map, sid).shots += 1;
      if !w.is_empty() {
        ensure_player_weapon(&mut player_weapon_map, sid, &w).shots += 1;
      }
    }
  }

  for h in hurt_batch {
    let d = &h.data;
    let w = norm_weapon_name(&d.weapon);
    if !w.is_empty() {
      let s = ensure_weapon(&mut weapon_map, &w);
      s.hits += 1;
      s.damage += d.dmg_health;
    }
    if let Some(sid) = non_empty(&d.attacker_steam_id) {
      if Some(sid) != d.victim_steam_id.as_deref() {
        if is_gun_weapon(d.weapon.as_deref().unwrap_or("")) {
          let p = ensure_acc(&mut acc_map, sid);
          p.hits += 1;
          let hg = hitgroup_to_int(&d.hitgroup);
          if hg != 0 {
            *p.hitgroups.entry(hg).or_insert(0) += 1;
          }
          p.hits_detail.push(HitDetail { hitgroup: hg, tick: h.tick });
        }
        if !w.is_empty() {
          let pw = ensure_player_weapon(&mut player_weapon_map, sid, &w);
          pw.hits += 1;
          pw.damage += d.dmg_health;
        }
      }
    }
  }

  let match_weapon_stats: Vec<MatchWeaponStat> = weapon_map
    .into_values()
    .into_iter()
    .filter(|s| s.kills > 0 || s.shots > 0 || s.damage > 0.0)
    .map(|s| MatchWeaponStat { weapon: s.weapon, kills: s.kills, hs_kills: s.hs_kills, shots: s.shots, hits: s.hits, damage: s.damage })
    .collect();

  let player_weapon_stats: Vec<PlayerWeaponStat> = player_weapon_map
    .into_values()
    .into_iter()
    .filter(|s| s.kills > 0 || s.shots > 0 || s.damage > 0.0)
    .map(|s| PlayerWeaponStat {
      steam_id: s.steam_id,
      weapon: s.weapon,
      kills: s.kills,
      hs_kills: s.hs_kills,
      shots: s.shots,
      hits: s.hits,
      damage: s.damage,
    })
    .collect();

  let player_accuracy_stats: Vec<PlayerAccuracyStat> = acc_map
    .into_entries()
    .into_iter()
    .map(|(steam_id, mut p)| {
      p.hits_detail.sort_by_key(|h| h.tick);
      PlayerAccuracyStat { steam_id, shots: p.shots, hits: p.hits, hitgroups: p.hitgroups, hits_detail: p.hits_detail }
    })
    .collect();

  (match_weapon_stats, player_weapon_stats, player_accuracy_stats)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::compute_events::{HurtData, WeaponFireData};
  use serde_json::json;

  fn fire(sid: &str, weapon: &str) -> WeaponFireBatchItem {
    let data: WeaponFireData = serde_json::from_value(json!({ "x": 0.0, "y": 0.0, "yaw": 0.0, "steamId": sid, "weapon": weapon })).unwrap();
    WeaponFireBatchItem { round_number: 1, tick: 10, data }
  }
  fn hurt(att: &str, vic: &str, weapon: &str, hg: &str, dmg: f64) -> HurtBatchItem {
    let data: HurtData = serde_json::from_value(json!({
      "attackerX": 0.0, "attackerY": 0.0, "x": 0.0, "y": 0.0, "weapon": weapon, "dmgHealth": dmg,
      "hitgroup": hg, "attackerSteamId": att, "victimSteamId": vic,
    }))
    .unwrap();
    HurtBatchItem { round_number: 1, tick: 10, data }
  }

  #[test]
  fn accuracy_counts_guns_only_but_weapon_rows_keep_he_damage() {
    let fires = vec![fire("A", "weapon_ak47"), fire("A", "weapon_ak47")];
    let hurts = vec![hurt("A", "B", "ak47", "head", 100.0), hurt("A", "B", "hegrenade", "generic", 40.0), hurt("A", "B", "knife", "chest", 25.0)];
    let kills: Vec<KillsBatchItem> = vec![];
    let (mw, pw, acc) = compute_weapon_accuracy_stats(&kills, &fires, &hurts);

    let a = acc.iter().find(|p| p.steam_id == "A").unwrap();
    assert_eq!((a.shots, a.hits), (2, 1), "HE/dao không vào hits accuracy");
    assert_eq!(a.hitgroups.get(&1), Some(&1));
    assert_eq!(a.hits_detail.len(), 1);

    let he = pw.iter().find(|r| r.weapon == "hegrenade").unwrap();
    assert_eq!((he.hits, he.damage as i64), (1, 40), "dòng per-weapon vẫn giữ damage HE");
    assert!(mw.iter().any(|r| r.weapon == "knife" && r.hits == 1));
    let ak = pw.iter().find(|r| r.steam_id == "A" && r.weapon == "ak47").unwrap();
    assert_eq!((ak.shots, ak.hits), (2, 1));
  }
}
