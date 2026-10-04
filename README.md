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
with `# noqa`, or selected findings with `# noqa: STY-001,API-005` or
`# colint: ignore[STY-001,API-005]`. Rule names work too, e.g.
`# colint: ignore[nested-import]`, and both forms may be mixed in the same
list. Codes are matched case-insensitively and spacing is flexible
(`#colint:ignore[ sty-001 ]`). `# noqa: E501`, which names only another
linter's codes, does not suppress colint findings. Suppressions must be real
comments on the finding's first line; text inside a string literal is ignored.
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
| DOC-001 | docstring-convention | warning | Mixed `*bar*`, `"bar"`, and `` `bar` `` parameter markup breaks generated documentation. |
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

### Rule details

- **Test code** (API-005 allows `assert`, STY-002 applies) is a file named
  `test_*.py`, `*_test.py`, `test.py`, `tests.py`, or `conftest.py`, or any
  file below a `test`, `tests`, or `testing` directory. Names that merely
  contain "test", such as `latest.py` or `contest/`, are production code.
- **API-001** reports a function whose first statement (after any docstring)
  is an `if` without `elif`/`else` whose only body statement is `return`.
- **API-002** counts real references to each parameter (not text in strings
  or comments) and handles defaults, annotations, and multi-line calls when
  naming the consumer. Methods decorated with `@override` are skipped because
  their signatures are fixed by the base class.
- **API-003** compares annotations ignoring whitespace, comments, trailing
  commas, and quoting. `self.method()`/`cls.method()` calls are compared with
  methods; plain calls with functions. `await` is unwrapped.
- **API-004** recognizes any empty `str` literal (`""`, `''`, `u""`, `r""`,
  ``, parenthesized or implicitly concatenated) and ignores returns in
  nested functions. `Optional[...]`/`Union[..., None]` count as including
  `None`.
- **API-006** reports `x is None`/`x is not None` in `if`/`elif` conditions
  when `x` is a parameter or annotated local declared `X | None`,
  `None | X`, `Optional[X]`, or `Union[X, None]`.
- **DOC-001** checks the function's own docstring (including raw strings) for
  parameter names wrapped in markup other than the configured
  `[docstring_variable_markup]` (`` `name` `` by default): `*name*`,
  `**name**`, `"name"`, `'name'`, ``` ``name`` ```, or curly quotes. Each
  occurrence is reported. Bare names, Google-style `name:` entries, doctest
  examples, fenced code blocks, and expressions such as `2*x*3` are ignored. A
  one-character name is reported only for `*x*`/`**x**` emphasis. Suppress it
  on the `def` line or after the docstring's closing quotes.
- **DOC-002** reads the Google-style `Raises:` section of the function's own
  docstring and reports each documented exception that the function does not
  raise directly. A bare `raise` re-raises the enclosing `except` types, and a
  documented base class covers a raised subclass defined in the same module.
- **STY-001** applies only inside functions and classes; module-level
  `if TYPE_CHECKING:` and `try:`/`except ImportError:` imports are not nested.
  A justification comment must start with `NOTE` (any case).
- **STY-003** checks the message argument of `debug`/`info`/`warning`/
  `error`/`exception`/`critical`/`log` calls on `logging`, `logger`, `log`,
  `LOGGER`, `self._log`, `logging.getLogger(...)`, and similar receivers.
  `%s` and `%(name)s` must be wrapped in matching quotes; `%r` already shows
  quotes, and `%%` is a literal percent sign.
- **STY-004** expects globals, then classes, then functions. Decorated
  definitions are classified by what they decorate, comments are ignored, and
  an `if __name__ == "__main__":` block may close the module.
- **GUI-001** reports each widget assignment (named `*Widget`, `*Button`,
  `*Label`, `*ComboBox`, or a local `QWidget` subclass) that has no reachable
  `setToolTip(...)` and no non-empty `toolTip=` keyword. Within a class, every
  method reachable through `self.` calls from, or leading to, the constructing
  method counts. A `# no tooltip: reason` comment exempts the scope.
- **GUI-002** treats `parent=self` or any non-literal positional argument as a
  parent. Constructing a project model subclass also requires a parent.
  Subclass initializers may forward `parent` by position, keyword,
  `*args`/`**kwargs`, `super(Class, self).__init__`, or `Base.__init__(self, ...)`.

## Configuration

Create `.colint.toml` in the working directory (or a parent directory):

```toml
warnings_as_errors = false
# API-002 skips private definitions by default. Private means an underscore-
# prefixed function or method, or any member of an underscore-prefixed class.
api002_skip_private_definitions = true
# Additional Python package roots used to resolve imported Qt model/proxy bases.
# Relative paths are resolved from this configuration file.
import_paths = ["../shared-python", "C:/work/company-python"]

# DOC-001 markup expected around parameter names in docstrings. Both default
# to a single backtick, the Markdown inline-code syntax.
[docstring_variable_markup]
start = "`"
end = "`"

[rules]
STY-001 = true
GUI-001 = false
```

Every rule is enabled by default. `--strict` enables every rule regardless of
configuration and treats warnings as errors. Exit status is `0` for clean runs
and ordinary warnings, `1` for warnings escalated to errors, and `2` whenever
an error-level finding exists.

`[docstring_variable_markup]` sets the `start` and `end` markup DOC-001
expects around parameter names in docstrings. The former
`docstring_convention` setting is no longer used and is ignored if present.
`api002_skip_private_definitions = false` makes API-002 lint private functions,
methods, and class members too.
`import_paths` supplements `PYTHONPATH` when `GUI-002` resolves imported
project classes. These roots are used for inheritance discovery only; colint
does not emit findings for files that were loaded solely from an import path.
