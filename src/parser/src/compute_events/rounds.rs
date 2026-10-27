// Chia round từ round_start/round_end (trước là computeRounds() TS ở packages/parse-core — đã xoá, Rust là nguồn duy nhất).
//
// GOTCHA `total_rounds_played` KHÁC nhau giữa round_start và round_end của CÙNG 1 round vật lý: round_start
// là 0-based (round vật lý k → trp = k-1), round_end là 1-based (round vật lý k → trp = k) → phải `+1` khi
// index round_start để key khớp rNum của round_end. Đo lại 2026-10-27 bằng CHÍNH parser Rust này
// (full_pipeline.rs `round_trp_probe`, spirit-vs-mouz-m3-ancient): round_start(tick=339, trp=0) ↔
// round_end(tick=14944, trp=1). Bản trước bỏ `+1` (ghi chú "cùng trp") làm mọi round lấy nhầm startTick
// của round SAU → startTick > endTick.
//
// Lọc round_end giả (pause/restore backup): demo có technical pause sinh round_end KHÔNG winner/reason
// (trp kẹt ở số cũ) + 1 round_end ở tick 0. Round_end thật luôn có winner (2/3 hoặc "T"/"CT") + reason;
// chỉ giữ round_end đầu tiên của mỗi số round (tránh trùng unique (matchId, roundNumber)).
use super::types::{ParsedRound, RawEvent};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub struct RoundsResult {
  pub rounds: Vec<ParsedRound>,
  pub valid_rounds: HashSet<i64>,
  pub round_start_tick_by_num: HashMap<i64, i64>,
}

// team_num convention: 2 = T, 3 = CT. Parser có thể trả số hoặc chuỗi "T"/"CT".
fn winner_side(winner: &Option<Value>) -> Option<&'static str> {
  match winner {
    Some(Value::Number(n)) => match n.as_i64() {
      Some(2) => Some("T"),
      Some(3) => Some("CT"),
      _ => None,
    },
    Some(Value::String(s)) if s == "T" => Some("T"),
    Some(Value::String(s)) if s == "CT" => Some("CT"),
    _ => None,
  }
}

// JS truthiness của `re.reason` (chuỗi rỗng/0/null = giả).
fn truthy(v: &Option<Value>) -> bool {
  match v {
    Some(Value::String(s)) => !s.is_empty(),
    Some(Value::Number(n)) => n.as_f64().map_or(false, |f| f != 0.0),
    Some(Value::Bool(b)) => *b,
    Some(Value::Null) | None => false,
    Some(_) => true,
  }
}

