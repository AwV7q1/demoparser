// Port 1:1 của buildWeaponFire (compute.ts L413-431) và buildHurt (L434-452). Cả 2 là replay-only
// (nguồn cho replay_event_chunks.rs), không đi vào bảng phân tích vĩnh viễn.

use super::types::{HurtData, ParsedEvent, RawEvent, WeaponFireData};
use std::collections::HashSet;

pub fn build_weapon_fire(fire_shots: &[RawEvent], valid_rounds: &HashSet<i64>) -> Vec<ParsedEvent<WeaponFireData>> {
  let non_gun = super::constants::non_gun();
  let mut out = Vec::new();
  for ev in fire_shots {
    let w = ev.weapon.clone().unwrap_or_default().to_lowercase();
    if non_gun.iter().any(|k| w.contains(k)) {
      continue;
    }
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) || ev.user_x.is_none() {
      continue;
    }
    let data = WeaponFireData {
      x: ev.user_x.unwrap(),
      y: ev.user_y.unwrap_or(0.0),
      yaw: ev.user_yaw,
      vel_x: ev.user_velocity_x,
      vel_y: ev.user_velocity_y,
      steam_id: ev.user_steamid.clone(),
      weapon: ev.weapon.clone(),
    };
    out.push(ParsedEvent { round_number: r_num, tick: ev.tick.unwrap_or(0), r#type: "WEAPON_FIRE", data });
  }
  out
}

pub fn build_hurt(hurt_events: &[RawEvent], valid_rounds: &HashSet<i64>) -> Vec<ParsedEvent<HurtData>> {
  let non_bullet = super::constants::non_bullet();
  let mut out = Vec::new();
  for ev in hurt_events {
    let w = ev.weapon.clone().unwrap_or_default().to_lowercase();
    if non_bullet.iter().any(|k| w.contains(k)) {
      continue;
    }
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) || ev.attacker_x.is_none() || ev.user_x.is_none() {
      continue;
    }
    let data = HurtData {
      attacker_x: ev.attacker_x.unwrap(),
      attacker_y: ev.attacker_y.unwrap_or(0.0),
      x: ev.user_x.unwrap(),
      y: ev.user_y.unwrap_or(0.0),
      z: ev.user_z,
      weapon: ev.weapon.clone(),
      dmg_health: ev.dmg_health.unwrap_or(0.0),
      hitgroup: ev.hitgroup.clone(),
      attacker_steam_id: ev.attacker_steamid.clone(),
      victim_steam_id: ev.user_steamid.clone(),
    };
    out.push(ParsedEvent { round_number: r_num, tick: ev.tick.unwrap_or(0), r#type: "HURT", data });
  }
  out
}
