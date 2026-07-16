//! ADR-007 lever #1 measurement — "gộp decode". full_pipeline.rs decodes the whole demo 5 separate
//! times (events / grenades / playerInfo / ticks-sampled / ticks-aim), each a fresh parse_demo().
//! This bench times each pass in isolation, then times ONE fused header-only pass (union of the 3
//! only_header passes) to measure how much CPU fusing 3->1 actually saves, and how big the decode
//! budget is overall. Read-only; no parity impact. Median of N iters (one warm-up dropped).
//!
//! Usage: decode_fusion_bench <demo.dem> [iters=5]

use ahash::AHashMap;
use memmap2::MmapOptions;
use parser::first_pass::parser_settings::{rm_user_friendly_names, ParserInputs};
use parser::parse_demo::{Parser, ParsingMode};
use parser::second_pass::parser_settings::create_huffman_lookup_table;
use std::env;
use std::fs::File;
use std::time::Instant;

fn all_event_names() -> Vec<String> {
    ["player_death","round_end","round_start","bomb_planted","bomb_defused","bomb_dropped","bomb_pickup",
     "smokegrenade_detonate","smokegrenade_expired","inferno_startburn","inferno_expire","hegrenade_detonate",
     "flashbang_detonate","player_hurt","weapon_fire","player_blind","item_pickup","buytime_ended"]
        .iter().map(|s| s.to_string()).collect()
}
fn all_event_player_fields() -> Vec<String> {
    ["X","Y","Z","attacker_X","attacker_Y","attacker_Z","team_num","attacker_team_num","user_team_num",
     "site","user_X","user_Y","user_Z","yaw","pitch","velocity_X","velocity_Y"]
        .iter().map(|s| s.to_string()).collect()
}
fn all_event_other_fields() -> Vec<String> {
    ["total_rounds_played","round_start_time","winner","reason"].iter().map(|s| s.to_string()).collect()
}
fn sampled_tick_fields() -> Vec<String> {
    ["X","Y","Z","pitch","yaw","is_alive","team_num","health","armor_value","active_weapon_name",
     "active_weapon_ammo","balance","current_equip_value","has_helmet","has_defuser","inventory",
     "flash_duration","last_place_name","is_defusing","is_scoped","velocity_X","velocity_Y","velocity_Z",
     "duck_amount","is_walking"]
        .iter().map(|s| s.to_string()).collect()
}
fn aim_tick_fields() -> Vec<String> {
    ["X","Y","Z","pitch","yaw","spotted","is_alive"].iter().map(|s| s.to_string()).collect()
}
// union(sampled, aim): sampled already has X,Y,Z,pitch,yaw,is_alive -> only "spotted" is new.
fn fused_tick_fields() -> Vec<String> {
    let mut v = sampled_tick_fields();
    v.push("spotted".to_string());
    v
}
const PLAYER_TICK_SAMPLE_STEP: i64 = 8;

fn base<'a>(huf: &'a Vec<(u8, u8)>) -> ParserInputs<'a> {
    ParserInputs {
        real_name_to_og_name: AHashMap::default(),
        wanted_players: vec![], wanted_player_props: vec![], wanted_other_props: vec![],
        wanted_prop_states: AHashMap::default(), wanted_events: vec![], wanted_ticks: vec![],
        parse_ents: true, parse_projectiles: false, only_header: true, list_props: false,
        only_convars: false, huffman_lookup_table: huf, order_by_steamid: false,
        fallback_bytes: None, parse_grenades: false,
    }
}

// ---- the passes exactly as full_pipeline.rs builds them ----
fn s_events<'a>(huf: &'a Vec<(u8, u8)>) -> ParserInputs<'a> {
    let np = rm_user_friendly_names(&all_event_player_fields()).unwrap();
    let no = rm_user_friendly_names(&all_event_other_fields()).unwrap();
    ParserInputs { wanted_player_props: np, wanted_other_props: no, wanted_events: all_event_names(), ..base(huf) }
}
fn s_grenades<'a>(huf: &'a Vec<(u8, u8)>) -> ParserInputs<'a> {
    ParserInputs { parse_projectiles: true, ..base(huf) }
}
fn s_playerinfo<'a>(huf: &'a Vec<(u8, u8)>) -> ParserInputs<'a> {
    ParserInputs { parse_ents: false, ..base(huf) }
}
fn s_ticks<'a>(huf: &'a Vec<(u8, u8)>, fields: Vec<String>, ticks: Vec<i32>) -> ParserInputs<'a> {
    let np = rm_user_friendly_names(&fields).unwrap();
    ParserInputs { wanted_player_props: np, wanted_ticks: ticks, only_header: false, ..base(huf) }
}
// FUSED: union of the 3 only_header passes in ONE decode (events + grenade projectiles + playerInfo).
fn s_fused<'a>(huf: &'a Vec<(u8, u8)>) -> ParserInputs<'a> {
    let np = rm_user_friendly_names(&all_event_player_fields()).unwrap();
    let no = rm_user_friendly_names(&all_event_other_fields()).unwrap();
    ParserInputs { wanted_player_props: np, wanted_other_props: no, wanted_events: all_event_names(),
                   parse_projectiles: true, parse_ents: true, ..base(huf) }
}

