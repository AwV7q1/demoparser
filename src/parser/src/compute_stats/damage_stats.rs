// Port of computeDamageStats (compute.ts L839-856).

use super::ordered_map::OrderedMap;
use super::types::{HurtBatchItem, RoundDamageStat};

pub fn compute_damage_stats(hurt_batch: &[HurtBatchItem]) -> Vec<RoundDamageStat> {
  if hurt_batch.is_empty() {
    return vec![];
  }
  let mut acc: OrderedMap<String, RoundDamageStat> = OrderedMap::new();
  for h in hurt_batch {
    let d = &h.data;
    let attacker = match &d.attacker_steam_id {
      Some(s) if !s.is_empty() => s.clone(),
      _ => continue,
    };
    let key = format!("{}|{}", h.round_number, attacker);
    let a = acc.entry_or_insert_with(key, || RoundDamageStat { round_number: h.round_number, steam_id: attacker.clone(), damage: 0.0, he_damage: 0.0 });
    a.damage += d.dmg_health;
    if d.weapon.as_deref().unwrap_or("").to_lowercase().contains("hegrenade") && d.attacker_steam_id != d.victim_steam_id {
      a.he_damage += d.dmg_health;
    }
  }
  acc.into_values()
}
