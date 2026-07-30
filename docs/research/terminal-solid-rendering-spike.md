# Terminal Solid Rendering Spike

Status: resolved direct-Ghostty evidence  
Date: 2026-07-30  
Question: can a disposable protocol-neutral viewport render and navigate representative solids through the sole supported direct Ghostty path with useful depth, selection feedback, bounded stale frames, and recoverable lifecycle behavior?

## Scope And Method

The supported scope is a direct local Ghostty session only: `TERM=xterm-ghostty`, `TERM_PROGRAM=ghostty`. Other terminals, multiplexers, SSH, and fallback transports are outside this spike and the MVP support matrix.

The disposable fixture, outside this repository, rendered two overlapping flat-shaded cuboids at an 800x600 pixel viewport with a small software rasterizer. It transmitted each new raster through Ghostty's Kitty graphics support, replacing image ID 1. The fixture provided static display, selection toggling, left/right orbit, a 60 Hz newest-state coalescing trace, resize observation, and alternate-screen/image cleanup on exit.

Run it in a direct Ghostty terminal with:

```sh
python3 /tmp/opencode/threeterm-terminal-render-spike/ghostty_viewer.py \
  --output /tmp/opencode/threeterm-terminal-render-spike/ghostty-direct.json
```

This is test evidence, not production renderer code.

## Direct Ghostty Result

The owner visually accepted the direct Ghostty workflow as usable. Screenshots confirmed:

- solid display with visible occlusion/depth;
- selected-solid feedback;
- orbit navigation; and
- the 800x600 fixture presentation.

Ghostty returned Kitty image acknowledgements (`ESC_Gi=1;OKESC_BACKSLASH`) for the transmitted image. The recorded trace contains the following fixture metrics:

| Measure | Result |
| --- | ---: |
| Requested camera states | 90 |
| Rendered camera states | 34 |
| Stale states dropped | 57 |
| Maximum fixture render time | 55.990 ms |

The trace requests states at 60 Hz and renders only the newest available state, so it does not retain a stale-frame queue. The counters demonstrate the fixture's coalescing behavior, not a general frame-rate or latency promise.

The tested session also produced 13 prior-run resize notifications. The fixture registers `SIGWINCH` and records terminal dimensions for those notifications. On exit it deleted image ID 1, restored the cursor, exited the alternate screen, and restored terminal attributes; the captured direct trace records `alternate_screen_restored: true` cleanup evidence.

## Decision

Direct Ghostty pixel rendering is a viable MVP renderer direction for shaded solid inspection and basic navigation. Retain a protocol-neutral projected/rasterized viewport and use newest-state coalescing so obsolete camera states are discarded rather than queued. Treat image acknowledgement and explicit terminal restoration as part of the renderer lifecycle.

## Limitations

- Performance is fixture-only: two flat-shaded cuboids at 800x600 are not representative CAD scenes or a production performance budget.
- This run did not measure input-to-photon latency.
- The software fixture is not a production renderer and does not establish architecture, throughput, memory, error recovery, or rendering quality for the product implementation.
- Evidence covers direct Ghostty only. It makes no claim about Kitty, WezTerm, foot, tmux, SSH, Sixel, Braille, or other terminals and transports.
- The direct trace records cleanup and the prior-run resize count, but this ticket does not resolve the broader keyboard, mouse, focus, or resize input-protocol contract; that remains issue 11.
