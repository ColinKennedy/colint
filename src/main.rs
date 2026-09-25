#![allow(clippy::too_many_arguments)]

use clap::Parser;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tree_sitter::{Node, Parser as TsParser};
use walkdir::{DirEntry, WalkDir};

const RULES: &[Rule] = &[
    Rule::new("COL-001", "predicate-in-function", Severity::Warning, "Move the predicate to the caller so this function always performs work."),
    Rule::new("COL-002", "loft-from-caller", Severity::Warning, "Pass the queried value instead of the complex object when the object is not otherwise used."),
    Rule::new("COL-003", "nested-import", Severity::Error, "Move the import to module scope, or add an immediately preceding NOTE comment explaining why it is nested."),
    Rule::new("COL-004", "docstring-convention", Severity::Warning, "Use the configured docstring markup for parameters mentioned in the summary."),
    Rule::new("COL-005", "redundant-type-hint", Severity::Error, "Remove the redundant local annotation; the locally defined callee already provides its return type."),
    Rule::new("COL-006", "pytest-expected-left", Severity::Warning, "Put the expected value on the left side of a pytest equality assertion."),
    Rule::new("COL-007", "logging-dynamic-quote", Severity::Warning, "Wrap a logging placeholder in quotes, for example `message \"%s\"`."),
    Rule::new("COL-008", "module-order", Severity::Warning, "Keep module declarations ordered as globals, classes, then functions."),
    Rule::new("COL-009", "empty-string-return", Severity::Error, "Return None for a not-found result and include None in the return annotation."),
    Rule::new("COL-010", "assert-in-production", Severity::Error, "Raise an explicit exception instead of assert in non-test code."),
    Rule::new("COL-011", "none-polymorphism", Severity::Warning, "Prefer a polymorphic truthiness check (`if value:`) for a Foo | None value."),
    Rule::new("COL-012", "direct-raises-only", Severity::Error, "Document Raises only for exceptions raised directly by this function."),
    Rule::new("COL-013", "widget-tooltip", Severity::Warning, "Set a non-empty static tooltip, or add a comment explaining why no tooltip is appropriate."),
    Rule::new("COL-014", "qt-model-parent", Severity::Error, "Construct Qt models and proxies with a parent; subclasses should forward parent to super().__init__()."),
];

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Warning,
    Error,
}
#[derive(Clone, Copy)]
struct Rule {
    code: &'static str,
    name: &'static str,
    severity: Severity,
    recommendation: &'static str,
}
impl Rule {
    const fn new(
        code: &'static str,
        name: &'static str,
        severity: Severity,
        recommendation: &'static str,
    ) -> Self {
        Self {
            code,
            name,
            severity,
            recommendation,
        }
    }
}
fn rule(code: &str) -> &'static Rule {
    RULES
        .iter()
        .find(|r| r.code == code)
        .expect("registered rule")
}

#[derive(Parser)]
#[command(version, about = "Opinionated Python static analysis")]
struct Cli {
    /// Files or directories to analyze (defaults to current directory)
    #[arg(default_value = ".")]
    paths: Vec<PathBuf>,
    /// Enable every rule and treat warnings as errors
    #[arg(long)]
    strict: bool,
    /// Emit machine-readable diagnostics
    #[arg(long)]
    json: bool,
    /// Prepend guidance for reading and suppressing diagnostics
    #[arg(long)]
    include_header: bool,
    /// Use this configuration file instead of discovering .colint.toml
    #[arg(long)]
    config: Option<PathBuf>,
}
#[derive(serde::Deserialize)]
struct Config {
    #[serde(default)]
    warnings_as_errors: bool,
    #[serde(default)]
    rules: HashMap<String, bool>,
    /// The markup used when referring to parameters in docstrings.
    #[serde(default = "default_docstring_convention")]
    docstring_convention: String,
    /// Extra package roots used when resolving imported base classes. Paths in
    /// `.colint.toml` are relative to that file unless already absolute.
    #[serde(default)]
    import_paths: Vec<PathBuf>,
}
fn default_docstring_convention() -> String {
    "mkdocs".into()
}
impl Default for Config {
    fn default() -> Self {
        Self {
            warnings_as_errors: false,
            rules: HashMap::new(),
            docstring_convention: default_docstring_convention(),
            import_paths: Vec::new(),
        }
    }
}
#[derive(Serialize)]
struct Finding {
    path: String,
    line: usize,
    column: usize,
    code: String,
    name: String,
    severity: Severity,
    message: String,
    recommendation: String,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = read_config(cli.config.as_deref());
    let files = python_files(&cli.paths);
    let sources: Vec<_> = files
        .into_iter()
        .filter_map(|path| match fs::read_to_string(&path) {
            Ok(source) => Some((path, source)),
            Err(e) => {
                eprintln!("colint: cannot read {}: {e}", path.display());
                None
            }
        })
        .collect();
    let mut discovery_sources = sources.clone();
    discovery_sources.extend(import_path_sources(&config));
    let qt_model_classes = qt_model_classes(&discovery_sources);
    let mut findings = Vec::new();
    for (path, source) in sources {
        findings.extend(analyze_with_qt_model_classes(
            &path,
            &source,
            &config,
            cli.strict,
            &qt_model_classes,
        ));
    }
    findings.sort_by(|a, b| {
        (&a.path, a.line, a.column, &a.code).cmp(&(&b.path, b.line, b.column, &b.code))
    });
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&findings).unwrap());
    } else {
        print!("{}", format_human_output(&findings, cli.include_header));
    }
    ExitCode::from(exit_status(
        &findings,
        config.warnings_as_errors || cli.strict,
    ))
}

