# Terminal Input-To-Render Latency Spike

Status: resolved direct-Ghostty fixture evidence  
Date: 2026-07-30  
Question: what latency does the direct local Ghostty path show from application receipt of terminal input bytes to Kitty frame submission and terminal acknowledgement?

## Scope And Method

The sole supported interactive MVP path is direct local Ghostty: Ghostty `1.3.1-arch2`, `TERM=xterm-ghostty`, and `TERM_PROGRAM=ghostty`, without tmux or SSH.

The disposable fixture at `/tmp/opencode/threeterm-input-render-latency-spike/measure.py` rendered deterministic 640x360 RGB frames, zlib-compressed them, and transmitted them through Kitty graphics. It timestamps:

- application receipt of input bytes on its monotonic clock;
- completion of the corresponding Kitty graphics sequence submission to the local PTY write path; and
- where available, receipt of the matching Ghostty Kitty `OK` acknowledgement.

The raw direct trace is `/tmp/opencode/threeterm-input-render-latency-spike/ghostty-direct.json`. Keyboard orbit and selection, mouse drag, and wheel actions were exercised. The fixture submits a frame for each received mouse press, motion, release, and wheel record, so a single human drag produces multiple frame samples. Its 1,116 samples therefore exceed the requested count of human mouse actions.

## Direct Ghostty Result

The owner reports that updates felt prompt throughout keyboard, drag, and wheel actions. This is qualitative visual evidence only.

| Measure | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| Received byte to frame submission | 1.873 ms | 2.614 ms | 2.688 ms |
| Received byte to Ghostty acknowledgement | 2.717 ms | 3.399 ms | 3.588 ms |

All 1,116 submitted frames received matching Ghostty acknowledgements. The trace records zero missing acknowledgements and no PTY write backpressure.

## Interpretation And Limits

Ghostty's acknowledgement establishes that the terminal responded to the Kitty graphics command; it is not a terminal-presentation acknowledgement. These metrics explicitly exclude hardware input sampling, terminal decode/upload completion, compositor presentation, display scanout, and input-to-photon latency.

The deterministic low-detail fixture is not a representative CAD scene, production renderer, throughput target, or product performance budget. It establishes only a bounded direct-Ghostty fixture baseline for received-byte-to-submission and received-byte-to-acknowledgement behavior. It makes no claim about other terminals, tmux, SSH, full-resolution rendering, kernel recomputation, or display presentation latency.
