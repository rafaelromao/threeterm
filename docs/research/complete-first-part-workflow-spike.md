# Complete First-Part Workflow Spike

Status: disposable L-shaped-bracket slice passed on 2026-07-30; issue 50 was closed by the owner after a concurrent second-run evidence consolidation.

## Question

Can one of the seven MVP validation parts traverse the selected command, worker, recomputation, viewport, persistence, validation, and automation boundaries coherently, and does that evidence answer the complete first-part usability and performance question?

## Scope and method

The disposable Rust prototype and C++ worker sources are retained at `/tmp/opencode/threeterm-first-part-spike/`. Final copied evidence is at `/tmp/opencode/threeterm-first-part-spike-final/`. No production application code was created.

A rootless `archlinux:latest` Podman container built and ran:

- Rust host `1:1.97.1-1`;
- OCCT `1:7.9.3-3` disposable workers;
- SolveSpace `libslvs` v3.2 at the previously pinned source revision, in disposable workers;
- lib3mf `2.5.0-1` as a separate canonical-mesh-to-3MF adapter.

The host machine was an Intel Core i7-7700K with 4 cores/8 threads, 33,571,483,648 bytes RAM, kernel `7.1.4-arch1-1`, and microcode `0xf8`. The chosen scenario was MVP validation part 1, an L-shaped bracket with two through holes and explicit millimetre dimensions.

## Exercised workflow

The final run passed these assertions:

1. CLI JSON and MCP `tools/call` envelopes decoded to the same `threeterm.command.bracket/1` semantic request.
2. Preview ran fresh `libslvs` and OCCT workers against revision zero, returned an input fingerprint and warning, created no transaction, and removed transient artifacts.
3. Commit revalidated the same semantic input, ran fresh workers, atomically promoted a B-rep, and wrote transaction `tx-0001` from source revision zero to revision one.
4. A historical width edit previewed against revision one, committed as `tx-0002`, dirtied and replaced the profile/solid results, invalidated Layer 1 and Layer 2 keys, and advanced atomically to revision two. One unchanged same-snapshot Layer 1 result was validated as a cache hit.
5. A non-positive-thickness preview returned fatal `DIMENSION_NON_POSITIVE` and `HOLE_BREAKS_WALL` diagnostics without changing revision two, either canonical transaction, or the current B-rep.
6. The thin-feature advisory remained an override-eligible warning. Export without override was gated; explicit override produced OCCT STL and STEP plus lib3mf 3MF. The 3MF adapter parsed OCCT's canonical ASCII triangle mesh, constructed the lib3mf model directly, wrote it, and read it back through lib3mf.
7. Thirty 160x100 complete RGB frames were zlib-compressed, Base64-chunked, and emitted as direct Kitty graphics byte streams with one image ID. These are captured streams, not acknowledged terminal presentations.
8. Two saves produced current and previous `.threeterm` generations. The current generation contains a sealed canonical JSON manifest, two-record NDJSON canonical log, checkpoint, and non-authoritative cache metadata. Reload authenticated the seal/log, accepted the matching checkpoint, and discarded an injected worker-fingerprint cache mismatch.
9. Every native request was one fresh process using one newline-framed `threeterm.worker/1` request and one validated completion. Across the run there were 67 worker requests. Host validation checked request/revision identity, staged path, byte count, SHA-256, and artifact metadata before promotion.

Final export hashes:

- STL, 61,748 bytes: `5df847410d3c8913ace440618a5a15105a8f4a8bf6d7b850bbc97d1fc887057d`
- 3MF, 6,950 bytes: `6f69ff4600078332b49aefc2945363c10c9bfae4fd6d25ed29b934b5a1e01287`
- STEP, 41,070 bytes: `c9363e351175c3e3fe35ae750a1573a2c8c4fc5c445aeff5811a7917f0de8c10`

## Measured bands

Nearest-rank measurements from this one run were:

| Class | Samples | p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: |
| Cold disposable `libslvs` solve | 32 | 3.337 ms | 3.889 ms | 4.990 ms |
| Cold disposable OCCT bracket build | 32 | 32.526 ms | 40.941 ms | 45.347 ms |
| Schematic RGB/zlib/Kitty stream encode | 30 | 0.055 ms | 0.125 ms | 0.210 ms |

Peak worker RSS was 7,596 KiB for `libslvs`, 29,684 KiB for OCCT, and 11,384 KiB for lib3mf. Individual STL, STEP, and 3MF worker times were 24.127 ms, 47.504 ms, and 12.202 ms; the complete export sequence was 84.430 ms. Two tiny persistence writes measured 0.079 ms and 0.085 ms.

These are disposable-fixture measured bands only. They are not MVP targets.

## Evidence paths

- Summary: `/tmp/opencode/threeterm-first-part-spike-final/workspace/evidence/results.json`
- Keyboard-flow representation: `/tmp/opencode/threeterm-first-part-spike-final/workspace/evidence/keyboard-trace.json`
- Current and previous project generations: `/tmp/opencode/threeterm-first-part-spike-final/workspace/project/`
- Exports: `/tmp/opencode/threeterm-first-part-spike-final/workspace/exports/`
- Kitty streams: `/tmp/opencode/threeterm-first-part-spike-final/workspace/viewport/`
- Environment: `/tmp/opencode/threeterm-first-part-spike-final/environment-*.txt`

## Limits and closure context

The slice demonstrates boundary coherence but does not provide live product evidence. Issue 50 asked for discoverable keyboard-first operation and performance validation on the production renderer in direct Ghostty. This session had no attached TTY, no production renderer exists under the map's no-production-code rule, and the command-palette sequence was asserted by the harness rather than performed and judged by the product owner. Therefore this run produced no Ghostty acknowledgement, presentation/input-to-photon measurement, visual correctness result, focus/resize/cleanup evidence, or human discoverability/usability result.

The run also covered only the L-shaped-bracket validation part, one small project, one machine, and one same-container package set. It did not exercise monolithic-call cancellation, stale-last-valid downstream failure recovery, hostile protocol/path/size inputs, all seven reference parts, or cross-version/platform/locale replay. Under the six-gate rule from `Set evidence-based performance budgets`, this run does not promote any measured band to an MVP target.

A concurrent owner session retained two runs at `/tmp/opencode/threeterm-evidence-v2/`, posted the detailed resolution at `https://github.com/rafaelromao/threeterm/issues/50#issuecomment-5136768075`, and closed issue 50 while explicitly retaining the fixture-vs-product limits above. The closure treats the rehearsal as sufficient to resolve the planning spike for part 1; it does not convert the bands into production-renderer, live-Ghostty, human-usability, or general performance claims. Parts 2-7 and future representative product rehearsals remain follow-up evidence, not claims established here.
