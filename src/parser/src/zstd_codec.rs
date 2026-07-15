//! ADR-007 §VI.2 (cs2-analytics): zstd compression for round-tick payloads, done in Rust instead
//! of shipping raw bytes across N-API for Node to compress with `zstdCompressSync`. Level 3
//! matches Node zlib's `zstdCompressSync` default (`ZSTD_CLEVEL_DEFAULT`), so on-disk blob size
//! doesn't change from the current Node-side-compression production behavior.

pub fn compress(raw: &[u8], level: i32) -> std::io::Result<Vec<u8>> {
    zstd::stream::encode_all(raw, level)
}

pub fn decompress(compressed: &[u8]) -> std::io::Result<Vec<u8>> {
    zstd::stream::decode_all(compressed)
}
