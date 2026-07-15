// Ported from packages/parse-core/src/constants.ts -- only the constants used by the "stats"
// domain (computeTickAggregates' survivor/roster/buy windows). Weapon-name/hitgroup logic reuses
// compute_events::helpers (norm_weapon_name/hitgroup_to_int), no duplication needed here.

pub const SURVIVOR_WINDOW: i64 = 128; // cửa sổ cuối round (survivor)
pub const ROSTER_WINDOW: i64 = 640; // 10s đầu round (side/roster)
pub const BUY_WINDOW: i64 = 1280; // ~20s buy phase (economy)
