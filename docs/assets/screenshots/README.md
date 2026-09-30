# Screenshot provenance

These images show ThreeTerm's actual production viewport and command-palette
output, using real OCCT geometry. They are **compact component captures from a
scripted session replayed in Ghostty**, not whole-screen live-app screenshots and
not proof that the live-terminal startup gate passed.

The recording uses `crates/tui/examples/docs_capture.rs`. It calls the production
`launch` function, with the same scripted terminal-capability fixture used by
the integration tests. Geometry is loaded/replayed by the real host and native
OCCT worker; input, capability observations, and frame acknowledgements are
scripted. `scripts/prepare-docs-captures.py` extracts the final unchanged viewport
image and the actual application status/palette text, removing earlier frames
and verbose conformance diagnostics. It places those components at the top of a
fresh terminal for capture. The image pixels and application text are unchanged;
the compact arrangement is for documentation, not a claim about live-app layout.
That Kitty graphics/text stream is displayed in real Ghostty and captured as PNG.

- `bracket.png`: a committed 40 × 20 × 30 mm bracket, 3 mm thickness, selected and orbited.
- `palette.png`: the command palette open over the committed model.
- `preview.png`: an uncommitted bracket draft with 50 × 20 × 35 mm dimensions.

This distinction matters: the current production startup probe requires positive
keyboard, mouse, focus, resize, and image-acknowledgement observations. An ordinary
Ghostty launch can fail that gate. The screenshots do not bypass the gate for the
installed application, and the installer does not change that capability policy.

## Recreate the output

After `make install`, create an example project with the installed CLI:

```sh
threeterm new-project "$PWD/target/docs-bracket.threeterm"
threeterm --machine bracket "$PWD/target/docs-bracket.threeterm" \
  --bracket-id my-bracket --length 40 --width 20 --height 30 --thickness 3

CARGO_TARGET_DIR="$PWD/target/install" \
THREETERM_OCCT_DIR="$PWD/target/install/native-workers/prefixes/occt" \
THREETERM_SLVS_DIR="$PWD/target/install/native-workers/prefixes/slvs" \
LD_LIBRARY_PATH="$PWD/target/install/native-workers/prefixes/occt/lib:$PWD/target/install/native-workers/prefixes/slvs/lib" \
THREETERM_PALETTE=catppuccin \
cargo run --release -p threeterm-tui --example docs_capture -- \
  "$PWD/target/docs-bracket.threeterm" "$PWD/target/docs-capture"
```

Prepare the compact presentation (Python is a documentation-only dependency):

```sh
python3 scripts/prepare-docs-captures.py target/docs-capture
```

Display an individual `.capture.ansi` recording in an 80-column Ghostty window
with echo disabled (`stty raw -echo`) so terminal replies are not printed. Keep
enough rows for the viewport and its status/palette text. Capture only those
components with your desktop screenshot tool. Restore terminal settings or close
that temporary window when finished. Use the original `.ansi` files if you want
the complete production stream, including the conformance diagnostics.

Do not use these scripted captures as evidence for the qualified graphical
runner. Live startup verification is documented separately in
[`docs/development.md`](../../development.md).
