# notecrumbs · Agent Briefing

## Project Snapshot
- notecrumbs is a Nostr OpenGraph renderer built on `nostrdb`, `egui`, and `skia`.
- The HTTP server lives in `src/main.rs`; rendering logic is split across `src/render.rs` and `src/html.rs`.
- Static assets (fonts, default profile picture) are loaded from the repo root, so run binaries from this directory.

## Environment & Tooling
- Requires a Rust toolchain with Skia prerequisites installed (`clang`, `cmake`, Python, build essentials).
- `nostrdb` stores data in the current working directory via `Ndb::new(".", …)`.
- `TIMEOUT_MS` environment variable tunes remote fetch waits (default 2000 ms).

## Workflow Notes
- Build: `cargo build --release` (first run is slow while Skia compiles).
- Tests: `cargo test` (currently empty but should stay green).
- Formatting: `cargo fmt`; lint: `cargo clippy -- -D warnings`.

## Coding Guidelines
- Use `tracing` macros for logging.
- HTML/attribute-escape user content before injecting into responses (`html.rs` has helpers).
- Rendering paths should avoid blocking the async runtime; reuse cached textures.
- `RenderData::complete` pairs nostrdb subscriptions with network fetches—keep it resilient to partially missing data.

## Testing Expectations
- Add unit tests around pure helpers (e.g., `abbrev.rs`) when modifying them.
- For nostrdb-heavy flows, prefer integration tests or mocked paths gated for CI.
- Manually spot-check PNG (`/<bech32>.png`) and HTML (`/<bech32>`) outputs when touching rendering code.

## Open Threads
- Consult `TODO` for active work (local relay model, formatting for unparsed notes).
- README “Status” checklist highlights upcoming features like profile picture rendering/caching.

## Pre-PR Checklist
- `cargo fmt --check`
- `cargo clippy -- -D warnings`
- `cargo test`
- Optional: manual PNG/HTML smoke tests against representative notes/profiles.

## Contribution Practices
1. Make every commit logically distinct.
2. Ensure each commit is standalone so it can be dropped later without breaking the build.
3. Keep code readable and reviewable by humans—favor clarity over cleverness.
