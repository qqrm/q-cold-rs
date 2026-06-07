# Rust Include Boundary Debt

Q-COLD now uses `src/lib.rs` as the shared crate root. The `cargo-qcold` and
`qcold` binaries are thin wrappers around that shared entrypoint, and the
`state` storage helpers are ordinary Rust modules.

Remaining intentional include sites:

- `src/lib.rs` still includes command fragments under `src/app/`. They share
  CLI command types, task-flow helpers, and rendering helpers. Split them after
  those command contracts have typed module APIs.
- `src/webapp.rs` still includes dashboard API, queue worker, snapshot, terminal,
  and test fragments. It is the next high-ROI backend slice, but it is tightly
  coupled to queue behavior and should stay separate from functional queue
  changes.
- `src/agents.rs` still includes agent registry, named-session, context reuse,
  terminal, and test fragments. Extract by feature area after the state API
  surface is stable.
- `src/queue.rs` still includes queue-tab display helpers and queue unit tests.
  Split it after queue tab display data has an explicit typed boundary.
- `src/webapp/models_assets.rs` and `src/webapp_assets/app_js_assets.rs` still
  assemble frontend assets at compile time. Frontend JavaScript files are now
  independent syntax units, and `xtask` consumes the production asset order for
  syntax checks. Keep this embedded asset path until Q-COLD deliberately grows
  a separate frontend build pipeline.
- `xtask` and test-support include sites are fixture or test aggregation debt,
  not production runtime boundaries.
