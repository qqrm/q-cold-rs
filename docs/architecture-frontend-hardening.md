# Architecture And Frontend Hardening Audit

This note records the final architecture/frontend hardening state after the
remediation queue. It is an integration audit, not a new feature contract.

This pass also fixed a frontend integration bug where graph queue display logic
compared `queueRun.status` with `QcoldQueueExecutionMode.Graph`. The dashboard
now carries `queueRun.executionMode` separately from run status.

## Confirmed

- Production frontend asset order is owned by
  `src/webapp_assets/app_js_assets.rs`. `src/webapp/models_assets.rs` and
  `xtask/src/task/verify.rs` both consume that list, and `xtask` runs
  `node --check` on each asset plus the concatenated production bundle.
- Frontend dashboard requests are centralized in `src/webapp_assets/app/api.js`.
  Other app assets are covered by `src/webapp/tests_assets.rs` so direct
  `fetch(...)` calls do not drift back into UI handlers.
- Mutating dashboard requests can require `X-QCOLD-Write-Token`. The browser
  stores the operator-entered token in session storage and the served assets do
  not embed `QCOLD_WEBAPP_WRITE_TOKEN`.
- Queue run status, item status, execution mode, and execution host are typed
  in `src/state/queue_types.rs`. Frontend helpers in
  `src/webapp_assets/app/queue_status.js` mirror those public strings.
- `QueueRunRow`, `QueueTabRow`, and `QueueItemRow` remain storage rows. Web
  snapshots map them through `WebQueueRun`, `WebQueueTab`, and `WebQueueItem`
  in `src/webapp/models_assets.rs`.
- Graph queue validation has one backend normalization path in
  `src/webapp/queue_graph.rs`. Queue run, append, and update APIs return
  `queue_graph` diagnostics, and the frontend renders those diagnostics through
  `QcoldApi.queueGraphResponseMessage(...)`.
- SQLite migration coverage compares a fresh database against a representative
  upgraded schema in `src/state/db.rs`.
- Queue worker ownership uses durable SQLite leases before launching executor
  work. Lease takeover, stale recovery, retryable items, and restart
  reconciliation are covered in `src/webapp/tests_queue_state_model.rs`.
- `src/main.rs` and `src/qcold.rs` are thin binary wrappers around the ordinary
  `qcold` library crate. The storage layer has also moved to ordinary modules
  under `src/state/`.
- The adapter registry still preserves the `xtask-process` behavior through
  `src/adapter/mod.rs`; unsupported adapters fail explicitly.

## Follow-Ups

- `src/webapp.rs` still aggregates dashboard API, queue worker, snapshot,
  terminal, and web test fragments through `include!`. Split this after queue
  behavior stabilizes behind typed module APIs.
- `src/lib.rs` still includes app command fragments from `src/app/`. Extract
  them after CLI command contracts have stable typed boundaries.
- `src/agents.rs` and `src/queue.rs` still include feature fragments. Split by
  feature area after the state and queue tab APIs stop moving.
- `xtask/src/main.rs` still includes task helper fragments. This is fixture and
  validation aggregation debt, not a Q-COLD runtime boundary.
- `src/webapp_assets/app/init_parse.js` still owns broad dashboard state. Move
  feature-local state out after the current API client and status helper
  contracts stay stable through another queue iteration.
- `src/webapp/models_assets.rs` still embeds frontend assets at compile time.
  Keep `xtask/src/task/verify.rs` tied to the production asset list if Q-COLD
  later grows a separate frontend build pipeline.
- `src/state/db.rs` still owns inline SQLite schema and migrations. The fresh
  versus upgraded equivalence tests make this safe enough for now, but a future
  extraction should keep migration IDs and schema signatures executable.
