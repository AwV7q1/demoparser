// Port của buildReplayEventChunks (compute.ts L892-907) + encodeReplayEventsBody
// (packages/replay-codec-core/src/replay-event-codec-core.ts L19-23). Khác tick_codec.rs (layout
// nhị phân cột hoá): đây CHỈ là JSON + zstd, nên "port" thực chất là serde_json::to_vec thay
// JSON.stringify -- không có logic mã hoá phức tạp nào khác cần dịch.
//
// LƯU Ý PARITY: JSON.stringify (JS) và serde_json::to_vec (Rust) không đảm bảo cùng thứ tự
// field/khoảng trắng dù data giống hệt nhau (ví dụ số nguyên `800.0` Rust in ra "800.0" còn JS in
// "800") -- vì vậy harness parity phải giải nén + JSON.parse cả 2 phía rồi so nội dung đã decode,
// KHÔNG so bytes thô như tick_codec (xem plan §"Parity harness").

use super::types::ReplayEventChunkOut;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
pub struct ReplayEventItem {
  pub round_number: i64,
  pub tick: i64,
  pub r#type: String,
  pub data: Value,
}

// Slim shape y hệt `{tick, type, data}` mà encodeReplayEventsBody serialize -- KHÔNG có
// roundNumber (đã tách theo round trước khi encode).
#[derive(Debug, Clone, Serialize)]
struct SlimReplayEvent {
  tick: i64,
  r#type: String,
  data: Value,
}

fn encode_replay_events_body(events: &[SlimReplayEvent]) -> Vec<u8> {
  if events.is_empty() {
    return Vec::new();
  }
  serde_json::to_vec(events).unwrap_or_default()
}

pub fn build_replay_event_chunks(items: Vec<ReplayEventItem>, zstd_level: i32) -> Vec<ReplayEventChunkOut> {
  if items.is_empty() {
    return Vec::new();
  }
  // BTreeMap giữ thứ tự tăng dần theo roundNumber -- khác `Map` insertion-order của JS, nhưng
  // không đổi Ý NGHĨA dữ liệu (harness parity so theo roundNumber, không so thứ tự mảng).
  let mut by_round: BTreeMap<i64, Vec<SlimReplayEvent>> = BTreeMap::new();
  for item in items {
    by_round.entry(item.round_number).or_default().push(SlimReplayEvent {
      tick: item.tick,
      r#type: item.r#type,
      data: item.data,
    });
  }

  let mut out = Vec::new();
  for (round_number, mut evs) in by_round {
    evs.sort_by_key(|e| e.tick);
    let event_count = evs.len() as i64;
    let raw = encode_replay_events_body(&evs);
    let compressed = crate::zstd_codec::compress(&raw, zstd_level).unwrap_or(raw);
    out.push(ReplayEventChunkOut { round_number, format: 1, event_count, data: compressed });
  }
  out
}
