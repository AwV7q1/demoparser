// Ported from packages/parse-core/src/constants.ts -- "aim" domain constants only.

pub const AIM_TICK_RATE: f64 = 64.0;
pub const AIM_PREAIM_WINDOW: i64 = 64; // 1s trước kill
pub const AIM_EYE_Z: f64 = 64.0; // chiều cao mắt attacker (origin demo là chân)
pub const AIM_TARGET_Z: f64 = 54.0; // ngắm vào thân victim (~ngực)
pub const ACCURACY_SPEED_FACTOR: f64 = 0.34;

// team_num keys omitted -- lookup is by normalized weapon name string, not by side.
pub fn rifle_max_speed(weapon: &str) -> Option<f64> {
  match weapon {
    "ak47" => Some(215.0),
    "m4a4" => Some(225.0),
    "m4a1" => Some(225.0),
    "m4a1s" => Some(225.0),
    "m4a1silencer" => Some(225.0),
    "galilar" => Some(215.0),
    "galil" => Some(215.0),
    "famas" => Some(220.0),
    "sg553" => Some(210.0),
    "sg556" => Some(210.0),
    "aug" => Some(220.0),
    _ => None,
  }
}