fn format_human_finding(finding: &Finding) -> String {
    format!(
        "{}:{}:{}: {} - {} [{}]\n  recommendation: {}",
        finding.path,
        finding.line,
        finding.column,
        finding.code,
        finding.message,
        match finding.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        },
        finding.recommendation
    )
}

const HUMAN_OUTPUT_HEADER: &str = "colint diagnostics\n\
Suppress a finding on its line with `# noqa`, or selected codes with\n\
`# colint: ignore[COL-003,COL-010]`. For COL-003 nested imports, use\n\
`# NOTE: reason` immediately above one import or an import group; blank lines\n\
do not end that group.\n\n";

fn format_human_output(findings: &[Finding], include_header: bool) -> String {
    let mut output = String::new();
    if include_header {
        output.push_str(HUMAN_OUTPUT_HEADER);
    }
    for finding in findings {
        output.push_str(&format_human_finding(finding));
        output.push('\n');
    }
    output
}

fn exit_status(findings: &[Finding], warnings_as_errors: bool) -> u8 {
    if findings
        .iter()
        .any(|finding| finding.severity == Severity::Error)
    {
        2
    } else if warnings_as_errors
        && findings
            .iter()
            .any(|finding| finding.severity == Severity::Warning)
    {
        1
    } else {
        0
    }
}

fn read_config(explicit: Option<&Path>) -> Config {
    let path = explicit.map(PathBuf::from).or_else(|| {
        let mut d = std::env::current_dir().ok()?;
        loop {
            let p = d.join(".colint.toml");
            if p.is_file() {
                return Some(p);
            }
            if !d.pop() {
                return None;
            }
        }
    });
    let Some(path) = path else {
        return Config::default();
    };
    let mut config: Config = fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default();
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    config.import_paths = config
        .import_paths
        .into_iter()
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        })
        .collect();
    config
}

fn import_path_sources(config: &Config) -> Vec<(PathBuf, String)> {
    let mut roots = config.import_paths.clone();
    if let Some(paths) = env::var_os("PYTHONPATH") {
        roots.extend(env::split_paths(&paths));
    }
    python_files(&roots)
        .into_iter()
        .filter_map(|path| fs::read_to_string(&path).ok().map(|source| (path, source)))
        .collect()
}
fn ignored(entry: &DirEntry) -> bool {
    entry.file_type().is_dir()
        && matches!(
            entry.file_name().to_string_lossy().as_ref(),
            ".git"
                | ".venv"
                | "venv"
                | "env"
                | "__pycache__"
                | "build"
                | "dist"
                | "node_modules"
                | ".tox"
                | ".mypy_cache"
                | ".pytest_cache"
        )
}
fn python_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .flat_map(|p| {
            if p.is_file() {
                vec![p.clone()]
            } else {
                WalkDir::new(p)
                    .into_iter()
                    .filter_entry(|e| !ignored(e))
                    .filter_map(Result::ok)
                    .filter(|e| {
                        e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "py")
                    })
                    .map(|e| e.into_path())
                    .collect()
            }
        })
        .collect()
}

#[cfg(test)]
fn analyze(path: &Path, src: &str, config: &Config, strict: bool) -> Vec<Finding> {
    analyze_with_qt_model_classes(path, src, config, strict, &HashSet::new())
}

