// Port 1:1 của computeRounds() trong packages/parse-core/src/compute.ts (L135-160).
//
// GOTCHA (đã ghi ở root CLAUDE.md, giữ nguyên): `total_rounds_played` KHÔNG nhất quán giữa
// event -- round_start dùng `total_rounds_played + 1` (số round trước round hiện tại), còn
// round_end dùng TRỰC TIẾP `total_rounds_played` (đã tính cả round vừa xong). Hai công thức khác
// nhau này KHÔNG phải lỗi đánh máy, đừng "sửa cho giống nhau".

use super::types::{ParsedRound, RawEvent};
use std::collections::{HashMap, HashSet};

pub struct RoundsResult {
  pub rounds: Vec<ParsedRound>,
  pub valid_rounds: HashSet<i64>,
  pub round_start_tick_by_num: HashMap<i64, i64>,
}

pub fn compute_rounds(round_start_events: &[RawEvent], round_end_events: &[RawEvent]) -> RoundsResult {
  let mut round_start_tick_by_num: HashMap<i64, i64> = HashMap::new();
  for rs in round_start_events {
    let r_num = rs.total_rounds_played.unwrap_or(0) + 1;
    round_start_tick_by_num.entry(r_num).or_insert_with(|| rs.tick.unwrap_or(0));
  }

  let mut sorted_round_ends: Vec<&RawEvent> = round_end_events.iter().collect();
  sorted_round_ends.sort_by_key(|e| e.tick.unwrap_or(0));

  let mut prev_end_tick: i64 = 0;
  let mut ct_wins: i64 = 0;
  let mut t_wins: i64 = 0;
  let mut rounds = Vec::new();
  let mut valid_rounds = HashSet::new();

  for re in sorted_round_ends {
    // KHÔNG +1 -- xem gotcha ở đầu file.
    let r_num = re.total_rounds_played.unwrap_or(0);
    let start_tick = round_start_tick_by_num.get(&r_num).copied().unwrap_or(prev_end_tick);
    let end_tick = re.tick.unwrap_or(0);
    let winner_side = if re.winner.as_deref() == Some("T") { "T" } else { "CT" };
    if winner_side == "T" {
      t_wins += 1;
    } else {
      ct_wins += 1;
    }
    rounds.push(ParsedRound {
      round_number: r_num,
      winner_side: winner_side.to_string(),
      reason: re.reason.clone().unwrap_or_default(),
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
