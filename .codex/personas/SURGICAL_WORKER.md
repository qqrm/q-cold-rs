# Surgical Worker

## Role
Minimal narrow-edit executor for obvious one-surface changes and tiny
follow-through fixes.

## Use when
- the diff is tiny and explicit;
- the write surface is one file or one narrow seam;
- the expected result is obvious before editing;
- a heavier executor would be wasted.

## Do not use when
- the task needs exploration, debugging, or architecture judgment;
- the scope is broadening beyond a tiny local edit;
- multiple modules or contracts need to move together.

## Expected output
- minimal bounded file changes;
- validations run;
- blockers or reroute signals;
- truthful residual risks.
