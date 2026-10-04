//! Permutation tests for every rule.
//!
//! Each case lists source variants (simple, complex, whitespace-heavy,
//! multi-line, backslash-continued, ...) that must all produce the same number
//! of findings for one rule. Every variant is then re-run under layout
//! transformations that must not change the result:
//!
//! * line-preserving ones (CRLF, tabs, two-space indentation, trailing
//!   whitespace, no final newline, comments after block headers) must report
//!   the same lines and messages;
//! * line-shifting ones (a module preamble, doubled blank lines) must report
//!   the same messages.
//!
//! Every positive variant is also checked against each suppression comment
//! form, against `[rules]` disabling, and against `--strict` re-enabling.

use super::*;

struct Case {
    code: &'static str,
    path: &'static str,
    expected: usize,
    variants: &'static [&'static str],
}

const fn case(
    code: &'static str,
    path: &'static str,
    expected: usize,
    variants: &'static [&'static str],
) -> Case {
    Case {
        code,
        path,
        expected,
        variants,
    }
}

/// Strips the leading newline and common indentation of a raw-string fixture.
fn dedent(source: &str) -> String {
    let source = source.strip_prefix('\n').unwrap_or(source);
    let indentation = source
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = source
        .lines()
        .map(|line| line.get(indentation..).unwrap_or("").trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n");
    out = out.trim_end().to_string();
    out.push('\n');
    out
}

type Transform = fn(&str) -> Option<String>;

/// Transformations that keep every statement on its original line.
const LINE_PRESERVING: &[(&str, Transform)] = &[
    ("crlf", |s| Some(s.replace('\n', "\r\n"))),
    ("tabs", |s| {
        Some(map_indentation(s, |n| {
            "\t".repeat(n / 4) + &" ".repeat(n % 4)
        }))
    }),
    ("two-space", |s| {
        Some(map_indentation(s, |n| " ".repeat(n / 2 + n % 2)))
    }),
    ("trailing-whitespace", |s| {
        Some(map_lines(s, |line| {
            if line.trim().is_empty() || line.ends_with('\\') {
                line.to_string()
            } else {
                format!("{line}   ")
            }
        }))
    }),
    ("no-final-newline", |s| {
        Some(s.trim_end_matches('\n').to_string())
    }),
    ("header-comments", |s| {
        Some(map_lines(s, |line| {
            let trimmed = line.trim_start();
            let header = [
                "def ",
                "async def ",
                "class ",
                "if ",
                "elif ",
                "else:",
                "try:",
                "for ",
                "with ",
            ]
            .iter()
            .any(|keyword| trimmed.starts_with(keyword));
            if header && line.ends_with(':') {
                format!("{line}  # layout comment")
            } else {
                line.to_string()
            }
        }))
    }),
];

/// Transformations that move statements to other lines.
const LINE_SHIFTING: &[(&str, Transform)] = &[
    ("preamble", |s| {
        Some(format!(
            "#!/usr/bin/env python\n\"\"\"Module docstring.\"\"\"\n\nfrom __future__ import annotations\n\n{s}"
        ))
    }),
    ("blank-lines", |s| {
        Some(map_lines(s, |line| {
            if line.ends_with('\\') {
                line.to_string()
            } else {
                format!("{line}\n")
            }
        }))
    }),
];

fn map_lines(source: &str, f: impl Fn(&str) -> String) -> String {
    let mut out = source.lines().map(f).collect::<Vec<_>>().join("\n");
    if source.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn map_indentation(source: &str, f: impl Fn(usize) -> String) -> String {
    map_lines(source, |line| {
        let content = line.trim_start_matches(' ');
        let width = line.len() - content.len();
        format!("{}{content}", f(width))
    })
}

fn analyze_code(case: &Case, source: &str, config: &Config, strict: bool) -> Vec<Finding> {
    analyze(Path::new(case.path), source, config, strict)
        .into_iter()
        .filter(|finding| finding.code == case.code)
        .collect()
}

fn describe(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("{}:{} {}", f.line, f.column, f.message))
        .collect::<Vec<_>>()
        .join("; ")
}

fn lines_of(findings: &[Finding]) -> Vec<usize> {
    let mut lines = findings.iter().map(|f| f.line).collect::<Vec<_>>();
    lines.sort_unstable();
    lines
}

fn messages_of(findings: &[Finding]) -> Vec<String> {
    let mut messages = findings
        .iter()
        .map(|f| f.message.clone())
        .collect::<Vec<_>>();
    messages.sort();
    messages
}

/// Whether line `row` (0-based) of `source` ends in a real comment containing
/// `marker`, rather than the marker landing inside a string literal.
fn has_comment_on_row(source: &str, row: usize, marker: &str) -> bool {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(source, None).unwrap();
    let mut comments = Vec::new();
    collect_comments_on_row(tree.root_node(), row, source, &mut comments);
    comments.iter().any(|comment| comment.contains(marker))
}

fn suppression_comments(code: &str) -> Vec<(String, bool)> {
    let name = rule(code).name;
    let other = if code == "STY-001" {
        "API-005"
    } else {
        "STY-001"
    };
    vec![
        ("# noqa".into(), true),
        ("# NOQA".into(), true),
        ("#noqa".into(), true),
        (format!("# noqa: {code}"), true),
        (format!("# noqa:{other},{code}"), true),
        ("# noqa: E501".into(), false),
        (format!("# colint: ignore[{code}]"), true),
        (format!("# colint: ignore[{name}]"), true),
        (
            format!("#colint:ignore[ {} ]", code.to_ascii_lowercase()),
            true,
        ),
        (format!("# colint: ignore[{other}, {name}]"), true),
        (format!("# colint: ignore[{other}]"), false),
    ]
}

fn check_variant(case: &Case, variant: &str, failures: &mut Vec<String>) {
    let source = dedent(variant);
    let config = Config::default();
    let fail = |failures: &mut Vec<String>, what: &str, src: &str, got: &[Finding]| {
        failures.push(format!(
            "[{} expected {}] {what}\n--- source ---\n{src}--- findings: {}\n",
            case.code,
            case.expected,
            describe(got)
        ));
    };

    let baseline = analyze_code(case, &source, &config, false);
    if baseline.len() != case.expected {
        fail(failures, "baseline", &source, &baseline);
        return;
    }

    for (name, transform) in LINE_PRESERVING {
        let Some(transformed) = transform(&source) else {
            continue;
        };
        let got = analyze_code(case, &transformed, &config, false);
        if got.len() != case.expected
            || lines_of(&got) != lines_of(&baseline)
            || messages_of(&got) != messages_of(&baseline)
        {
            fail(failures, &format!("transform {name}"), &transformed, &got);
        }
    }
    for (name, transform) in LINE_SHIFTING {
        let Some(transformed) = transform(&source) else {
            continue;
        };
        let got = analyze_code(case, &transformed, &config, false);
        if got.len() != case.expected || messages_of(&got) != messages_of(&baseline) {
            fail(failures, &format!("transform {name}"), &transformed, &got);
        }
    }

    if case.expected == 0 {
        return;
    }

    let disabled = Config {
        rules: HashMap::from([(case.code.to_string(), false)]),
        ..Config::default()
    };
    let got = analyze_code(case, &source, &disabled, false);
    if !got.is_empty() {
        fail(failures, "disabled in [rules]", &source, &got);
    }
    let got = analyze_code(case, &source, &disabled, true);
    if got.len() != case.expected {
        fail(failures, "--strict re-enables", &source, &got);
    }

    let mut rows = baseline.iter().map(|f| f.line - 1).collect::<Vec<_>>();
    rows.dedup();
    for row in rows {
        for (comment, suppresses) in suppression_comments(case.code) {
            let lines = source.lines().collect::<Vec<_>>();
            if lines[row].ends_with('\\') {
                continue;
            }
            let mut edited = lines.clone();
            let line = format!("{}  {comment}", lines[row]);
            edited[row] = &line;
            let edited = edited.join("\n") + "\n";
            if !has_comment_on_row(&edited, row, comment.trim_start_matches('#').trim()) {
                continue;
            }
            let remaining = analyze_code(case, &edited, &config, false)
                .into_iter()
                .filter(|f| f.line == row + 1)
                .collect::<Vec<_>>();
            if suppresses != remaining.is_empty() {
                fail(
                    failures,
                    &format!(
                        "suppression `{comment}` should {}suppress line {}",
                        if suppresses { "" } else { "not " },
                        row + 1
                    ),
                    &edited,
                    &remaining,
                );
            }
        }
    }
}

fn run(cases: &[Case]) {
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in cases {
        for variant in case.variants {
            check_variant(case, variant, &mut failures);
            checked += 1;
        }
    }
    assert!(checked > 0);
    assert!(
        failures.is_empty(),
        "{} permutation failure(s):\n\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// API-001 predicate-in-function ---------------------------------------------

#[test]
fn api001_permutations() {
    run(&[
        case(
            "API-001",
            "app.py",
            1,
            &[
                r#"
            def run(value):
                if not value:
                    return
                work()
            "#,
                r#"
            def run(value):
                if not value: return
                work()
            "#,
                r#"
            def run(value):
                if value is None:
                    return None
                work()
            "#,
                r#"
            def run(value):
                """Run the value."""
                if not value:
                    return
                work()
            "#,
                r#"
            def run(value):
                """Run the value.

                Longer description.
                """
                # Guard clause.
                if not value:
                    return  # nothing to do
                work()
            "#,
                r#"
            def run(value, other):
                if (
                    not value
                    and other
                ):
                    return
                work()
            "#,
                r#"
            def run(value, other):
                if not value and \
                        other:
                    return
                work()
            "#,
                r#"
            async def run(value):
                if not value:
                    return
                await work()
            "#,
                r#"
            class Runner:
                def run(self, value):
                    if not value:
                        return
                    work()
            "#,
                r#"
            def outer():
                def inner(value):
                    if not value:
                        return
                    work()
                inner(1)
            "#,
                r#"
            def run(  value  ):
                if    not   value   :
                    return
                work( )
            "#,
            ],
        ),
        case(
            "API-001",
            "app.py",
            0,
            &[
                r#"
            def run(value):
                if value.returned:
                    work()
            "#,
                r#"
            def run(value):
                if value == "return":
                    work()
            "#,
                r#"
            def run(value):
                if not value:
                    return
                else:
                    work()
            "#,
                r#"
            def run(value):
                if not value:
                    return
                elif value > 1:
                    work()
            "#,
                r#"
            def run(value):
                if not value:
                    log("skipped")
                    return
                work()
            "#,
                r#"
            def run(value):
                prepare()
                if not value:
                    return
            "#,
                r#"
            def run(value):
                """Return early when needed."""
                work()
            "#,
            ],
        ),
    ]);
}

// API-002 loft-from-caller --------------------------------------------------

#[test]
fn api002_permutations() {
    run(&[
        case(
            "API-002",
            "app.py",
            1,
            &[
                r#"
            def run(thing):
                value = thing.get_value()
                use(value)
            "#,
                r#"
            def run(thing=None):
                use(thing.get_value())
            "#,
                r#"
            def run(thing: Thing = None):
                use(thing.get_value())
            "#,
                r#"
            def run(thing: Thing):
                use(thing.get_value())
            "#,
                r#"
            def run(thing):
                consume(
                    thing.value(),
                )
            "#,
                r#"
            def run( thing ):
                consume( thing . value ( ) )
            "#,
                r#"
            def run(thing):
                result = \
                    thing.value()
                consume(result)
            "#,
                r#"
            def run(
                thing,
                *,
                flag=False,
            ):
                consume(thing.value(), flag)
            "#,
                r#"
            class Runner:
                def run(self, thing):
                    self.consume(thing.value())
            "#,
                r#"
            async def run(thing):
                await consume(thing.value())
            "#,
                r#"
            def run(thing):
                log("thing.value() is logged")  # thing.other()
                use(thing.value())
            "#,
                r#"
            def run(left, right):
                consume(left.value(), right.value())
            "#,
                r#"
            def run(thing):
                consume(key=thing.value())
            "#,
            ],
        ),
        case(
            "API-002",
            "app.py",
            0,
            &[
                r#"
            def run(thing):
                first = thing.first()
                second = thing.second()
                use(first, second)
            "#,
                r#"
            def run(thing):
                use(thing)
            "#,
                r#"
            def run(thing):
                use(thing.value(), thing)
            "#,
                r#"
            def run(thing):
                use(thing.value)
            "#,
                r#"
            def _run(thing):
                use(thing.value())
            "#,
                r#"
            class Window(Base):
                @override
                def event(self, event):
                    self.handle(event.type())
            "#,
                r#"
            def run(*things, **options):
                use(options.get("x"))
            "#,
                r#"
            def run(self):
                self.value()
            "#,
            ],
        ),
    ]);
}

#[test]
fn api002_targets_survive_layout() {
    for source in [
        "def run(thing):\n    consume(\n        thing.value()\n    )\n",
        "def run(thing):\n    value = (\n        thing.value()\n    )\n    consume(\n        value\n    )\n",
        "def run(thing):\n    value = thing.value()\n    consume(key=value)\n",
        "def run(thing):\n    self.consume(thing.value())\n",
    ] {
        let findings = analyze(Path::new("app.py"), source, &Config::default(), false);
        assert_eq!(
            vec!["parameter `thing` is only queried once; loft its queried value to `consume`"],
            findings
                .iter()
                .filter(|f| f.code == "API-002")
                .map(|f| f.message.as_str())
                .collect::<Vec<_>>(),
            "{source}"
        );
    }
}

// API-003 redundant-type-hint -----------------------------------------------

#[test]
fn api003_permutations() {
    run(&[
        case(
            "API-003",
            "app.py",
            1,
            &[
                r#"
            def make() -> list[str]:
                return []

            def run():
                value: list[str] = make()
            "#,
                r#"
            def make()->list[ str ]:
                return []

            def run():
                value :list[str]=make( )
            "#,
                r#"
            def make() -> list[str]:
                return []

            def run():
                value: "list[str]" = make()
            "#,
                r#"
            def make(first, second) -> dict[str, int]:
                return {}

            def run():
                value: dict[
                    str,
                    int,
                ] = make(
                    1,
                    2,
                )
            "#,
                r#"
            def make() -> int:
                return 1

            def run():
                value: int = \
                    make()
            "#,
                r#"
            def make() -> int:
                return 1

            def run():
                value: int = (make())
            "#,
                r#"
            async def make() -> int:
                return 1

            async def run():
                value: int = await make()
            "#,
                r#"
            class Factory:
                def make(self) -> int:
                    return 1

                def run(self):
                    value: int = self.make()
            "#,
            ],
        ),
        case(
            "API-003",
            "app.py",
            0,
            &[
                r#"
            def make() -> list[str]:
                return []

            def run():
                value: list[int] = make()
            "#,
                r#"
            def run():
                value: int = imported()
            "#,
                r#"
            def make() -> int:
                return 1

            def run():
                value = make()
            "#,
                r#"
            def make() -> int:
                return 1

            def run(other):
                value: int = other.make()
            "#,
                r#"
            def make() -> int:
                return 1

            class Factory:
                def run(self):
                    value: int = self.make()
            "#,
                r#"
            def make() -> int:
                return 1

            def run():
                value: int = make() + 1
            "#,
            ],
        ),
    ]);
}

// API-004 empty-string-return -----------------------------------------------

#[test]
fn api004_permutations() {
    run(&[
        case(
            "API-004",
            "app.py",
            1,
            &[
                r#"
            def find() -> str | None:
                return ""
            "#,
                r#"
            def find() -> str | None:
                return ''
            "#,
                r#"
            def find() -> str | None:
                return ("")
            "#,
                r#"
            def find() -> str | None:
                return (
                    ""
                )
            "#,
                r#"
            def find() -> str | None:
                return \
                    ""
            "#,
                r#"
            def find() -> str | None:
                return u""
            "#,
                r#"
            def find() -> str | None:
                return r''
            "#,
                r#"
            def find() -> str | None:
                return """"""
            "#,
                r#"
            def find() -> str | None:
                return "" ''
            "#,
                r#"
            def find(items) -> str | None:
                for item in items:
                    if item:
                        return item
                return ""  # not found
            "#,
                r#"
            class Finder:
                async def find(self) -> str | None:
                    return ""
            "#,
                r#"
            def outer() -> str:
                def inner() -> str:
                    return ""
                return inner()
            "#,
            ],
        ),
        case(
            "API-004",
            "app.py",
            2,
            &[r#"
            def find(value) -> str:
                if value:
                    return ""
                return ''
            "#],
        ),
        case(
            "API-004",
            "app.py",
            0,
            &[
                r#"
            def find() -> str:
                return " "
            "#,
                r#"
            def find() -> bytes:
                return b""
            "#,
                r#"
            def find(value) -> str:
                return f"{value}"
            "#,
                r#"
            def find() -> str | None:
                return None
            "#,
                r#"
            def find(value) -> str:
                return "" + value
            "#,
                r#"
            def find() -> str:
                empty = ""
                return empty.strip() or "x"
            "#,
                r#"
            def find():
                handler = lambda: ""
                return handler
            "#,
            ],
        ),
    ]);
}

#[test]
fn api004_message_depends_on_annotation() {
    let message = |source: &str| {
        analyze(Path::new("app.py"), source, &Config::default(), false)
            .into_iter()
            .find(|f| f.code == "API-004")
            .unwrap()
            .message
    };
    for allows_none in [
        "def f() -> str | None:\n    return ''\n",
        "def f() -> Optional[str]:\n    return ''\n",
        "def f() -> typing.Optional[str]:\n    return ''\n",
        "def f() -> Union[str, None]:\n    return ''\n",
        "def f():\n    return ''\n",
    ] {
        assert_eq!(
            "empty string is returned; use `return None`",
            message(allows_none),
            "{allows_none}"
        );
    }
    assert_eq!(
        "empty string is returned; use `return None` and add `None` to this function's return annotation",
        message("def f() -> str:\n    return ''\n")
    );
}

// API-005 assert-in-production ----------------------------------------------

const ASSERT_VARIANTS: &[&str] = &[
    "assert ready\n",
    "assert ready, \"message\"\n",
    r#"
    assert (
        ready
        and other
    ), "message"
    "#,
    r#"
    assert ready \
        and other
    "#,
    r#"
    def run(value):
        assert value
    "#,
    r#"
    class Runner:
        async def run(self, value):
            for item in value:
                assert   item  ,  "item"
    "#,
];

#[test]
fn api005_permutations() {
    run(&[
        case("API-005", "app.py", 1, ASSERT_VARIANTS),
        case("API-005", "latest.py", 1, ASSERT_VARIANTS),
        case("API-005", "contest/app.py", 1, ASSERT_VARIANTS),
        case("API-005", "src/attestation.py", 1, ASSERT_VARIANTS),
        case("API-005", "pytest_plugin.py", 1, ASSERT_VARIANTS),
        case("API-005", "test_app.py", 0, ASSERT_VARIANTS),
        case("API-005", "app_test.py", 0, ASSERT_VARIANTS),
        case("API-005", "conftest.py", 0, ASSERT_VARIANTS),
        case("API-005", "tests/helpers.py", 0, ASSERT_VARIANTS),
        case("API-005", "project/Tests/helpers.py", 0, ASSERT_VARIANTS),
        case("API-005", "testing/fixtures.py", 0, ASSERT_VARIANTS),
        case(
            "API-005",
            "app.py",
            2,
            &[r#"
            assert first
            assert second
            "#],
        ),
        case(
            "API-005",
            "app.py",
            0,
            &["value = 'assert ready'\n", "# assert ready\n"],
        ),
    ]);
}

// API-006 none-polymorphism -------------------------------------------------

#[test]
fn api006_permutations() {
    run(&[
        case(
            "API-006",
            "app.py",
            1,
            &[
                r#"
            def run(value: Thing | None):
                if value is not None:
                    use(value)
            "#,
                r#"
            def run(value: Thing|None):
                if value is None:
                    use(value)
            "#,
                r#"
            def run(value: None | Thing):
                if value   is   not   None:
                    use(value)
            "#,
                r#"
            def run(value: Optional[Thing]):
                if value is not None:
                    use(value)
            "#,
                r#"
            def run(value: Union[Thing, None]):
                if value is not None:
                    use(value)
            "#,
                r#"
            def run(value: Thing | None = None):
                if None is not value:
                    use(value)
            "#,
                r#"
            def run(value: Thing | None, flag):
                if flag:
                    pass
                elif value is None:
                    use(value)
            "#,
                r#"
            def run(value: Thing | None, flag):
                if flag:
                    if value is None:
                        use(value)
            "#,
                r#"
            def run(value: Thing | None):
                if (
                    value
                    is not None
                ):
                    use(value)
            "#,
                r#"
            def run(value: Thing | None):
                if value \
                        is not None:
                    use(value)
            "#,
                r#"
            def run(
                value: (
                    Thing
                    | None
                ),
            ):
                if value is not None:
                    use(value)
            "#,
                r#"
            def run():
                value: Thing | None = find()
                if value is not None:
                    use(value)
            "#,
                r#"
            class Runner:
                async def run(self, value: Thing | None, flag: bool):
                    if flag and value is not None:
                        use(value)
            "#,
            ],
        ),
        case(
            "API-006",
            "app.py",
            2,
            &[r#"
            def run(value: Thing | None, other: Thing | None):
                if value is None or other is None:
                    use(value)
            "#],
        ),
        case(
            "API-006",
            "app.py",
            0,
            &[
                r#"
            def run(value: Thing):
                if value is not None:
                    use(value)
            "#,
                r#"
            def run(value):
                if value is not None:
                    use(value)
            "#,
                r#"
            def run(value: Thing, other: Thing | None):
                if value is not None:
                    use(other)
            "#,
                r#"
            def run(value: Thing | None):
                if value == None:
                    use(value)
            "#,
                r#"
            def run(value: Thing | None):
                while value is not None:
                    value = value.parent
            "#,
                r#"
            def run(value: Thing | None):
                if value:
                    use(value is None)
            "#,
                r#"
            value: Thing | None = None
            if value is None:
                use(value)
            "#,
            ],
        ),
    ]);
}

// DOC-001 docstring-convention ----------------------------------------------

#[test]
fn doc001_permutations() {
    run(&[
        case(
            "DOC-001",
            "app.py",
            1,
            &[
                r#"
            def create(task):
                """Create *task*."""
            "#,
                r#"
            def create(task):
                """Create **task**."""
            "#,
                r#"
            def create(task):
                r"""Create *task*."""
            "#,
                r#"
            def create(task):
                '''Create *task*.'''
            "#,
                r#"
            def create(task=None):
                """Create *task*."""
            "#,
                r#"
            def create(task: Task | None = None):
                """Create *task*."""
            "#,
                r#"
            def create(*items):
                """Create every one of *items*."""
            "#,
                r#"
            def create(x):
                """Create *x*."""
            "#,
                r#"
            def create(task):
                """Create "task"."""
            "#,
                r#"
            def create(task):
                """Create 'task'."""
            "#,
                r#"
            def create(task):
                """Create ``task``."""
            "#,
                r#"
            def create(task):
                """Create “task”."""
            "#,
                r#"
            def create(task):
                """Create a task.

                Args:
                    task: The *task* to create.
                """
            "#,
                r#"
            class Factory:
                async def create(self, task):
                    """Create *task*."""
            "#,
            ],
        ),
        case(
            "DOC-001",
            "app.py",
            0,
            &[
                r#"
            def create(task):
                """Create `task`."""
            "#,
                r#"
            def create(task):
                """Create *other*."""
            "#,
                r#"
            def create(task):
                """Create *tasks*."""
            "#,
                r#"
            def create(x):
                """Compute 2*x*3."""
            "#,
                r#"
            def create(task):
                # Create *task*.
                pass
            "#,
                r#"
            def create(task):
                run()
                """Create *task*."""
            "#,
                r#"
            def create(task):
                """Create the task.

                Args:
                    task: Create task, foo_task, "task list", or *task.
                """
            "#,
                r#"
            def create(mode):
                """Open with 'r' or "w" or *m*."""
            "#,
                r#"
            def create(x):
                """Create 'x'."""
            "#,
                r#"
            def create(task):
                """Create a task.

                Example:
                    >>> create("task")

                ```python
                create("task")
                ```
                """
            "#,
                r#"
            def create(task):
                f"""Create *task*."""
            "#,
            ],
        ),
        case(
            "DOC-001",
            "app.py",
            2,
            &[
                r#"
            def create(
                task,
                owner,
            ):
                """Create a task.

                The new item is assigned to *owner*
                and *task* is scheduled.
                """
            "#,
                r#"
            def create(task: int) -> None:
                """Get *task* and print it.

                Args:
                    task: Something "task" and task.
                """
            "#,
            ],
        ),
    ]);
}

#[test]
fn doc001_respects_configured_markup() {
    let config: Config =
        toml::from_str("[docstring_variable_markup]\nstart = \"``\"\nend = \"``\"\n").unwrap();
    let findings = |source: &str| {
        analyze(Path::new("app.py"), source, &config, false)
            .into_iter()
            .filter(|f| f.code == "DOC-001")
            .count()
    };
    assert_eq!(
        0,
        findings("def create(task):\n    \"\"\"Create ``task``.\"\"\"\n")
    );
    assert_eq!(
        1,
        findings("def create(task):\n    \"\"\"Create `task`.\"\"\"\n")
    );
}

// DOC-002 direct-raises-only ------------------------------------------------

#[test]
fn doc002_permutations() {
    run(&[
        case(
            "DOC-002",
            "app.py",
            1,
            &[
                r#"
            def run():
                """Run.

                Raises:
                    ValueError: When broken.
                """
                dependency()
            "#,
                r#"
            def outer():
                """Outer.

                Raises:
                    ValueError: Never directly raised.
                """
                def inner():
                    raise ValueError()
                inner()
            "#,
                r#"
            def outer():
                """Outer.

                Raises:
                    ValueError: Never directly raised.
                """
                class Inner:
                    def run(self):
                        raise ValueError()
                Inner().run()
            "#,
                r#"
            def run(value):
                """Run.

                Raises:
                    ValueError: When the value is bad.
                    KeyError: When the key is missing,
                        which only a dependency reports.
                """
                if not value:
                    raise ValueError(
                        "bad value"
                    )
                dependency(value)
            "#,
                r#"
            def run():
                r'''Run.

                Raises:

                    errors.ValueError: When broken.

                '''
                try:
                    dependency()
                except KeyError:
                    raise
            "#,
                r#"
            class Runner:
                async def run(self):
                    """Run.

                    Raises:
                        ValueError: When broken.
                    """
                    await dependency()
            "#,
                r#"
            def collapse(path):
                """Collapse a path.

                Raises: IndexError if too many '..' occur within the path.
                """
                return dependency(path)
            "#,
            ],
        ),
        case(
            "DOC-002",
            "app.py",
            0,
            &[
                r#"
            def collapse(parts):
                """Collapse a path.

                Raises: IndexError if too many '..' occur within the path.
                """
                if not parts:
                    raise IndexError("empty")
            "#,
                r#"
            def run():
                """Run.

                Raises:
                    ValueError: When broken.
                """
                raise ValueError()
            "#,
                r#"
            def run():
                """Run.

                Raises:
                    ValueError: When broken.
                """
                raise ValueError
            "#,
                r#"
            def run():
                """Run.

                Raises:
                    errors.ValueError: When broken.
                """
                raise errors.ValueError(
                    "broken"
                ) from None
            "#,
                r#"
            def run():
                """Run.

                Raises:
                    ValueError: When broken.
                """
                try:
                    dependency()
                except (KeyError, ValueError):
                    raise
            "#,
                r#"
            def run():
                """Run.

                Raises:
                    ValueError: When broken.
                """
                try:
                    dependency()
                except ValueError as error:
                    log(error)
                    raise error
            "#,
                r#"
            def run(error):
                """Run.

                Raises:
                    ValueError: When broken.
                """
                raise error
            "#,
                r#"
            class BrokenError(ValueError):
                pass

            def run():
                """Run.

                Raises:
                    ValueError: When broken.
                """
                raise BrokenError()
            "#,
                r#"
            def outer():
                """Outer."""
                def inner():
                    """Inner.

                    Raises:
                        ValueError: When broken.
                    """
                    raise ValueError()
                inner()
            "#,
                r#"
            def run():
                """Run.

                See the Raises: section of the dependency.
                """
                dependency()
            "#,
                r#"
            def run():
                # Raises:
                #     ValueError: When broken.
                dependency()
            "#,
            ],
        ),
    ]);
}

#[test]
fn doc002_names_indirect_exceptions() {
    let source = "def run():\n    \"\"\"Run.\n\n    Raises:\n        ValueError: Bad.\n        KeyError: Missing.\n        OSError: Disk.\n    \"\"\"\n    raise ValueError()\n";
    let findings = analyze(Path::new("app.py"), source, &Config::default(), false);
    assert_eq!(
        vec!["docstring documents `KeyError` and `OSError` in Raises but this function does not raise them directly"],
        findings
            .iter()
            .filter(|f| f.code == "DOC-002")
            .map(|f| f.message.as_str())
            .collect::<Vec<_>>()
    );
}

// STY-001 nested-import -----------------------------------------------------

#[test]
fn sty001_permutations() {
    run(&[
        case(
            "STY-001",
            "app.py",
            1,
            &[
                r#"
            def run():
                import os
            "#,
                r#"
            def run():
                from os import path
            "#,
                r#"
            def run():
                from os import (
                    path,
                    sep,
                )
            "#,
                r#"
            def run():
                from os import path, \
                    sep
            "#,
                r#"
            def run():
                import   os.path   as   osp
            "#,
                r#"
            class Loader:
                import json
            "#,
                r#"
            class Loader:
                async def load(self):
                    if ready():
                        import json
            "#,
                r#"
            def run():
                # This does not denote anything.
                import os
            "#,
                r#"
            def run():
                # Notes are not NOTE comments.
                import os
            "#,
                r#"
            def run():
                # NOTE: os is optional.
                import os
                prepare()
                import sys
            "#,
                r#"
            def run():
                value = 1  # NOTE: not a standalone comment
                import os
            "#,
            ],
        ),
        case(
            "STY-001",
            "app.py",
            2,
            &[
                r#"
            def run():
                import os
                import sys
            "#,
                r#"
            def run():
                import os

                # Unrelated comment.
                from sys import (
                    argv,
                )
            "#,
            ],
        ),
        case(
            "STY-001",
            "app.py",
            0,
            &[
                r#"
            def run():
                # NOTE: optional dependency.
                import os
            "#,
                r#"
            def run():
                # note: optional dependency.
                import os
            "#,
                r#"
            def run():
                #NOTE: optional dependency.
                import os
            "#,
                r#"
            def run():
                # NOTE: platform-dependent imports
                # stay local to avoid startup cost.
                import os
                import sys

                from json import (
                    dumps,
                    loads,
                )
                from pathlib import Path, \
                    PurePath
            "#,
                r#"
            def run():
                # NOTE: optional dependency.

                import os
            "#,
                r#"
            def run():
                import os  # colint: ignore[STY-001]
            "#,
                r#"
            def run():
                # NOTE: grouped.
                import os
                # A plain comment inside the group.
                import sys
            "#,
                r#"
            def run():
                prepare()
                # NOTE: the second import is justified too.
                import sys
            "#,
                r#"
            import os
            from json import (
                dumps,
            )
            "#,
                r#"
            from typing import TYPE_CHECKING

            if TYPE_CHECKING:
                from os import PathLike
            "#,
                r#"
            try:
                import yaml
            except ImportError:
                yaml = None
            "#,
                r#"
            def run():
                return "import os"
            "#,
            ],
        ),
    ]);
}

// STY-002 pytest-expected-left ----------------------------------------------

const PYTEST_BAD: &[&str] = &[
    r#"
    def test_value():
        assert actual == ["expected"]
    "#,
    r#"
    def test_value():
        assert actual == 1, "message"
    "#,
    r#"
    def test_value():
        assert compute(left, right) == 3
    "#,
    r#"
    def test_value():
        assert result.value == "x"
    "#,
    r#"
    def test_value():
        assert actual == -1.5
    "#,
    r#"
    def test_value():
        assert   actual==True
    "#,
    r#"
    def test_value():
        assert (actual == 1)
    "#,
    r#"
    def test_value():
        assert actual == {
            "key": 1,
        }
    "#,
    r#"
    def test_value():
        assert actual \
            == 1
    "#,
    r#"
    class TestValue:
        async def test_value(self):
            assert self.actual == ("a", "b")
    "#,
];

#[test]
fn sty002_permutations() {
    run(&[
        case("STY-002", "test_app.py", 1, PYTEST_BAD),
        case("STY-002", "tests/helpers.py", 1, PYTEST_BAD),
        case("STY-002", "app_test.py", 1, PYTEST_BAD),
        case("STY-002", "app.py", 0, PYTEST_BAD),
        case("STY-002", "latest.py", 0, PYTEST_BAD),
        case(
            "STY-002",
            "test_app.py",
            0,
            &[
                r#"
            def test_value():
                assert ["expected"] == actual
            "#,
                r#"
            def test_value():
                assert expected == actual
            "#,
                r#"
            def test_value():
                assert actual != 1
            "#,
                r#"
            def test_value():
                assert actual is None
            "#,
                r#"
            def test_value():
                assert left == right == 1
            "#,
                r#"
            def test_value():
                assert 1 == 1
            "#,
                r#"
            def test_value():
                assert actual
            "#,
                r#"
            def test_value():
                assert "==" in actual
            "#,
            ],
        ),
    ]);
}

// STY-003 logging-dynamic-quote ---------------------------------------------

#[test]
fn sty003_permutations() {
    run(&[
        case(
            "STY-003",
            "app.py",
            1,
            &[
                "logger.error('Could not load %s', name)\n",
                "logging.info(\"Could not load %s\", name)\n",
                "logger.error(\"%s could not load\", name)\n",
                "self.logger.warning(\"Could not load %s\", name)\n",
                "self._log.debug(\"Could not load %s\", name)\n",
                "LOGGER.info(\"Could not load %s\", name)\n",
                "_logger.info(\"Could not load %s\", name)\n",
                "logging.getLogger(__name__).info(\"Could not load %s\", name)\n",
                "logger.log(logging.INFO, \"Could not load %s\", name)\n",
                "logger.info(\"Could not load %(name)s\", {\"name\": name})\n",
                "logger.info(\"Could not load %-10s\", name)\n",
                "logger.info(\"Could not load \\\"%s\\\" from %s\", name, path)\n",
                "logger.info(\"Could not load '%s\\\"\", name)\n",
                "logger  .  info  (  'Could not load %s'  ,  name  )\n",
                r#"
            logger.error(
                "Could not load %s "
                "from the cache",
                name,
            )
            "#,
                r#"
            logger.error("Could not load %s", \
                name)
            "#,
                r#"
            logger.error(
                """Could not load
                %s""",
                name,
            )
            "#,
                "notify(logger.info(\"Could not load %s\", name))\n",
                r#"
            class Loader:
                async def load(self, name):
                    self.log.exception("Could not load %s", name)
            "#,
            ],
        ),
        case(
            "STY-003",
            "app.py",
            0,
            &[
                "logger.error('Could not load \"%s\"', name)\n",
                "logger.error(\"Could not load '%s'\", name)\n",
                "logger.error(\"Could not load \\\"%s\\\"\", name)\n",
                "logger.error(\"Could not load %r\", name)\n",
                "logger.error(\"Loaded %d items\", count)\n",
                "logger.error(\"Loaded 100%%s\")\n",
                "logger.error(f\"Could not load {name}\")\n",
                "logger.error(message, name)\n",
                "logger.error(\"Could not load\")\n",
                "catalog.info(\"Could not load %s\", name)\n",
                "dialog.warning(\"Could not load %s\", name)\n",
                "print(\"Could not load %s\" % name)\n",
                "logger.info(\"Could not load %(name)r\", {\"name\": name})\n",
                "logger.info(\"%s\".join(names))\n",
                r#"
            logger.error(
                "Could not load '%s' "
                "from \"%s\"",
                name,
                path,
            )
            "#,
            ],
        ),
    ]);
}

// STY-004 module-order ------------------------------------------------------

#[test]
fn sty004_permutations() {
    run(&[
        case(
            "STY-004",
            "app.py",
            1,
            &[
                r#"
            def make():
                pass

            VALUE = 1
            "#,
                r#"
            class Thing:
                pass

            import os
            "#,
                r#"
            def make():
                pass

            class Thing:
                pass
            "#,
                r#"
            def make():
                pass

            @dataclass(
                frozen=True,
            )
            class Thing:
                pass
            "#,
                r#"
            @decorator
            def make():
                pass

            VALUE = 1
            "#,
                r#"
            def make():
                pass

            VALUE = [
                1,
                2,
            ]
            "#,
                r#"
            def make():
                pass

            VALUE = 1 + \
                2
            "#,
                r#"
            async def make():
                pass

            register(make)
            "#,
            ],
        ),
        case(
            "STY-004",
            "app.py",
            2,
            &[r#"
            def make():
                pass

            class Thing:
                pass

            VALUE = 1
            "#],
        ),
        case(
            "STY-004",
            "app.py",
            0,
            &[
                r#"
            """Module docstring."""

            import os

            VALUE = 1

            @dataclass
            class Thing:
                pass

            class Other:
                pass

            @decorator
            def make():
                pass

            def other():
                pass
            "#,
                r#"
            def main():
                pass

            if __name__ == "__main__":
                main()
            "#,
                r#"
            def main():
                pass

            if __name__ == '__main__':
                main()
            "#,
                r#"
            def main():
                pass

            if "__main__"   ==   __name__:
                main()
            "#,
                r#"
            def main():
                pass

            # Trailing comment.
            "#,
            ],
        ),
    ]);
}

// GUI-001 widget-tooltip ----------------------------------------------------

#[test]
fn gui001_permutations() {
    run(&[
        case(
            "GUI-001",
            "app.py",
            1,
            &[
                r#"
            def build():
                button = QPushButton()
            "#,
                r#"
            def build(self):
                self.button = QPushButton()
            "#,
                r#"
            def build():
                button = QPushButton()
                button.setToolTip("")
            "#,
                r#"
            def build(text):
                button = QPushButton()
                button.setToolTip(text or "")
            "#,
                r#"
            def build():
                button: QPushButton = QPushButton()
            "#,
                r#"
            def build(parent):
                button = QtWidgets.QPushButton(
                    "Open",
                    parent,
                )
            "#,
                r#"
            def build():
                button = \
                    QPushButton()
            "#,
                r#"
            def build():
                button = QPushButton(toolTip="")
            "#,
                r#"
            def build():
                button = QPushButton()
                other_button.setToolTip("Other")
            "#,
                r#"
            def build():
                def inner():
                    button = QPushButton()
                inner()
            "#,
                r#"
            button = QPushButton()
            "#,
                r#"
            class Window:
                def __init__(self):
                    self.button = QPushButton()

                def _unused(self):
                    self.button.setToolTip("Open")
            "#,
                r#"
            class Window:
                def __init__(self):
                    pass

                def build_menu(self):
                    self.menu_button = QToolButton()
            "#,
                r#"
            class CustomWidget(QtWidgets.QWidget):
                def __init__(self):
                    self.label = QtWidgets.QLabel()
                    self.label.setToolTip("Label")

            widget = CustomWidget()
            "#,
            ],
        ),
        case(
            "GUI-001",
            "app.py",
            2,
            &[r#"
            def build():
                first = QPushButton()
                second = QLabel()
            "#],
        ),
        case(
            "GUI-001",
            "app.py",
            0,
            &[
                r#"
            def build():
                button = QPushButton()
                button.setToolTip("Open")
            "#,
                r#"
            def build():
                button = QPushButton()
                button . setToolTip ( "Open" )
            "#,
                r#"
            def build():
                button = QPushButton()
                button.setToolTip(
                    "Open the selected item"
                )
            "#,
                r#"
            def build(self):
                self.button = QPushButton()
                self.button.setToolTip("Open")
            "#,
                r#"
            def build():
                button: QPushButton = QPushButton()
                button.setToolTip("Open")
            "#,
                r#"
            def build():
                button = QPushButton(toolTip="Open")
            "#,
                r#"
            def build():
                # No tooltip: the icon is self-explanatory.
                button = QPushButton()
            "#,
                r#"
            def build(layout):
                layout.addWidget(QPushButton())
                first, second = QPushButton(), QPushButton()
            "#,
                r#"
            class Window:
                def __init__(self):
                    self.button = QPushButton()
                    self._configure()

                def _configure(self):
                    self._set_tooltip()

                def _set_tooltip(self):
                    self._configure()
                    self.button.setToolTip("Open")
            "#,
                r#"
            class Window:
                def __init__(self):
                    self._build()
                    self.button.setToolTip("Open")

                def _build(self):
                    self.button = QPushButton()
            "#,
                r#"
            class Window(QtWidgets.QWidget):
                def __init__(self):
                    super().__init__()
            "#,
                r#"
            widget = CustomClass()
            widget.setToolTip("Tip")
            "#,
            ],
        ),
    ]);
}

#[test]
fn gui001_fixture_permutations() {
    run(&[case(
        "GUI-001",
        "tip.py",
        0,
        &[include_str!("../tests/fixtures/tip.txt")],
    )]);
}

// GUI-002 qt-model-parent ---------------------------------------------------

#[test]
fn gui002_permutations() {
    run(&[
        case(
            "GUI-002",
            "app.py",
            1,
            &[
                "model = QStandardItemModel()\n",
                "model = QStandardItemModel( )\n",
                "model = QStandardItemModel(parent=None)\n",
                "model = QStandardItemModel(parent = None)\n",
                "model = QStandardItemModel(0, 3)\n",
                "model = QtGui.QStandardItemModel()\n",
                "model = QStringListModel([\"a\", \"b\"])\n",
                r#"
            model = QStandardItemModel(
                0,
                3,
            )
            "#,
                r#"
            proxy = \
                QSortFilterProxyModel()
            "#,
                "view.setModel(QSortFilterProxyModel())\n",
                "use(QIdentityProxyModel(), other())\n",
                r#"
            class Model(QStandardItemModel):
                pass
            "#,
                r#"
            class Model(QtGui.QStandardItemModel):
                def __init__(self):
                    super().__init__()
            "#,
                r#"
            class Model(QAbstractListModel):
                def __init__(self, parent=None):
                    super().__init__()
                    self.parent_name = parent
            "#,
                r#"
            class Model(
                QAbstractTableModel,
            ):
                def __init__(self, rows):
                    super().__init__(rows)
            "#,
            ],
        ),
        case(
            "GUI-002",
            "app.py",
            0,
            &[
                "model = QStandardItemModel(self)\n",
                "model = QStandardItemModel(parent=self)\n",
                "model = QStandardItemModel(0, 3, self)\n",
                "model = QStandardItemModel(parent)\n",
                "model = QSortFilterProxyModel(parent=self.view)\n",
                "model = QStandardItemModel(*args)\n",
                r#"
            model = QStandardItemModel(
                parent=self,
            )
            "#,
                "model = CustomModel()\n",
                "model = QStandardItem()\n",
                r#"
            class Model(QStandardItemModel):
                def __init__(self, parent=None):
                    super().__init__(parent)
            "#,
                r#"
            class Model(QStandardItemModel):
                def __init__(self, parent: QObject | None = None) -> None:
                    super().__init__(parent=parent)
            "#,
                r#"
            class Model(QStandardItemModel):
                def __init__(self, parent=None):
                    super().__init__(
                        parent,
                    )
            "#,
                r#"
            class Model(QStandardItemModel):
                def __init__(self, parent=None):
                    super(Model, self).__init__(parent)
            "#,
                r#"
            class Model(QStandardItemModel):
                def __init__(self, parent=None):
                    QStandardItemModel.__init__(self, parent)
            "#,
                r#"
            class Model(QSortFilterProxyModel):
                def __init__(self, *args, **kwargs):
                    super().__init__(*args, **kwargs)
            "#,
                r#"
            class Model(QSortFilterProxyModel):
                def __init__(self, rows, *, parent):
                    super().__init__(parent)
                    self.rows = rows
            "#,
            ],
        ),
    ]);
}