pub fn compute_rounds(round_start_events: &[RawEvent], round_end_events: &[RawEvent]) -> RoundsResult {
  let mut round_start_tick_by_num: HashMap<i64, i64> = HashMap::new();
  for rs in round_start_events {
    let r_num = rs.total_rounds_played.unwrap_or(0) + 1;
    round_start_tick_by_num.entry(r_num).or_insert_with(|| rs.tick.unwrap_or(0));
  }

  let mut sorted_round_ends: Vec<&RawEvent> =
    round_end_events.iter().filter(|re| winner_side(&re.winner).is_some() && truthy(&re.reason)).collect();
  sorted_round_ends.sort_by_key(|e| e.tick.unwrap_or(0));
  let mut seen_round_nums: HashSet<i64> = HashSet::new();
  sorted_round_ends.retain(|re| seen_round_nums.insert(re.total_rounds_played.unwrap_or(0)));

  let mut prev_end_tick: i64 = 0;
  let mut ct_wins: i64 = 0;
  let mut t_wins: i64 = 0;
  let mut rounds = Vec::new();
  let mut valid_rounds = HashSet::new();

  for re in sorted_round_ends {
    let r_num = re.total_rounds_played.unwrap_or(0);
    let start_tick = round_start_tick_by_num.get(&r_num).copied().unwrap_or(prev_end_tick);
    let end_tick = re.tick.unwrap_or(0);
    let winner = winner_side(&re.winner).unwrap_or("CT");
    if winner == "T" {
      t_wins += 1;
    } else {
      ct_wins += 1;
    }
    rounds.push(ParsedRound {
      round_number: r_num,
      winner_side: winner.to_string(),
      // Port của TS `String(re.reason || '')`.
      reason: match &re.reason {
        Some(Value::String(s)) => s.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => String::new(),
      },
      t_score: t_wins,
      ct_score: ct_wins,
      start_tick,
      end_tick,
    });
    valid_rounds.insert(r_num);
    prev_end_tick = end_tick;
  }

  RoundsResult { rounds, valid_rounds, round_start_tick_by_num }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn ev(name: &str, tick: i64, trp: i64, winner: Value, reason: Value) -> RawEvent {
    serde_json::from_value(json!({ "event_name": name, "tick": tick, "total_rounds_played": trp, "winner": winner, "reason": reason })).unwrap()
  }

  // Trích từ spirit-vs-mouz-m3-ancient: round_end giả tick 0 + chuỗi pause/restore kẹt trp=12.
  #[test]
  fn plus_one_on_start_and_drops_fake_round_ends() {
    let starts = vec![
      ev("round_start", 0, 0, Value::Null, Value::Null),
      ev("round_start", 339, 0, Value::Null, Value::Null),
      ev("round_start", 15392, 1, Value::Null, Value::Null),
      ev("round_start", 100939, 12, Value::Null, Value::Null),
      ev("round_start", 116469, 12, Value::Null, Value::Null),
    ];
    let ends = vec![
      ev("round_end", 0, 0, Value::Null, Value::Null),
      ev("round_end", 14944, 1, json!("T"), json!("ct_killed")),
      ev("round_end", 24329, 2, json!(2), json!("bomb_exploded")),
      ev("round_end", 116469, 12, Value::Null, Value::Null),
      ev("round_end", 99979, 12, json!(3), json!("t_killed")),
      ev("round_end", 99990, 12, json!("CT"), json!("t_killed")),
    ];
    let r = compute_rounds(&starts, &ends);
    let nums: Vec<i64> = r.rounds.iter().map(|x| x.round_number).collect();
    assert_eq!(nums, vec![1, 2, 12], "bỏ round 0 + round_end không winner + trùng số");
    assert_eq!((r.rounds[0].start_tick, r.rounds[0].end_tick), (0, 14944), "round 1 bắt đầu ở round_start trp=0");
    assert_eq!(r.rounds[1].start_tick, 15392, "round 2 ↔ round_start trp=1");
    assert_eq!(r.rounds[2].start_tick, 24329, "round 12 không có round_start trp=11 → lùi về end của round trước");
    assert_eq!(r.rounds[2].winner_side, "CT", "giữ round_end thật đầu tiên của số 12");
  }

  #[test]
  fn sorts_by_end_tick_and_accumulates_score() {
    let starts = vec![
      ev("round_start", 100, 0, Value::Null, Value::Null),
      ev("round_start", 1000, 1, Value::Null, Value::Null),
      ev("round_start", 2000, 2, Value::Null, Value::Null),
    ];
    let ends = vec![
      ev("round_end", 1900, 2, json!(2), json!("t_win")),
      ev("round_end", 900, 1, json!(3), json!("ct_win")),
      ev("round_end", 2900, 3, json!("T"), json!("t_win")),
    ];
    let r = compute_rounds(&starts, &ends);
    let ends_sorted: Vec<i64> = r.rounds.iter().map(|x| x.end_tick).collect();
    assert_eq!(ends_sorted, vec![900, 1900, 2900]);
    assert!(r.rounds.iter().all(|x| x.start_tick < x.end_tick));
    let last = &r.rounds[2];
    assert_eq!((last.t_score, last.ct_score, last.winner_side.as_str()), (2, 1, "T"));
  }
}
