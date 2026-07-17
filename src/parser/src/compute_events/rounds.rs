// Port 1:1 của computeRounds() trong packages/parse-core/src/compute.ts (L135-160).
//
// The root CLAUDE.md gotcha ("round_start dùng total_rounds_played + 1... round_end dùng TRỰC
// TIẾP total_rounds_played") describes how KILL/GRENADE events (mid-round, tagged BEFORE the
// counter increments) get their `roundNumber` -- that asymmetry is real and intentional THERE.
// It does NOT apply to matching a round_start event back to ITS OWN round_end here: empirically
// (ADR-007 (4) online-aggregate work, 2026-07-17, real demo) round_start and round_end for the
// SAME physical round read the IDENTICAL `total_rounds_played` value (no increment happens
// between them) -- e.g. round_start(tick=65, trp=0) and round_end(tick=8971, trp=0) are the same
// round; round_start(tick=9419, trp=1) and round_end(tick=15232, trp=1) are the next one. Using
// `trp + 1` here (copied from the kill/grenade convention) shifts every `round_start_tick_by_num`
// entry by one key, so every round_end's start_tick lookup silently grabs the PREVIOUS physical
// round's start_tick instead of its own -- a real bug, found because `compute_full_pipeline_async`
// (the live production entry point, see nativeDemoEngine.ts) was exercised end-to-end for the
// first time while building the online-aggregate work. Confirmed via `compute.ts`'s TS source:
// same formula, same bug, inherited by this 1:1 port rather than introduced by it.
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
    let r_num = rs.total_rounds_played.unwrap_or(0);
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
    // team_num convention (root CLAUDE.md): 2 = T, 3 = CT. Real demos emit `winner` as this
    // numeric team_num, not the literal "T"/"CT" string the TS port's `re.winner === 'T'` assumed
    // -- handle both shapes (see RawEvent.winner doc comment).
    let winner_side = match re.winner.as_ref().and_then(|v| v.as_i64()) {
      Some(2) => "T",
      Some(3) => "CT",
      _ => if re.winner.as_ref().and_then(|v| v.as_str()) == Some("T") { "T" } else { "CT" },
    };
    if winner_side == "T" {
      t_wins += 1;
    } else {
      ct_wins += 1;
    }
    rounds.push(ParsedRound {
      round_number: r_num,
      winner_side: winner_side.to_string(),
      // Port of TS `String(re.reason || '')` -- coerces whichever JSON type `reason` actually is.
      reason: match &re.reason {
        Some(serde_json::Value::String(s)) => s.clone(),
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
