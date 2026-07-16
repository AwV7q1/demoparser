//! ADR-007 "hướng 1" (cs2-analytics .claude/note/plan-128tick-tick-decode-optimization.md §3,
//! "Gộp 3 lượt header") -- đo lại con số −13% (ghi trong adr-007-decode-fusion-measurement.md,
//! đo trên host 16-core dev, KHÔNG checked-in) bằng 1 bin thật trong repo, chạy dưới `--cpus=4`
//! (khớp harness GĐ5) để có số đại diện VPS 4 vCPU thay vì host 16-core.
//!
//! Đo THUẦN TÚY thời gian raw (3 lượt only_header rời vs 1 lượt gộp union-flag) -- KHÔNG cài đặt
//! lớp lọc prop_infos/df cần để lượt gộp ra ĐÚNG shape JSON như 3 lượt rời (xem
//! `slice_tick_columns` trong full_pipeline.rs -- đó là kỹ thuật cần lặp lại cho hướng này nếu
//! làm production). Bin này chỉ trả lời "có đáng công lọc đó không" bằng số đo thật trước khi viết.
//!
//! Usage: header_fusion_bench <demo.dem> [iters=5]

use ahash::AHashMap;
use memmap2::MmapOptions;
use parser::first_pass::parser_settings::{rm_user_friendly_names, ParserInputs};
use parser::parse_demo::{Parser, ParsingMode};
use parser::second_pass::parser_settings::create_huffman_lookup_table;
use std::env;
use std::fs::File;
use std::time::Instant;

// -- field lists: y hệt full_pipeline.rs (all_event_names/all_event_player_fields/
// all_event_other_fields) -- copy lại vì bin là crate root riêng, không import được `fn` private
// của crate `laihoe_demoparser2` (src/node), chỉ dùng chung crate `parser`.
fn all_event_names() -> Vec<String> {
  [
    "player_death", "round_end", "round_start",
    "bomb_planted", "bomb_defused", "bomb_dropped", "bomb_pickup",
    "smokegrenade_detonate", "smokegrenade_expired",
    "inferno_startburn", "inferno_expire",
    "hegrenade_detonate", "flashbang_detonate",
    "player_hurt", "weapon_fire", "player_blind",
    "item_pickup", "buytime_ended",
  ].iter().map(|s| s.to_string()).collect()
}
fn all_event_player_fields() -> Vec<String> {
  [
    "X", "Y", "Z", "attacker_X", "attacker_Y", "attacker_Z",
    "team_num", "attacker_team_num", "user_team_num",
    "site", "user_X", "user_Y", "user_Z", "yaw",
    "pitch", "velocity_X", "velocity_Y",
  ].iter().map(|s| s.to_string()).collect()
}
fn all_event_other_fields() -> Vec<String> {
  ["total_rounds_played", "round_start_time", "winner", "reason"].iter().map(|s| s.to_string()).collect()
}

fn mmap_path(path: &str) -> memmap2::Mmap {
  let file = File::open(path).expect("open demo");
  unsafe { MmapOptions::new().map(&file).expect("mmap") }
}

fn name_map(props: &Vec<String>) -> AHashMap<String, String> {
  let real = rm_user_friendly_names(props).expect("rm_user_friendly_names");
  real.iter().zip(props).map(|(r, o)| (r.clone(), o.clone())).collect()
}

// -- (a) 3 lượt rời, y hệt run_parse_events/run_parse_grenades/run_parse_player_info --

