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

Rules have stable codes in the `COL-NNN` form. A line may suppress all findings
with `# noqa`, or selected findings with `# colint: ignore[COL-003,COL-010]`.
For a nested import (COL-003), use `# NOTE: reason` immediately above the
import or contiguous import group; blank lines do not split that group. Use
this NOTE form for any intentional nested import, especially generated code.

Pass `--include-header` to prepend this suppression guidance to ordinary
human-readable output. The header is not included by default and is never
included in `--json` output.

The complete agreed rule and behavior contract is in [DESIGN.md](DESIGN.md).

## Configuration

Create `.colint.toml` in the working directory (or a parent directory):

```toml
warnings_as_errors = false
docstring_convention = "mkdocs"
# COL-002 skips private definitions by default. Private means an underscore-
# prefixed function or method, or any member of an underscore-prefixed class.
col002_skip_private_definitions = true
# Additional Python package roots used to resolve imported Qt model/proxy bases.
# Relative paths are resolved from this configuration file.
import_paths = ["../shared-python", "C:/work/company-python"]

[rules]
COL-003 = true
COL-013 = false
```

Every rule is enabled by default. `--strict` enables every rule regardless of
configuration and treats warnings as errors. Exit status is `0` for clean runs
and ordinary warnings, `1` for warnings escalated to errors, and `2` whenever
an error-level finding exists.

`docstring_convention = "mkdocs"` enables the MkDocs parameter-markup check.
`col002_skip_private_definitions = false` makes COL-002 lint private functions,
methods, and class members too.
`import_paths` supplements `PYTHONPATH` when `COL-014` resolves imported
project classes. These roots are used for inheritance discovery only; colint
does not emit findings for files that were loaded solely from an import path.