fn analyze_with_qt_model_classes(
    path: &Path,
    src: &str,
    config: &Config,
    strict: bool,
    qt_model_classes: &HashSet<String>,
) -> Vec<Finding> {
    let mut parser = TsParser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .unwrap();
    let tree = match parser.parse(src, None) {
        Some(t) => t,
        None => return vec![],
    };
    let mut out = Vec::new();
    let root = tree.root_node();
    let defined_returns = local_return_types(root, src);
    walk(root, &mut |n| match n.kind() {
        "import_statement" | "import_from_statement"
            if n.parent().is_some_and(|p| p.kind() != "module") =>
        {
            if !nested_import_has_note(n, src) {
                add(
                    &mut out,
                    path,
                    src,
                    n,
                    "COL-003",
                    "nested import requires a preceding NOTE comment",
                    config,
                    strict,
                );
            }
        }
        "assert_statement" if !path.to_string_lossy().contains("test") => add(
            &mut out,
            path,
            src,
            n,
            "COL-010",
            "assert is used outside test code",
            config,
            strict,
        ),
        "assignment" => check_assignment(&mut out, path, src, n, &defined_returns, config, strict),
        "call" => {
            check_call(&mut out, path, src, n, config, strict);
        }
        "function_definition" => {
            check_function(&mut out, path, src, n, config, strict);
            if !is_inside_class(n) {
                check_widget_tooltips(&mut out, path, src, n, config, strict);
            }
        }
        "class_definition" => {
            check_qt_model_subclass(&mut out, path, src, n, qt_model_classes, config, strict);
            check_class_widget_tooltips(&mut out, path, src, n, config, strict);
        }
        "assert_statement" => check_pytest_assertion(&mut out, path, src, n, config, strict),
        "if_statement" => check_none_comparison(&mut out, path, src, n, config, strict),
        _ => {}
    });
    check_module_order(&mut out, path, src, root, config, strict);
    out
}
fn walk(node: Node, f: &mut impl FnMut(Node)) {
    f(node);
    let mut c = node.walk();
    for child in node.children(&mut c) {
        walk(child, f);
    }
}
fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}
fn is_inside_class(node: Node) -> bool {
    let mut parent = node.parent();
    while let Some(current) = parent {
        if current.kind() == "class_definition" {
            return true;
        }
        parent = current.parent();
    }
    false
}
fn enabled(code: &str, cfg: &Config, strict: bool) -> bool {
    strict || cfg.rules.get(code).copied().unwrap_or(true)
}
fn suppressed(node: Node, src: &str, code: &str) -> bool {
    let line = src[..node.start_byte()].rfind('\n').map_or(0, |i| i + 1);
    let end = src[line..].find('\n').map_or(src.len(), |i| line + i);
    let line = &src[line..end];
    if line.contains("# noqa") {
        return true;
    }
    line.find("# colint: ignore[")
        .and_then(|i| line[i..].split_once('['))
        .and_then(|(_, rest)| rest.split_once(']'))
        .is_some_and(|(codes, _)| codes.split(',').any(|c| c.trim() == code))
}
fn add(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    node: Node,
    code: &str,
    message: &str,
    cfg: &Config,
    strict: bool,
) {
    if !enabled(code, cfg, strict) || suppressed(node, src, code) {
        return;
    }
    let r = rule(code);
    let p = node.start_position();
    out.push(Finding {
        path: path.display().to_string(),
        line: p.row + 1,
        column: p.column + 1,
        code: code.into(),
        name: r.name.into(),
        severity: r.severity,
        message: message.into(),
        recommendation: r.recommendation.into(),
    });
}

fn nested_import_has_note(n: Node, src: &str) -> bool {
    let before = &src[..n.start_byte()];
    let mut meaningful = before.lines().rev().filter(|l| !l.trim().is_empty());
    match meaningful.next() {
        Some(line) if line.trim_start().starts_with('#') => {
            let mut comments = vec![line];
            for l in meaningful {
                if l.trim_start().starts_with('#') {
                    comments.push(l);
                } else {
                    break;
                }
            }
            comments
                .iter()
                .any(|l| l.to_ascii_lowercase().contains("note"))
        }
        _ => false,
    }
}
fn local_return_types(root: Node, src: &str) -> HashMap<String, String> {
    let mut x = HashMap::new();
    walk(root, &mut |n| {
        if n.kind() == "function_definition" {
            if let (Some(name), Some(ret)) = (
                n.child_by_field_name("name"),
                n.child_by_field_name("return_type"),
            ) {
                x.insert(text(name, src).to_string(), text(ret, src).to_string());
            }
        }
    });
    x
}
fn check_assignment(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    n: Node,
    returns: &HashMap<String, String>,
    cfg: &Config,
    strict: bool,
) {
    let t = text(n, src);
    if let Some((left, right)) = t.split_once('=') {
        if left.contains(':') {
            let annotation = left.split(':').nth(1).unwrap_or("").trim();
            let rhs = right.trim();
            let callee = rhs.split('(').next().unwrap_or("").trim();
            if returns.get(callee).is_some_and(|r| r.trim() == annotation) {
                add(
                    out,
                    path,
                    src,
                    n,
                    "COL-005",
                    "local annotation duplicates the direct callee return type",
                    cfg,
                    strict,
                );
            }
        }
    }
}
fn check_call(out: &mut Vec<Finding>, path: &Path, src: &str, n: Node, cfg: &Config, strict: bool) {
    let t = text(n, src);
    if (t.contains("logging.") || t.contains("logger."))
        && (t.contains("%s") || t.contains("%r"))
        && !t.contains("\"%s\"")
        && !t.contains("'%s'")
        && !t.contains("\"%r\"")
        && !t.contains("'%r'")
    {
        add(
            out,
            path,
            src,
            n,
            "COL-007",
            "logging placeholder is not quoted",
            cfg,
            strict,
        );
    }
    if (t.contains("QStandardItemModel(") || t.contains("QSortFilterProxyModel("))
        && (t.ends_with("()") || t.contains("(parent=None"))
    {
        add(
            out,
            path,
            src,
            n,
            "COL-014",
            "Qt model or proxy is constructed without a parent",
            cfg,
            strict,
        );
    }
    if t.contains("setToolTip(")
        && (t.contains("\"\"")
            || t.contains("''")
            || t.contains(" or \"\"")
            || t.contains(" or ''"))
    {
        add(
            out,
            path,
            src,
            n,
            "COL-013",
            "tooltip may be empty",
            cfg,
            strict,
        );
    }
}
fn check_function(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    n: Node,
    cfg: &Config,
    strict: bool,
) {
    let t = text(n, src);
    let body = n.child_by_field_name("body");
    check_empty_string_returns(out, path, src, n, cfg, strict);
    check_lofting(out, path, src, n, cfg, strict);
    check_docstring_markup(out, path, src, n, &cfg.docstring_convention, cfg, strict);
    if let Some(b) = body {
        let first = b.named_child(0);
        if first.is_some_and(|x| x.kind() == "if_statement") {
            let i = text(first.unwrap(), src);
            if i.contains("return") && i.lines().count() <= 3 {
                add(
                    out,
                    path,
                    src,
                    first.unwrap(),
                    "COL-001",
                    "function immediately returns based on a predicate",
                    cfg,
                    strict,
                );
            }
        }
    }
    if t.contains("Raises:") {
        let direct = body.is_some_and(|b| contains_direct_kind(b, "raise_statement"));
        if !direct {
            add(
                out,
                path,
                src,
                n,
                "COL-012",
                "docstring documents Raises but this function has no direct raise",
                cfg,
                strict,
            );
        }
    }
}

