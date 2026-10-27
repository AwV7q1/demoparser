// Port của mergePlayerMatchStats / mergeRoundPlayerStats (compute.ts, db-redesign P1.5): gộp các bảng
// trùng grain trước khi trả ParsedMatch -- PlayerMatchStat = base + aim + accuracy + rank;
// RoundPlayerStat = survivor + economy + damage. Thiếu bước này là lý do nhánh Rust từng trả shape cũ
// (playerAccuracyStats/roundSurvivorStats/...) làm persist nổ.

use super::ordered_map::OrderedMap;
use super::types::{HitDetail, PlayerAccuracyStat, PlayerMatchStat, RoundDamageStat, RoundEconomyStat, RoundSurvivorStat};
use crate::compute_aim::PlayerAimStat;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergedPlayerMatchStat {
  #[serde(flatten)]
  pub base: PlayerMatchStat,
  pub comp_rank: Option<i64>,
  pub comp_rank_type: Option<i64>,
  pub engagements: i64,
  pub preaim_deg: Option<f64>,
  pub time_to_kill_ms: Option<i64>,
  pub reaction_samples: i64,
  pub rifle_shots: i64,
  pub moving_shots: i64,
  pub avg_speed: Option<i64>,
  pub has_velocity_data: bool,
  pub shots: i64,
  pub hits: i64,
  pub hitgroups: Option<BTreeMap<i64, i64>>,
  pub hits_detail: Option<Vec<HitDetail>>,
}

#[derive(Debug, Clone, Copy)]
pub struct PlayerRank {
  pub comp_rank: i64,
  pub comp_rank_type: i64,
}

