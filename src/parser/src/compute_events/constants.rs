// Ported 1:1 from packages/parse-core/src/constants.ts (events-domain subset only -- tick/aim
// constants stay out of scope for this phase).

pub const GRENADE_DOWNSAMPLE: usize = 2; // giữ mỗi N tick (luôn giữ điểm cuối)
// Sample projectile liên tục từng tick (đo trên 4 demo pro: gap tối đa 1) -- gap lớn hơn ngưỡng này
// nghĩa là entity id đã bị tái sử dụng cho quả khác → tách chuyến.
pub const PROJECTILE_SPLIT_GAP: i64 = 64;
// smoke/HE/flash: chuyến phải còn sống tới sát tick nổ.
pub const DETONATE_END_SLACK: i64 = 8;
// inferno_startburn mang entity của inferno (không phải projectile) → ghép theo tick projectile biến mất.
pub const INFERNO_MATCH_TICKS: i64 = 16;
// weapon_fire của nade đứng trước tick spawn projectile 6-14 tick (đo trên 4 demo pro).
pub const THROW_BEFORE_SPAWN_MAX: i64 = 32;
// Fallback khi không tìm được chuyến projectile: weapon_fire gần nhất trước lúc nổ trong cửa sổ này.
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

// 'flashbang' không chứa chữ "grenade" nên phải liệt kê riêng (thiếu nó thì mỗi cú ném flash bị tính
// là 1 phát bắn).
pub fn non_gun() -> &'static [&'static str] {
  &[
    "knife", "bayonet", "karambit", "grenade", "flashbang", "molotov", "incendiary", "decoy", "c4", "healthshot",
  ]
}

/// Súng (bắn đạn) -- accuracy/đạn đạo chỉ tính các vũ khí này. Nhận tên thô hoặc đã chuẩn hoá;
/// taser tính là súng. Port của isGunWeapon (constants.ts).
pub fn is_gun_weapon(weapon: &str) -> bool {
  let w = weapon.to_lowercase();
  !w.is_empty() && !non_gun().iter().any(|k| w.contains(k))
}
pub fn non_bullet() -> &'static [&'static str] {
  &["inferno", "molotov", "incendiary", "smokegrenade", "decoy", "flashbang"]
}
pub fn pickup_exclude() -> &'static [&'static str] {
  &["knife", "bayonet", "karambit", "vest", "vesthelm", "defuser", "c4"]
}
