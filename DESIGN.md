# colint design decisions

This file records the initial product decisions agreed on 2026-09-23.

## Rule identity and default severity

Every diagnostic has a stable `COL-NNN` code and a descriptive rule name. The code is the configuration and suppression identifier.

| Code | Rule | Default severity |
| --- | --- | --- |
| COL-001 | predicate-in-function | warning |
| COL-002 | loft-from-caller | warning |
| COL-003 | nested-import | error |
| COL-004 | docstring-convention | warning |
| COL-005 | redundant-type-hint | error |
| COL-006 | pytest-expected-left | warning |
| COL-007 | logging-dynamic-quote | warning |
| COL-008 | module-order | warning |
| COL-009 | empty-string-return | error |
| COL-010 | assert-in-production | error |
| COL-011 | none-polymorphism | warning |
| COL-012 | direct-raises-only | error |
| COL-013 | widget-tooltip | warning |
| COL-014 | qt-model-parent | error |

`COL-009` recommends `None` for a not-found result and requires a return annotation that includes `None`, including unions with other result types. `COL-005` only compares direct calls to functions defined in the same parsed module; imported symbols are deliberately out of scope. `COL-006` applies only to ordinary pytest equality assertions. `COL-012` follows Google-style `Raises:` documentation and permits it only where the function itself directly raises. `COL-002` identifies the queried parameter(s) and, when it can identify a direct consumer, the function to which the queried value should be lofted. `COL-013` applies to widget instances, not custom widget-class definitions or their constructors.

## Configuration and strict mode

`.colint.toml` is discovered from the current directory upward. Its top-level `warnings_as_errors` boolean escalates warning-only results. `[rules]` maps a rule code to `true` or `false`, enabling or disabling it.

`col002_skip_private_definitions` defaults to `true`. It skips COL-002 for
underscore-prefixed functions and methods, plus every member of an
underscore-prefixed class. Set it to `false` to lint those definitions.

`--strict` overrides individual rule disables, enables every registered rule, and treats warnings as errors.

## Exit statuses

| Status | Meaning |
| --- | --- |
| 0 | No errors; warnings are permitted unless escalated. |
| 1 | Warnings only, escalated through `warnings_as_errors` or `--strict`. |
| 2 | At least one native error-level finding. |

## Suppressions

`# noqa` suppresses all diagnostics on its source line. `# colint: ignore[COL-003,COL-010]` suppresses only the listed rule codes on that line. For nested imports, use a `# NOTE: reason` justification comment immediately before one import or a contiguous import group; it may be multiline and blank lines do not split the group. This explicit NOTE syntax is the required guidance for generated code.

## Input and output

The CLI accepts Python files and directories, recursively scans `*.py`, and ignores common generated and virtual-environment directories. Diagnostics use `path:line:column: COL-NNN - message [severity]`, include a fix recommendation, and `--json` emits structured diagnostics. `--include-header` prepends suppression guidance to human-readable output only. Python 3.10+ syntax is targeted. Automatic rewriting is intentionally not part of this first release.

## Rust quality gates

The repository uses `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`. GitHub Actions runs all three checks on every push and pull request.
