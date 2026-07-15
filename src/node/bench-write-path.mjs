// ADR-007 §VI.2o (cs2-analytics) -- direction 1 of the 3-way write-path investigation: does
// reusing a single Node Buffer for the "read round back from temp file, insert into Postgres"
// loop measurably lower peak RSS versus allocating a fresh Buffer per round + relying on V8 GC
// heuristics between awaits? bench-stream-tofile.mjs only measured the Rust native call itself;
// this script covers the phase that call deliberately never touches: reading the round bytes
// back and writing them into the real `replay_chunks` table, exactly as production would.
//
// Usage: node bench-write-path.mjs <demo.dem> <naive|pooled> <postgres-connection-string>
import { createRequire } from 'node:module';
import { open } from 'node:fs/promises';
import pg from 'pg';

const require = createRequire(import.meta.url);
const demo = require('./index.js');

const [, , demoPath, mode, connStr] = process.argv;
if (!demoPath || !mode || !connStr) {
  console.error('usage: node bench-write-path.mjs <demo.dem> <naive|pooled> <postgres-connection-string>');
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

let peakRss = 0;
const sample = () => {
  const rss = process.memoryUsage().rss;
  if (rss > peakRss) peakRss = rss;
};
const sampler = setInterval(sample, 20);

const outPath = `/tmp/bench-write-path-${mode}.bin`;
const matchId = `bench-${mode}-${process.pid}`;

const client = new pg.Client({ connectionString: connStr });
await client.connect();
// Clean slate for this matchId (repeated runs / previous crashed run).
await client.query('DELETE FROM replay_chunks WHERE "matchId" = $1', [matchId]);

const rounds = [];
const t0 = performance.now();

// Native call: fully synchronous, writes zstd-compressed bytes straight to outPath, JS callback
// only ever sees small numbers (never a Buffer) -- this part is unaffected by which of the two
// modes below we're testing; only what happens AFTER this call returns differs.
const result = demo.parseTicksStreamingToFile(demoPath, wantedProps, outPath, (round) => {
  rounds.push(round);
  sample();
});

const fh = await open(outPath, 'r');
const maxLen = Math.max(...rounds.map((r) => r.len));
// `pooled` mode: allocate ONCE, reuse for every round via fsPromises.read() filling this same
// buffer (async -- does not block the event loop, unlike readSync). `naive` mode allocates a
// fresh Buffer.alloc(len) per round, standing in for the current fs.read()-with-no-buffer-arg
// behavior that relies on GC catching up between awaits.
const sharedBuffer = mode === 'pooled' ? Buffer.allocUnsafe(maxLen) : null;

for (const round of rounds) {
  const buf = mode === 'pooled' ? sharedBuffer : Buffer.allocUnsafe(round.len);
  await fh.read(buf, 0, round.len, round.offset);
  const data = buf.subarray(0, round.len);
  await client.query(
    `INSERT INTO replay_chunks ("matchId", "roundNumber", format, "tickStart", "tickEnd", "sampleStep", "playerCount", data)
     VALUES ($1, $2, 1, $3, $4, 8, 0, $5)
     ON CONFLICT ("matchId", "roundNumber") DO UPDATE SET data = EXCLUDED.data`,
    [matchId, round.tick, round.tick, round.tick, data],
  );
  sample();
}
await fh.close();

const secs = (performance.now() - t0) / 1000;
sample();
clearInterval(sampler);

await client.query('DELETE FROM replay_chunks WHERE "matchId" = $1', [matchId]);
await client.end();

console.log(
  `[write-path:${mode}] wall ${secs.toFixed(3)}s rounds=${rounds.length} tailRows=${result.tailRows} ` +
  `peakRSS=${(peakRss / 1e6).toFixed(1)}MB`,
);
