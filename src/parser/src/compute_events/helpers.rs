// Ported 1:1 from packages/parse-core/src/helpers.ts (small stateless helpers only --
// grenade-trajectory helpers live in grenades.rs).

use serde_json::Value;

// Chuẩn hoá tên vũ khí (trùng norm() ở match.service.ts / weaponIcon.ts). KHÔNG dùng trong phạm
// vi "events" (weapon output giữ nguyên chuỗi thô) -- dùng bởi compute_stats
// (computeWeaponAccuracyStats/computeAimStats đều gọi normWeaponName).
pub fn norm_weapon_name(w: &Option<String>) -> String {
  let s = w.as_deref().unwrap_or("").to_lowercase();
  let cleaned: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
  cleaned.strip_prefix("weapon").unwrap_or(&cleaned).to_string()
}

// hitgroup demoparser2 trả CHUỖI nhãn → mã hitgroup CS2 (0-10). Đồng bộ với match.service.ts.
// KHÔNG dùng trong phạm vi "events" (chỉ pass-through raw) -- dùng bởi compute_stats
// (computeWeaponAccuracyStats).
pub fn hitgroup_to_int(v: &Option<Value>) -> i64 {
  match v {
    Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
    Some(Value::String(s)) => {
      let norm: String = s.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect();
      match norm.as_str() {
        "head" => 1,
        "chest" => 2,
        "stomach" => 3,
        "leftarm" => 4,
        "rightarm" => 5,
        "leftleg" => 6,
        "rightleg" => 7,
        "neck" => 8,
        "gear" => 10,
        _ => 0,
      }
    }
    _ => 0,
  }
}

// demoparser2 đôi khi trả name là OBJECT → ép an toàn; chuỗi (kể cả object serialize) giữ nguyên.
pub fn clean_name(name: &Option<Value>, steam_id: &str, fallback_name: Option<&Value>) -> String {
  let norm = |v: &Option<&Value>| -> String {
    match v {
      Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
      _ => String::new(),
    }
  };
  let primary = norm(&name.as_ref());
  if !primary.is_empty() {
    return primary;
  }
  let fb = norm(&fallback_name);
  if !fb.is_empty() {
    return fb;
  }
  let tail: String = steam_id.chars().rev().take(6).collect::<String>().chars().rev().collect();
  format!("Player_{tail}")
}

// `team_num === 2 ? 'T' : team_num === 3 ? 'CT' : null` -- dùng ở buildKills (attacker/victim
// side) và buildBlind (flasher/victim side). KHÔNG suy ra 'CT' mặc định khi team_num lạ/thiếu.
pub fn side_or_null(team_num: Option<i64>) -> Option<String> {
  match team_num {
    Some(2) => Some("T".to_string()),
    Some(3) => Some("CT".to_string()),
    _ => None,
  }
}

// `team_num === 2 ? 'T' : 'CT'` -- dùng ở buildBombPlantDefuse/buildGrenadeEffects/
// buildInstantNade. KHÁC side_or_null: KHÔNG BAO GIỜ null, mặc định 'CT' khi team_num != 2.
pub fn side_2_else_ct(team_num: Option<i64>) -> String {
  if team_num == Some(2) { "T".to_string() } else { "CT".to_string() }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  // Chuyển từ apps/api hitgroup-to-int.spec.ts (bản TS đã xoá).
  #[test]
  fn hitgroup_labels_map_to_cs2_codes() {
    let hg = |v: Value| hitgroup_to_int(&Some(v));
    let labels = [("head", 1), ("chest", 2), ("stomach", 3), ("left arm", 4), ("right arm", 5), ("left leg", 6), ("right leg", 7), ("neck", 8), ("gear", 10)];
    for (label, code) in labels {
      assert_eq!(hg(json!(label)), code, "{label}");
    }
    assert_eq!(hg(json!("Left Arm")), 4);
    assert_eq!(hg(json!("HEAD")), 1);
    assert_eq!(hg(json!(1)), 1);
    assert_eq!(hg(json!("generic")), 0);
    assert_eq!(hg(json!("totally-not-a-hitgroup")), 0);
    assert_eq!(hitgroup_to_int(&None), 0);
    assert_eq!(hg(Value::Null), 0);
  }
}
