// Port 1:1 của buildBurn (compute.ts L372-385) và buildBlind (L388-410).

use super::helpers::{clean_name, side_or_null};
use super::types::{BlindData, BurnData, ParsedEvent, RawEvent};
use std::collections::HashSet;

pub fn build_burn(hurt_events: &[RawEvent], valid_rounds: &HashSet<i64>) -> Vec<ParsedEvent<BurnData>> {
  let mut out = Vec::new();
  for ev in hurt_events {
    let w = ev.weapon.clone().unwrap_or_default().to_lowercase();
    if !w.contains("inferno") && !w.contains("molotov") && !w.contains("incendiary") {
      continue;
    }
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let data = BurnData {
      victim_steam_id: ev.user_steamid.clone(),
      attacker_steam_id: ev.attacker_steamid.clone(),
      dmg: ev.dmg_health,
    };
    out.push(ParsedEvent { round_number: r_num, tick: ev.tick.unwrap_or(0), r#type: "BURN", data });
  }
  out
}

pub fn build_blind(blinded_events: &[RawEvent], valid_rounds: &HashSet<i64>) -> Vec<ParsedEvent<BlindData>> {
  let mut out = Vec::new();
  for ev in blinded_events {
    let r_num = ev.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }
    let flasher_side = side_or_null(ev.attacker_team_num);
    let victim_side = side_or_null(ev.user_team_num);

    let flasher_name = match &ev.attacker_steamid {
      Some(sid) => Some(serde_json::Value::String(clean_name(&ev.attacker_name, sid, None))),
      None => ev.attacker_name.clone(),
    };
    let victim_name = match &ev.user_steamid {
      Some(sid) => Some(serde_json::Value::String(clean_name(&ev.user_name, sid, None))),
      None => ev.user_name.clone(),
    };

    let is_enemy_flash = flasher_side.is_some() && victim_side.is_some() && flasher_side != victim_side;

    let data = BlindData {
      flasher_steam_id: ev.attacker_steamid.clone(),
      flasher_name,
      flasher_side,
      victim_steam_id: ev.user_steamid.clone(),
      victim_name,
      victim_side,
      blind_duration: ev.blind_duration.unwrap_or(0.0),
      is_enemy_flash,
    };
    out.push(ParsedEvent { round_number: r_num, tick: ev.tick.unwrap_or(0), r#type: "FLASH_BLIND", data });
  }
  out
}
