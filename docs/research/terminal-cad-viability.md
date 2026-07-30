# Terminal-Native CAD Viability

Status: research note, not an architecture decision<br>
Date: 2026-07-30<br>
Scope: in-terminal 3D viewport rendering and input across local terminals, SSH, and tmux

## Purpose

ThreeTerm is intended to be a Linux-only, keyboard-first parametric CAD application with mouse support and a central 3D viewport rendered inside the terminal. It must not open a separate X11 or Wayland window. This note tests whether current terminal graphics and input mechanisms can support that product shape, where their transport costs appear, and which claims still need measured evidence.

This note deliberately does not select a final graphics protocol, rendering backend, terminal support matrix, or fallback policy.

### Evidence labels

- **Fact**: behavior documented by a primary specification, first-party documentation, or pinned source revision. Citations use `[S#]` and are collected in [Sources](#sources).
- **Calculation**: arithmetic from documented wire formats and stated assumptions. It is not a throughput measurement.
- **Hypothesis**: a ThreeTerm-specific inference that needs validation.
- **Unknown**: a question for a benchmark or compatibility fixture.
- **Decision**: a product or architecture choice that evidence alone cannot make.
- **Constraint**: a requirement from the supplied ThreeTerm vision.

## Executive Summary

1. **Hypothesis:** Terminal-native parametric CAD is technically plausible if "CAD correctness" belongs to the model and geometry kernel, while the terminal viewport is a derived visualization. **Fact:** Kitty graphics, Sixel, and Unicode cells all display raster or sampled output; none transmits exact CAD geometry or provides a 3D scene API.[S1][S4][S5][S6]
2. **Hypothesis:** ThreeTerm can keep selection, constraints, dimensions, topology, and saved output exact even when the live view is sampled into terminal cells. **Decision:** ThreeTerm must define which viewport interactions may be approximate and which must be verified against exact geometry.
3. **Fact:** The Kitty graphics protocol has the richest relevant image lifecycle: RGB/RGBA/PNG transfer, zlib compression, image IDs, reusable placements, replacement, deletion, and animation/frame composition. Direct remote transfer is Base64 and must be chunked into payloads no larger than 4096 bytes.[S1]
4. **Fact:** Sixel is a palette-oriented bitmap stream embedded in a DCS sequence. It has broad historical semantics and a native tmux path, but no standard equivalent to Kitty image IDs and placements. Palette construction, encoding cost, image lifetime, cursor behavior, and update strategy are implementation-sensitive.[S3][S4][S11]
5. **Fact:** Unicode Braille and Block Elements need no image protocol and use ordinary terminal text paths through PTYs, SSH, and tmux. They are cell-grid output, not pixels. Braille exposes eight binary dots per character and Block Elements expose halves, eighths, shades, and quadrants.[S5][S6][S8][S10]
6. **Calculation:** An uncompressed 1200x800 RGB frame is 2.88 MB. Kitty direct transfer expands it to 3.84 MB before control overhead because Base64 maps three input bytes to four output characters. At 30 frames per second this is 115.2 MB/s and about 938 Kitty payload chunks per frame. Compression can change this substantially, but only measurements on representative CAD views can establish how much.[S1][S7]
7. **Fact:** Under SSH, PTY output is ordinary flow-controlled channel data. Direct Kitty payloads, Sixel streams, and Unicode redraws share the server-to-client session path with other output; input uses the reverse direction of the same SSH session and connection. Kitty file and shared-memory media are local optimizations and are not generally usable when the application and terminal emulator are on different hosts.[S1][S8]
8. **Fact:** tmux is not transparent. `allow-passthrough` defaults to `off`; when enabled it accepts explicit wrapped DCS passthrough and `on` only passes visible panes. A tmux maintainer states that passthrough cannot realistically be used for protocols requiring a response. Native Sixel support, added in tmux 3.4, is compile-time optional.[S10][S11][S12][S13]
9. **Fact:** Current tmux source parses, retains, clips, scales, and re-encodes Sixel per attached terminal. It can emit a text placeholder when the client lacks Sixel or pixel dimensions. Text overlap and scrolling can delete or crop retained images, and retained image count is globally bounded.[S11]
10. **Fact:** Input capability is independent of graphics capability. Kitty keyboard progressive enhancements can disambiguate legacy keys and optionally report press, repeat, and release events. Xterm mouse modes provide cell coordinates through SGR 1006 and pixel coordinates through SGR-Pixels 1016.[S2][S3]
11. **Fact:** Current tmux supports its own limited extended-key modes and SGR cell mouse translation, not full transparent Kitty keyboard event semantics or SGR-Pixels 1016. A `csi-u` tmux setting must not be treated as full Kitty keyboard protocol support.[S10][S11]
12. **Unknown:** None of the protocol specifications establishes ThreeTerm's input-to-photon latency, sustained frame rate, parser cost, compression ratio, memory behavior, or resize stability. These are product-critical benchmark questions, especially over SSH and through tmux.
13. **Hypothesis:** A robust cell renderer can provide the widest emergency or low-bandwidth operating envelope, while a pixel protocol can provide the higher-fidelity path. Maintaining both as first-class interactive experiences is meaningful product and test cost, not a free fallback.
14. **Decision:** Viability depends less on whether any terminal can show an image and more on the support contract: required terminals, required tmux topology, remote link envelope, minimum viewport quality, acceptable degradation, and latency budget.

## What "Viable" Must Mean

The question has at least four independent dimensions:

| Dimension | Necessary question | Evidence status |
| --- | --- | --- |
| Geometric correctness | Can exact model operations remain independent of display sampling? | Architecturally possible; product semantics still need definition |
| Visual usefulness | Can users understand depth, topology, selection, and edits at the chosen resolution? | Unknown until representative scenes are tested |
| Interaction | Can keyboard and mouse events express orbit, pan, zoom, selection, and commands without ambiguity? | Protocol mechanisms exist; path compatibility differs |
| Delivery | Can frames and input cross local PTYs, SSH, and tmux within a usable latency and bandwidth budget? | Unknown until end-to-end benchmarks |

Showing a static mesh once proves only basic display feasibility. It does not prove interactive camera motion, stale-frame avoidance, picking fidelity, resize behavior, remote usability, or terminal portability.

### Preserve the precision boundary

**Constraint:** Parametric geometry and constraints must remain exact enough for CAD use.

**Fact:** The graphics mechanisms considered here consume pixels or character cells, not B-reps, feature graphs, constraints, or exact curves.[S1][S4][S5][S6]

**Hypothesis:** ThreeTerm should treat every viewport as a disposable projection of an authoritative scene:

```text
exact parametric model
        |
        v
tessellation / curves / annotations ----> picking acceleration
        |                                      |
        v                                      v
camera projection and raster/sample      exact verification
        |
        v
Kitty image | Sixel image | Unicode cells
```

This separation has several consequences:

- A coarser Unicode view need not change model tolerances or stored geometry.
- A mouse hit can generate a ray or coarse candidate from viewport coordinates, then verify the candidate against authoritative geometry.
- Dimensions and coordinates should be shown as exact textual values rather than inferred from screen pixels.
- A rendered edge that disappears under downsampling must not imply that the topological edge disappeared.
- Export and headless calculations must not depend on terminal glyph metrics or image protocol behavior.

**Decision:** Define whether mouse picking itself must be exact at the initial click, may present a disambiguation list, or may snap to exact candidates after a coarse hit. That choice changes whether cell-coordinate mouse input is sufficient.

## Rendering Paths

### Shared application-side pipeline

**Fact:** None of the candidate terminal protocols accepts a triangle mesh, camera, light, or CAD primitive. Kitty accepts RGB, RGBA, or PNG data; Sixel encodes bitmap columns; Unicode renders glyphs.[S1][S4][S5][S6]

Therefore, all paths share substantial application work:

1. Convert exact geometry into display curves and/or tessellated surfaces.
2. Apply camera and clipping transforms.
3. Resolve visibility, depth, selection highlighting, and annotations.
4. Produce either a pixel buffer or a sampled cell grid.
5. Encode and schedule output without allowing stale frames to queue indefinitely.

**Decision:** CPU software rasterization, offscreen GPU rasterization, and hybrid rendering remain separate choices from the terminal wire protocol. The no-external-window constraint does not by itself resolve that choice.

### Kitty graphics protocol

#### Relevant mechanics

**Fact:** Kitty graphics commands are APC sequences. Payloads are Base64 encoded. Required image formats are 24-bit RGB, 32-bit RGBA, and PNG; raw RGB/RGBA dimensions are explicit.[S1]

**Fact:** The protocol supports zlib compression before Base64 encoding. It offers direct, regular-file, temporary-file, and shared-memory transmission media. Its own remote-client guidance says clients unable to share files or memory must send direct, Base64-encoded chunks no larger than 4096 bytes.[S1]

**Fact:** Image data and display placement are separate concepts. A transmitted image can be placed multiple times, and reusing an image or replacing a placement can move, crop, or resize static content without retransmitting its pixels. Retransmitting an existing image ID replaces its data and removes its placements.[S1]

**Fact:** The protocol includes client-driven and terminal-driven animation, partial frame data, and rectangle composition between frames. The specification explicitly motivates terminal-driven animation by unknown and variable client-to-terminal latency, especially over SSH.[S1]

**Fact:** Terminals are expected to enforce storage quotas. Kitty documents a 320 MB image quota per buffer and separate animation storage behavior.[S1]

#### CAD implications

- **Hypothesis:** IDs and placements are valuable for static overlays, legends, thumbnails, or moving an unchanged image region.
- **Hypothesis:** They do less for free camera rotation because most shaded pixels change and the terminal has no arbitrary 3D transform operation.
- **Hypothesis:** Frame composition may help localized highlights, cursors, or selection changes, but only if target terminals implement the exact operations consistently.
- **Unknown:** Whether raw-plus-zlib, PNG, or another application-side representation produces the best latency for representative shaded and wireframe CAD scenes.
- **Unknown:** Whether a terminal acknowledges acceptance early enough to help pacing. The protocol response is not documented as a "presented on display" timestamp.
- **Risk:** Quota eviction, image replacement semantics, scrollback, alt-screen transitions, and crashes require explicit cleanup and recovery behavior.

#### Compatibility warning

"Supports Kitty graphics" is not a sufficient capability level. The application may need to know separately whether direct RGB/RGBA, PNG, zlib, placement replacement, Unicode placeholders, animation, and local shared memory work.

### Sixel

#### Relevant mechanics

**Fact:** Sixel is a DCS bitmap representation. One data character encodes six vertical pixels. The stream includes repeat, raster-attribute, palette selection/definition, carriage-return, and newline operations.[S4]

**Fact:** Sixel color is selected through color-map entries. The original VT340 exposed 16 entries, while the protocol syntax can name entries up to 255. Xterm exposes queries for the configured number of color registers and graphics geometry, and reports Sixel as feature 4 in primary device attributes when available.[S3][S4]

**Fact:** Xterm graphics support can vary at build time and runtime, and its maximum graphics geometry is configurable. A DA response or terminal name therefore needs to be interpreted as a capability report, not a universal size/performance guarantee.[S3]

#### CAD implications

- **Hypothesis:** A shaded viewport needs palette quantization and deliberate palette reuse. Wireframe and flat-shaded modes may encode more cheaply than noisy anti-aliased gradients.
- **Hypothesis:** Sixel repeat operations can compress runs well, but encoded size is highly scene- and encoder-dependent. Raw-pixel arithmetic does not predict Sixel bandwidth.
- **Fact:** Sixel has no standard image-ID and placement lifecycle comparable to Kitty graphics.[S4]
- **Hypothesis:** Rectangular overdraw can update part of a view, but transparency, palette state, cursor movement, erase behavior, and terminal image retention make this less portable than it appears.
- **Unknown:** Whether full-frame replacement, dirty rectangles, or a coarse-while-moving/high-quality-on-idle policy gives the best experience.

#### Native tmux path

Sixel has a material advantage specifically inside recent tmux builds: tmux can understand the image rather than blindly pass its bytes. That advantage also makes tmux part of the rendering pipeline, with its own storage, scaling, clipping, and fallback behavior. See [tmux is a terminal boundary](#tmux-is-a-terminal-boundary).

### Unicode cell rendering

#### Relevant mechanics

**Fact:** Unicode defines 256 Braille Patterns from U+2800 through U+28FF. Their named dot combinations provide eight binary samples in the conventional two-by-four dot layout of one character cell.[S5]

**Fact:** Block Elements include horizontal and vertical eighths, half blocks, full blocks, shades, and two-by-two quadrant combinations.[S6]

**Fact:** This output is normal Unicode text plus ordinary terminal attributes. It does not depend on APC or DCS image handling, local files, image storage quotas, or image passthrough.[S3][S5][S6][S9]

#### CAD implications

- Braille can represent a binary 2x4 sample grid per cell, useful for dense wireframe, silhouettes, axes, and edge emphasis.
- Half blocks can use foreground and background colors as two vertical color samples per cell, subject to terminal color support.
- Quadrant blocks provide 2x2 shape samples but only one glyph foreground plus cell background without more elaborate compromises.
- Cell-based redraws can be diffed so unchanged cells are not emitted.
- Labels, dimensions, command UI, and viewport can share one ordinary terminal layout without image placement semantics.

**Hypothesis:** A carefully designed edge-first view can remain useful at much lower data volume than a full pixel frame. That does not establish that it is sufficient for dense assemblies, subtle curvature, occluded selections, or shaded surface inspection.

**Unknown:** Font choice, line height, Braille dot shape, anti-aliasing, glyph gaps, color rendering, and terminal scaling materially affect visual quality. Unicode assigns characters, not identical glyph rasterization across terminals.

**Unknown:** A cell-diff renderer may perform very differently on local GPU terminals, remote links, and tmux because attribute changes and terminal text shaping still have cost.

#### Illustrative resolution and byte floor

For a 120x40-cell viewport:

- Braille exposes a nominal binary sample lattice of 240x160, or 38,400 samples.
- A full screen contains 4,800 glyphs.
- U+2800 through U+28FF encode as three UTF-8 bytes each, so glyph bytes alone are about 14.4 kB per full redraw.
- At 60 redraws per second, glyph bytes alone are about 0.864 MB/s.

**Calculation:** These figures exclude cursor movement, color/style sequences, framing, and unchanged-cell optimization. They are a lower-bound illustration, not measured terminal throughput.

### Direct comparison

| Concern | Kitty graphics | Sixel | Unicode cells |
| --- | --- | --- | --- |
| Display unit | RGB/RGBA/PNG image | Palette bitmap stream | Character cells and attributes |
| Resolution | Pixel | Pixel | Cell sub-samples |
| Color model | RGB/RGBA or PNG | Palette registers | Terminal foreground/background per cell |
| Built-in compression | zlib option; PNG may already compress | Run-oriented Sixel encoding | Cell diffing and repeated terminal state |
| Persistent identity | Images and placements | No comparable standard identity | Grid coordinates in app state |
| Partial update tools | Placement/replacement, frame composition | Overdraw/rectangles, implementation-sensitive | Changed cells |
| Local out-of-band medium | File/shared memory protocol media | No standard equivalent | Not applicable |
| Remote representation | Direct Base64 chunks | In-band DCS | In-band text |
| tmux path | Explicit passthrough only; response routing is unsafe | Native when tmux was built with Sixel | Native text path |
| Potential visual fidelity | Pixel-level full color | Pixel-level with palette/encoder constraints | Deliberately approximate sub-cell sampling |
| Principal risk | Compatibility subset and frame transport | Encoding/palette/lifecycle differences | Product usefulness at low resolution |

### A multi-backend renderer is not automatically simpler

**Hypothesis:** It is tempting to promise Kitty, Sixel, and Unicode and choose at runtime. That creates at least three output encoders, multiple viewport coordinate models, protocol-specific resize and cleanup behavior, and a larger conformance matrix.

The reusable portion should be the scene and camera pipeline, not an assumption that all backends behave alike:

```text
scene snapshot -> projected primitives -> visibility / selection
                                      |
                 +--------------------+--------------------+
                 |                    |                    |
             RGB(A) raster       indexed raster       cell sampler
                 |                    |                    |
              Kitty                Sixel              Unicode
```

**Decision:** Decide whether fallback means "same interactions at lower fidelity," "read-only preview," "wireframe only," or "unsupported environment with a diagnostic." These are different products.

## Representative Terminal Evidence

The following table records evidence, not a promised compatibility matrix. "Not established" means this research did not verify the capability; it does not mean the capability is absent.

| Terminal | Kitty graphics evidence | Sixel evidence | Input evidence | Important qualification |
| --- | --- | --- | --- | --- |
| kitty | Protocol owner and reference documentation.[S1] | Not established here | Protocol owner for Kitty keyboard.[S2] | Richest protocol documentation is not proof of other terminals matching every operation |
| WezTerm | Changelog records Kitty Image Protocol support, later shared-memory transfer, and multiple compatibility fixes.[S20] | Changelog records Sixel support and later correctness/performance fixes.[S20] | Kitty keyboard handling is an explicit configuration capability.[S21] | Support changed over time; test pinned versions and operations |
| foot | APC content is ignored in pinned source, so Kitty graphics must not be inferred.[S16] | Control-sequence docs list Sixel; configuration defaults processing to enabled.[S14][S15] | Docs list Kitty keyboard query/push/pop/update and SGR-Pixels 1016.[S14] | Strong Sixel/input evidence does not imply Kitty graphics |
| Ghostty | Official docs advertise Kitty graphics.[S17] | Not established here | Official docs advertise Kitty keyboard; source lists SGR-Pixels 1016.[S17][S19] | Pinned graphics source lists shared-memory transmit, Unicode virtual placement, and animation as TODO; exact feature conformance needs testing.[S18] |
| xterm | Official control-sequence docs say APC content is ignored.[S3] | Sixel is optional and can be runtime-limited.[S3] | Documents SGR 1006 and SGR-Pixels 1016.[S3] | Build and resource configuration matter |

**Fact:** The WezTerm changelog is direct evidence that "protocol support" matures operation by operation: it records early Sixel support, parser and geometry fixes, Kitty support enabled with animation initially absent, later shared-memory transfer, and later compatibility fixes.[S20]

**Fact:** Ghostty's pinned source has an especially useful warning sign. Its graphics module header lists three TODO features, while the tree also contains partial virtual-placement structures. This internal feature-level evolution is another reason to probe exact operations rather than infer conformance from a terminal brand.[S18]

## Input Protocols

### Keyboard

**Fact:** Legacy terminal keyboard encoding maps distinct physical events to identical byte strings. The Kitty keyboard specification calls out ambiguous modified keys, inability to distinguish an Escape key from the start of an escape sequence without timing, and absence of distinct press/repeat/release reporting.[S2]

**Fact:** Kitty keyboard uses progressive enhancement flags. Applications can independently request disambiguated escape codes, event types, alternate keys, all keys as escape codes, and associated text. They can query, push, and pop the mode state. Event types distinguish press, repeat, and release.[S2]

Potential ThreeTerm value:

- Reliable multi-modifier keyboard shortcuts.
- Escape without an arbitrary timeout.
- Explicit repeat/release for held navigation or camera controls.
- Layout-aware shortcut matching when alternate keys are available.

**Hypothesis:** ThreeTerm should not require release events for its core command model unless continuous key-held camera movement is a product requirement. Edge-triggered commands and terminal-generated repeat can preserve a larger compatibility envelope.

**Decision:** Specify the minimum keyboard semantic set separately from the preferred set. "Keyboard-first" does not itself require the full Kitty keyboard protocol.

### Mouse

**Fact:** Xterm private modes separate tracking from coordinate encoding. Mode 1000 reports button press/release, 1002 adds motion while a button is pressed, and 1003 reports all motion. SGR mode 1006 reports coordinates in terminal cells; SGR-Pixels 1016 uses pixels.[S3]

For a CAD viewport:

- 1002 plus 1006 is enough to express drag-to-orbit/pan at cell granularity.
- 1003 may support hover highlighting but can create a high input event rate.
- 1016 can support finer picking and smoother deltas when the entire path preserves it.
- Wheel events can express zoom but need normalization and terminal testing.

**Hypothesis:** Relative camera changes can feel smoother than cell positions suggest if the application accumulates deltas, but precise click picking and small-object selection remain limited by cell coordinates.

**Decision:** Decide whether pixel-coordinate mouse input is required, preferred, or unnecessary. If it is required, current tmux behavior is a direct compatibility concern.

### Input and frame scheduling are coupled

**Hypothesis:** A renderer that blocks the event loop while compressing or writing a large frame can negate a high-fidelity keyboard or mouse protocol. Input should update the newest desired camera state even when an older render or terminal write is still in flight.

A candidate scheduling invariant is:

```text
at most one frame being encoded/written
+ at most one newest pending scene/camera state
+ never queue every intermediate mouse event as a complete frame
```

This is an option-neutral latency control, not evidence that any target frame budget can be met.

### Synchronized updates

**Fact:** Terminal private mode 2026 lets an application bracket synchronized updates. foot documents the mode, and current tmux can recognize application synchronized output and request the corresponding feature from supporting client terminals.[S10][S11][S14]

**Hypothesis:** Synchronized updates may prevent users from seeing an intermediate mixture of TUI cells and viewport output. They do not reduce encoded bytes, avoid SSH/tmux backpressure, or establish when a completed update reaches the display.

**Unknown:** Test whether synchronized updates materially reduce tearing for each renderer and whether large update groups increase latency or memory pressure.

## Transport Volume

### Direct Kitty frame arithmetic

For an uncompressed frame:

```text
raw bytes = width * height * bytes_per_pixel
Base64 bytes = 4 * ceil(raw bytes / 3)
chunks = ceil(Base64 bytes / 4096)
stream rate = Base64 bytes * frames_per_second
```

**Fact:** RGB uses three bytes per pixel, RGBA uses four, Base64 represents each 24 input bits as four characters, and Kitty direct chunks are at most 4096 bytes.[S1][S7]

| Viewport | Format | Raw/frame | Base64/frame | 4096-byte chunks/frame | 30 fps | 60 fps |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 800x600 | RGB | 1.44 MB | 1.92 MB | 469 | 57.6 MB/s | 115.2 MB/s |
| 800x600 | RGBA | 1.92 MB | 2.56 MB | 625 | 76.8 MB/s | 153.6 MB/s |
| 1200x800 | RGB | 2.88 MB | 3.84 MB | 938 | 115.2 MB/s | 230.4 MB/s |
| 1200x800 | RGBA | 3.84 MB | 5.12 MB | 1,250 | 153.6 MB/s | 307.2 MB/s |
| 1920x1080 | RGB | 6.2208 MB | 8.2944 MB | 2,025 | 248.832 MB/s | 497.664 MB/s |
| 1920x1080 | RGBA | 8.2944 MB | 11.0592 MB | 2,700 | 331.776 MB/s | 663.552 MB/s |

**Calculation:** Values use decimal MB and exclude APC framing, control keys, SSH framing/encryption, tmux wrapping, and retransmission. They are an upper-pressure illustration for uncompressed full frames, not a prediction of production traffic.

### Compression is not a constant

**Fact:** Kitty permits zlib compression before Base64 and also accepts PNG.[S1]

**Unknown:** Compression ratio and CPU time for these scene classes:

- sparse wireframe on a flat background;
- flat shaded solids with a small palette;
- anti-aliased edges and text overlays;
- gradients, ambient occlusion, or noisy dithering;
- rapid camera rotation where every frame changes;
- localized selection highlight on a stable camera.

**Hypothesis:** A low-resolution or simplified in-motion frame followed by a higher-quality idle frame may improve responsiveness across all pixel protocols. It must be tested for visual stability and not assumed.

### Sixel volume cannot be inferred from raw size alone

Sixel can repeat runs and reuse palette entries, so flat regions may encode compactly. More colors, dithering, and fragmented runs may expand output and increase encoder/parser work.[S4]

**Unknown:** Bytes per frame, palette-build time, Sixel encode time, terminal parse time, and tmux decode/re-encode time for the same scene corpus are required for comparison.

### Unicode volume depends on changed cells

A full Unicode redraw has a small glyph floor compared with full pixels, but truecolor attributes and cursor motion can dominate. A cell diff may be much smaller for localized changes and nearly a full redraw during camera motion.

**Unknown:** Measure both bytes emitted and terminal render time. Low byte volume does not guarantee low latency if thousands of independently styled cells are expensive to parse or shape.

## Latency Model

No cited protocol provides a ThreeTerm-level input-to-photon guarantee. The relevant path is:

```text
terminal input sampling
  -> PTY / SSH / tmux input translation
  -> ThreeTerm event loop
  -> camera or model update
  -> tessellation or scene update, if needed
  -> raster/sample
  -> compression and protocol encoding
  -> PTY write and backpressure
  -> tmux parse/scale/re-encode, if present
  -> SSH encryption/network/flow control, if present
  -> terminal parse/decode/upload
  -> terminal redraw and desktop compositor
  -> display scanout
```

The stages vary by interaction:

- Orbiting should not need exact model recomputation, but it does require reprojection and redraw.
- Editing a feature may require kernel recomputation before a correct viewport exists.
- Selection highlighting may permit a localized overlay if the chosen path supports it reliably.
- Resizing changes cells, pixels per cell, image placement, and possibly render resolution.

**Unknown:** Required measurements include input-to-app, app update, render, encode, write-blocked time, terminal decode, and input-to-visible-result. p50 alone is insufficient; p95/p99 and maximum stale-frame age matter for interaction.

**Hypothesis:** Throughput saturation is more dangerous than a single slow frame. Once output queues contain old complete frames, the screen can remain behind the user's input even after rendering becomes fast again.

## SSH

### What SSH preserves

**Fact:** SSH interactive sessions use channels. Channels are flow-controlled with byte windows, and session stdout travels as channel data. PTY allocation communicates character and pixel dimensions, though the dimensions are informational; window changes can update both.[S8]

**Fact:** SSH carries session output as uninterpreted channel data and does not define a graphics-specific session message. If graphics escape bytes survive all intermediaries, the local terminal interprets them.[S8]

### What SSH changes

- Pixel frame bytes now consume network bandwidth and SSH channel window space.
- Responses and input share network RTT with output delivery.
- Encryption, packetization, and optional transport compression add implementation-dependent work.
- A slow receiver or narrow link creates write backpressure.
- Remote `$TERM` and terminfo describe the requested terminal interface, not measured end-to-end behavior.

**Fact:** Kitty's file and shared-memory media require the terminal emulator to access the named local object. Its remote guidance directs clients without that shared access to direct transfer.[S1]

**Hypothesis:** For conventional `ssh host threeterm`, direct Kitty, Sixel, and Unicode are all in-band. A separate local renderer or purpose-built side channel could change that, but it would be a different architecture and should not be smuggled into the meaning of "works over SSH."

### SSH acceptance needs an envelope

"Works over SSH" is not testable without at least:

- RTT;
- available downstream bandwidth;
- packet loss or jitter expectations;
- viewport size;
- interaction frame-rate target;
- scene class;
- whether the remote host renders pixels or only computes geometry;
- whether tmux is outside SSH, inside SSH, or both.

## tmux Is a Terminal Boundary

An application inside tmux writes to a tmux-controlled PTY. tmux parses output into its own screen model and emits a different stream to each attached client. Treating it as a byte-transparent pipe produces incorrect capability and lifecycle assumptions.[S10][S11]

### Passthrough

**Fact:** tmux passthrough requires a DCS wrapper with the `tmux;` prefix. `allow-passthrough` accepts `off`, `on`, or `all`; its source default is `off`. `on` permits passthrough only for a visible pane, while `all` includes invisible panes.[S10][S11]

**Fact:** A tmux maintainer explicitly says passthrough cannot realistically be used for anything requiring a response because tmux cannot know that response bytes were generated by the terminal rather than typed by the user.[S13]

Consequences for Kitty graphics:

- Fire-and-forget display may work after user configuration and correct escaping.
- The recommended Kitty capability query and acknowledgement path is not reliable through passthrough.
- Visibility, multiple clients, nested multiplexers, and detach/reattach need dedicated behavior.
- `$TERM` inside tmux names tmux, not the outer terminal, so terminal-name allowlists are especially unsafe.

**Decision:** If tmux is first-class, decide whether configuration-dependent, unqueryable Kitty passthrough is an acceptable supported path or merely an unsupported best effort.

### Native Sixel

**Fact:** tmux 3.4 added basic Sixel support when built with `--enable-sixel`. Current source still guards the parser and output code behind that build option. The `sixel_support` format reports server build support and is always zero on OpenBSD according to the changelog.[S11][S12]

**Fact:** A Sixel-enabled tmux answers primary device attributes with feature 4. It parses incoming Sixel into retained image data, associates it with screen cells, and later scales/clips and re-encodes it for each client.[S11]

**Fact:** Current output falls back to a text box when the attached terminal lacks Sixel capability or usable pixel dimensions. Thus `sixel_support=1` reports the tmux server's parser build, not a guarantee that every attached client receives pixels.[S11]

**Fact:** Current source globally bounds retained images with `MAX_IMAGE_COUNT 20` and evicts the oldest as that threshold is reached. Writes overlapping image lines/areas remove retained images. Scrolling moves, crops, or deletes images depending on their position.[S11]

Implications:

- Native Sixel gives tmux enough information to preserve pane clipping and scale to client cell geometry.
- tmux adds decode, storage, possible scaling, palette handling, and re-encoding to the latency path.
- Image behavior is tied to tmux's cell model. Ordinary TUI redraws that overlap the viewport can remove image state.
- Multiple attached clients may receive Sixel or a text placeholder from the same retained image.
- Detach, copy mode, reflow, scroll, resize, and terminal switching are compatibility cases, not incidental UI details.

### tmux input translation

**Fact:** Current tmux requests and parses SGR 1006 mouse events and sends pane-relative cell coordinates to applications. Its mode table handles 1000, 1002, 1003, 1005, and 1006, but not 1016; the source tree contains no SGR-Pixels mode.[S11]

**Fact:** Current tmux has `extended-keys` and can emit either `xterm` or `csi-u` modified-key forms. The option defaults to off in the pinned source. Its parser accepts the simple `number;modifier` form and does not parse Kitty's colon event-type field, alternate-key field, or associated-text field.[S10][S11]

Therefore:

- A `csi-u` label in tmux configuration is not proof of full Kitty keyboard progressive enhancements.
- Pixel-coordinate mouse precision is not preserved through current tmux's native mouse path.
- ThreeTerm needs a tmux-specific input capability level rather than inheriting outer-terminal claims.

## Topology Matrix

| Path | Graphics endpoint seen by ThreeTerm | Pixel-frame transport | Principal trap |
| --- | --- | --- | --- |
| Local direct terminal | Terminal emulator | Local PTY; Kitty may also use file/shared memory | Terminal feature subsets and presentation latency |
| SSH to remote ThreeTerm | Local terminal through SSH session channel | In-band network stream; Kitty direct only in the normal split-host case | Bandwidth, RTT, and output backlog |
| Local tmux, local ThreeTerm | tmux virtual terminal | Unicode native; Sixel native if built; Kitty explicit passthrough | tmux is not transparent and passthrough defaults off |
| Local tmux, SSH, remote ThreeTerm | tmux virtual terminal across SSH | Same protocols cross SSH before local tmux processes them | Remote app cannot assume access to the local tmux server; query the wire path |
| SSH, remote tmux, remote ThreeTerm | Remote tmux virtual terminal | App output enters remote tmux, then SSH carries tmux client output | Both tmux processing and SSH costs apply |
| Nested tmux | Innermost tmux | Repeated wrappers or native image processing | Capability and response routing compound |

**Hypothesis:** Product support should name topologies, not merely say "SSH and tmux supported." The order of intermediaries changes what can be queried and where bytes are processed.

## Capability Detection

### Do not reduce capability to terminal identity

**Fact:** terminfo is a database of declared terminal capabilities. ncurses also permits user-defined extended capabilities, whose interpretation is application-defined.[S9]

Terminfo remains useful for baseline cursor, color, key, and terminal-mode behavior. It is insufficient as the sole graphics decision because:

- the remote host may lack or have a stale entry;
- `$TERM` names tmux or another multiplexer instead of the outer terminal;
- build- or runtime-disabled features may disagree with a broad terminal name;
- Kitty graphics has a protocol query rather than a standard terminfo capability;
- tmux server build support and attached-client support are different facts.

Environment variables such as `KITTY_WINDOW_ID`, `TERM_PROGRAM`, or terminal-specific names can be diagnostic hints, but they should not override a failed active probe.

### Detect a capability vector

A useful result is not one enum named `graphics_protocol`. It is a vector such as:

```text
render:
  unicode_utf8
  truecolor_cells
  sixel_decode
  kitty_direct_rgb
  kitty_png
  kitty_zlib
  kitty_placement_replace
  kitty_local_shared_memory

input:
  legacy_keys
  disambiguated_keys
  key_event_types
  mouse_button_motion
  mouse_all_motion
  mouse_cell_coordinates
  mouse_pixel_coordinates

presentation:
  synchronized_updates

path:
  ssh
  tmux
  tmux_native_sixel
  tmux_passthrough_configured
```

Not every bit must be discovered automatically. Some can be conservative defaults, some active probes, and some explicit overrides.

### Candidate startup sequence

1. Read termios, dimensions, locale, `$TERM`, and baseline terminfo without treating terminal name as conformance.
2. Detect known intermediaries from `$TMUX`, tmux-style `$TERM`, and SSH environment hints. Record topology uncertainty, especially when tmux may be outside SSH.
3. Establish UTF-8 and color-cell fallback capability.
4. Outside tmux, issue the Kitty query followed by primary DA exactly as the specification recommends. Parse interleaved responses with a bounded timeout.[S1]
5. Probe Sixel using DA1 feature 4 and, where useful, XTSMGRAPHICS geometry/register queries.[S3]
6. Inside a directly accessible tmux server, inspect version and `#{sixel_support}` as additional evidence. Do not equate it with outer-client pixels.[S10][S11]
7. Query Kitty keyboard progressive-enhancement state and delimit with DA only when the path can return responses reliably.[S2]
8. Enable only the input flags and mouse modes needed by the selected interaction level. Push/pop state where supported and always restore modes on normal exit.
9. Run a minimal display operation before allocating a large viewport image. Treat errors, timeout, malformed replies, and size limits as capability downgrade events.
10. Expose an explicit renderer/input override and a diagnostic report so users can recover from false positives and file actionable compatibility reports.

**Hypothesis:** Capability results should be scoped to the current terminal attachment and invalidated on meaningful reattach/resize/path changes. A long-running tmux session can later be viewed through a different terminal.

### Probe safety

- Use unique image IDs/nonces so unrelated terminal input cannot be mistaken for a reply.
- Bound payload and response sizes.
- Do not leave a probe image visible or retained.
- Keep normal keyboard input while probing; do not swallow bytes that fail exact response parsing.
- Avoid a long startup pause. Fall back conservatively when the response boundary is unreliable.
- Log which fact selected each capability: terminfo, active response, tmux query, override, or default.

## Failure Modes to Design Explicitly

| Failure | User-visible risk | Required policy question |
| --- | --- | --- |
| Unsupported graphics sequence | Blank or stale viewport | Downgrade automatically, diagnose, or refuse? |
| False-positive capability | Corrupt layout or invisible image | How is runtime failure detected and cached? |
| Output queue saturation | View continues moving after input stops | Drop/coalesce which pending frames? |
| Partial frame on process death | Terminal parser or image left incomplete | How are writes framed and cleanup attempted? |
| Resize during transfer | Wrong scale, crop, or placement | Cancel old frame or finish then redraw? |
| Terminal image quota eviction | View disappears | Periodic replacement, error-driven recovery, or full retransmit? |
| Text redraw overlaps Sixel in tmux | Retained image deleted | Reserve viewport rows and coordinate TUI/image redraws? |
| tmux pane hidden | Passthrough suppressed or useless output generated | Pause render when visibility is unknown? |
| tmux detach/reattach | New client has different capabilities | Re-probe or choose tmux-native lowest common behavior? |
| SSH bandwidth collapse | Input competes with stale output | Adaptive resolution/frame rate and bounded writes? |
| Mouse cell coordinates | Wrong small-object pick | Snap, disambiguate, keyboard selection, or require pixels? |
| Missing key release | Continuous action remains active | Avoid release-dependent core controls or add watchdog semantics? |
| Font renders fallback glyph poorly | Unicode view loses topology cues | Supported font contract or dynamic visual self-test? |

## Provisional Viability Assessment

### What the evidence supports

- **Fact:** A terminal can host a central, non-external-window image or sampled viewport using multiple current mechanisms.
- **Hypothesis:** ThreeTerm can keep exact CAD state independent of terminal output.
- **Fact:** Keyboard, drag, motion, wheel, cell coordinates, and on some paths pixel coordinates are representable.
- **Fact:** SSH transports all candidate in-band representations without a graphics-specific SSH extension.
- **Fact:** tmux has a native Sixel implementation and an explicit generic passthrough mechanism.

### What the evidence does not support yet

- A claim of smooth full-resolution shaded interaction at a named viewport size.
- A claim of acceptable remote interaction over an unspecified SSH link.
- A claim that Kitty, WezTerm, Ghostty, foot, and xterm provide equivalent protocol subsets.
- A claim that tmux transparently preserves graphics queries, Kitty keyboard events, or pixel mouse coordinates.
- A claim that Unicode fallback is useful enough for the intended CAD workflows.
- A claim that one encoder and pacing strategy is best for local, remote, and tmux paths.
- A claim that "supports Sixel" or "supports Kitty graphics" is granular enough for release support.

### Bottom line

**Hypothesis:** Terminal-native CAD is viable as a product direction if ThreeTerm defines an explicit environment and degradation envelope and keeps geometric truth outside the viewport. It is not yet proven viable as a high-fidelity, low-latency, terminal-agnostic experience across local terminals, SSH, and tmux.

No protocol should be selected from feature lists alone. The decision turns on measured latency and visual usefulness in the product's required topologies.

## Sources

Sources were accessed 2026-07-30. Specifications, first-party documentation, and pinned first-party source are preferred. Source snapshots demonstrate behavior at those revisions, not permanent compatibility promises.

- **[S1]** kitty, *Terminal graphics protocol*: wire form, image formats, compression, transmission media, remote chunking, support query, placements, animation, and quotas. <https://sw.kovidgoyal.net/kitty/graphics-protocol/>
- **[S2]** kitty, *Comprehensive keyboard handling in terminals*: legacy ambiguity, event types, progressive enhancement, state query, and detection. <https://sw.kovidgoyal.net/kitty/keyboard-protocol/>
- **[S3]** XTerm, *XTerm Control Sequences*, patch 410 documentation: APC handling, DA1, Sixel graphics/query behavior, mouse modes 1000/1002/1003/1006/1016, and runtime/build qualifications. <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html>
- **[S4]** Digital Equipment Corporation, *VT330/VT340 Programmer Reference Manual, Chapter 14: Sixel Graphics*: DCS format, six-pixel encoding, raster attributes, palette operations, repeats, and cursor/scroller behavior. <https://vt100.net/docs/vt3xx-gp/chapter14.html>
- **[S5]** Unicode, *Braille Patterns, U+2800-U+28FF*: complete named eight-dot pattern repertoire. <https://www.unicode.org/charts/nameslist/n_2800.html>
- **[S6]** Unicode, *Block Elements, U+2580-U+259F*: halves, eighths, shades, full blocks, and quadrants. <https://www.unicode.org/charts/nameslist/n_2580.html>
- **[S7]** IETF RFC 4648, *The Base16, Base32, and Base64 Data Encodings*, section 4: three input octets encoded as four Base64 characters. <https://www.rfc-editor.org/rfc/rfc4648.html#section-4>
- **[S8]** IETF RFC 4254, *The Secure Shell (SSH) Connection Protocol*: channel flow control/data, PTY dimensions, session data, and window changes. <https://www.rfc-editor.org/rfc/rfc4254.html>
- **[S9]** ncurses, *terminfo(5)* and *user_caps(5)*: capability database semantics and user-defined extensions. <https://invisible-island.net/ncurses/man/terminfo.5.html> and <https://invisible-island.net/ncurses/man/user_caps.5.html>
- **[S10]** tmux, *tmux(1)* current manual: terminal boundary, `allow-passthrough`, synchronized updates, `extended-keys`, `extended-keys-format`, `terminal-features`, and `sixel_support`. <https://man.openbsd.org/tmux.1>
- **[S11]** tmux source at commit `31dccb6bc9521b0ea46307974d071ad7f09f0e9b`: passthrough defaults/dispatch, synchronized updates, native Sixel build flag/parser/storage/output, DA detection, extended-key parsing, and mouse translation. <https://github.com/tmux/tmux/tree/31dccb6bc9521b0ea46307974d071ad7f09f0e9b>
- **[S12]** tmux, `CHANGES` at commit `31dccb6bc9521b0ea46307974d071ad7f09f0e9b`: basic Sixel in 3.4, `--enable-sixel`, `sixel_support`, and subsequent fixes. <https://github.com/tmux/tmux/blob/31dccb6bc9521b0ea46307974d071ad7f09f0e9b/CHANGES>
- **[S13]** Nicholas Marriott, tmux maintainer comment on response-requiring passthrough, 2025-02-25. <https://github.com/tmux/tmux/issues/4386#issuecomment-2681737611>
- **[S14]** foot, *Control Sequences* at commit `8db88cceb758b5be23e7db1fe74a48102ab07dc0`: Sixel, synchronized updates, Kitty keyboard controls, SGR mouse, and SGR-Pixels mouse. <https://codeberg.org/dnkl/foot/src/commit/8db88cceb758b5be23e7db1fe74a48102ab07dc0/doc/foot-ctlseqs.7.scd>
- **[S15]** foot, *foot.ini* at commit `8db88cceb758b5be23e7db1fe74a48102ab07dc0`: Sixel processing default. <https://codeberg.org/dnkl/foot/src/commit/8db88cceb758b5be23e7db1fe74a48102ab07dc0/doc/foot.ini.5.scd>
- **[S16]** foot source at commit `8db88cceb758b5be23e7db1fe74a48102ab07dc0`, `vt.c`: APC/SOS/PM string content is ignored. <https://codeberg.org/dnkl/foot/src/commit/8db88cceb758b5be23e7db1fe74a48102ab07dc0/vt.c>
- **[S17]** Ghostty, *Features*: advertised Kitty graphics and Kitty keyboard support. <https://ghostty.org/docs/features>
- **[S18]** Ghostty source at commit `70c498ac3273661aebf6cce9904c0d42b2e5d299`, Kitty graphics module and storage: implementation scope and TODO qualifications. <https://github.com/ghostty-org/ghostty/blob/70c498ac3273661aebf6cce9904c0d42b2e5d299/src/terminal/kitty/graphics.zig> and <https://github.com/ghostty-org/ghostty/blob/70c498ac3273661aebf6cce9904c0d42b2e5d299/src/terminal/kitty/graphics_storage.zig>
- **[S19]** Ghostty source at commit `70c498ac3273661aebf6cce9904c0d42b2e5d299`, terminal modes: SGR 1006 and SGR-Pixels 1016. <https://github.com/ghostty-org/ghostty/blob/70c498ac3273661aebf6cce9904c0d42b2e5d299/src/terminal/modes.zig>
- **[S20]** WezTerm, *Change Log*: Sixel evolution, Kitty Image Protocol support, shared-memory transfer, and compatibility fixes. <https://wezterm.org/changelog.html>
- **[S21]** WezTerm, `enable_kitty_keyboard`: Kitty keyboard protocol handling. <https://wezterm.org/config/lua/config/enable_kitty_keyboard.html>

## Product Decisions Needed Before Choosing Technology

1. **Minimum viewport contract:** Is the MVP required to show shaded solids at pixel resolution, or can an edge-first cell view satisfy the first usable workflow?
2. **Precision semantics:** Which actions require exact click picking, which may snap or disambiguate, and which can be keyboard-only?
3. **Terminal support set:** Which named terminal versions are release targets? Is support based on a tested version matrix rather than broad protocol labels?
4. **tmux status:** Is tmux first-class, best effort, or unsupported for the initial viewport? If first-class, are minimum version, compile flags, and user configuration acceptable?
5. **tmux topology:** Must both `tmux -> ssh -> ThreeTerm` and `ssh -> tmux -> ThreeTerm` work, including nested or multi-client sessions?
6. **SSH envelope:** What RTT, bandwidth, and viewport size must remain interactive? Is low-bandwidth mode a required product behavior?
7. **Latency budget:** What are acceptable p50, p95, and maximum input-to-visible-update times for orbit, pan, zoom, selection, and feature edits?
8. **Frame-rate policy:** Is the target a fixed minimum, adaptive quality, render-on-input, or high-quality settle after interaction?
9. **Input baseline:** Are legacy key press/repeat and cell mouse coordinates sufficient, or are disambiguated keys, release events, hover, and pixel coordinates required?
10. **Visual modes:** Which of wireframe, hidden-line, flat shading, smooth shading, section view, selected-face highlight, dimensions, and large assemblies are MVP requirements?
11. **Fallback meaning:** Does degradation preserve editing, become inspect-only, switch visual mode, or stop with a clear diagnostic?
12. **Lifecycle expectations:** Must graphics survive resize, scrollback, copy mode, tmux detach/reattach, multiple clients, and terminal reconnection?
13. **Resource policy:** What CPU, memory, and bandwidth may rendering consume, and can remote rendering reduce resolution automatically?
14. **Renderer ownership:** Is a software rasterizer acceptable, may ThreeTerm use an offscreen GPU, and which dependencies are tolerable?
15. **Support override:** Can users force a backend or disable active probing, and what diagnostic information may they attach to bug reports?

## Recommended Evidence Spikes

### E1: Protocol conformance fixture

**Question:** Which exact graphics operations work on the required terminal versions and paths?

**Fixture:** A deterministic test tool that queries capabilities, draws a numbered image, replaces it, moves/crops it where supported, deletes it, redraws surrounding text, resizes, scrolls, enters/exits alt screen, and records replies and a screen capture.

**Matrix:** Direct kitty, WezTerm, Ghostty, foot, and xterm where applicable; tmux builds with and without Sixel; passthrough off/on; SSH on each side of tmux; nested tmux only if required.

**Measure:** Exact operations accepted, response correctness, image position/size, stale artifacts, cleanup, retained memory, detach/reattach, and failure diagnostics.

**Exit criterion:** A versioned operation-level capability matrix. Do not reduce the result to one `supports_kitty` or `supports_sixel` boolean.

### E2: Representative viewport corpus

**Question:** Which display qualities remain useful for real CAD work?

**Fixture:** Small prismatic part, curved part, dense sketch, fillets, overlapping bodies, transparent/section view, selected face/edge, dimensions, and a larger assembly. Include adversarial thin edges and nearly coincident geometry.

**Compare:** Braille monochrome/colored wireframe, block-based color sampling, indexed raster suitable for Sixel, and RGB/RGBA raster suitable for Kitty.

**Review:** Blind task-based evaluation for identifying topology, selecting small features, reading dimensions, understanding depth, and noticing failed/changed geometry.

**Exit criterion:** Minimum useful viewport resolution and required visual modes, including a documented failure envelope for each degraded path.

### E3: Render and encode benchmark

**Question:** Where does frame time go before bytes reach the PTY?

**Fixture:** The E2 scenes at 120x40 cells and pixel viewports of 800x600, 1200x800, and 1920x1080 where supported; static, continuous orbit, and localized highlight cases.

**Compare:** Full and incremental Unicode, Sixel encoders/palette strategies, Kitty RGB/RGBA with zlib, PNG, and any viable local medium. Test coarse-while-moving/high-quality-on-idle.

**Measure:** Projection/raster time, compression/quantization time, encoded bytes, allocations, CPU, memory, and maximum sustainable newest-frame rate.

**Exit criterion:** A measured local frame budget and rejection of options that cannot avoid stale-frame buildup.

### E4: End-to-end input-to-photon benchmark

**Question:** What latency does a user actually experience?

**Method:** Inject timestamped keyboard and mouse events or use a camera/high-speed capture path; mark the resulting frame visually. Separate app receipt, render completion, write completion, and visible update where instrumentation permits.

**Paths:** Direct terminal, native Sixel tmux, Kitty passthrough tmux if still considered, and SSH with controlled RTT/bandwidth.

**Measure:** p50/p95/p99 input-to-app and input-to-visible latency, jitter, dropped/coalesced inputs, stale-frame age, and recovery after a burst.

**Exit criterion:** Evidence against the product latency budget from Decision 7, not a subjective "looks smooth" report.

### E5: Controlled SSH envelope

**Question:** At what link conditions does each path cease to be interactive?

**Matrix:** At least 1/10/50/100 ms RTT and 10/50/100/1000 Mbit/s downstream limits, with and without jitter. Use the same camera trace and scene frames for every run.

**Measure:** Bytes/frame, achieved displayed updates, input latency, write-blocked duration, SSH CPU, backlog depth, and time to catch up after motion stops.

**Exit criterion:** A supported remote envelope and an adaptive downgrade policy with measured thresholds.

### E6: Input semantics matrix

**Question:** Which controls can be made consistent without requiring the richest protocol?

**Fixture:** Escape, Ctrl/Shift/Alt combinations, non-US layout shortcuts, held navigation, repeat/release, drag orbit, drag pan, wheel zoom, hover, click picking, and viewport-edge behavior.

**Paths:** Required terminals direct, through tmux, and over SSH in both required topologies.

**Measure:** Exact bytes/events, ambiguity, lost modifiers, repeat behavior, release behavior, cell/pixel coordinates, event rate, and user-visible control result.

**Exit criterion:** A minimum input contract and explicit enhanced behaviors, including controls that must change under tmux.

### E7: Lifecycle and fault injection

**Question:** Can the TUI recover from terminal and transport lifecycle events without corruption?

**Inject:** Resize during frame, partial write, process kill, terminal quota pressure, tmux image eviction, text overlap, scroll, pane hide/show, copy mode, detach/reattach from a different terminal, SSH disconnect/reconnect, and malformed replies.

**Measure:** Terminal state restoration, stale artifacts, memory, re-probe behavior, time to a correct frame, and diagnostic clarity.

**Exit criterion:** A renderer lifecycle contract and bounded recovery path for every supported environment.

## Ticket-Sharpening Implications

Research should sharpen implementation work into behavior slices rather than a ticket named "add terminal graphics":

1. A capability-probe ticket should name exact queries, timeout behavior, tmux topology, parser fixtures, overrides, and the diagnostic output contract.
2. A scene-projection ticket should produce protocol-neutral projected primitives and stable picking data, proving that display degradation cannot change model truth.
3. A renderer spike should target one named terminal/path/viewport/scene and a numeric latency/throughput exit criterion. It should not silently become the production protocol decision.
4. A frame scheduler ticket should specify bounded in-flight work, newest-state coalescing, resize cancellation, write backpressure, and stale-frame metrics.
5. An input ticket should state its semantic level: legacy, disambiguated, event types, cell mouse, or pixel mouse. "Mouse support" and "Kitty keyboard support" are too broad.
6. A tmux ticket should distinguish native Sixel from Kitty passthrough and should include build flags, server/client capability differences, image lifecycle, and response-routing constraints.
7. An SSH ticket should declare RTT/bandwidth fixtures and topology order. "Works remotely" is not an acceptance criterion.
8. A fallback ticket should name the user workflow it preserves and include visual usability tests, not only successful character output.
9. A compatibility ticket should pin terminal and tmux versions and test operations, not brands or protocol names.
10. A production technology-selection ticket should remain blocked on the product decisions and evidence exits above; feature-list comparison alone is insufficient.
