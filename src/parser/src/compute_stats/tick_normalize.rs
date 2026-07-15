// Port of toTickRow/normalizeTicks (compute.ts L692-746) -- restricted to the fields
// computeTickAggregates actually reads (see types.rs TickRow header comment). Handles BOTH shapes
// parser.parseTicks() can return, exactly like the JS: AoS (`Value::Array` of row objects) and
// SoA (`Value::Object` of column arrays, detected via `cols["X"]` being an array).

use super::types::TickRow;
use crate::compute_events::side_or_null;
use serde_json::Value;

fn or_zero(v: Option<&Value>) -> f64 {
  v.and_then(Value::as_f64).unwrap_or(0.0)
}

// `?? null` -- None only for a truly-absent field or explicit JSON null; a present 0 stays 0.
fn nullish_f64(v: Option<&Value>) -> Option<f64> {
  match v {
    None | Some(Value::Null) => None,
    Some(other) => other.as_f64(),
  }
}

fn nullish_str(v: Option<&Value>) -> Option<String> {
  match v {
    None | Some(Value::Null) => None,
    Some(other) => other.as_str().map(|s| s.to_string()),
  }
}

// `g('steamid') || ''` -- steamid is normally a string (see tick_codec.rs's own note that U64
// props serialize as JS strings); tolerate a stray number defensively, never seen in practice.
fn steamid_or_empty(v: Option<&Value>) -> String {
  match v {
    Some(Value::String(s)) if !s.is_empty() => s.clone(),
    Some(Value::Number(n)) => n.to_string(),
    _ => String::new(),
  }
}

fn to_tick_row<'a>(g: impl Fn(&str) -> Option<&'a Value>) -> Option<TickRow> {
  let x = g("X");
  let y = g("Y");
  if x.is_none() || y.is_none() {
    return None;
  }
  let health = nullish_f64(g("health"));
  let is_alive = match g("is_alive") {
    Some(Value::Bool(false)) => false,
    _ => health.unwrap_or(1.0) > 0.0,
  };
  let team_num = g("team_num").and_then(Value::as_i64);
  Some(TickRow {
    steam_id: steamid_or_empty(g("steamid")),
    tick: or_zero(g("tick")) as i64,
    x: or_zero(x),
    y: or_zero(y),
    is_alive,
    side: side_or_null(team_num),
    money: nullish_f64(g("balance")),
    equip_value: nullish_f64(g("current_equip_value")),
    last_place: nullish_str(g("last_place_name")),
  })
}

pub fn normalize_ticks(tick_data: &Value) -> Vec<TickRow> {
  match tick_data {
    Value::Array(arr) => arr.iter().filter_map(|row| to_tick_row(|f| row.get(f))).collect(),
    Value::Object(cols) => {
      let n = match cols.get("X").and_then(Value::as_array) {
        Some(xs) => xs.len(),
        None => return vec![],
      };
      (0..n)
        .filter_map(|i| to_tick_row(|f| cols.get(f).and_then(Value::as_array).and_then(|a| a.get(i))))
        .collect()
    }
    _ => vec![],
  }
}