fn run_events(path: &str, huf: &Vec<(u8, u8)>) -> f64 {
  let player_props = all_event_player_fields();
  let other_props = all_event_other_fields();
  let mut real_name_to_og_name = name_map(&player_props);
  real_name_to_og_name.extend(name_map(&other_props));
  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: rm_user_friendly_names(&player_props).unwrap(),
    wanted_other_props: rm_user_friendly_names(&other_props).unwrap(),
    wanted_prop_states: AHashMap::default(),
    wanted_events: all_event_names(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mmap = mmap_path(path);
  let t = Instant::now();
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let out = parser.parse_demo(&mmap).expect("parse events");
  std::hint::black_box(&out.game_events);
  t.elapsed().as_secs_f64()
}

fn run_grenades(path: &str, huf: &Vec<(u8, u8)>) -> f64 {
  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_events: vec![],
    wanted_prop_states: AHashMap::default(),
    parse_ents: true,
    wanted_ticks: vec![],
    parse_projectiles: true,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mmap = mmap_path(path);
  let t = Instant::now();
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let out = parser.parse_demo(&mmap).expect("parse grenades");
  std::hint::black_box(&out.df);
  t.elapsed().as_secs_f64()
}

fn run_player_info(path: &str, huf: &Vec<(u8, u8)>) -> f64 {
  let settings = ParserInputs {
    wanted_players: vec![],
    real_name_to_og_name: AHashMap::default(),
    wanted_player_props: vec![],
    wanted_other_props: vec![],
    wanted_prop_states: AHashMap::default(),
    wanted_events: vec![],
    parse_ents: false,
    wanted_ticks: vec![],
    parse_projectiles: false,
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mmap = mmap_path(path);
  let t = Instant::now();
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let out = parser.parse_demo(&mmap).expect("parse player_info");
  std::hint::black_box(&out.player_md);
  t.elapsed().as_secs_f64()
}

// -- (b) 1 lượt gộp union-flag: parse_ents | parse_projectiles | wanted_events tất cả bật cùng
// lúc. Đo RAW thời gian walk -- KHÔNG lọc output cho đúng shape (xem doc comment đầu file).
fn run_merged(path: &str, huf: &Vec<(u8, u8)>) -> f64 {
  let player_props = all_event_player_fields();
  let other_props = all_event_other_fields();
  let mut real_name_to_og_name = name_map(&player_props);
  real_name_to_og_name.extend(name_map(&other_props));
  let settings = ParserInputs {
    real_name_to_og_name,
    wanted_players: vec![],
    wanted_player_props: rm_user_friendly_names(&player_props).unwrap(),
    wanted_other_props: rm_user_friendly_names(&other_props).unwrap(),
    wanted_prop_states: AHashMap::default(),
    wanted_events: all_event_names(),
    parse_ents: true,        // union: events(true) | grenades(true) | player_info(false)
    wanted_ticks: vec![],
    parse_projectiles: true, // union: events(false) | grenades(true) | player_info(false)
    only_header: true,
    list_props: false,
    only_convars: false,
    huffman_lookup_table: huf,
    order_by_steamid: false,
    fallback_bytes: None,
    parse_grenades: false,
  };
  let mmap = mmap_path(path);
  let t = Instant::now();
  let mut parser = Parser::new(settings, ParsingMode::Normal);
  let out = parser.parse_demo(&mmap).expect("parse merged header");
  std::hint::black_box((&out.game_events, &out.df, &out.player_md));
  t.elapsed().as_secs_f64()
}

fn median(mut v: Vec<f64>) -> f64 {
  v.sort_by(|a, b| a.partial_cmp(b).unwrap());
  v[v.len() / 2]
}

fn bench(label: &str, iters: usize, mut f: impl FnMut() -> f64) -> f64 {
  let _ = f(); // warm-up (page cache), loại khỏi thống kê
  let times: Vec<f64> = (0..iters).map(|_| f()).collect();
  let m = median(times.clone());
  let fmt: Vec<String> = times.iter().map(|t| format!("{t:.3}")).collect();
  println!("{label:>16}: median {m:.3}s  (n={iters}, all=[{}])", fmt.join(", "));
  m
}

fn main() {
  let demo_path = env::args().nth(1).expect("usage: header_fusion_bench <demo.dem> [iters=5]");
  let iters: usize = env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(5);
  let huf = create_huffman_lookup_table();

  let mmap_probe = mmap_path(&demo_path);
  println!("demo: {demo_path}  ({:.1} MB)", mmap_probe.len() as f64 / 1e6);
  drop(mmap_probe);

  let e = bench("events", iters, || run_events(&demo_path, &huf));
  let g = bench("grenades", iters, || run_grenades(&demo_path, &huf));
  let p = bench("playerInfo", iters, || run_player_info(&demo_path, &huf));
  let sum3 = e + g + p;
  let m = bench("MERGED (1 pass, KHÔNG an toàn)", iters, || run_merged(&demo_path, &huf));

  println!("\n3 lượt rời : {sum3:.3}s");
  println!("1 lượt gộp (raw, unsafe) : {m:.3}s  ({:+.1}%)", (m - sum3) / sum3 * 100.0);

  // Safe subset thật (production, xem run_parse_events_and_player_info trong full_pipeline.rs):
  // events' settings là superset của playerInfo -- chạy CHUNG 1 pass với đúng settings của events,
  // grenades giữ nguyên riêng. Đo lại timing thật của cấu hình 2-lượt (thay vì 3) này.
  let ep = bench("events+playerInfo (SAFE merged)", iters, || run_events(&demo_path, &huf));
  let safe_total = ep + g;
  println!("\n2 lượt (events+playerInfo gộp AN TOÀN + grenades riêng): {safe_total:.3}s  ({:+.1}% so 3 lượt rời)", (safe_total - sum3) / sum3 * 100.0);
}
