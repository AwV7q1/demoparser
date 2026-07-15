// ADR-007 §VI.2n (cs2-analytics): measures REAL peak RSS through Node.js for
// parse_ticks_streaming_to_file with zstd wired in (§VI.2j), so the RAM number isn't just the
// pure-Rust bin harness (parse_stream_bench) anymore but the actual N-API path production would
// call. Compare against the prior N-API numbers in the ADR (bulk parseTicks 429.5MB, streaming
// summary-only 240.5MB, streaming+JSON 382.8MB, streaming+real codec pre-zstd 379.7MB/279.7MB
// direct-to-file) -- this run adds real zstd compression on top of the direct-to-file variant.
//
// Usage: node bench-stream-tofile.mjs <demo.dem> <out.bin>
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const demo = require('./index.js');

const demoPath = process.argv[2];
const outPath = process.argv[3] || '/tmp/stream-tofile-out.bin';
if (!demoPath) {
  console.error('usage: node bench-stream-tofile.mjs <demo.dem> <out.bin>');
  process.exit(1);
}

const wantedProps = [
  'tick', 'health', 'X', 'Y', 'Z',
  'velocity_X', 'velocity_Y', 'velocity_Z',
  'CCSPlayerPawn.m_angEyeAngles',
  'is_alive', 'team_num', 'active_weapon_name',
  'FORWARD', 'LEFT', 'RIGHT', 'BACK', 'FIRE', 'is_walking', 'is_airborne',
  'flash_duration', 'armor_value', 'balance',
];

let rounds = 0;
let totalLen = 0;
let totalRawLen = 0;
let peakRss = 0;
const sample = () => {
  const rss = process.memoryUsage().rss;
  if (rss > peakRss) peakRss = rss;
};

const sampler = setInterval(sample, 20);
const t0 = performance.now();
const result = demo.parseTicksStreamingToFile(demoPath, wantedProps, outPath, (round) => {
  rounds++;
  totalLen += round.len;
  totalRawLen += round.rawLen;
  sample();
});
sample();
clearInterval(sampler);
const secs = (performance.now() - t0) / 1000;

console.log(
  `[stream-to-file+zstd via N-API] wall ${secs.toFixed(3)}s rounds=${rounds} ` +
  `raw=${totalRawLen} compressed=${totalLen} tailRows=${result.tailRows} ` +
  `peakRSS=${(peakRss / 1e6).toFixed(1)}MB`,
);