fn contains_direct_kind(node: Node, kind: &str) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return true;
        }
        if child.kind() != "function_definition" && contains_direct_kind(child, kind) {
            return true;
        }
    }
    false
}

fn check_qt_model_subclass(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    class: Node,
    qt_model_classes: &HashSet<String>,
    cfg: &Config,
    strict: bool,
) {
    let Some(superclasses) = class.child_by_field_name("superclasses") else {
        return;
    };
    let bases = class_base_names(superclasses, src);
    if !bases
        .iter()
        .any(|base| is_qt_model_base(base) || qt_model_classes.contains(base))
    {
        return;
    }
    let class_text = text(class, src);
    let has_parent_parameter =
        class_text.contains("def __init__(") && class_text.contains("parent");
    let forwards_parent = class_text.contains("super().__init__(parent")
        || class_text.contains("super().__init__(parent=");
    if !has_parent_parameter || !forwards_parent {
        add(
            out,
            path,
            src,
            class,
            "COL-014",
            "Qt model subclass initializer must accept parent and forward it to super().__init__()",
            cfg,
            strict,
        );
    }
}

/// Finds Qt model/proxy subclasses across all input files.  This deliberately
/// uses imported symbol names rather than module paths: it lets a class inherit
/// from a project-local model imported with an alias, while keeping the rule a
/// lightweight static check.
fn qt_model_classes(sources: &[(PathBuf, String)]) -> HashSet<String> {
    let mut classes = Vec::new();
    let mut aliases = Vec::new();
    for (_, src) in sources {
        let mut parser = TsParser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("Python grammar");
        let Some(tree) = parser.parse(src, None) else {
            continue;
        };
        walk(tree.root_node(), &mut |node| match node.kind() {
            "class_definition" => {
                if let (Some(name), Some(superclasses)) = (
                    node.child_by_field_name("name"),
                    node.child_by_field_name("superclasses"),
                ) {
                    classes.push((
                        text(name, src).to_string(),
                        class_base_names(superclasses, src),
                    ));
                }
            }
            "import_from_statement" => aliases.extend(imported_aliases(text(node, src))),
            _ => {}
        });
    }

    let mut known = HashSet::new();
    loop {
        let before = known.len();
        for (name, bases) in &classes {
            if bases
                .iter()
                .any(|base| is_qt_model_base(base) || known.contains(base))
            {
                known.insert(name.clone());
            }
        }
        for (original, alias) in &aliases {
            if known.contains(original) {
                known.insert(alias.clone());
            }
        }
        if known.len() == before {
            return known;
        }
    }
}

fn is_qt_model_base(name: &str) -> bool {
    matches!(
        name,
        "QAbstractItemModel"
            | "QAbstractListModel"
            | "QAbstractTableModel"
            | "QAbstractProxyModel"
            | "QIdentityProxyModel"
            | "QSortFilterProxyModel"
            | "QStandardItemModel"
    )
}

fn class_base_names(superclasses: Node, src: &str) -> Vec<String> {
    text(superclasses, src)
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .filter_map(|base| base.trim().rsplit('.').next())
        .map(|base| base.trim().to_string())
        .filter(|base| !base.is_empty())
        .collect()
}

fn imported_aliases(statement: &str) -> Vec<(String, String)> {
    let normalized = statement.replace('\n', " ");
    let Some((_, imported)) = normalized.split_once(" import ") else {
        return vec![];
    };
    imported
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .filter_map(|item| {
            let mut parts = item.split_whitespace();
            let original = parts.next()?;
            let alias = match (parts.next(), parts.next()) {
                (Some("as"), Some(alias)) => alias,
                _ => original,
            };
            Some((original.to_string(), alias.to_string()))
        })
        .collect()
}
fn check_none_comparison(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    n: Node,
    cfg: &Config,
    strict: bool,
) {
    let t = text(n, src);
    let function = n.parent().and_then(|mut p| {
        while p.kind() != "function_definition" {
            p = p.parent()?;
        }
        Some(p)
    });
    let optional = function.is_some_and(|f| text(f, src).contains(" | None"));
    if optional && (t.contains(" is not None") || t.contains(" is None")) {
        add(
            out,
            path,
            src,
            n,
            "COL-011",
            "consider a polymorphic truthiness check instead of `is not None`",
            cfg,
            strict,
        );
    }
}