// baseRows (computePlayerStats, gồm mọi player trong roster/kill) là superset của aim/accuracy
// population → merge theo base, default 0/null/false cho player không có aim/accuracy/rank.
pub fn merge_player_match_stats(
  base_rows: Vec<PlayerMatchStat>,
  aim_rows: Vec<PlayerAimStat>,
  acc_rows: Vec<PlayerAccuracyStat>,
  ranks: &HashMap<String, PlayerRank>,
) -> Vec<MergedPlayerMatchStat> {
  let mut aim_map: HashMap<String, PlayerAimStat> = aim_rows.into_iter().map(|r| (r.steam_id.clone(), r)).collect();
  let mut acc_map: HashMap<String, PlayerAccuracyStat> = acc_rows.into_iter().map(|r| (r.steam_id.clone(), r)).collect();
  base_rows
    .into_iter()
    .map(|base| {
      let aim = aim_map.remove(&base.steam_id);
      let acc = acc_map.remove(&base.steam_id);
      let rank = ranks.get(&base.steam_id);
      MergedPlayerMatchStat {
        comp_rank: rank.map(|r| r.comp_rank),
        comp_rank_type: rank.map(|r| r.comp_rank_type),
        engagements: aim.as_ref().map_or(0, |a| a.engagements),
        preaim_deg: aim.as_ref().and_then(|a| a.preaim_deg),
        time_to_kill_ms: aim.as_ref().and_then(|a| a.time_to_kill_ms),
        reaction_samples: aim.as_ref().map_or(0, |a| a.reaction_samples),
        rifle_shots: aim.as_ref().map_or(0, |a| a.rifle_shots),
        moving_shots: aim.as_ref().map_or(0, |a| a.moving_shots),
        avg_speed: aim.as_ref().and_then(|a| a.avg_speed),
        has_velocity_data: aim.as_ref().map_or(false, |a| a.has_velocity_data),
        shots: acc.as_ref().map_or(0, |a| a.shots),
        hits: acc.as_ref().map_or(0, |a| a.hits),
        hitgroups: acc.as_ref().map(|a| a.hitgroups.clone()),
        hits_detail: acc.map(|a| a.hits_detail),
        base,
      }
    })
    .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundPlayerStat {
  pub round_number: i64,
  pub steam_id: String,
  pub side: String,
  pub alive: bool,
  pub equip_buy: f64,
  pub money_end: f64,
  pub equip_at_death: Option<f64>,
  pub damage: f64,
  pub he_damage: f64,
}

// Union 3 key-set theo (roundNumber, steamId), giữ thứ tự chèn như JS Map. side ưu tiên economy (đã
// xác định CT/T) -- economy chỉ ghi dòng có side nên ghi đè an toàn sau survivor (rosterSide có thể "").
pub fn merge_round_player_stats(
  survivor_rows: &[RoundSurvivorStat],
  economy_rows: &[RoundEconomyStat],
  damage_rows: &[RoundDamageStat],
) -> Vec<RoundPlayerStat> {
  let mut map: OrderedMap<String, RoundPlayerStat> = OrderedMap::new();
  fn ensure<'a>(m: &'a mut OrderedMap<String, RoundPlayerStat>, round_number: i64, steam_id: &str) -> &'a mut RoundPlayerStat {
    m.entry_or_insert_with(format!("{round_number}|{steam_id}"), || RoundPlayerStat {
      round_number,
      steam_id: steam_id.to_string(),
      side: String::new(),
      alive: false,
      equip_buy: 0.0,
      money_end: 0.0,
      equip_at_death: None,
      damage: 0.0,
      he_damage: 0.0,
    })
  }
  for s in survivor_rows {
    let r = ensure(&mut map, s.round_number, &s.steam_id);
    r.alive = s.alive;
    if !s.side.is_empty() {
      r.side = s.side.clone();
    }
  }
  for e in economy_rows {
    let r = ensure(&mut map, e.round_number, &e.steam_id);
    r.side = e.side.clone();
    r.equip_buy = e.equip_buy;
    r.money_end = e.money_end;
    r.equip_at_death = e.equip_at_death;
  }
  for d in damage_rows {
    let r = ensure(&mut map, d.round_number, &d.steam_id);
    r.damage = d.damage;
    r.he_damage = d.he_damage;
  }
  map.into_values()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn base(sid: &str) -> PlayerMatchStat {
    PlayerMatchStat {
      steam_id: sid.into(),
      player_name: sid.into(),
      side: Some("T".into()),
      kills: 0,
      deaths: 0,
      assists: 0,
      headshot_kills: 0,
      damage: 0.0,
      flash_assists: 0,
      slot_order: 0,
    }
  }

  #[test]
  fn player_merge_defaults_and_serializes_flat() {
    let acc = vec![PlayerAccuracyStat { steam_id: "A".into(), shots: 10, hits: 4, hitgroups: BTreeMap::from([(1, 2)]), hits_detail: vec![] }];
    let mut ranks = HashMap::new();
    ranks.insert("A".to_string(), PlayerRank { comp_rank: 15000, comp_rank_type: 11 });
    let out = merge_player_match_stats(vec![base("A"), base("B")], vec![], acc, &ranks);
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v[0]["steamId"], "A");
    assert_eq!(v[0]["shots"], 10);
    assert_eq!(v[0]["compRank"], 15000);
    assert_eq!(v[0]["hitgroups"]["1"], 2);
    assert_eq!(v[1]["shots"], 0);
    assert!(v[1]["compRank"].is_null());
    assert!(v[1]["hitgroups"].is_null());
    assert_eq!(v[1]["hasVelocityData"], false);
  }

  #[test]
  fn round_merge_unions_keys_and_economy_overrides_side() {
    let surv = vec![RoundSurvivorStat { round_number: 1, steam_id: "A".into(), alive: true, side: "".into() }];
    let eco = vec![RoundEconomyStat { round_number: 1, steam_id: "A".into(), side: "CT".into(), equip_buy: 4700.0, money_end: 300.0, equip_at_death: None }];
    let dmg = vec![RoundDamageStat { round_number: 2, steam_id: "B".into(), damage: 120.0, he_damage: 20.0 }];
    let out = merge_round_player_stats(&surv, &eco, &dmg);
    assert_eq!(out.len(), 2);
    assert_eq!((out[0].side.as_str(), out[0].alive, out[0].equip_buy as i64), ("CT", true, 4700));
    assert_eq!((out[1].round_number, out[1].steam_id.as_str(), out[1].he_damage as i64, out[1].alive), (2, "B", 20, false));
  }
}
