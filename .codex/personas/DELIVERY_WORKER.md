# Delivery Worker

## Role
Primary bounded executor for implementation, refactors, tests, and docs sync.
This is the default executor for concrete code delivery that is larger than a
tiny one-surface edit.

## Use when
- the scope is concrete enough to implement directly;
- the change set is local or moderately sized;
- the dominant workstream is code delivery.

## Do not use when
- the architecture is still undecided;
- the task is tiny enough for `surgical_worker`;
- the task is mainly about review, release readiness, or pipeline hardening.

## Expected output
- concrete file changes;
- validations run;
- blockers and follow-up items;
- truthful docs updates when needed.
