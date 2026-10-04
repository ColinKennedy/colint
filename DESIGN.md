# colint design decisions

This file records the initial product decisions agreed on 2026-09-23.

## Rule identity and default severity

Every diagnostic has a stable `GRP-NNN` code and a descriptive rule name. The
code is the configuration and suppression identifier. The prefix groups rules
by check category, and each group's number restarts at 1: `API` (function
design and correctness), `DOC` (documentation), `STY` (style and
organization), and `GUI` (Qt/PySide widgets).

| Code | Rule | Default severity |
| --- | --- | --- |
| API-001 | predicate-in-function | warning |
| API-002 | loft-from-caller | warning |
| API-003 | redundant-type-hint | error |
| API-004 | empty-string-return | error |
| API-005 | assert-in-production | error |
| API-006 | none-polymorphism | warning |
| DOC-001 | docstring-convention | warning |
| DOC-002 | direct-raises-only | error |
| STY-001 | nested-import | error |
| STY-002 | pytest-expected-left | warning |
| STY-003 | logging-dynamic-quote | warning |
| STY-004 | module-order | warning |
| GUI-001 | widget-tooltip | warning |
| GUI-002 | qt-model-parent | error |

`API-004` recommends `None` for a not-found result and requires a return annotation that includes `None`, including unions with other result types. `API-003` only compares direct calls to functions defined in the same parsed module; imported symbols are deliberately out of scope. `STY-002` applies only to ordinary pytest equality assertions. `DOC-001` reports a function parameter name in that function's docstring only when it is wrapped by a matching pair of markup characters (`*`, `**`, `"`, `'`, backticks, or curly quotes) other than the configured `[docstring_variable_markup]` `start`/`end` (a single backtick each by default). Bare names are ignored because they may not refer to the parameter, as are Google-style `name:` entries, doctest examples, and fenced code blocks. `DOC-002` follows Google-style `Raises:` documentation and permits it only where the function itself directly raises. `API-002` identifies the queried parameter(s) and, when it can identify a direct consumer, the function to which the queried value should be lofted. `GUI-001` applies to widget instances, not custom widget-class definitions or their constructors.

## Configuration and strict mode

`.colint.toml` is discovered from the current directory upward. Its top-level `warnings_as_errors` boolean escalates warning-only results. `[rules]` maps a rule code to `true` or `false`, enabling or disabling it.

`api002_skip_private_definitions` defaults to `true`. It skips API-002 for
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

`# noqa` suppresses all diagnostics on its source line. `# noqa: STY-001,API-005` and `# colint: ignore[STY-001,API-005]` suppress only the listed rule codes on that line; entries may also use rule names (e.g. `nested-import`) interchangeably with codes in the same bracket. For nested imports, use a `# NOTE: reason` justification comment immediately before one import or a contiguous import group; it may be multiline and blank lines do not split the group. This explicit NOTE syntax is the required guidance for generated code.

## Input and output

The CLI accepts Python files and directories, recursively scans `*.py`, and ignores common generated and virtual-environment directories. Diagnostics use `path:line:column: GRP-NNN - message [severity]`, include a fix recommendation, and `--json` emits structured diagnostics. `--include-header` prepends suppression guidance to human-readable output only. Python 3.10+ syntax is targeted. Automatic rewriting is intentionally not part of this first release.

## Robustness

Every check works on the Tree-sitter syntax tree rather than on source text, so
results do not depend on whitespace, CRLF line endings, tab indentation,
comments, or backslash line continuations, and nested definitions are never
reported twice. `src/permutation_tests.rs` enforces this: each rule's cases are
re-run under layout transformations and every suppression form.

## Rust quality gates

The repository uses `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`. GitHub Actions runs all three checks on every push and pull request.
