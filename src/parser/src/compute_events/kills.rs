// Port 1:1 của buildKills() trong compute.ts (L163-197).

use super::helpers::{clean_name, side_or_null};
use super::types::{KillData, ParsedEvent, RawEvent};
use std::collections::HashSet;

pub fn build_kills(kill_events: &[RawEvent], valid_rounds: &HashSet<i64>) -> Vec<ParsedEvent<KillData>> {
  let mut out = Vec::new();
  for kill in kill_events {
    let r_num = kill.total_rounds_played.unwrap_or(0) + 1;
    if !valid_rounds.contains(&r_num) {
      continue;
    }

    let attacker_name = match &kill.attacker_steamid {
      Some(sid) => Some(serde_json::Value::String(clean_name(&kill.attacker_name, sid, None))),
      None => kill.attacker_name.clone(),
    };
    let victim_name = match &kill.user_steamid {
      Some(sid) => Some(serde_json::Value::String(clean_name(&kill.user_name, sid, None))),
      None => kill.user_name.clone(),
    };
    // `kill.assister_steamid ? cleanName(...) : (kill.assister_name || null)` -- false branch đã
    // có `|| null`, luôn có giá trị (khác attacker/victim name -- xem gotcha ở KillData). JS `||`
    // coi chuỗi rỗng là falsy → null; object/chuỗi non-empty đi qua nguyên trạng.
    let assister_name = match &kill.assister_steamid {
      Some(sid) => Some(serde_json::Value::String(clean_name(&kill.assister_name, sid, None))),
      None => match &kill.assister_name {
        Some(serde_json::Value::String(s)) if s.is_empty() => None,
        Some(v) if !v.is_null() => Some(v.clone()),
        _ => None,
      },
    };

    let suicide = kill.attacker_steamid.is_none()
      || (kill.attacker_steamid.is_some() && kill.attacker_steamid == kill.user_steamid);

    let data = KillData {
      attacker_name,
      attacker_steam_id: kill.attacker_steamid.clone(),
      attacker_side: side_or_null(kill.attacker_team_num),
      victim_name,
      victim_steam_id: kill.user_steamid.clone(),
      victim_side: side_or_null(kill.user_team_num),
      assister_name,
      assister_steam_id: kill.assister_steamid.clone(),
      weapon: kill.weapon.clone(),
      headshot: kill.headshot.unwrap_or(false),
      hitgroup: kill.hitgroup.clone(),
      assisted_flash: kill.assistedflash.unwrap_or(false),
      penetrated: kill.penetrated.unwrap_or(0) > 0,
      noscope: kill.noscope.unwrap_or(false),
      thrusmoke: kill.thrusmoke.unwrap_or(false),
      attacker_blind: kill.attackerblind.unwrap_or(false),
      suicide,
      distance: kill.distance,
      x: kill.user_x.or(kill.bare_x),
      y: kill.user_y.or(kill.bare_y),
      attacker_x: kill.attacker_x,
      attacker_y: kill.attacker_y,
    };

    out.push(ParsedEvent { round_number: r_num, tick: kill.tick.unwrap_or(0), r#type: "KILL", data });
  }
  out
}
