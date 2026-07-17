# ADR-007 ④ online-aggregate (round-streaming tick) — lưu trữ tham khảo

**Trạng thái: KHÔNG deploy.** Nhánh này lưu lại toàn bộ code đã build + verify cho đòn tối ưu RAM
"④ online-aggregate" (parse xong round nào nén+gộp thống kê+vứt tick round đó ngay, thay vì giữ
nguyên tick cả trận trong RAM rồi mới xử lý) — để tham khảo sau, KHÔNG dùng trong production.

## Vì sao không deploy

Tiền đề ban đầu (tài liệu derisk cũ): parse 1 demo giữ nguyên tick cả trận trong RAM → đỉnh RAM
~475MB/job; nếu xử lý xong round nào vứt round đó → đỉnh RAM còn ~300MB/job (giảm ~175MB, ~37%).

**Đo RAM thật (demo thật 174.5MB, peak VmHWM qua `/proc/self/status`, chạy cách ly từng bản trong
Docker) cho kết quả khác hẳn:**

| Allocator | Materialize (cũ) | Streaming (④, mới) | Giảm được |
|---|---|---|---|
| glibc (mặc định Docker `rust:bookworm`) | ~595MB | ~569MB | ~26MB (~4.5%) |
| **jemalloc (đúng config production — xem `Dockerfile` root repo `cs2-analytics`, `LD_PRELOAD`+`MALLOC_CONF=...,dirty_decay_ms:0,muzzy_decay_ms:0`)** | **~469MB** | **~461MB** | **~7MB (~1.5%)** |

Số ~469MB (materialize, dưới jemalloc) khớp gần đúng với con số ~475MB đã ghi sẵn trong
`cs2-analytics/CLAUDE.md` ("RAM VPS ≈ PARSE_CONCURRENCY × ~475MB/job dưới jemalloc") — xác nhận
cách đo phản ánh đúng hành vi production thật, không phải sai số công cụ đo.

**Nguyên nhân đỉnh RAM thật không nằm ở tick materialize:** đào sâu bằng cách cắm thêm điểm đo RSS
tại từng bước trong pipeline (không còn trong code, đã gỡ sau khi xác nhận xong) phát hiện: ~95%
đỉnh RAM đã bị chiếm dụng TRƯỚC KHI bước xử lý tick (dù cũ hay mới) kịp chạy — nằm ở giai đoạn parse
events/player_info + parse grenades + `compute_events` (2-3 lượt quét demo riêng biệt, mỗi lượt tự
có đỉnh RSS tạm thời rồi hụt xuống). Dưới glibc, đỉnh lịch sử (VmHWM) cộng dồn qua các lượt vì
allocator không trả bộ nhớ lại OS ngay; dưới jemalloc (purge cực mạnh,
`dirty_decay_ms:0,muzzy_decay_ms:0`) hiệu ứng cộng dồn này gần như biến mất — nên khoảng cách "cũ
vs mới" ở jemalloc còn nhỏ hơn cả glibc (7MB so với 26MB), chứ không phải lớn hơn.

**Kết luận: đòn ④ không đáng đánh đổi.** ~7MB không bõ thêm độ phức tạp/rủi ro thật (cơ chế
`round_flush`/`flush_at_ticks`/velocity-carry mới chạy trong parser production). Nếu muốn giảm RAM
tiếp, hướng có tiềm năng hơn là giảm SỐ LƯỢT quét demo riêng biệt (events+player_info và grenades
đang là 2 lượt tách biệt — xem comment ở bước 1 của `run_full_pipeline_core`,
`.claude/note/adr-007-header-fusion-and-resolve-cost-followup.md` bên repo `cs2-analytics` từng từ
chối gộp 2 lượt này vì lý do đúng-sai dữ liệu — CHƯA re-verify hướng này có còn đúng không), KHÔNG
phải hướng tick-streaming.

## Nội dung đã build (Phase 1-4, parity-verified, KHÔNG deploy)

- **Phase 1**: carry-cache velocity qua ranh giới round (giữ 2 dòng tick gần nhất/người chơi thay
  vì vứt sạch) + cơ chế `flush_at_ticks` (ranh giới round lấy từ round_end thật của bước parse
  events, thay cho cơ chế nội bộ `round_boundary_hit` không đáng tin cậy — xem
  `parser_settings.rs`).
- **Phase 2**: `build_replay_chunks` nhận dữ liệu theo từng round (đã có sẵn vòng lặp per-round bên
  trong, chỉ cần tách hàm).
- **Phase 3**: fold `compute_tick_aggregates` (survivor/economy/zone) theo từng round, `ZoneFold`
  làm accumulator giữ thứ tự.
- **Phase 4**: `run_full_pipeline_core_streaming` (trong `src/node/src/full_pipeline.rs`) — pipeline
  đầy đủ dùng round-streaming, chạy song song với `run_full_pipeline_core` cũ (không thay thế) để
  so sánh trực tiếp.
- Test parity: `mod b4_round_streaming_parity` trong `full_pipeline.rs` — tất cả PASS, output khớp
  tuyệt đối bit-for-bit với bản materialize cũ (kể cả replay chunk nén, sau khi vá 1 bug thật phát
  hiện được: `build_replay_chunks` lọc rõ `[start_tick,end_tick]`, streaming ban đầu không lọc nên
  lẫn tick buy-time vào round kế tiếp — đã fix).
- 2 test đo RAM thủ công (`#[ignore]`, cần set `RAM_BENCH_DEMO=/path/to/real.dem`):
  `bench_ram_materialize` / `bench_ram_streaming` trong `full_pipeline.rs`.

## Bug thật tìm được, đã tách sang nhánh `main` (KHÔNG nằm trong nhánh archive này)

Trong lúc build Phase 3, phát hiện `compute_rounds` (rounds.rs) có bug LIVE PRODUCTION thật:
`round_start_tick_by_num` dùng công thức `total_rounds_played + 1` (sao chép nhầm từ quy ước
kill/grenade event) thay vì `total_rounds_played` — khiến MỌI `round.start_tick` bị lệch sang round
vật lý TRƯỚC đó. Bug này ảnh hưởng `compute_full_pipeline_async` (entry point production thật, xem
`nativeDemoEngine.ts` bên `cs2-analytics`), KHÔNG liên quan gì đến đòn ④ — đã fix riêng trên `main`
(không nằm trong nhánh archive này). Cùng bug tồn tại ở `packages/parse-core/src/compute.ts`
(`computeRounds`, repo `cs2-analytics`) — CHƯA fix bên đó (khác repo/session).