fn check_empty_string_returns(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    function: Node,
    cfg: &Config,
    strict: bool,
) {
    let Some(body) = function.child_by_field_name("body") else {
        return;
    };
    let return_type = function
        .child_by_field_name("return_type")
        .map(|n| text(n, src))
        .unwrap_or("None");
    walk(body, &mut |node| {
        if node.kind() == "return_statement"
            && matches!(text(node, src).trim(), "return \"\"" | "return ''")
        {
            let message = if return_type.contains("None") {
                "empty string is returned; use `return None`"
            } else {
                "empty string is returned; use `return None` and add `None` to this function's return annotation"
            };
            add(out, path, src, node, "COL-009", message, cfg, strict);
        }
    });
}

fn check_lofting(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    function: Node,
    cfg: &Config,
    strict: bool,
) {
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return;
    };
    let Some(body) = function.child_by_field_name("body") else {
        return;
    };
    let body_text = text(body, src);
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        let parameter_text = text(parameter, src);
        let name = parameter_text.split(':').next().unwrap_or("").trim();
        if name.is_empty() || name == "self" || name == "cls" {
            continue;
        }
        let uses: Vec<_> = body_text.match_indices(name).collect();
        if uses.len() == 1 {
            let suffix = &body_text[uses[0].0 + name.len()..];
            if suffix.trim_start().starts_with('.') {
                add(out, path, src, function, "COL-002", "parameter is only queried once; consider lofting the queried value into the caller", cfg, strict);
            }
        }
    }
}

fn check_docstring_markup(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    function: Node,
    convention: &str,
    cfg: &Config,
    strict: bool,
) {
    if !convention.eq_ignore_ascii_case("mkdocs") {
        return;
    }
    let Some(parameters) = function.child_by_field_name("parameters") else {
        return;
    };
    let Some(body) = function.child_by_field_name("body") else {
        return;
    };
    let Some(first) = body.named_child(0) else {
        return;
    };
    let docstring = text(first, src);
    if !(docstring.starts_with("\"") || docstring.starts_with("'")) {
        return;
    }
    let mut cursor = parameters.walk();
    for parameter in parameters.named_children(&mut cursor) {
        let name = text(parameter, src).split(':').next().unwrap_or("").trim();
        if name.len() > 1 && docstring.contains(&format!("*{name}*")) {
            add(
                out,
                path,
                src,
                first,
                "COL-004",
                "MkDocs parameter references use backticks, not asterisks",
                cfg,
                strict,
            );
            break;
        }
    }
}

fn check_pytest_assertion(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    assertion: Node,
    cfg: &Config,
    strict: bool,
) {
    if !path.to_string_lossy().to_ascii_lowercase().contains("test") {
        return;
    }
    let statement = text(assertion, src).trim_start_matches("assert").trim();
    let Some((left, right)) = statement.split_once("==") else {
        return;
    };
    let left = left.trim();
    let right = right.trim();
    let right_is_literal = matches!(right.chars().next(), Some('\'' | '\"' | '[' | '{' | '('))
        || right.parse::<f64>().is_ok()
        || matches!(right, "True" | "False" | "None");
    let left_is_name = left
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && !left.contains(' ');
    if left_is_name && right_is_literal {
        add(
            out,
            path,
            src,
            assertion,
            "COL-006",
            "pytest equality has the actual value on the left",
            cfg,
            strict,
        );
    }
}

fn check_widget_tooltips(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    function: Node,
    cfg: &Config,
    strict: bool,
) {
    let source = text(function, src);
    let mut widgets = HashSet::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some((name, value)) = trimmed.split_once('=') {
            if value.contains("Widget(")
                || value.contains("Button(")
                || value.contains("Label(")
                || value.contains("ComboBox(")
            {
                widgets.insert(name.trim().trim_start_matches("self.").to_string());
            }
        }
    }
    for widget in widgets {
        if !source.contains(&format!("{widget}.setToolTip(")) && !source.contains("no tooltip") {
            add(
                out,
                path,
                src,
                function,
                "COL-013",
                "widget has no static tooltip on every construction path",
                cfg,
                strict,
            );
        }
    }
}

