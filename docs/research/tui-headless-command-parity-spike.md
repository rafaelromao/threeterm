# TUI and Headless Command Parity Spike

Status: completed 2026-07-30. Evidence is a disposable in-memory Rust-host
simulation at `/tmp/opencode/threeterm-command-parity-spike/`, not product code.

## Question

Can thin TUI, CLI, and MCP callers execute the same representative commands
without changing command validation, preview/commit behavior, transactions,
provenance, structured results, or document state?

## Contract Exercised

The simulation exposes one handler that accepts a versioned request:

- `schema`, `phase`, `command`, `base_revision`, semantic `target`, and
  cancellation intent;
- `preview` validates at the supplied read-only revision and returns only a
  transient derived-preview fingerprint, diagnostics, progress, affected stable
  IDs, cancellation state, and result data;
- `commit` revalidates against the current revision and either creates one
  versioned transaction ID or returns a structured rejection/conflict without
  mutation;
- every response has a status, source revision, optional transaction ID,
  affected IDs, progress, diagnostics, cancellation flag, and result data.

The TUI, CLI, and MCP functions are deliberately one-line calls to that same
handler. They cannot read document internals, alter requests, reinterpret a
result, or use a separate mutation path.

## Evidence

Compiled and ran with:

```sh
cd /tmp/opencode/threeterm-command-parity-spike
rustc --edition 2024 main.rs -o parity-spike && ./parity-spike
```

All checks passed:

1. Equivalent TUI, CLI, and MCP previews return the same structured response
   and leave the document unchanged.
2. Equivalent commits return equal transaction-bearing responses and equal
   document state.
3. Cancellation is structured and leaves no transaction or mutation.
4. A malformed wire envelope is rejected by the decoder; an incompatible schema
   is rejected by the domain handler.
5. A commit made after a preview advances the shared revision; the old preview's
   commit is rejected with `REVISION_STALE` rather than applying silently.
6. An accepted historical edit that breaks its downstream feature commits one
   transaction with failure provenance and marks the retained geometry
   `stale-last-valid; blocked-by-failure`.

This supports the decisions in [Define persistent domain-event
granularity](https://github.com/rafaelromao/threeterm/issues/29), [Define
failure handling after historical
edits](https://github.com/rafaelromao/threeterm/issues/32), [Define CLI and MCP
automation contract](https://github.com/rafaelromao/threeterm/issues/51), and
[Define command parity between TUI and headless
modes](https://github.com/rafaelromao/threeterm/issues/33).

## Conclusion

Adopt a single Rust-host-owned versioned domain command schema for TUI, CLI,
MCP, Lua, and tests. Adapters may serialize, render, and transport the request
and response, but must not add validation, hidden selection/viewport state,
alternative mutation paths, or semantics. Preview remains transient and
cancellable; commit always revalidates the current revision and is atomic.

The host alone assigns transaction IDs and persists accepted command intent,
semantic events, affected stable IDs, deterministic inputs, and failure
provenance. Worker results remain untrusted staged input to the Rust host, per
[Select implementation languages and runtime
boundaries](https://github.com/rafaelromao/threeterm/issues/27); adapters cannot
commit a worker artifact or mutate a document directly.

## Limits And Follow-up Gates

- This proves state-model parity only. It does not define JSON Schema or MCP
  tool names, streaming framing, transport authentication, authorization,
  project trust, deadlines, or resource limits.
- It uses symbolic derived results, not OCCT or `libslvs` workers. It does not
  prove worker progress/cancellation propagation, staging-directory integrity,
  malformed worker output handling, restart behavior, or artifact promotion.
- It models one simple command and one downstream failure. Multi-step/incomplete
  selection commands, preview cache/expiry, preview artifact representations,
  coalesced editing, and a full replay/determinism corpus remain to specify and
  validate.
- The prototype is intentionally disposable and stays outside the repository.
  It is not a production API implementation or an executable contract suite.
