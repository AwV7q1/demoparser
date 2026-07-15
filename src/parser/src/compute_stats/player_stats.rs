// Port of computePlayerStats (compute.ts L514-586).
//
// GOTCHA (unlike every builder in compute_events): this function does NOT filter by validRounds
// -- it walks ALL raw kill/hurt events unconditionally (warmup/knife-round kills included), unlike
// buildKills/buildHurt which drop anything outside a valid round. Do not "fix" this to match --
// it is the real compute.ts behavior for match-total player stats.
use super::ordered_map::OrderedMap;
use super::types::{PlayerMatchStat, RawHurtRow, RawKillRow, RawPlayerInfo};
use crate::compute_events::{clean_name, side_2_else_ct, side_or_null};
use std::collections::HashMap;

fn truthy(s: &Option<String>) -> Option<&str> {
  match s {
    Some(v) if !v.is_empty() => Some(v.as_str()),
    _ => None,
  }
}

pub fn compute_player_stats(kills: &[RawKillRow], player_info: &[RawPlayerInfo], hurt_events: &[RawHurtRow]) -> Vec<PlayerMatchStat> {
  let mut stats_map: OrderedMap<String, PlayerMatchStat> = OrderedMap::new();
  let mut order_map: HashMap<String, i64> = HashMap::new();
  let mut pi_name_map: HashMap<String, Option<serde_json::Value>> = HashMap::new();

  for (i, pi) in player_info.iter().enumerate() {
    if let Some(sid) = truthy(&pi.steamid) {
      // JS `.set()` always overwrites on a repeated key -- mirror that (not entry-or-insert).
      order_map.insert(sid.to_string(), i as i64);
      pi_name_map.insert(sid.to_string(), pi.name.clone());
    }
  }

  for kill in kills {
    if let Some(a_sid) = truthy(&kill.attacker_steamid) {
      if !stats_map.contains_key(&a_sid.to_string()) {
        let fallback = pi_name_map.get(a_sid).cloned().flatten();
        stats_map.insert(
          a_sid.to_string(),
          PlayerMatchStat {
            steam_id: a_sid.to_string(),
            player_name: clean_name(&kill.attacker_name, a_sid, fallback.as_ref()),
            side: Some(side_2_else_ct(kill.attacker_team_num)),
            kills: 0, deaths: 0, assists: 0, headshot_kills: 0, damage: 0.0, flash_assists: 0, slot_order: 999,
          },
        );
      }
      let s = stats_map.get_mut(&a_sid.to_string()).unwrap();
      s.kills += 1;
      if kill.headshot.unwrap_or(false) {
        s.headshot_kills += 1;
      }
      if kill.assistedflash.unwrap_or(false) {
        s.flash_assists += 1;
      }
    }

    if let Some(v_sid) = truthy(&kill.user_steamid) {
      if !stats_map.contains_key(&v_sid.to_string()) {
        let fallback = pi_name_map.get(v_sid).cloned().flatten();
        stats_map.insert(
          v_sid.to_string(),
          PlayerMatchStat {
            steam_id: v_sid.to_string(),
            player_name: clean_name(&kill.user_name, v_sid, fallback.as_ref()),
            side: Some(side_2_else_ct(kill.user_team_num)),
            kills: 0, deaths: 0, assists: 0, headshot_kills: 0, damage: 0.0, flash_assists: 0, slot_order: 999,
          },
        );
      }
      stats_map.get_mut(&v_sid.to_string()).unwrap().deaths += 1;
    }

    if let Some(as_sid) = truthy(&kill.assister_steamid) {
      if !stats_map.contains_key(&as_sid.to_string()) {
        let fallback = pi_name_map.get(as_sid).cloned().flatten();
        stats_map.insert(
          as_sid.to_string(),
          PlayerMatchStat {
            steam_id: as_sid.to_string(),
            player_name: clean_name(&kill.assister_name, as_sid, fallback.as_ref()),
            side: side_or_null(kill.attacker_team_num),
            kills: 0, deaths: 0, assists: 0, headshot_kills: 0, damage: 0.0, flash_assists: 0, slot_order: 999,
          },
        );
      }
      stats_map.get_mut(&as_sid.to_string()).unwrap().assists += 1;
    }
  }

  for pi in player_info {
    let sid = truthy(&pi.steamid).unwrap_or("").to_string();
    if sid.is_empty() || stats_map.contains_key(&sid) {
      continue;
    }
    stats_map.insert(
      sid.clone(),
      PlayerMatchStat {
        steam_id: sid.clone(),
        player_name: clean_name(&pi.name, &sid, None),
        side: Some(side_2_else_ct(pi.team_number)),
        kills: 0, deaths: 0, assists: 0, headshot_kills: 0, damage: 0.0, flash_assists: 0, slot_order: 999,
      },
    );
  }

  for ev in hurt_events {
    let sid = match truthy(&ev.attacker_steamid) {
      Some(s) => s.to_string(),
      None => continue,
    };
    if Some(sid.as_str()) == ev.user_steamid.as_deref() {
      continue;
    }
    if let Some(s) = stats_map.get_mut(&sid) {
      s.damage += ev.dmg_health.unwrap_or(0.0);
    }
  }

  stats_map
    .into_entries()
    .into_iter()
    .map(|(steam_id, mut s)| {
      s.slot_order = order_map.get(&steam_id).copied().unwrap_or(999);
      s
    })
    .collect()
}