/// Check widgets constructed while initializing a class.  Configuration is often
/// factored into helpers, so follow `self.method()` calls starting at `__init__`.
/// The visited set deliberately makes recursive helper graphs finite.
fn check_class_widget_tooltips(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    class: Node,
    cfg: &Config,
    strict: bool,
) {
    let Some(body) = class.child_by_field_name("body") else {
        return;
    };
    let mut methods = HashMap::new();
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "function_definition" {
            if let Some(name) = child.child_by_field_name("name") {
                methods.insert(text(name, src).to_string(), child);
            }
        }
    }
    let Some(init) = methods.get("__init__").copied() else {
        return;
    };

    let mut reachable = HashSet::new();
    let mut pending = vec!["__init__".to_string()];
    while let Some(name) = pending.pop() {
        if !reachable.insert(name.clone()) {
            continue;
        }
        let Some(method) = methods.get(&name).copied() else {
            continue;
        };
        for called in called_instance_methods(method, src) {
            if methods.contains_key(&called) && !reachable.contains(&called) {
                pending.push(called);
            }
        }
    }

    let reachable_source = reachable
        .iter()
        .filter_map(|name| methods.get(name))
        .map(|method| text(*method, src))
        .collect::<Vec<_>>()
        .join("\n");
    let mut widgets = HashSet::new();
    for line in reachable_source.lines() {
        let trimmed = line.trim();
        if let Some((name, value)) = trimmed.split_once('=') {
            if value.contains("Widget(")
                || value.contains("Button(")
                || value.contains("Label(")
                || value.contains("ComboBox(")
            {
                widgets.insert(name.trim().trim_start_matches("self.").to_string());
            }
        }
    }
    for widget in widgets {
        if !reachable_source.contains(&format!("{widget}.setToolTip("))
            && !reachable_source.contains("no tooltip")
        {
            add(
                out,
                path,
                src,
                init,
                "COL-013",
                "widget has no static tooltip on every construction path",
                cfg,
                strict,
            );
        }
    }
}

fn called_instance_methods(method: Node, src: &str) -> HashSet<String> {
    let mut calls = HashSet::new();
    let Some(body) = method.child_by_field_name("body") else {
        return calls;
    };
    walk_method_nodes(body, &mut |node| {
        if node.kind() != "call" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let Some(attribute) = function.child_by_field_name("attribute") else {
            return;
        };
        let Some(object) = function.child_by_field_name("object") else {
            return;
        };
        if text(object, src) == "self" {
            calls.insert(text(attribute, src).to_string());
        }
    });
    calls
}