fn time_pass<'a, F>(mmap: &[u8], make: F, iters: usize) -> f64
where F: Fn() -> (ParserInputs<'a>, ParsingMode) {
    // one warm-up (page cache / branch predictors), excluded
    {
        let (s, m) = make();
        let mut p = Parser::new(s, m);
        std::hint::black_box(&p.parse_demo(mmap).expect("parse").df);
    }
    let mut ts: Vec<f64> = Vec::with_capacity(iters);
    for _ in 0..iters {
        let (s, m) = make();
        let mut p = Parser::new(s, m);
        let t = Instant::now();
        let out = p.parse_demo(mmap).expect("parse");
        ts.push(t.elapsed().as_secs_f64());
        std::hint::black_box(&out.df);
    }
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ts[ts.len() / 2]
}

fn main() {
    let demo = env::args().nth(1).expect("usage: decode_fusion_bench <demo.dem> [iters]");
    let iters: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let huf = create_huffman_lookup_table();
    let file = File::open(&demo).expect("open");
    let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };
    let bytes: &[u8] = &mmap[..];
    println!("demo: {demo}  ({:.1} MB)  iters={iters} (median)\n", mmap.len() as f64 / 1e6);

    // sampled-tick set: every 8th up to 400k (parser only emits ticks that exist).
    let mut sampled: Vec<i32> = Vec::new();
    let mut t = 0i64;
    while t <= 400_000 { sampled.push(t as i32); t += PLAYER_TICK_SAMPLE_STEP; }
    // aim-tick set: SPARSE (every 512th) — represents ~kill-window ticks. Used to prove ST decode is
    // walk-bound (a sparse tick set still costs a full sequential demo walk).
    let mut aim_ticks: Vec<i32> = Vec::new();
    let mut a = 0i64;
    while a <= 400_000 { aim_ticks.push(a as i32); a += 512; }
    // fused-tick set: union(sampled, aim) — walk-bound so ~= sampled cost.
    let mut fused_ticks = sampled.clone();
    fused_ticks.extend(&aim_ticks);
    fused_ticks.sort_unstable(); fused_ticks.dedup();

    let ev = time_pass(bytes, || (s_events(&huf), ParsingMode::Normal), iters);
    let gr = time_pass(bytes, || (s_grenades(&huf), ParsingMode::Normal), iters);
    let pi = time_pass(bytes, || (s_playerinfo(&huf), ParsingMode::Normal), iters);
    let fused_h = time_pass(bytes, || (s_fused(&huf), ParsingMode::Normal), iters);
    let sp = time_pass(bytes, || (s_ticks(&huf, sampled_tick_fields(), sampled.clone()), ParsingMode::ForceSingleThreaded), iters);
    let aim = time_pass(bytes, || (s_ticks(&huf, aim_tick_fields(), aim_ticks.clone()), ParsingMode::ForceSingleThreaded), iters);
    let fused_t = time_pass(bytes, || (s_ticks(&huf, fused_tick_fields(), fused_ticks.clone()), ParsingMode::ForceSingleThreaded), iters);

    let header_sum = ev + gr + pi;
    println!("=== (a) 3 luot only_header (Normal/MT) — sampled={} aim_ticks={} ===", sampled.len(), aim_ticks.len());
    println!("  events      : {ev:.3}s");
    println!("  grenades    : {gr:.3}s");
    println!("  playerInfo  : {pi:.3}s");
    println!("  TONG 3 luot : {header_sum:.3}s");
    println!("  FUSED 1 luot: {fused_h:.3}s   => tiet kiem {:.3}s ({:.0}%)\n",
             header_sum - fused_h, 100.0 * (header_sum - fused_h) / header_sum);

    let tick_sum = sp + aim;
    println!("=== (b) 2 luot tick (ForceST) ===");
    println!("  ticks-sampled (25 field, {} tick) : {sp:.3}s", sampled.len());
    println!("  ticks-aim     ( 7 field, {} tick, THUA): {aim:.3}s   <- thua ma van ~sampled => DECODE WALK-BOUND", aim_ticks.len());
    println!("  TONG 2 luot tick : {tick_sum:.3}s");
    println!("  FUSED-tick 1 luot: {fused_t:.3}s   => tiet kiem {:.3}s ({:.0}%)\n",
             tick_sum - fused_t, 100.0 * (tick_sum - fused_t) / tick_sum);

    let all5 = header_sum + tick_sum;
    let opt_header = fused_h + tick_sum;
    let opt_tick = header_sum + fused_t;
    let opt_both = fused_h + fused_t;
    println!("=== TONG KET decode-only ({} luot -> it hon) ===", 5);
    println!("  5 luot roi (hien tai): {all5:.3}s");
    println!("  chi fuse 3 header    : {opt_header:.3}s  (giam {:.0}%)", 100.0*(all5-opt_header)/all5);
    println!("  chi fuse 2 tick      : {opt_tick:.3}s  (giam {:.0}%)", 100.0*(all5-opt_tick)/all5);
    println!("  fuse CA header+tick  : {opt_both:.3}s  (giam {:.0}%)", 100.0*(all5-opt_both)/all5);
}
