//! Streaming-prototype benchmark (ADR-007 §VI.2, cs2-analytics): compares peak RAM of the
//! current bulk second-pass (accumulate everything, single call) against a per-round streaming
//! variant (drain self.output/self.game_events at each round boundary, handing them to a
//! callback that stands in for "encode+zstd then write", then continue with an empty
//! accumulator). Both modes run ForceSingleThreaded so the ONLY difference under test is
//! "hold the whole demo's decoded data" vs "hold at most one round's worth" -- multi-threaded
//! mode is a separate, orthogonal axis (see ADR-007 for why streaming requires ST).
//!
//! The flush hook itself lives in the library: `RoundFlushChunk`/`with_round_flush` in
//! second_pass/parser_settings.rs, the ungated round-boundary flag in entities.rs
//! (parse_packet_ents), and the drain call site in parser.rs (right after collect_entities()).
//!
//! Usage: parse_stream_bench <demo.dem> [bulk|stream]
//! Run bulk and stream as SEPARATE process invocations -- Windows' PeakWorkingSetSize is an
//! all-time high for the process, so running both modes in one process would let an earlier
//! mode's peak leak into the other's number.

use ahash::AHashMap;
use memmap2::MmapOptions;
use parser::first_pass::parser::FirstPassOutput;
use parser::first_pass::parser_settings::{FirstPassParser, ParserInputs};
use parser::first_pass::read_bits::DemoParserError;
use parser::second_pass::parser_settings::{create_huffman_lookup_table, RoundFlushChunk, SecondPassParser};
use std::cell::RefCell;
use std::env;
use std::fs::File;
use std::rc::Rc;
use std::time::Instant;

/// Same representative prop set as parse_bench.rs, so bulk/stream numbers here are comparable
/// to the existing ST/MT speed baseline.
fn wanted_props() -> Vec<String> {
    [
        "tick", "health", "X", "Y", "Z",
        "velocity_X", "velocity_Y", "velocity_Z",
        "CCSPlayerPawn.m_angEyeAngles",
        "is_alive", "team_num", "active_weapon_name",
        "FORWARD", "LEFT", "RIGHT", "BACK", "FIRE", "is_walking", "is_airborne",
        "flash_duration", "armor_value", "balance",
    ]
    .iter().map(|s| s.to_string()).collect()
}

fn settings<'a>(huf: &'a Vec<(u8, u8)>) -> ParserInputs<'a> {
    let wanted = wanted_props();
    ParserInputs {
        wanted_player_props: wanted.clone(),
        // MUST stay empty: collect_data.rs's collect_entities() early-returns (skips prop
        // collection entirely) whenever wanted_events is non-empty. Round-boundary detection
        // here does NOT depend on wanted_events (see entities.rs) so this is safe.
        wanted_events: vec![],
        real_name_to_og_name: AHashMap::default(),
        wanted_other_props: wanted,
        parse_ents: true,
        wanted_players: vec![],
        wanted_ticks: vec![],
        parse_projectiles: false,
        parse_grenades: false,
        only_header: false,
        list_props: false,
        only_convars: false,
        huffman_lookup_table: huf,
        order_by_steamid: false,
        wanted_prop_states: AHashMap::default(),
        fallback_bytes: None,
    }
}

#[derive(Default)]
struct StreamStats {
    rounds: usize,
    total_events: usize,
    total_rows: usize,
}

fn run_bulk(mmap: &[u8], huf: &Vec<(u8, u8)>) {
    use parser::parse_demo::{Parser, ParsingMode};
    let mut p = Parser::new(settings(huf), ParsingMode::ForceSingleThreaded);
    let t = Instant::now();
    let out = p.parse_demo(mmap).expect("parse");
    let secs = t.elapsed().as_secs_f64();
    let rows: usize = out.df.values().map(|c| c.len()).sum();
    println!(
        "[bulk]   wall {:.3}s  cols={}  rows={}  events={}",
        secs,
        out.df.len(),
        rows,
        out.game_events.len()
    );
    std::hint::black_box(&out);
}

