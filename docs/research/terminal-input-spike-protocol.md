# Terminal Input Reliability Spike Protocol

Status: resolved for the direct local Ghostty MVP minimum; not a cross-terminal reliability claim.

## Scope and Evidence

The sole supported interactive MVP terminal is direct local Ghostty. Kitty, WezTerm, foot, tmux, and SSH are outside the MVP support matrix. This result does not validate those terminals, transports, or any broad terminal compatibility contract.

The human-operated traces were collected on 2026-07-30 through direct Ghostty with `TERM_PROGRAM=ghostty` and `TERM=xterm-ghostty`; no tmux or SSH path was involved. The resolution environment reports Ghostty `1.3.1-arch2` (tip channel). The raw trace metadata does not itself contain a Ghostty version, so this records the accepted package version for the resolution environment, not a version value captured by the recorder at the instant of each event.

The raw recorder and evidence are retained outside the repository:

- Full staged trace: `/tmp/opencode/threeterm-terminal-input-spike/ghostty-direct.json`
- Supplemental expanded legacy trace: `/tmp/opencode/threeterm-terminal-input-spike/ghostty-final.json`
- Recorder: `/tmp/opencode/threeterm-terminal-input-spike/record.py`

The operator reports that all intended tested behavior looked normal. They have no middle mouse button; middle click is untested and excluded from the MVP minimum. The recorder intentionally gave no visual click feedback, so these traces do not establish application UI feedback by themselves.

## Recorder Contract

The disposable raw-mode recorder preserves exact received bytes with monotonic timestamps before interpreting them. It enables its enhanced modes one stage at a time, records resize notifications, and restores terminal state on normal completion, timeout, SIGINT, and SIGTERM. Cleanup writes disables for pixel/cell/motion mouse, focus, and Kitty keyboard modes, restores cursor, alternate screen, attributes, and termios settings.

## Physical Raw Evidence

`ghostty-direct.json` completed every stage, recording 176 raw-byte records, four protocol-enable records, four resize notifications, and cleanup with `terminal_modes_disabled: true`. After exit, the operator confirmed normal shell keyboard, selection, and scrolling behavior.

- Kitty keyboard enhancement: after `CSI > 3 u`, the trace contains event-type sequences such as `CSI 110;1:3 u`, `CSI 116;1:3 u`, `CSI 101;1:3 u`, and `CSI 115;1:3 u`; type `3` is the exercised release-event form. This is Ghostty's implementation of the Kitty keyboard protocol, not evidence from the Kitty terminal.
- SGR 1006 cell mouse: the trace contains `CSI < ... M/m` sequences for press, drag/motion over multiple cells, release, and both wheel directions, including a press at cell `19;4`.
- SGR 1016 pixel mouse: the trace contains SGR-family press, drag/motion, release, and wheel sequences with pixel coordinates, for example `363;64` and `533;116`.
- Focus and resize: the trace contains `CSI I` and `CSI O` focus-in/out records, a focus-stage resize, and three repeated resize-stage notifications. Together these are four resize records.

`ghostty-final.json` is supplemental physical evidence only. It expanded the legacy-keyboard stage and captured printable repeats; arrows (`CSI A/B/C/D`); Escape, Enter, Tab, Backspace; Home/End (`CSI H/F`); Page Up/Down (`CSI 5~/6~`); repeated arrows; and seven legacy-stage resize notifications. It intentionally timed out at that first stage and then recorded cleanup with `terminal_modes_disabled: true`. The timeout is procedural supplemental evidence, not a successful full-stage run.

Neither trace proves exhaustive modifier coverage, non-US keyboard layouts, input latency, every lifecycle edge, every physical mouse button, or any terminal beyond direct Ghostty.

## Direct Ghostty MVP Minimum Contract

The application must be correct with this conservative contract, while accepting the demonstrated enhanced events as optional improvements:

- Every command required for selection, sketch placement, orbit, pan, zoom, cancellation, property editing, and overlays has keyboard-first access. Pointer input improves direct manipulation; it is not the only route to a command.
- Middle-button input is never required. Orbit, pan, and zoom have keyboard-first command access; wheel input may enhance zoom where received.
- Correctness does not depend on hover, a release event arriving, or pixel-coordinate precision. Cell coordinates may aid targeting, pixel coordinates may refine it, and neither may be the sole way to reach a result.
- A pointer press visibly identifies the pending target and active tool. An active drag visibly shows its transient operation. A completed selection visibly identifies the selected state. These acknowledgements make action state observable even though the recorder did not render click feedback.
- Focus loss, explicit cancellation, an incomplete/ambiguous pointer sequence, or resize during a gesture visibly cancels the transient gesture, clears its active-drag state, and preserves the last committed model state. The interface then visibly restores focus/readiness and accepts a new keyboard or pointer command.
- Resize recomputes the presentation from authoritative state; it cannot silently retain stale hit regions or require a pixel-coordinate replay.

## Explicit Fallbacks

| Unavailable or ambiguous condition | Required product fallback |
| --- | --- |
| Pointer button, motion, hover, or release is absent/ambiguous | Keep the committed state unchanged, cancel the transient gesture visibly, and expose the same operation through keyboard-first commands. |
| Middle click | No binding required; keyboard-first orbit/pan/zoom remains sufficient. |
| Pixel coordinates unavailable, clipped, or changed by resize | Use non-pixel targeting or keyboard-first commands; do not make pixel location a correctness precondition. |
| Focus loss during interaction | Cancel the transient gesture visibly and require a fresh press/command after focus recovery. |
| Drag interrupted by terminal mode reset, close, or missing release | Treat it as cancellation, not completion; retain last committed state and show recovery-ready UI. |
| Resize during interaction | Cancel the transient gesture visibly, rebuild layout/hit regions, and allow immediate retry. |
| Enhanced keyboard reporting, modifiers, non-US layout, or repeat semantics unavailable | Preserve printable/text input where received and provide discoverable keyboard-first command access without relying on a particular modified physical key. |
| No measured latency guarantee | Do not promise a latency threshold from this spike; show active, cancelled, selected, focus, resize, and recovered states rather than implying an unobserved action completed. |

## Limits and Resolution

This evidence supports the direct Ghostty MVP minimum contract and its explicit fallbacks. It is intentionally narrower than a full input certification: it makes no claim of exhaustive modifiers, non-US layouts, latency percentiles, all terminal lifecycle failures, middle-button behavior, or other terminals/transports. The evidence is sufficient to define a state machine that does not assume hover, release, or pixel events are available for correctness.

## Evidence Context

Protocol facts and terminal-specific caveats are summarized in [`terminal-cad-viability.md`](./terminal-cad-viability.md). This result records observed direct-Ghostty behavior and product constraints; it does not generalize terminal-brand support.