fn walk_method_nodes(node: Node, f: &mut impl FnMut(Node)) {
    f(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(child.kind(), "function_definition" | "class_definition") {
            continue;
        }
        walk_method_nodes(child, f);
    }
}
fn check_module_order(
    out: &mut Vec<Finding>,
    path: &Path,
    src: &str,
    root: Node,
    cfg: &Config,
    strict: bool,
) {
    let mut stage = 0;
    let mut c = root.walk();
    for n in root.named_children(&mut c) {
        let next = match n.kind() {
            "class_definition" => 1,
            "function_definition" => 2,
            _ => 0,
        };
        if next < stage && next == 0 {
            add(
                out,
                path,
                src,
                n,
                "COL-008",
                "module declaration appears after a class or function",
                cfg,
                strict,
            );
        }
        stage = stage.max(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn findings(source: &str, path: &str) -> Vec<Finding> {
        analyze(Path::new(path), source, &Config::default(), false)
    }

    fn codes(source: &str, path: &str) -> Vec<String> {
        findings(source, path)
            .into_iter()
            .map(|finding| finding.code)
            .collect()
    }

    #[test]
    fn local_return_annotation_is_found() {
        let src = "def value() -> list[str]:\n    return []\n";
        let mut parser = TsParser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .unwrap();
        assert_eq!(
            local_return_types(parser.parse(src, None).unwrap().root_node(), src).get("value"),
            Some(&"list[str]".to_string())
        );
    }

    #[test]
    fn human_findings_use_real_newlines() {
        let finding = Finding {
            path: "app.py".into(),
            line: 2,
            column: 5,
            code: "COL-010".into(),
            name: "assert-in-production".into(),
            severity: Severity::Error,
            message: "assert is used outside test code".into(),
            recommendation: "Raise an explicit exception instead of assert in non-test code."
                .into(),
        };
        let rendered = format_human_finding(&finding);
        assert!(rendered.contains('\n'));
        assert!(!rendered.contains(r"\n"));
        assert_eq!(rendered.lines().count(), 2);
    }

    #[test]
    fn human_output_header_is_opt_in() {
        let finding = Finding {
            path: "app.py".into(),
            line: 1,
            column: 1,
            code: "COL-010".into(),
            name: "assert-in-production".into(),
            severity: Severity::Error,
            message: "assert is used outside test code".into(),
            recommendation: "Raise an explicit exception instead of assert in non-test code."
                .into(),
        };
        let without_header = format_human_output(&[finding], false);
        assert!(!without_header.contains("colint diagnostics"));

        let with_header = format_human_output(&[], true);
        assert!(with_header.starts_with("colint diagnostics\n"));
        assert!(with_header.contains("# noqa"));
        assert!(with_header.contains("# colint: ignore[COL-003,COL-010]"));
        assert!(with_header.contains("# NOTE: reason"));
        assert!(!with_header.contains(r"\n"));
    }

    #[test]
    fn reports_core_rules_and_honors_line_suppressions() {
        let source = r#"
def source() -> str:
    return "ok"

def consumer():
    value: str = source()
    import os
    assert value
    return ""

def ignored():
    import sys  # colint: ignore[COL-003]
"#;
        let codes = codes(source, "production.py");
        assert!(codes.contains(&"COL-003".to_string()));
        assert!(codes.contains(&"COL-005".to_string()));
        assert!(codes.contains(&"COL-009".to_string()));
        assert!(codes.contains(&"COL-010".to_string()));
        assert_eq!(codes.iter().filter(|code| *code == "COL-003").count(), 1);
    }

    #[test]
    fn pytest_literals_belong_on_the_left() {
        let codes = codes(
            "def test_value():\n    assert actual == [\"expected\"]\n",
            "test_value.py",
        );
        assert!(codes.contains(&"COL-006".to_string()));
    }

    #[test]
    fn configuration_can_disable_a_rule_unless_strict() {
        let config = Config {
            warnings_as_errors: false,
            rules: HashMap::from([("COL-010".to_string(), false)]),
            docstring_convention: default_docstring_convention(),
            import_paths: Vec::new(),
        };
        let normal = analyze(
            Path::new("production.py"),
            "assert active\n",
            &config,
            false,
        );
        let strict = analyze(Path::new("production.py"), "assert active\n", &config, true);
        assert!(normal.is_empty());
        assert!(strict.iter().any(|finding| finding.code == "COL-010"));
    }

    #[test]
    fn predicate_guard_is_reported() {
        assert!(codes(
            "def run(value):\n    if not value:\n        return\n    work()\n",
            "app.py"
        )
        .contains(&"COL-001".to_string()));
    }

    #[test]
    fn lofting_single_query_is_reported() {
        assert!(codes(
            "def run(thing):\n    value = thing.get_value()\n    use(value)\n",
            "app.py"
        )
        .contains(&"COL-002".to_string()));
    }

    #[test]
    fn nested_import_requires_note_but_accepts_multiline_note() {
        let bad = codes("def run():\n    import os\n", "app.py");
        let good = codes("def run():\n    # NOTE: platform-dependent import\n    # kept local to avoid startup cost\n    import os\n", "app.py");
        assert!(bad.contains(&"COL-003".to_string()));
        assert!(!good.contains(&"COL-003".to_string()));
    }

    #[test]
    fn mkdocs_docstrings_reject_asterisk_parameter_markup() {
        assert!(codes(
            "def create(task):\n    \"\"\"Create *task*.\"\"\"\n",
            "app.py"
        )
        .contains(&"COL-004".to_string()));
    }

    #[test]
    fn quoted_logging_placeholder_is_allowed() {
        let bad = codes("logger.error('Could not load %s', name)\n", "app.py");
        let good = codes("logger.error('Could not load \"%s\"', name)\n", "app.py");
        assert!(bad.contains(&"COL-007".to_string()));
        assert!(!good.contains(&"COL-007".to_string()));
    }

    #[test]
    fn module_assignment_after_function_is_reported() {
        assert!(codes("def make():\n    pass\n\nVALUE = 1\n", "app.py")
            .contains(&"COL-008".to_string()));
    }

    #[test]
    fn empty_return_requires_none() {
        assert!(codes("def find() -> str | int:\n    return ''\n", "app.py")
            .contains(&"COL-009".to_string()));
    }

    #[test]
    fn asserts_are_allowed_in_test_files_only() {
        assert!(codes("assert ready\n", "app.py").contains(&"COL-010".to_string()));
        assert!(!codes("assert ready\n", "test_app.py").contains(&"COL-010".to_string()));
    }

    #[test]
    fn optional_none_comparisons_need_optional_annotation() {
        assert!(codes(
            "def run(value: Thing | None):\n    if value is not None:\n        use(value)\n",
            "app.py"
        )
        .contains(&"COL-011".to_string()));
        assert!(!codes(
            "def run(value: Thing):\n    if value is not None:\n        use(value)\n",
            "app.py"
        )
        .contains(&"COL-011".to_string()));
    }

    #[test]
    fn indirect_raises_documentation_is_reported() {
        assert!(codes("def run():\n    \"\"\"Run.\n\n    Raises:\n        ValueError: When broken.\n    \"\"\"\n    dependency()\n", "app.py").contains(&"COL-012".to_string()));
        assert!(!codes("def run():\n    \"\"\"Run.\n\n    Raises:\n        ValueError: When broken.\n    \"\"\"\n    raise ValueError()\n", "app.py").contains(&"COL-012".to_string()));
    }

    #[test]
    fn missing_or_empty_widget_tooltips_are_reported() {
        let missing = codes(
            "def build(self):\n    self.button = QPushButton()\n",
            "app.py",
        );
        let empty = codes(
            "def build(self):\n    self.button = QPushButton()\n    self.button.setToolTip(\"\")\n",
            "app.py",
        );
        assert!(missing.contains(&"COL-013".to_string()));
        assert!(empty.contains(&"COL-013".to_string()));
    }

    #[test]
    fn tooltip_helpers_reachable_from_init_are_accepted() {
        let source = r#"
class Window:
    def __init__(self):
        self.button = QPushButton()
        self._configure()

    def _configure(self):
        self._set_button_tooltip()

    def _set_button_tooltip(self):
        self.button.setToolTip("Open the selected item")
"#;
        assert!(!codes(source, "app.py").contains(&"COL-013".to_string()));
    }

    #[test]
    fn recursive_tooltip_helper_graphs_are_accepted() {
        let source = r#"
class Window:
    def __init__(self):
        self.button = QPushButton()
        self._configure()

    def _configure(self):
        self._set_button_tooltip()

    def _set_button_tooltip(self):
        self._configure()
        self.button.setToolTip("Open the selected item")
"#;
        assert!(!codes(source, "app.py").contains(&"COL-013".to_string()));
    }

    #[test]
    fn unreachable_tooltip_helpers_do_not_satisfy_init_widgets() {
        let source = r#"
class Window:
    def __init__(self):
        self.button = QPushButton()

    def _set_button_tooltip(self):
        self.button.setToolTip("Open the selected item")
"#;
        assert!(codes(source, "app.py").contains(&"COL-013".to_string()));
    }

    #[test]
    fn qt_model_parent_is_required_for_construction() {
        let bad = codes("model = QStandardItemModel()\n", "app.py");
        let good = codes("model = QStandardItemModel(parent=parent)\n", "app.py");
        assert!(bad.contains(&"COL-014".to_string()));
        assert!(!good.contains(&"COL-014".to_string()));
    }

    #[test]
    fn qt_model_parent_is_required_for_subclasses_of_imported_project_models() {
        let sources = vec![
            (
                PathBuf::from("models.py"),
                "class ProjectModel(QStandardItemModel):\n    def __init__(self, parent):\n        super().__init__(parent)\n"
                    .to_string(),
            ),
            (
                PathBuf::from("views.py"),
                "from models import ProjectModel as BaseModel\n\nclass ViewModel(BaseModel):\n    pass\n"
                    .to_string(),
            ),
        ];
        let known = qt_model_classes(&sources);
        let findings = analyze_with_qt_model_classes(
            Path::new("views.py"),
            &sources[1].1,
            &Config::default(),
            false,
            &known,
        );
        assert!(findings.iter().any(|finding| finding.code == "COL-014"));
    }

    #[test]
    fn abstract_qt_proxy_models_are_tracked_through_imports() {
        let sources = vec![
            (
                PathBuf::from("shared/models.py"),
                "class ProjectProxy(QAbstractProxyModel):\n    def __init__(self, parent):\n        super().__init__(parent)\n"
                    .to_string(),
            ),
            (
                PathBuf::from("app/view.py"),
                "from shared.models import ProjectProxy\n\nclass ViewProxy(ProjectProxy):\n    pass\n"
                    .to_string(),
            ),
        ];
        let known = qt_model_classes(&sources);
        let findings = analyze_with_qt_model_classes(
            Path::new("app/view.py"),
            &sources[1].1,
            &Config::default(),
            false,
            &known,
        );
        assert!(findings.iter().any(|finding| finding.code == "COL-014"));
    }

    #[test]
    fn noqa_and_selected_suppressions_do_not_overreach() {
        assert!(codes(
            "def run():\n    import os  # colint: ignore[COL-010]\n",
            "app.py"
        )
        .contains(&"COL-003".to_string()));
        assert!(!codes("assert value  # noqa\n", "app.py").contains(&"COL-010".to_string()));
    }

    #[test]
    fn nested_function_raises_do_not_justify_outer_raises_docstring() {
        let source = "def outer():\n    \"\"\"Outer.\n\n    Raises:\n        ValueError: Never directly raised.\n    \"\"\"\n    def inner():\n        raise ValueError()\n";
        assert!(codes(source, "app.py").contains(&"COL-012".to_string()));
    }

    #[test]
    fn strict_reenables_disabled_warning_rules() {
        let config = Config {
            warnings_as_errors: false,
            rules: HashMap::from([("COL-001".to_string(), false)]),
            docstring_convention: default_docstring_convention(),
            import_paths: Vec::new(),
        };
        let source = "def run(value):\n    if not value:\n        return\n    work()\n";
        assert!(!analyze(Path::new("app.py"), source, &config, false)
            .iter()
            .any(|finding| finding.code == "COL-001"));
        assert!(analyze(Path::new("app.py"), source, &config, true)
            .iter()
            .any(|finding| finding.code == "COL-001"));
    }

    #[test]
    fn non_mkdocs_convention_disables_mkdocs_markup_advice() {
        let config = Config {
            warnings_as_errors: false,
            rules: HashMap::new(),
            docstring_convention: "google".to_string(),
            import_paths: Vec::new(),
        };
        let source = "def create(task):\n    \"\"\"Create *task*.\"\"\"\n";
        assert!(!analyze(Path::new("app.py"), source, &config, false)
            .iter()
            .any(|finding| finding.code == "COL-004"));
    }

    #[test]
    fn exit_status_distinguishes_warnings_escalation_and_errors() {
        let warning = findings(
            "def run(value):\n    if not value:\n        return\n",
            "app.py",
        );
        let error = findings("assert ready\n", "app.py");
        assert_eq!(exit_status(&[], false), 0);
        assert_eq!(exit_status(&warning, false), 0);
        assert_eq!(exit_status(&warning, true), 1);
        assert_eq!(exit_status(&error, false), 2);
    }

    #[test]
    fn multiline_calls_and_import_groups_preserve_diagnostics() {
        let source = "def run():\n    import os\n    import sys\n\nlogger.error(\n    'Failed for %s',\n    value,\n)\n";
        let codes = codes(source, "app.py");
        assert_eq!(codes.iter().filter(|code| *code == "COL-003").count(), 2);
        assert!(codes.contains(&"COL-007".to_string()));
    }
}