fn run_stream(mmap: &[u8], huf: &Vec<(u8, u8)>) -> Result<(), DemoParserError> {
    let input = settings(huf);
    let mut first_pass_parser = FirstPassParser::new(&input);
    let first_pass_output: FirstPassOutput = first_pass_parser.parse_demo(mmap, false)?;

    let stats = Rc::new(RefCell::new(StreamStats::default()));
    let stats_cb = stats.clone();
    let cb: Box<dyn FnMut(RoundFlushChunk)> = Box::new(move |chunk: RoundFlushChunk| {
        let mut s = stats_cb.borrow_mut();
        s.rounds += 1;
        s.total_events += chunk.game_events.len();
        s.total_rows += chunk.output.values().map(|c| c.len()).sum::<usize>();
        // chunk drops here -> frees this round's props/events. Stands in for
        // "columnar+quantize+zstd encode, then write" (already prototyped separately,
        // byte-identical -- see prototypes/adr-007-parse-core/README.md).
    });

    // offset=16, parse_all_packets=true, start_end_offset=None: same args
    // second_pass_single_threaded() uses in parse_demo.rs.
    let mut second_pass = SecondPassParser::new(first_pass_output.clone(), 16, true, None)?.with_round_flush(cb);
    let t = Instant::now();
    second_pass.start(mmap)?;
    let secs = t.elapsed().as_secs_f64();

    // Whatever's left after the last round boundary (match-end tail) is still in the live
    // accumulator -- count it so total rows/events can be sanity-checked against bulk mode.
    let tail_rows: usize = second_pass.output.values().map(|c| c.len()).sum();
    let tail_events = second_pass.game_events.len();

    let s = stats.borrow();
    println!(
        "[stream] wall {:.3}s  rounds_flushed={}  rows={}  events={}  (+ tail rows={} events={})",
        secs,
        s.rounds,
        s.total_rows,
        s.total_events,
        tail_rows,
        tail_events
    );
    Ok(())
}

/// Same as run_stream, but the round_flush callback does the REAL encode (tick_codec::
/// encode_round_tick_body -- dict/quantize/pack, byte-identical to replay-codec-core.ts) then
/// drops the resulting bytes, instead of just counting. Isolates: does the ENCODING
/// computation itself (Vec<Row> intermediate, dictionaries, packed column buffers) cost
/// meaningful RAM on its own, with NO Node/V8 involved at all?
fn run_stream_encode(mmap: &[u8], huf: &Vec<(u8, u8)>) -> Result<(), DemoParserError> {
    let input = settings(huf);
    let mut first_pass_parser = FirstPassParser::new(&input);
    let first_pass_output: FirstPassOutput = first_pass_parser.parse_demo(mmap, false)?;
    let prop_infos = first_pass_output.prop_controller.prop_infos.clone();
    let name_to_id = parser::tick_codec::build_name_to_id(&prop_infos);

    let stats = Rc::new(RefCell::new(StreamStats::default()));
    let stats_cb = stats.clone();
    let cb: Box<dyn FnMut(RoundFlushChunk)> = Box::new(move |chunk: RoundFlushChunk| {
        let mut s = stats_cb.borrow_mut();
        s.rounds += 1;
        s.total_events += chunk.game_events.len();
        let encoded = parser::tick_codec::encode_round_tick_body(&chunk, &name_to_id);
        s.total_rows += encoded.len(); // reuse total_rows field to report total encoded bytes
        std::hint::black_box(&encoded);
        // encoded drops here -> frees this round's encoded bytes, same as chunk did in run_stream
    });

    let mut second_pass = SecondPassParser::new(first_pass_output.clone(), 16, true, None)?.with_round_flush(cb);
    let t = Instant::now();
    second_pass.start(mmap)?;
    let secs = t.elapsed().as_secs_f64();

    let s = stats.borrow();
    println!(
        "[stream-encode] wall {:.3}s  rounds_flushed={}  total_encoded_bytes={}  events={}",
        secs, s.rounds, s.total_rows, s.total_events
    );
    Ok(())
}

fn main() {
    let demo_path = env::args().nth(1).expect("usage: parse_stream_bench <demo.dem> [bulk|stream|stream-encode]");
    let mode = env::args().nth(2).unwrap_or_else(|| "stream".to_string());

    let huf = create_huffman_lookup_table();
    let file = File::open(&demo_path).expect("open demo");
    let mmap = unsafe { MmapOptions::new().map(&file).unwrap() };
    println!("demo: {demo_path}  ({:.1} MB)  mode={mode}", mmap.len() as f64 / 1e6);

    match mode.as_str() {
        "bulk" => run_bulk(&mmap, &huf),
        "stream" => run_stream(&mmap, &huf).expect("stream parse"),
        "stream-encode" => run_stream_encode(&mmap, &huf).expect("stream-encode parse"),
        other => panic!("mode must be bulk|stream|stream-encode, got {other}"),
    }

    #[cfg(windows)]
    print_peak_rss();
}

#[cfg(windows)]
fn print_peak_rss() {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) != 0 {
            println!("peak working set: {:.1} MB", counters.PeakWorkingSetSize as f64 / 1e6);
        }
    }
}
