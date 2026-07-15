// Ported 1:1 from packages/parse-core/src/constants.ts (events-domain subset only -- tick/aim
// constants stay out of scope for this phase).

pub const GRENADE_MAX_FLIGHT_TICKS: i64 = 384; // cửa sổ lùi tối đa từ điểm nổ (~6s @64Hz)
pub const GRENADE_RUN_GAP: i64 = 16; // gap tick > ngưỡng = quả/lượt ném khác → cắt
pub const GRENADE_DOWNSAMPLE: usize = 2; // giữ mỗi N tick (luôn giữ điểm cuối)
pub const GRENADE_SOLO_MAX_GAP: i64 = GRENADE_MAX_FLIGHT_TICKS; // smoke/molotov đơn/round: gap = qua mái
pub const THROW_MATCH_MAX_TICKS: i64 = 64 * 15;

pub const SMOKE_FALLBACK_TICKS: i64 = 64 * 18;
pub const FIRE_FALLBACK_TICKS: i64 = 64 * 7;

pub fn smoke_proj() -> &'static [&'static str] {
  &["CSmokeGrenadeProjectile"]
}
pub fn fire_proj() -> &'static [&'static str] {
  &["CMolotovProjectile", "CIncendiaryGrenade"]
}
pub fn he_proj() -> &'static [&'static str] {
  &["CHEGrenadeProjectile"]
}
pub fn flash_proj() -> &'static [&'static str] {
  &["CFlashbangProjectile"]
}
pub fn smoke_fire_weapon() -> &'static [&'static str] {
  &["smokegrenade"]
}
pub fn fire_fire_weapon() -> &'static [&'static str] {
  &["molotov", "incgrenade", "incendiary"]
}
pub fn he_fire_weapon() -> &'static [&'static str] {
  &["hegrenade"]
}
pub fn flash_fire_weapon() -> &'static [&'static str] {
  &["flashbang"]
}

pub fn non_gun() -> &'static [&'static str] {
  &[
    "knife", "bayonet", "karambit", "grenade", "molotov", "incendiary", "decoy", "c4", "healthshot",
  ]
}
pub fn non_bullet() -> &'static [&'static str] {
  &["inferno", "molotov", "incendiary", "smokegrenade", "decoy", "flashbang"]
}
pub fn pickup_exclude() -> &'static [&'static str] {
  &["knife", "bayonet", "karambit", "vest", "vesthelm", "defuser", "c4"]
}
