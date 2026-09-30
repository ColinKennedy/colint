# colint

`colint` is an MIT-licensed Rust command-line linter for opinionated Python
code reviews. It parses Python with Tree-sitter, so diagnostics include stable
source locations even for multiline expressions.

## Install

Once published, install the `ColinKennedy` PyPI distribution with:

```powershell
python -m pip install colint
```

```powershell
cargo run -- path\to\project
cargo run -- --strict --json src
cargo run -- --include-header src
```

Contributors can run the same quality gates as CI with `cargo fmt --check`,
`cargo clippy --all-targets -- -D warnings`, and `cargo test --all-targets`.

Rules have stable codes in the `GRP-NNN` form, grouped by check category
(`API`, `DOC`, `STY`, `GUI` — see [Checks](#checks) below). A line may
suppress all findings
with `# noqa`, or selected findings with `# colint: ignore[STY-001,API-005]`.
Rule names work too, e.g. `# colint: ignore[nested-import]`, and both forms
may be mixed in the same bracket.
For a nested import (STY-001), use `# NOTE: reason` immediately above the
import or contiguous import group; blank lines do not split that group. Use
this NOTE form for any intentional nested import, especially generated code.

Pass `--include-header` to prepend this suppression guidance to ordinary
human-readable output. The header is not included by default and is never
included in `--json` output.

The complete agreed rule and behavior contract is in [DESIGN.md](DESIGN.md).

## Checks

Rule codes are grouped by check category. Each group's number restarts at 1.

### API — Function Design & Correctness

| Code | Rule | Severity | Justification |
| --- | --- | --- | --- |
| API-001 | predicate-in-function | warning | A conditional no-op hides control flow the caller should own. |
| API-002 | loft-from-caller | warning | Passing a whole object hides the one value the callee needs. |
| API-003 | redundant-type-hint | error | A duplicated annotation can silently drift from the real type. |
| API-004 | empty-string-return | error | An empty string return is easily confused with a real value. |
| API-005 | assert-in-production | error | `python -O` strips asserts, silently disabling the check. |
| API-006 | none-polymorphism | warning | Treating `None` as falsy avoids a second, redundant check. |

### DOC — Documentation

| Code | Rule | Severity | Justification |
| --- | --- | --- | --- |
| DOC-001 | docstring-convention | warning | Inconsistent parameter markup breaks generated documentation. |
| DOC-002 | direct-raises-only | error | Documenting inherited raises hides which errors this call owns. |

### STY — Style & Organization

| Code | Rule | Severity | Justification |
| --- | --- | --- | --- |
| STY-001 | nested-import | error | Nested imports hide dependencies and cost every call to reload. |
| STY-002 | pytest-expected-left | warning | Expected-first assertions give clearer, consistent failures. |
| STY-003 | logging-dynamic-quote | warning | Unquoted placeholders hide whitespace or empty values in logs. |
| STY-004 | module-order | warning | A fixed layout makes every module easier to scan and navigate. |

### GUI — Qt/PySide Widgets

| Code | Rule | Severity | Justification |
| --- | --- | --- | --- |
| GUI-001 | widget-tooltip | warning | A missing tooltip leaves users without in-app guidance. |
| GUI-002 | qt-model-parent | error | Without a parent, Qt cannot manage the model's lifetime. |

## Configuration

Create `.colint.toml` in the working directory (or a parent directory):

```toml
warnings_as_errors = false
docstring_convention = "mkdocs"
# API-002 skips private definitions by default. Private means an underscore-
# prefixed function or method, or any member of an underscore-prefixed class.
api002_skip_private_definitions = true
# Additional Python package roots used to resolve imported Qt model/proxy bases.
# Relative paths are resolved from this configuration file.
import_paths = ["../shared-python", "C:/work/company-python"]

[rules]
STY-001 = true
GUI-001 = false
```

Every rule is enabled by default. `--strict` enables every rule regardless of
configuration and treats warnings as errors. Exit status is `0` for clean runs
and ordinary warnings, `1` for warnings escalated to errors, and `2` whenever
an error-level finding exists.

`docstring_convention = "mkdocs"` enables the MkDocs parameter-markup check.
`api002_skip_private_definitions = false` makes API-002 lint private functions,
methods, and class members too.
`import_paths` supplements `PYTHONPATH` when `GUI-002` resolves imported
project classes. These roots are used for inheritance discovery only; colint
does not emit findings for files that were loaded solely from an import path.
