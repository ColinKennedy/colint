#![allow(clippy::too_many_arguments)]

use clap::Parser;
use regex::Regex;
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};
use tree_sitter::{Node, Parser as TsParser, Point};
use walkdir::{DirEntry, WalkDir};

/// A regex compiled once, on first use.
macro_rules! static_regex {
    ($pattern:expr) => {{
        static REGEX: std::sync::LazyLock<Regex> =
            std::sync::LazyLock::new(|| Regex::new($pattern).expect("valid regex"));
        &*REGEX
    }};
}

const RULES: &[Rule] = &[
    // API — function design and correctness
    Rule::new("API-001", "predicate-in-function", Severity::Warning, "Move the predicate to the caller so this function always performs work."),
    Rule::new("API-002", "loft-from-caller", Severity::Warning, "Pass the queried value instead of the complex object when the object is not otherwise used."),
    Rule::new("API-003", "redundant-type-hint", Severity::Error, "Remove the redundant local annotation; the locally defined callee already provides its return type."),
    Rule::new("API-004", "empty-string-return", Severity::Error, "Return None for a not-found result and include None in the return annotation."),
    Rule::new("API-005", "assert-in-production", Severity::Error, "Raise an explicit exception instead of assert in non-test code."),
    Rule::new("API-006", "none-polymorphism", Severity::Warning, "Prefer a polymorphic truthiness check (`if value:`) for a Foo | None value."),
    // DOC — documentation
    Rule::new("DOC-001", "docstring-convention", Severity::Warning, "Wrap parameter names in docstrings with the configured variable markup (backticks by default)."),
    Rule::new("DOC-002", "direct-raises-only", Severity::Error, "Document Raises only for exceptions raised directly by this function."),
    // STY — style and organization
    Rule::new("STY-001", "nested-import", Severity::Error, "Move the import to module scope, or add an immediately preceding NOTE comment explaining why it is nested."),
    Rule::new("STY-002", "pytest-expected-left", Severity::Warning, "Put the expected value on the left side of a pytest equality assertion."),
    Rule::new("STY-003", "logging-dynamic-quote", Severity::Warning, "Wrap a logging placeholder in quotes, for example `message \"%s\"`."),
    Rule::new("STY-004", "module-order", Severity::Warning, "Keep module declarations ordered as globals, classes, then functions."),
    // GUI — Qt/PySide widgets
    Rule::new("GUI-001", "widget-tooltip", Severity::Warning, "Set a non-empty static tooltip, or add a comment explaining why no tooltip is appropriate."),
    Rule::new("GUI-002", "qt-model-parent", Severity::Error, "Construct Qt models and proxies with a parent; subclasses should forward parent to super().__init__()."),
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
    /// Whether API-002 skips underscore-prefixed functions, methods, and classes.
    #[serde(default = "default_api002_skip_private_definitions")]
    api002_skip_private_definitions: bool,
    /// Extra package roots used when resolving imported base classes. Paths in
    /// `.colint.toml` are relative to that file unless already absolute.
    #[serde(default)]
    import_paths: Vec<PathBuf>,
    /// The markup DOC-001 expects around parameter names in docstrings.
    #[serde(default)]
    docstring_variable_markup: VariableMarkup,
}
#[derive(serde::Deserialize, Clone, PartialEq, Eq, Debug)]
struct VariableMarkup {
    #[serde(default = "default_variable_markup_delimiter")]
    start: String,
    #[serde(default = "default_variable_markup_delimiter")]
    end: String,
}
fn default_variable_markup_delimiter() -> String {
    "`".into()
}
impl Default for VariableMarkup {
    fn default() -> Self {
        Self {
            start: default_variable_markup_delimiter(),
            end: default_variable_markup_delimiter(),
        }
    }
}
fn default_api002_skip_private_definitions() -> bool {
    true
}
impl Default for Config {
    fn default() -> Self {
        Self {
            warnings_as_errors: false,
            rules: HashMap::new(),
            api002_skip_private_definitions: default_api002_skip_private_definitions(),
            import_paths: Vec::new(),
            docstring_variable_markup: VariableMarkup::default(),
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
Suppress a finding on its line with `# noqa`, or selected codes or rule\n\
names with `# colint: ignore[STY-001,API-005]` or\n\
`# colint: ignore[nested-import]`. For STY-001 nested imports, use\n\
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
    let root = tree.root_node();
    let mut lint = Linter {
        path,
        src,
        cfg: config,
        strict,
        is_test: is_test_path(path),
        out: Vec::new(),
    };
    let functions = local_return_types(root, src);
    let methods = local_method_return_types(root, src);
    let widget_classes = custom_widget_classes(root, src);
    let class_bases = local_class_bases(root, src);
    walk(root, &mut |n| match n.kind() {
        "import_statement" | "import_from_statement" => lint.check_nested_import(n),
        "assert_statement" => {
            lint.check_assert(n);
            lint.check_pytest_assertion(n);
        }
        "assignment" => {
            lint.check_redundant_annotation(n, &functions, &methods);
            lint.check_widget_assignment(n, &widget_classes);
        }
        "call" => {
            lint.check_logging_call(n);
            lint.check_model_construction(n, qt_model_classes);
            lint.check_empty_tooltip(n);
        }
        "function_definition" => lint.check_function(n, &class_bases),
        "class_definition" => lint.check_qt_model_subclass(n, qt_model_classes),
        "if_statement" => lint.check_none_comparison(n),
        _ => {}
    });
    lint.check_module_order(root);
    lint.out
}

/// Per-file state shared by every check.
struct Linter<'a> {
    path: &'a Path,
    src: &'a str,
    cfg: &'a Config,
    strict: bool,
    is_test: bool,
    out: Vec<Finding>,
}

impl<'a> Linter<'a> {
    fn text(&self, node: Node) -> &'a str {
        text(node, self.src)
    }

    fn report(&mut self, node: Node, code: &str, message: &str) {
        if !enabled(code, self.cfg, self.strict) || suppressed(node, self.src, code) {
            return;
        }
        self.push(node.start_position(), code, message);
    }

    /// Records a finding at `p` without checking enablement or suppressions.
    fn push(&mut self, p: Point, code: &str, message: &str) {
        let r = rule(code);
        self.out.push(Finding {
            path: self.path.display().to_string(),
            line: p.row + 1,
            column: p.column + 1,
            code: code.into(),
            name: r.name.into(),
            severity: r.severity,
            message: message.into(),
            recommendation: r.recommendation.into(),
        });
    }

    fn check_function(&mut self, function: Node, class_bases: &HashMap<String, Vec<String>>) {
        self.check_predicate_guard(function);
        self.check_lofting(function);
        self.check_empty_string_returns(function);
        self.check_docstring_markup(function);
        self.check_raises_documentation(function, class_bases);
    }

    // API-001 -------------------------------------------------------------

    /// Reports a function whose first statement (after any docstring) is an
    /// `if` with no `elif`/`else` whose only body statement is a `return`.
    fn check_predicate_guard(&mut self, function: Node) {
        let Some(body) = function.child_by_field_name("body") else {
            return;
        };
        let statements = code_children(body);
        let skip = usize::from(docstring(function).is_some());
        let Some(&statement) = statements.get(skip) else {
            return;
        };
        if statement.kind() != "if_statement"
            || statement.child_by_field_name("alternative").is_some()
        {
            return;
        }
        let Some(consequence) = statement.child_by_field_name("consequence") else {
            return;
        };
        let guarded = code_children(consequence);
        if let [only] = guarded.as_slice() {
            if only.kind() == "return_statement" {
                self.report(
                    statement,
                    "API-001",
                    "function immediately returns based on a predicate",
                );
            }
        }
    }

    // API-002 -------------------------------------------------------------

    fn check_lofting(&mut self, function: Node) {
        if self.cfg.api002_skip_private_definitions && is_private_definition(function, self.src) {
            return;
        }
        if has_decorator(function, self.src, "override") {
            return;
        }
        let Some(parameters) = function.child_by_field_name("parameters") else {
            return;
        };
        let Some(body) = function.child_by_field_name("body") else {
            return;
        };
        let mut candidates = Vec::new();
        for parameter in code_children(parameters) {
            let Some((name, splat)) = parameter_name(parameter, self.src) else {
                continue;
            };
            if splat || name == "self" || name == "cls" {
                continue;
            }
            let uses = references(body, name, self.src);
            let [only] = uses.as_slice() else {
                continue;
            };
            let Some(query) = query_call(*only) else {
                continue;
            };
            candidates.push((name.to_string(), lofting_target(query, body, self.src)));
        }
        if candidates.is_empty() {
            return;
        }
        let names = candidates
            .iter()
            .map(|(name, _)| format!("`{name}`"))
            .collect::<Vec<_>>();
        let parameters = join_human(&names);
        let targets = candidates
            .iter()
            .filter_map(|(_, target)| target.as_deref())
            .collect::<HashSet<_>>();
        let singular = candidates.len() == 1;
        let destination =
            if targets.len() == 1 && candidates.iter().all(|(_, target)| target.is_some()) {
                format!("to `{}`", targets.into_iter().next().expect("one target"))
            } else {
                "into the caller".to_string()
            };
        let message = format!(
            "{} {} {} only queried once; loft {} queried {} {}",
            if singular { "parameter" } else { "parameters" },
            parameters,
            if singular { "is" } else { "are" },
            if singular { "its" } else { "their" },
            if singular { "value" } else { "values" },
            destination,
        );
        self.report(function, "API-002", &message);
    }

    // API-003 -------------------------------------------------------------

    fn check_redundant_annotation(
        &mut self,
        assignment: Node,
        functions: &HashMap<String, String>,
        methods: &HashMap<String, String>,
    ) {
        let (Some(annotation), Some(right)) = (
            assignment.child_by_field_name("type"),
            assignment.child_by_field_name("right"),
        ) else {
            return;
        };
        let mut value = unparen(right);
        if value.kind() == "await" {
            let Some(&awaited) = code_children(value).first() else {
                return;
            };
            value = unparen(awaited);
        }
        if value.kind() != "call" {
            return;
        }
        let Some(callee) = value.child_by_field_name("function") else {
            return;
        };
        let declared = match callee.kind() {
            "identifier" => functions.get(self.text(callee)),
            "attribute" => {
                let object = callee.child_by_field_name("object");
                let attribute = callee.child_by_field_name("attribute");
                match (object, attribute) {
                    (Some(object), Some(attribute))
                        if matches!(self.text(object), "self" | "cls") =>
                    {
                        methods.get(self.text(attribute))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if declared.is_some_and(|declared| {
            normalize_annotation(declared) == normalize_annotation(self.text(annotation))
        }) {
            self.report(
                assignment,
                "API-003",
                "local annotation duplicates the direct callee return type",
            );
        }
    }

    // API-004 -------------------------------------------------------------

    fn check_empty_string_returns(&mut self, function: Node) {
        let Some(body) = function.child_by_field_name("body") else {
            return;
        };
        let allows_none = function
            .child_by_field_name("return_type")
            .map_or(true, |annotation| {
                annotation_allows_none(self.text(annotation))
            });
        let message = if allows_none {
            "empty string is returned; use `return None`"
        } else {
            "empty string is returned; use `return None` and add `None` to this function's return annotation"
        };
        let mut returns = Vec::new();
        walk_scope(body, &mut |node| {
            if node.kind() == "return_statement"
                && code_children(node)
                    .first()
                    .is_some_and(|value| is_empty_string(*value, self.src))
            {
                returns.push(node);
            }
        });
        for node in returns {
            self.report(node, "API-004", message);
        }
    }

    // API-005 -------------------------------------------------------------

    fn check_assert(&mut self, assertion: Node) {
        if !self.is_test {
            self.report(assertion, "API-005", "assert is used outside test code");
        }
    }

    // API-006 -------------------------------------------------------------

    /// Inspects only the `if`/`elif` conditions, so a comparison is reported
    /// once at its own location rather than for every enclosing `if`.
    fn check_none_comparison(&mut self, if_statement: Node) {
        let Some(function) = enclosing_function(if_statement) else {
            return;
        };
        let optional = optional_names(function, self.src);
        if optional.is_empty() {
            return;
        }
        let mut conditions = Vec::from_iter(if_statement.child_by_field_name("condition"));
        let mut cursor = if_statement.walk();
        for alternative in if_statement.children_by_field_name("alternative", &mut cursor) {
            if alternative.kind() == "elif_clause" {
                conditions.extend(alternative.child_by_field_name("condition"));
            }
        }
        let mut comparisons = Vec::new();
        for condition in conditions {
            walk(condition, &mut |node| {
                if node.kind() != "comparison_operator" {
                    return;
                }
                let operands = code_children(node);
                let [left, right] = operands.as_slice() else {
                    return;
                };
                let operator = comparison_operator(node, self.src);
                if operator != "is" && operator != "is not" {
                    return;
                }
                let (left, right) = (unparen(*left), unparen(*right));
                let name = match (left.kind(), right.kind()) {
                    ("identifier", "none") => left,
                    ("none", "identifier") => right,
                    _ => return,
                };
                if optional.contains(self.text(name)) {
                    comparisons.push(node);
                }
            });
        }
        for comparison in comparisons {
            let message = format!(
                "consider a polymorphic truthiness check instead of `{}`",
                collapse_whitespace(self.text(comparison))
            );
            self.report(comparison, "API-006", &message);
        }
    }

    // DOC-001 -------------------------------------------------------------

    /// Reports each parameter name in the function's own docstring that is
    /// wrapped in markup other than the configured `[docstring_variable_markup]`,
    /// such as `*bar*`, `"bar"`, or ``` ``bar`` ``` instead of `` `bar` ``.
    fn check_docstring_markup(&mut self, function: Node) {
        let code = "DOC-001";
        if !enabled(code, self.cfg, self.strict) {
            return;
        }
        let Some(parameters) = function.child_by_field_name("parameters") else {
            return;
        };
        let Some(docstring) = docstring(function) else {
            return;
        };
        let Some(body) = string_body_range(docstring, self.src) else {
            return;
        };
        let mut names = Vec::new();
        for parameter in code_children(parameters) {
            if let Some((name, _)) = parameter_name(parameter, self.src) {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
        }
        let markup = &self.cfg.docstring_variable_markup;
        let references = wrapped_parameter_references(&self.src[body.clone()], &names, markup);
        // A comment can only follow the closing quotes, so a suppression there
        // or on the `def` line covers the whole docstring.
        let docstring_suppressed = suppressed(function, self.src, code)
            || suppressed_on_row(function, docstring.end_position().row, self.src, code);
        for reference in references {
            let byte = body.start + reference.offset;
            let p = point_at(self.src, byte);
            if docstring_suppressed || suppressed_on_row(function, p.row, self.src, code) {
                continue;
            }
            let WrappedReference {
                name, open, close, ..
            } = reference;
            let VariableMarkup { start, end } = markup;
            self.push(
                p,
                code,
                &format!(
                    "parameter {name} is written as {open}{name}{close}; use {start}{name}{end}"
                ),
            );
        }
    }

    // DOC-002 -------------------------------------------------------------

    fn check_raises_documentation(
        &mut self,
        function: Node,
        class_bases: &HashMap<String, Vec<String>>,
    ) {
        let Some(docstring) = docstring(function) else {
            return;
        };
        let Some(documented) = documented_raises(self.text(docstring)) else {
            return;
        };
        let Some(body) = function.child_by_field_name("body") else {
            return;
        };
        let raised = direct_raises(body, self.src);
        if !raised.any {
            self.report(
                function,
                "DOC-002",
                "docstring documents Raises but this function has no direct raise",
            );
            return;
        }
        if raised.unknown {
            return;
        }
        let indirect = documented
            .iter()
            .filter(|documented| {
                !raised
                    .names
                    .iter()
                    .any(|raised| is_same_or_subclass(raised, documented, class_bases))
            })
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>();
        if !indirect.is_empty() {
            let message = format!(
                "docstring documents {} in Raises but this function does not raise {} directly",
                join_human(&indirect),
                if indirect.len() == 1 { "it" } else { "them" },
            );
            self.report(function, "DOC-002", &message);
        }
    }

    // STY-001 -------------------------------------------------------------

    /// Only imports inside a function or class are nested. Module-level
    /// conditional imports (`if TYPE_CHECKING:`, `try: ... except ImportError:`)
    /// are still evaluated once at import time and are not reported.
    fn check_nested_import(&mut self, import: Node) {
        if enclosing_definition(import).is_none() || nested_import_has_note(import, self.src) {
            return;
        }
        self.report(
            import,
            "STY-001",
            "nested import requires a preceding NOTE comment",
        );
    }

    // STY-002 -------------------------------------------------------------

    fn check_pytest_assertion(&mut self, assertion: Node) {
        if !self.is_test {
            return;
        }
        let Some(condition) = code_children(assertion).first().map(|n| unparen(*n)) else {
            return;
        };
        if condition.kind() != "comparison_operator" {
            return;
        }
        let operands = code_children(condition);
        let [left, right] = operands.as_slice() else {
            return;
        };
        if comparison_operator(condition, self.src) != "==" {
            return;
        }
        if !is_literal(*left) && is_literal(*right) {
            self.report(
                assertion,
                "STY-002",
                "pytest equality has the actual value on the left",
            );
        }
    }

    // STY-003 -------------------------------------------------------------

    fn check_logging_call(&mut self, call: Node) {
        let Some(message) = logging_message_argument(call, self.src) else {
            return;
        };
        let mut strings = Vec::new();
        match message.kind() {
            "string" => strings.push(message),
            "concatenated_string" => strings.extend(code_children(message)),
            _ => return,
        }
        let unquoted = strings
            .into_iter()
            .filter_map(|string| string_body(string, self.src))
            .any(has_unquoted_placeholder);
        if unquoted {
            self.report(call, "STY-003", "logging placeholder is not quoted");
        }
    }

    // STY-004 -------------------------------------------------------------

    fn check_module_order(&mut self, root: Node) {
        let mut stage = 0;
        for node in code_children(root) {
            let definition = if node.kind() == "decorated_definition" {
                node.child_by_field_name("definition").unwrap_or(node)
            } else {
                node
            };
            let next = match definition.kind() {
                "class_definition" => 1,
                "function_definition" => 2,
                _ if is_main_guard(node, self.src) => continue,
                _ => 0,
            };
            if next < stage {
                let message = if next == 0 {
                    "module declaration appears after a class or function"
                } else {
                    "class appears after a function"
                };
                self.report(node, "STY-004", message);
            }
            stage = stage.max(next);
        }
    }

    // GUI-001 -------------------------------------------------------------

    /// Reports a widget constructed and bound to a name when no
    /// `name.setToolTip(...)` call is reachable from the construction scope.
    /// Inside a class, the scope is every method reachable through `self.`
    /// calls from the constructing method or from any method that reaches it.
    fn check_widget_assignment(&mut self, assignment: Node, widget_classes: &HashSet<String>) {
        let (Some(left), Some(right)) = (
            assignment.child_by_field_name("left"),
            assignment.child_by_field_name("right"),
        ) else {
            return;
        };
        if !matches!(left.kind(), "identifier" | "attribute") {
            return;
        }
        let call = unparen(right);
        if call.kind() != "call" {
            return;
        }
        let Some(constructor) = call
            .child_by_field_name("function")
            .and_then(|function| last_segment(function, self.src))
        else {
            return;
        };
        if !is_widget_class(constructor, widget_classes) {
            return;
        }
        if keyword_argument(call, "toolTip", self.src)
            .is_some_and(|value| !is_empty_string(value, self.src) && value.kind() != "none")
        {
            return;
        }
        let target: String = self
            .text(left)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let name = target.strip_prefix("self.").unwrap_or(&target);
        let scope = tooltip_scope(assignment, self.src);
        let tooltip = Regex::new(&format!(
            r"(?:^|[^\w.]|\bself\s*\.\s*){}\s*\.\s*setToolTip\s*\(",
            regex::escape(name).replace(r"\.", r"\s*\.\s*")
        ))
        .expect("escaped widget name is a valid regex");
        let exempt = static_regex!(r"(?im)#[^\n]*\bno tooltip\b");
        if !scope
            .iter()
            .any(|s| tooltip.is_match(s) || exempt.is_match(s))
        {
            let message =
                format!("widget `{target}` has no static tooltip on every construction path");
            self.report(assignment, "GUI-001", &message);
        }
    }

    fn check_empty_tooltip(&mut self, call: Node) {
        let Some(function) = call.child_by_field_name("function") else {
            return;
        };
        if function.kind() != "attribute"
            || function
                .child_by_field_name("attribute")
                .map_or(true, |attribute| self.text(attribute) != "setToolTip")
        {
            return;
        }
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        let Some(value) = code_children(arguments).first().map(|n| unparen(*n)) else {
            return;
        };
        let empty = is_empty_string(value, self.src)
            || (value.kind() == "boolean_operator"
                && value
                    .child_by_field_name("right")
                    .is_some_and(|right| is_empty_string(right, self.src)));
        if empty {
            self.report(call, "GUI-001", "tooltip may be empty");
        }
    }

    // GUI-002 -------------------------------------------------------------

    fn check_model_construction(&mut self, call: Node, known: &HashSet<String>) {
        let Some(name) = call
            .child_by_field_name("function")
            .and_then(|function| last_segment(function, self.src))
        else {
            return;
        };
        if !is_qt_model_base(name) && !known.contains(name) {
            return;
        }
        let Some(arguments) = call.child_by_field_name("arguments") else {
            return;
        };
        if arguments.kind() != "argument_list" {
            return;
        }
        let has_parent =
            code_children(arguments)
                .into_iter()
                .any(|argument| match argument.kind() {
                    "keyword_argument" => {
                        argument
                            .child_by_field_name("name")
                            .is_some_and(|key| self.text(key) == "parent")
                            && argument
                                .child_by_field_name("value")
                                .is_some_and(|value| unparen(value).kind() != "none")
                    }
                    "list_splat" | "dictionary_splat" => true,
                    _ => !is_literal(argument),
                });
        if !has_parent {
            self.report(
                call,
                "GUI-002",
                "Qt model or proxy is constructed without a parent",
            );
        }
    }

    fn check_qt_model_subclass(&mut self, class: Node, known: &HashSet<String>) {
        let Some(superclasses) = class.child_by_field_name("superclasses") else {
            return;
        };
        let bases = class_base_names(superclasses, self.src);
        if !bases
            .iter()
            .any(|base| is_qt_model_base(base) || known.contains(base))
        {
            return;
        }
        let forwards = class
            .child_by_field_name("body")
            .and_then(|body| method(body, "__init__", self.src))
            .is_some_and(|init| init_forwards_parent(init, self.src));
        if !forwards {
            self.report(
                class,
                "GUI-002",
                "Qt model subclass initializer must accept parent and forward it to super().__init__()",
            );
        }
    }
}

// Syntax helpers ---------------------------------------------------------

fn walk<'t>(node: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    f(node);
    let mut c = node.walk();
    for child in node.children(&mut c) {
        walk(child, f);
    }
}

/// Walks one function or class body without entering nested definitions or
/// lambdas, whose statements belong to a different scope.
fn walk_scope<'t>(node: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    f(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "function_definition" | "class_definition" | "lambda"
        ) {
            continue;
        }
        walk_scope(child, f);
    }
}

fn text<'a>(n: Node, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

/// Named children, skipping the comment and backslash `line_continuation`
/// extras that Tree-sitter may place between any two tokens.
fn code_children(node: Node) -> Vec<Node> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| !is_extra(*child))
        .collect()
}

fn is_extra(node: Node) -> bool {
    matches!(node.kind(), "comment" | "line_continuation")
}

fn unparen(mut node: Node) -> Node {
    while node.kind() == "parenthesized_expression" {
        match code_children(node).first() {
            Some(inner) => node = *inner,
            None => break,
        }
    }
    node
}

/// The operator tokens of a two-operand comparison (`==`, `is not`, ...),
/// ignoring comments and line continuations between the operands.
fn comparison_operator(comparison: Node, src: &str) -> String {
    let mut cursor = comparison.walk();
    let tokens = comparison
        .children(&mut cursor)
        .filter(|child| !child.is_named())
        .map(|child| text(child, src))
        .collect::<Vec<_>>();
    collapse_whitespace(&tokens.join(" "))
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The final name in `name` or `object.attribute`.
fn last_segment<'a>(node: Node, src: &'a str) -> Option<&'a str> {
    match node.kind() {
        "identifier" => Some(text(node, src)),
        "attribute" => node
            .child_by_field_name("attribute")
            .map(|attribute| text(attribute, src)),
        _ => None,
    }
}

fn enclosing_function(node: Node) -> Option<Node> {
    let mut parent = node.parent();
    while let Some(current) = parent {
        if current.kind() == "function_definition" {
            return Some(current);
        }
        parent = current.parent();
    }
    None
}

fn enclosing_definition(node: Node) -> Option<Node> {
    let mut parent = node.parent();
    while let Some(current) = parent {
        if matches!(current.kind(), "function_definition" | "class_definition") {
            return Some(current);
        }
        parent = current.parent();
    }
    None
}

/// The class whose body directly defines `function`, if it is a method.
fn enclosing_class(function: Node) -> Option<Node> {
    let mut parent = function.parent()?;
    if parent.kind() == "decorated_definition" {
        parent = parent.parent()?;
    }
    if parent.kind() != "block" {
        return None;
    }
    parent
        .parent()
        .filter(|class| class.kind() == "class_definition")
}

fn has_decorator(function: Node, src: &str, name: &str) -> bool {
    let Some(decorated) = function
        .parent()
        .filter(|parent| parent.kind() == "decorated_definition")
    else {
        return false;
    };
    let mut cursor = decorated.walk();
    let found = decorated.named_children(&mut cursor).any(|decorator| {
        decorator.kind() == "decorator"
            && code_children(decorator)
                .first()
                .and_then(|expression| last_segment(*expression, src))
                == Some(name)
    });
    found
}

/// The parameter's bound name, and whether it is a `*args`/`**kwargs` splat.
fn parameter_name<'a>(parameter: Node, src: &'a str) -> Option<(&'a str, bool)> {
    match parameter.kind() {
        "identifier" => Some((text(parameter, src), false)),
        "typed_parameter" => code_children(parameter)
            .first()
            .and_then(|inner| parameter_name(*inner, src)),
        "default_parameter" | "typed_default_parameter" => parameter
            .child_by_field_name("name")
            .and_then(|name| parameter_name(name, src)),
        "list_splat_pattern" | "dictionary_splat_pattern" => code_children(parameter)
            .first()
            .map(|name| (text(*name, src), true)),
        _ => None,
    }
}

fn parameter_annotation(parameter: Node) -> Option<Node> {
    matches!(
        parameter.kind(),
        "typed_parameter" | "typed_default_parameter"
    )
    .then(|| parameter.child_by_field_name("type"))
    .flatten()
}

/// The string literal that is the first statement of a function or class.
fn docstring(definition: Node) -> Option<Node> {
    let body = definition.child_by_field_name("body")?;
    let first = *code_children(body).first()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let string = *code_children(first).first()?;
    matches!(string.kind(), "string" | "concatenated_string").then_some(string)
}

/// The raw source between a string's quotes, unless it is a bytes or
/// f-string literal.
fn string_body<'a>(string: Node, src: &'a str) -> Option<&'a str> {
    string_body_range(string, src).map(|range| &src[range])
}

/// The byte range of [`string_body`] within `src`.
fn string_body_range(string: Node, src: &str) -> Option<std::ops::Range<usize>> {
    if string.kind() != "string" {
        return None;
    }
    let mut cursor = string.walk();
    let children = string.children(&mut cursor).collect::<Vec<_>>();
    let start = children.iter().find(|c| c.kind() == "string_start")?;
    let end = children.iter().rev().find(|c| c.kind() == "string_end")?;
    let prefix = text(*start, src).to_ascii_lowercase();
    if prefix.contains('b') || prefix.contains('f') {
        return None;
    }
    Some(start.end_byte()..end.start_byte())
}

fn is_empty_string(node: Node, src: &str) -> bool {
    let node = unparen(node);
    match node.kind() {
        "string" => string_body(node, src).is_some_and(str::is_empty),
        "concatenated_string" => code_children(node)
            .into_iter()
            .all(|part| is_empty_string(part, src)),
        _ => false,
    }
}

fn is_literal(node: Node) -> bool {
    let node = unparen(node);
    match node.kind() {
        "string"
        | "concatenated_string"
        | "integer"
        | "float"
        | "true"
        | "false"
        | "none"
        | "list"
        | "dictionary"
        | "tuple"
        | "set" => true,
        "unary_operator" => node
            .child_by_field_name("argument")
            .is_some_and(|argument| matches!(argument.kind(), "integer" | "float")),
        _ => false,
    }
}

fn keyword_argument<'t>(call: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    let arguments = call.child_by_field_name("arguments")?;
    code_children(arguments).into_iter().find_map(|argument| {
        (argument.kind() == "keyword_argument"
            && argument
                .child_by_field_name("name")
                .is_some_and(|key| text(key, src) == name))
        .then(|| argument.child_by_field_name("value"))
        .flatten()
    })
}

fn method<'t>(class_body: Node<'t>, name: &str, src: &str) -> Option<Node<'t>> {
    class_methods(class_body, src).remove(name)
}

fn class_methods<'t>(class_body: Node<'t>, src: &str) -> HashMap<String, Node<'t>> {
    let mut methods = HashMap::new();
    for child in code_children(class_body) {
        let function = if child.kind() == "decorated_definition" {
            child.child_by_field_name("definition")
        } else {
            Some(child)
        };
        if let Some(function) = function.filter(|f| f.kind() == "function_definition") {
            if let Some(name) = function.child_by_field_name("name") {
                methods.insert(text(name, src).to_string(), function);
            }
        }
    }
    methods
}

// Suppressions ------------------------------------------------------------

/// Suppressions are read from real comments on the finding's first line, so
/// `# noqa` inside a string literal does not hide anything.
fn suppressed(node: Node, src: &str, code: &str) -> bool {
    suppressed_on_row(node, node.start_position().row, src, code)
}

/// Whether a real comment on `row` of the tree containing `node` suppresses
/// `code`.
fn suppressed_on_row(node: Node, row: usize, src: &str, code: &str) -> bool {
    let mut root = node;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    let mut comments = Vec::new();
    collect_comments_on_row(root, row, src, &mut comments);
    comments
        .iter()
        .any(|comment| comment_suppresses(comment, code))
}

fn collect_comments_on_row<'a>(node: Node, row: usize, src: &'a str, out: &mut Vec<&'a str>) {
    if node.start_position().row > row || node.end_position().row < row {
        return;
    }
    if node.kind() == "comment" {
        out.push(text(node, src));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_comments_on_row(child, row, src, out);
    }
}

/// `# noqa` suppresses everything; `# noqa: A,B` and
/// `# colint: ignore[A,B]` suppress only the listed rule codes or names.
fn comment_suppresses(comment: &str, code: &str) -> bool {
    let noqa = static_regex!(r"(?i)#\s*noqa\b(?:\s*:\s*([\w\-]+(?:\s*,\s*[\w\-]+)*))?");
    let ignore = static_regex!(r"(?i)#\s*colint\s*:\s*ignore\s*\[([^\]]*)\]");
    let name = rule(code).name;
    let lists = |list: &str| {
        list.split(',').any(|entry| {
            let entry = entry.trim();
            entry.eq_ignore_ascii_case(code) || entry.eq_ignore_ascii_case(name)
        })
    };
    noqa.captures_iter(comment)
        .any(|captures| captures.get(1).map_or(true, |list| lists(list.as_str())))
        || ignore
            .captures_iter(comment)
            .any(|captures| lists(&captures[1]))
}

// Configuration -------------------------------------------------------------

fn enabled(code: &str, cfg: &Config, strict: bool) -> bool {
    strict || cfg.rules.get(code).copied().unwrap_or(true)
}

/// A path is test code when its file is `test_*.py`, `*_test.py`,
/// `conftest.py`, or it lives below a `test`, `tests`, or `testing` directory.
/// Substrings such as `latest.py` or `contest/` are not test code.
fn is_test_path(path: &Path) -> bool {
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = file.strip_suffix(".py").unwrap_or(&file);
    if stem.starts_with("test_")
        || stem.ends_with("_test")
        || matches!(stem, "test" | "tests" | "conftest")
    {
        return true;
    }
    path.parent().is_some_and(|parent| {
        parent.components().any(|component| {
            matches!(
                component
                    .as_os_str()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .as_str(),
                "test" | "tests" | "testing"
            )
        })
    })
}

// API-002 helpers -----------------------------------------------------------

/// Identifier nodes that read `name`, excluding attribute names
/// (`other.name`) and keyword names (`call(name=...)`).
fn references<'t>(scope: Node<'t>, name: &str, src: &str) -> Vec<Node<'t>> {
    let mut found = Vec::new();
    walk(scope, &mut |node| {
        if node.kind() != "identifier" || text(node, src) != name {
            return;
        }
        let Some(parent) = node.parent() else {
            found.push(node);
            return;
        };
        let field = |field| parent.child_by_field_name(field) == Some(node);
        let is_attribute_name = parent.kind() == "attribute" && field("attribute");
        let is_keyword_name = parent.kind() == "keyword_argument" && field("name");
        if !is_attribute_name && !is_keyword_name {
            found.push(node);
        }
    });
    found
}

/// The `name.method(...)` call when `reference` is its receiver.
fn query_call(reference: Node) -> Option<Node> {
    let attribute = reference
        .parent()
        .filter(|parent| parent.kind() == "attribute")?;
    if attribute.child_by_field_name("object") != Some(reference) {
        return None;
    }
    attribute.parent().filter(|call| {
        call.kind() == "call" && call.child_by_field_name("function") == Some(attribute)
    })
}

/// The function that directly consumes a queried value, either as an
/// argument (`consume(thing.value())`) or through a local
/// (`value = thing.value(); consume(value)`).
fn lofting_target(query: Node, body: Node, src: &str) -> Option<String> {
    let mut node = query;
    loop {
        let parent = node.parent()?;
        match parent.kind() {
            "parenthesized_expression" | "keyword_argument" => node = parent,
            "argument_list" => {
                return parent
                    .parent()
                    .and_then(|call| call.child_by_field_name("function"))
                    .and_then(|function| last_segment(function, src))
                    .map(str::to_string);
            }
            "assignment" if parent.child_by_field_name("right") == Some(node) => {
                let local = parent
                    .child_by_field_name("left")
                    .filter(|left| left.kind() == "identifier")?;
                return consumer_of(body, text(local, src), src);
            }
            _ => return None,
        }
    }
}

fn consumer_of(body: Node, local: &str, src: &str) -> Option<String> {
    let mut consumer = None;
    walk(body, &mut |node| {
        if consumer.is_some() || node.kind() != "call" {
            return;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let passes_local = code_children(arguments).into_iter().any(|argument| {
            let value = if argument.kind() == "keyword_argument" {
                argument.child_by_field_name("value")
            } else {
                Some(argument)
            };
            value.is_some_and(|value| {
                let value = unparen(value);
                value.kind() == "identifier" && text(value, src) == local
            })
        });
        if passes_local {
            consumer = node
                .child_by_field_name("function")
                .and_then(|function| last_segment(function, src))
                .map(str::to_string);
        }
    });
    consumer
}

fn join_human(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [item] => item.clone(),
        [first, second] => format!("{first} and {second}"),
        _ => format!(
            "{}, and {}",
            items[..items.len() - 1].join(", "),
            items.last().unwrap()
        ),
    }
}

/// A definition is private when its own name starts with `_`, or when it is a
/// method of a class whose name starts with `_`. This includes dunder members.
fn is_private_definition(function: Node, src: &str) -> bool {
    if function
        .child_by_field_name("name")
        .is_some_and(|name| text(name, src).starts_with('_'))
    {
        return true;
    }
    let mut parent = function.parent();
    while let Some(node) = parent {
        if node.kind() == "class_definition"
            && node
                .child_by_field_name("name")
                .is_some_and(|name| text(name, src).starts_with('_'))
        {
            return true;
        }
        parent = node.parent();
    }
    false
}

// API-003 helpers -----------------------------------------------------------

/// Return annotations of functions that are not methods, keyed by name.
fn local_return_types(root: Node, src: &str) -> HashMap<String, String> {
    return_types(root, src, false)
}

/// Return annotations of methods, keyed by method name.
fn local_method_return_types(root: Node, src: &str) -> HashMap<String, String> {
    return_types(root, src, true)
}

fn return_types(root: Node, src: &str, methods: bool) -> HashMap<String, String> {
    let mut x = HashMap::new();
    walk(root, &mut |n| {
        if n.kind() == "function_definition" && enclosing_class(n).is_some() == methods {
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

/// Removes whitespace, trailing commas, and one level of string quoting, so
/// `list[ str ]`, `list[str,]`, and `"list[str]"` compare equal.
fn normalize_annotation(annotation: &str) -> String {
    let compact = static_regex!(r"#[^\n]*")
        .replace_all(annotation, "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace(",]", "]");
    for quote in ['"', '\''] {
        if let Some(inner) = compact
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_string();
        }
    }
    compact
}

// API-004 / API-006 helpers -------------------------------------------------

fn annotation_allows_none(annotation: &str) -> bool {
    static_regex!(r"\bNone\b|\bOptional\s*\[").is_match(annotation)
}

/// Parameters and annotated locals of `function` whose annotation includes
/// `None` (`X | None`, `None | X`, `Optional[X]`, `Union[X, None]`).
fn optional_names(function: Node, src: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(parameters) = function.child_by_field_name("parameters") {
        for parameter in code_children(parameters) {
            if let (Some((name, false)), Some(annotation)) = (
                parameter_name(parameter, src),
                parameter_annotation(parameter),
            ) {
                if annotation_allows_none(text(annotation, src)) {
                    names.insert(name.to_string());
                }
            }
        }
    }
    if let Some(body) = function.child_by_field_name("body") {
        walk_scope(body, &mut |node| {
            if node.kind() != "assignment" {
                return;
            }
            if let (Some(left), Some(annotation)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("type"),
            ) {
                if left.kind() == "identifier" && annotation_allows_none(text(annotation, src)) {
                    names.insert(text(left, src).to_string());
                }
            }
        });
    }
    names
}

// DOC-001 helpers -----------------------------------------------------------

/// A parameter name in a docstring wrapped by a matching pair of markup
/// characters, e.g. `*bar*`; `offset` is the byte offset of the opening markup.
#[derive(Debug, PartialEq, Eq)]
struct WrappedReference<'a> {
    offset: usize,
    name: &'a str,
    open: &'a str,
    close: &'a str,
}

/// Characters that may wrap a parameter name to mark it up in a docstring.
fn is_variable_wrapper(c: char) -> bool {
    matches!(
        c,
        '*' | '"' | '\'' | '`' | '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}'
    )
}

fn is_identifier_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// The closing markup expected for an opening run, e.g. `**` for `**` and a
/// right curly quote for a left curly quote.
fn mirrored_wrapper(open: &str) -> String {
    open.chars()
        .rev()
        .map(|c| match c {
            '\u{201C}' => '\u{201D}',
            '\u{2018}' => '\u{2019}',
            other => other,
        })
        .collect()
}

/// Every reference to one of `names` in `docstring` that is wrapped in markup
/// other than `markup`. Bare names are ignored because they may not refer to
/// the parameter, as are Google-style `name:` entries, doctest examples, fenced
/// code blocks, and words embedded in an expression such as `2*x*3`. A
/// one-character name is only reported for `*x*`/`**x**` emphasis because a
/// quoted single character is usually a value, such as mode `'r'`.
fn wrapped_parameter_references<'a>(
    docstring: &'a str,
    names: &[&str],
    markup: &VariableMarkup,
) -> Vec<WrappedReference<'a>> {
    let start_run: String = {
        let run: Vec<char> = markup
            .start
            .chars()
            .rev()
            .take_while(|c| is_variable_wrapper(*c))
            .collect();
        run.into_iter().rev().collect()
    };
    let end_run: String = markup
        .end
        .chars()
        .take_while(|c| is_variable_wrapper(*c))
        .collect();
    let mut references = Vec::new();
    let mut in_fence = false;
    let mut in_doctest = false;
    let mut line_start = 0;
    for raw_line in docstring.split_inclusive('\n') {
        let offset = line_start;
        line_start += raw_line.len();
        let line = raw_line.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if trimmed.is_empty() {
            in_doctest = false;
            continue;
        }
        if trimmed.starts_with(">>>") {
            in_doctest = true;
        }
        if in_fence || in_doctest {
            continue;
        }
        let mut matches: Vec<_> = names
            .iter()
            .flat_map(|name| line.match_indices(*name))
            .collect();
        matches.sort_unstable();
        for (i, name) in matches {
            let before = &line[..i];
            let after = &line[i + name.len()..];
            if before.chars().next_back().is_some_and(is_identifier_char)
                || after.chars().next().is_some_and(is_identifier_char)
            {
                continue;
            }
            let open_len: usize = before
                .chars()
                .rev()
                .take_while(|c| is_variable_wrapper(*c))
                .map(char::len_utf8)
                .sum();
            let close_len: usize = after
                .chars()
                .take_while(|c| is_variable_wrapper(*c))
                .map(char::len_utf8)
                .sum();
            let open = &before[before.len() - open_len..];
            let close = &after[..close_len];
            if open.is_empty() || close != mirrored_wrapper(open) {
                continue;
            }
            let outside_before = before[..before.len() - open_len].chars().next_back();
            let outside_after = after[close_len..].chars().next();
            if outside_before.is_some_and(is_identifier_char)
                || outside_after.is_some_and(is_identifier_char)
            {
                continue;
            }
            if name.chars().count() == 1 && open.chars().any(|c| c != '*') {
                continue;
            }
            let configured = before.ends_with(markup.start.as_str())
                && after.starts_with(markup.end.as_str())
                && open == start_run
                && close == end_run;
            if configured {
                continue;
            }
            references.push(WrappedReference {
                offset: offset + i - open_len,
                name,
                open,
                close,
            });
        }
    }
    references
}

/// The row and byte column of `byte` in `src`, as Tree-sitter reports them.
fn point_at(src: &str, byte: usize) -> Point {
    let line_start = src[..byte].rfind('\n').map_or(0, |i| i + 1);
    Point {
        row: src[..byte].matches('\n').count(),
        column: byte - line_start,
    }
}

// DOC-002 helpers -----------------------------------------------------------

/// The exception names listed in a Google-style `Raises:` section, or `None`
/// when the docstring has no such section.
fn documented_raises(docstring: &str) -> Option<Vec<String>> {
    let lines = docstring.lines().collect::<Vec<_>>();
    let header = lines
        .iter()
        .position(|line| line.trim_start().starts_with("Raises:"))?;
    let indentation = |line: &str| line.len() - line.trim_start().len();
    let header_indentation = indentation(lines[header]);
    let entry = static_regex!(r"^([A-Za-z_][\w.]*)\s*(?::|$)");
    let mut entry_indentation = None;
    let mut names = Vec::new();
    // The inline form, `Raises: ValueError if ...`.
    let inline = lines[header].trim_start()["Raises:".len()..].trim();
    if let Some(name) = static_regex!(r"^([A-Z][\w.]*)").captures(inline) {
        let name = &name[1];
        names.push(name.rsplit('.').next().unwrap_or(name).to_string());
    }
    for line in &lines[header + 1..] {
        if line.trim().is_empty() {
            continue;
        }
        let current = indentation(line);
        if current <= header_indentation {
            break;
        }
        if current != *entry_indentation.get_or_insert(current) {
            continue;
        }
        if let Some(captures) = entry.captures(line.trim()) {
            let name = &captures[1];
            names.push(name.rsplit('.').next().unwrap_or(name).to_string());
        }
    }
    Some(names)
}

#[derive(Default)]
struct DirectRaises {
    /// The function has at least one direct `raise`.
    any: bool,
    /// A raise whose exception type cannot be determined statically.
    unknown: bool,
    names: HashSet<String>,
}

fn direct_raises(body: Node, src: &str) -> DirectRaises {
    let mut raises = DirectRaises::default();
    walk_scope(body, &mut |node| {
        if node.kind() != "raise_statement" {
            return;
        }
        raises.any = true;
        let exception = code_children(node).first().map(|n| unparen(*n));
        let resolved = match exception {
            None => handled_exceptions(node, None, src),
            Some(exception) => {
                let class = if exception.kind() == "call" {
                    exception.child_by_field_name("function")
                } else {
                    Some(exception)
                };
                match class.and_then(|class| last_segment(class, src)) {
                    Some(name) if name.starts_with(|c: char| c.is_ascii_uppercase()) => {
                        Some(vec![name.to_string()])
                    }
                    Some(name) => handled_exceptions(node, Some(name), src),
                    None => None,
                }
            }
        };
        match resolved {
            Some(names) => raises.names.extend(names),
            None => raises.unknown = true,
        }
    });
    raises
}

/// The exception types of the nearest enclosing `except` clause, for a bare
/// `raise` (`alias` is `None`) or `raise alias` of that clause's `as` name.
fn handled_exceptions(node: Node, alias: Option<&str>, src: &str) -> Option<Vec<String>> {
    let mut parent = node.parent();
    while let Some(current) = parent {
        if matches!(current.kind(), "function_definition" | "class_definition") {
            return None;
        }
        if current.kind() == "except_clause" {
            let body = code_children(current)
                .into_iter()
                .find(|child| child.kind() == "block")?;
            let header = src[current.start_byte()..body.start_byte()]
                .trim()
                .trim_start_matches("except")
                .trim_start_matches('*')
                .trim()
                .trim_end_matches(':');
            let (types, bound) = match header.rsplit_once(" as ") {
                Some((types, bound)) => (types, Some(bound.trim())),
                None => (header, None),
            };
            if alias.is_some() && alias != bound {
                return None;
            }
            let names = types
                .trim()
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split(',')
                .map(|name| name.trim().rsplit('.').next().unwrap_or("").to_string())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>();
            return (!names.is_empty()).then_some(names);
        }
        parent = current.parent();
    }
    None
}

fn local_class_bases(root: Node, src: &str) -> HashMap<String, Vec<String>> {
    let mut bases = HashMap::new();
    walk(root, &mut |node| {
        if node.kind() != "class_definition" {
            return;
        }
        if let (Some(name), Some(superclasses)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("superclasses"),
        ) {
            bases.insert(
                text(name, src).to_string(),
                class_base_names(superclasses, src),
            );
        }
    });
    bases
}

fn is_same_or_subclass(
    raised: &str,
    documented: &str,
    bases: &HashMap<String, Vec<String>>,
) -> bool {
    let mut pending = vec![raised.to_string()];
    let mut seen = HashSet::new();
    while let Some(name) = pending.pop() {
        if name == documented {
            return true;
        }
        if seen.insert(name.clone()) {
            pending.extend(bases.get(&name).into_iter().flatten().cloned());
        }
    }
    false
}

// STY-001 helpers -----------------------------------------------------------

/// An import is justified by a `# NOTE` comment block directly above it, or
/// above an earlier import in the same contiguous group. Blank lines and
/// other comments do not split a group; any other statement does.
fn nested_import_has_note(import: Node, src: &str) -> bool {
    let mut current = import;
    loop {
        if preceding_comments_have_note(current, src) {
            return true;
        }
        let mut previous = current.prev_named_sibling();
        while let Some(node) = previous.filter(|node| is_extra(*node)) {
            previous = node.prev_named_sibling();
        }
        match previous {
            Some(node) if matches!(node.kind(), "import_statement" | "import_from_statement") => {
                current = node;
            }
            _ => return false,
        }
    }
}

fn preceding_comments_have_note(node: Node, src: &str) -> bool {
    let note = static_regex!(r"(?i)^#+\s*note\b");
    src[..node.start_byte()]
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take_while(|line| line.starts_with('#'))
        .any(|line| note.is_match(line))
}

// STY-003 helpers -----------------------------------------------------------

const LOGGING_METHODS: &[&str] = &[
    "debug",
    "info",
    "warning",
    "warn",
    "error",
    "exception",
    "critical",
    "fatal",
    "log",
];

/// The message argument of a call such as `logger.info(message, ...)`,
/// `logging.log(level, message, ...)`, or `self._log.warning(message)`.
fn logging_message_argument<'t>(call: Node<'t>, src: &str) -> Option<Node<'t>> {
    let function = call.child_by_field_name("function")?;
    if function.kind() != "attribute" {
        return None;
    }
    let method = text(function.child_by_field_name("attribute")?, src);
    if !LOGGING_METHODS.contains(&method) {
        return None;
    }
    let object = unparen(function.child_by_field_name("object")?);
    let is_logger = match object.kind() {
        "call" => object
            .child_by_field_name("function")
            .and_then(|f| last_segment(f, src))
            .is_some_and(|name| name == "getLogger"),
        _ => last_segment(object, src).is_some_and(|name| {
            static_regex!(r"(?i)(?:^|_)(?:log|logger|logging)$").is_match(name)
        }),
    };
    if !is_logger {
        return None;
    }
    let arguments = call.child_by_field_name("arguments")?;
    let positional = code_children(arguments)
        .into_iter()
        .filter(|argument| argument.kind() != "keyword_argument")
        .collect::<Vec<_>>();
    positional
        .get(usize::from(method == "log"))
        .map(|argument| unparen(*argument))
}

/// Whether a `%s` placeholder (including `%(name)s` and `%-10s`) is not
/// wrapped in matching quotes. `%r` already shows quotes and `%%` is a literal.
fn has_unquoted_placeholder(body: &str) -> bool {
    let placeholder =
        static_regex!(r"^%(?:\([^)]*\))?[#0 +\-]*(?:\*|\d+)?(?:\.(?:\*|\d+))?([a-zA-Z])");
    let mut index = 0;
    while let Some(offset) = body[index..].find('%') {
        let start = index + offset;
        if body[start + 1..].starts_with('%') {
            index = start + 2;
            continue;
        }
        let Some(captures) = placeholder.captures(&body[start..]) else {
            index = start + 1;
            continue;
        };
        let end = start + captures[0].len();
        if &captures[1] == "s" {
            let before = body[..start].chars().next_back();
            let after = body[end..].trim_start_matches('\\').chars().next();
            let quoted = matches!(before, Some('"' | '\'')) && before == after;
            if !quoted {
                return true;
            }
        }
        index = end;
    }
    false
}

// STY-004 helpers -----------------------------------------------------------

fn is_main_guard(node: Node, src: &str) -> bool {
    if node.kind() != "if_statement" {
        return false;
    }
    let Some(condition) = node.child_by_field_name("condition") else {
        return false;
    };
    let condition = text(condition, src)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .replace('\'', "\"");
    matches!(
        condition.as_str(),
        "__name__==\"__main__\"" | "\"__main__\"==__name__"
    )
}

// GUI-001 helpers -----------------------------------------------------------

const WIDGET_SUFFIXES: &[&str] = &["Widget", "Button", "Label", "ComboBox"];

fn is_widget_class(name: &str, custom: &HashSet<String>) -> bool {
    custom.contains(name)
        || (name.starts_with(|c: char| c.is_ascii_uppercase())
            && WIDGET_SUFFIXES.iter().any(|suffix| name.ends_with(suffix)))
}

/// Classes defined in this module that derive, directly or through another
/// local class, from a `*Widget` base.
fn custom_widget_classes(root: Node, src: &str) -> HashSet<String> {
    let bases = local_class_bases(root, src);
    let mut classes = HashSet::new();
    loop {
        let before = classes.len();
        for (name, parents) in &bases {
            if parents
                .iter()
                .any(|parent| parent.ends_with("Widget") || classes.contains(parent))
            {
                classes.insert(name.clone());
            }
        }
        if classes.len() == before {
            return classes;
        }
    }
}

/// The source texts that may configure a widget constructed by `node`.
fn tooltip_scope<'a>(node: Node, src: &'a str) -> Vec<&'a str> {
    let Some(mut function) = enclosing_function(node) else {
        return vec![src];
    };
    while enclosing_class(function).is_none() {
        match enclosing_function(function) {
            Some(outer) => function = outer,
            None => return vec![text(function, src)],
        }
    }
    let Some(body) = enclosing_class(function).and_then(|c| c.child_by_field_name("body")) else {
        return vec![text(function, src)];
    };
    let methods = class_methods(body, src);
    let Some(name) = function
        .child_by_field_name("name")
        .map(|name| text(name, src))
    else {
        return vec![text(function, src)];
    };
    let mut scope = HashSet::new();
    for root in methods.keys() {
        let reachable = reachable_methods(root, &methods, src);
        if reachable.contains(name) {
            scope.extend(reachable);
        }
    }
    let mut names = scope.into_iter().collect::<Vec<_>>();
    names.sort();
    names
        .iter()
        .filter_map(|name| methods.get(name))
        .map(|method| text(*method, src))
        .collect()
}

/// Methods reachable from `start` through `self.method()` calls. The visited
/// set deliberately makes recursive helper graphs finite.
fn reachable_methods(start: &str, methods: &HashMap<String, Node>, src: &str) -> HashSet<String> {
    let mut reachable = HashSet::new();
    let mut pending = vec![start.to_string()];
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
    reachable
}

fn called_instance_methods(method: Node, src: &str) -> HashSet<String> {
    let mut calls = HashSet::new();
    let Some(body) = method.child_by_field_name("body") else {
        return calls;
    };
    walk_scope(body, &mut |node| {
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

// GUI-002 helpers -----------------------------------------------------------

/// Whether `__init__` accepts a parent (by name or through `*args`/`**kwargs`)
/// and passes it to `super().__init__(...)` or `Base.__init__(self, ...)`.
fn init_forwards_parent(init: Node, src: &str) -> bool {
    let Some(parameters) = init.child_by_field_name("parameters") else {
        return false;
    };
    let forwarded = code_children(parameters)
        .into_iter()
        .filter_map(|parameter| parameter_name(parameter, src))
        .filter(|(name, splat)| *splat || *name == "parent")
        .map(|(name, _)| name)
        .collect::<HashSet<_>>();
    if forwarded.is_empty() {
        return false;
    }
    let Some(body) = init.child_by_field_name("body") else {
        return false;
    };
    let mut forwards = false;
    walk_scope(body, &mut |node| {
        if forwards || node.kind() != "call" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        if last_segment(function, src) != Some("__init__") {
            return;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        forwards = code_children(arguments).into_iter().any(|argument| {
            let value = match argument.kind() {
                "keyword_argument" => argument.child_by_field_name("value"),
                "list_splat" | "dictionary_splat" => code_children(argument).first().copied(),
                _ => Some(argument),
            };
            value.is_some_and(|value| {
                let value = unparen(value);
                value.kind() == "identifier" && forwarded.contains(text(value, src))
            })
        });
    });
    forwards
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
            "import_from_statement" => aliases.extend(imported_aliases(node, src)),
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
            | "QConcatenateTablesProxyModel"
            | "QFileSystemModel"
            | "QIdentityProxyModel"
            | "QSortFilterProxyModel"
            | "QStandardItemModel"
            | "QStringListModel"
            | "QTransposeProxyModel"
    )
}

/// The final name of each positional base class (`pkg.Base[T]` -> `Base`).
fn class_base_names(superclasses: Node, src: &str) -> Vec<String> {
    code_children(superclasses)
        .into_iter()
        .filter_map(|base| {
            let base = if base.kind() == "subscript" {
                base.child_by_field_name("value")?
            } else {
                base
            };
            last_segment(base, src).map(str::to_string)
        })
        .collect()
}

/// `(original, alias)` pairs bound by a `from module import ...` statement.
fn imported_aliases(statement: Node, src: &str) -> Vec<(String, String)> {
    let mut cursor = statement.walk();
    statement
        .children_by_field_name("name", &mut cursor)
        .filter_map(|name| {
            let (original, alias) = if name.kind() == "aliased_import" {
                (
                    name.child_by_field_name("name")?,
                    name.child_by_field_name("alias")?,
                )
            } else {
                (name, name)
            };
            let original = text(original, src);
            let original = original.rsplit('.').next().unwrap_or(original);
            let alias = text(alias, src);
            Some((original.to_string(), alias.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod permutation_tests;

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
            code: "API-005".into(),
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
            code: "API-005".into(),
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
        assert!(with_header.contains("# colint: ignore[STY-001,API-005]"));
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
    import sys  # colint: ignore[STY-001]
"#;
        let codes = codes(source, "production.py");
        assert!(codes.contains(&"STY-001".to_string()));
        assert!(codes.contains(&"API-003".to_string()));
        assert!(codes.contains(&"API-004".to_string()));
        assert!(codes.contains(&"API-005".to_string()));
        assert_eq!(codes.iter().filter(|code| *code == "STY-001").count(), 1);
    }

    #[test]
    fn pytest_literals_belong_on_the_left() {
        let codes = codes(
            "def test_value():\n    assert actual == [\"expected\"]\n",
            "test_value.py",
        );
        assert!(codes.contains(&"STY-002".to_string()));
    }

    #[test]
    fn configuration_can_disable_a_rule_unless_strict() {
        let config = Config {
            warnings_as_errors: false,
            rules: HashMap::from([("API-005".to_string(), false)]),
            api002_skip_private_definitions: default_api002_skip_private_definitions(),
            import_paths: Vec::new(),
            docstring_variable_markup: VariableMarkup::default(),
        };
        let normal = analyze(
            Path::new("production.py"),
            "assert active\n",
            &config,
            false,
        );
        let strict = analyze(Path::new("production.py"), "assert active\n", &config, true);
        assert!(normal.is_empty());
        assert!(strict.iter().any(|finding| finding.code == "API-005"));
    }

    #[test]
    fn predicate_guard_is_reported() {
        assert!(codes(
            "def run(value):\n    if not value:\n        return\n    work()\n",
            "app.py"
        )
        .contains(&"API-001".to_string()));
    }

    #[test]
    fn lofting_single_query_is_reported() {
        assert!(codes(
            "def run(thing):\n    value = thing.get_value()\n    use(value)\n",
            "app.py"
        )
        .contains(&"API-002".to_string()));
    }

    #[test]
    fn lofting_messages_name_parameters_and_inferred_targets() {
        let one = findings(
            "def run(thing):\n    value = thing.get_value()\n    use(value)\n",
            "app.py",
        );
        assert_eq!(
            one.iter()
                .find(|finding| finding.code == "API-002")
                .unwrap()
                .message,
            "parameter `thing` is only queried once; loft its queried value to `use`"
        );

        let many = findings(
            "def run(left, right):\n    consume(left.value(), right.value())\n",
            "app.py",
        );
        assert_eq!(
            many.iter().find(|finding| finding.code == "API-002").unwrap().message,
            "parameters `left` and `right` are only queried once; loft their queried values to `consume`"
        );

        let unknown = findings("def run(thing):\n    thing.get_value()\n", "app.py");
        assert_eq!(
            unknown
                .iter()
                .find(|finding| finding.code == "API-002")
                .unwrap()
                .message,
            "parameter `thing` is only queried once; loft its queried value into the caller"
        );
    }

    #[test]
    fn api002_skips_private_definitions_by_default_and_can_be_enabled() {
        let source = r#"
def public(thing):
    thing.value()

def _private(thing):
    thing.value()

class _PrivateClass:
    def public_method(self, thing):
        thing.value()

class PublicClass:
    def _private_method(self, thing):
        thing.value()
"#;
        let default_findings = findings(source, "app.py");
        assert_eq!(
            default_findings
                .iter()
                .filter(|finding| finding.code == "API-002")
                .count(),
            1
        );

        let config = Config {
            api002_skip_private_definitions: false,
            ..Config::default()
        };
        assert_eq!(
            analyze(Path::new("app.py"), source, &config, false)
                .iter()
                .filter(|finding| finding.code == "API-002")
                .count(),
            4
        );
        assert!(
            toml::from_str::<Config>("")
                .unwrap()
                .api002_skip_private_definitions
        );
    }

    #[test]
    fn nested_import_requires_note_but_accepts_multiline_note() {
        let bad = codes("def run():\n    import os\n", "app.py");
        let good = codes("def run():\n    # NOTE: platform-dependent import\n    # kept local to avoid startup cost\n    import os\n", "app.py");
        assert!(bad.contains(&"STY-001".to_string()));
        assert!(!good.contains(&"STY-001".to_string()));
    }

    #[test]
    fn nested_import_notes_cover_groups_and_multiline_import_styles() {
        let fixture = include_str!("../imports_example.py");
        assert!(!codes(fixture, "imports_example.py").contains(&"STY-001".to_string()));

        let source = r#"
def run():
    # NOTE: optional dependencies stay local.
    from package import (
        alpha,
        beta,
    )

    from other_package import gamma, \\
        delta
"#;
        assert!(!codes(source, "app.py").contains(&"STY-001".to_string()));
    }

    #[test]
    fn docstrings_reject_asterisk_parameter_markup() {
        assert!(codes(
            "def create(task):\n    \"\"\"Create *task*.\"\"\"\n",
            "app.py"
        )
        .contains(&"DOC-001".to_string()));
    }

    fn doc001_messages(source: &str, config: &Config) -> Vec<String> {
        analyze(Path::new("app.py"), source, config, false)
            .into_iter()
            .filter(|finding| finding.code == "DOC-001")
            .map(|finding| format!("{}:{} {}", finding.line, finding.column, finding.message))
            .collect()
    }

    #[test]
    fn docstring_variable_markup_flags_wrapped_parameters_only() {
        let source = r#"def foo(bar: int) -> None:
    """Get *bar* and print it.

    Args:
        bar: Something something "bar" and *bar* and bar.

    """
    print(bar)
"#;
        assert_eq!(
            vec![
                "2:12 parameter bar is written as *bar*; use `bar`",
                "5:34 parameter bar is written as \"bar\"; use `bar`",
                "5:44 parameter bar is written as *bar*; use `bar`",
            ],
            doc001_messages(source, &Config::default())
        );
    }

    #[test]
    fn docstring_variable_markup_flags_other_wrappers_and_splat_names() {
        let source = "def foo(bar, *args, mode='r', **kwargs):\n    \"\"\"Use ``bar``, 'mode', *args*, and **kwargs**.\"\"\"\n";
        assert_eq!(
            vec![
                "2:12 parameter bar is written as ``bar``; use `bar`",
                "2:21 parameter mode is written as 'mode'; use `mode`",
                "2:29 parameter args is written as *args*; use `args`",
                "2:41 parameter kwargs is written as **kwargs**; use `kwargs`",
            ],
            doc001_messages(source, &Config::default())
        );
    }

    #[test]
    fn docstring_variable_markup_is_configurable() {
        let config: Config =
            toml::from_str("[docstring_variable_markup]\nstart = \"``\"\nend = \"``\"\n").unwrap();
        let source = "def foo(bar, baz):\n    \"\"\"Use ``bar`` and `baz`.\"\"\"\n";
        assert_eq!(
            vec!["2:24 parameter baz is written as `baz`; use ``baz``"],
            doc001_messages(source, &config)
        );
        // `docstring_convention` was removed; existing configs must still load.
        let legacy: Config = toml::from_str("docstring_convention = \"google\"\n").unwrap();
        assert_eq!(VariableMarkup::default(), legacy.docstring_variable_markup);
    }

    #[test]
    fn docstring_variable_markup_suppressions_cover_the_whole_docstring() {
        let multiline = "def foo(bar):\n    \"\"\"Use *bar*.\n\n    And \"bar\".\n    \"\"\"  # noqa: DOC-001\n";
        assert!(doc001_messages(multiline, &Config::default()).is_empty());
        let on_def =
            "def foo(bar):  # colint: ignore[docstring-convention]\n    \"\"\"Use *bar*.\"\"\"\n";
        assert!(doc001_messages(on_def, &Config::default()).is_empty());
    }

    #[test]
    fn quoted_logging_placeholder_is_allowed() {
        let bad = codes("logger.error('Could not load %s', name)\n", "app.py");
        let good = codes("logger.error('Could not load \"%s\"', name)\n", "app.py");
        assert!(bad.contains(&"STY-003".to_string()));
        assert!(!good.contains(&"STY-003".to_string()));
    }

    #[test]
    fn module_assignment_after_function_is_reported() {
        assert!(codes("def make():\n    pass\n\nVALUE = 1\n", "app.py")
            .contains(&"STY-004".to_string()));
    }

    #[test]
    fn empty_return_requires_none() {
        assert!(codes("def find() -> str | int:\n    return ''\n", "app.py")
            .contains(&"API-004".to_string()));
    }

    #[test]
    fn asserts_are_allowed_in_test_files_only() {
        assert!(codes("assert ready\n", "app.py").contains(&"API-005".to_string()));
        assert!(!codes("assert ready\n", "test_app.py").contains(&"API-005".to_string()));
    }

    #[test]
    fn optional_none_comparisons_need_optional_annotation() {
        assert!(codes(
            "def run(value: Thing | None):\n    if value is not None:\n        use(value)\n",
            "app.py"
        )
        .contains(&"API-006".to_string()));
        assert!(!codes(
            "def run(value: Thing):\n    if value is not None:\n        use(value)\n",
            "app.py"
        )
        .contains(&"API-006".to_string()));
    }

    #[test]
    fn indirect_raises_documentation_is_reported() {
        assert!(codes("def run():\n    \"\"\"Run.\n\n    Raises:\n        ValueError: When broken.\n    \"\"\"\n    dependency()\n", "app.py").contains(&"DOC-002".to_string()));
        assert!(!codes("def run():\n    \"\"\"Run.\n\n    Raises:\n        ValueError: When broken.\n    \"\"\"\n    raise ValueError()\n", "app.py").contains(&"DOC-002".to_string()));
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
        assert!(missing.contains(&"GUI-001".to_string()));
        assert!(empty.contains(&"GUI-001".to_string()));
    }

    #[test]
    fn custom_widget_definitions_are_not_instances_but_instances_need_tooltips() {
        let fixture = include_str!("../tests/fixtures/tip.txt");
        let fixture_findings = findings(fixture, "tip.txt");
        let fixture_codes = fixture_findings
            .iter()
            .map(|finding| format!("{}:{}: {}", finding.code, finding.line, finding.message))
            .collect::<Vec<_>>();
        assert!(
            !fixture_findings
                .iter()
                .any(|finding| finding.code == "GUI-001"),
            "unexpected findings: {fixture_codes:?}"
        );

        let missing = r#"
class CustomWidget(QtWidgets.QWidget):
    def __init__(self):
        self.label = QtWidgets.QLabel()
        self.label.setToolTip("Label")

widget = CustomWidget()
"#;
        assert!(codes(missing, "app.py").contains(&"GUI-001".to_string()));
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
        assert!(!codes(source, "app.py").contains(&"GUI-001".to_string()));
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
        assert!(!codes(source, "app.py").contains(&"GUI-001".to_string()));
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
        assert!(codes(source, "app.py").contains(&"GUI-001".to_string()));
    }

    #[test]
    fn qt_model_parent_is_required_for_construction() {
        let bad = codes("model = QStandardItemModel()\n", "app.py");
        let good = codes("model = QStandardItemModel(parent=parent)\n", "app.py");
        assert!(bad.contains(&"GUI-002".to_string()));
        assert!(!good.contains(&"GUI-002".to_string()));
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
        assert!(findings.iter().any(|finding| finding.code == "GUI-002"));
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
        assert!(findings.iter().any(|finding| finding.code == "GUI-002"));
    }

    #[test]
    fn noqa_and_selected_suppressions_do_not_overreach() {
        assert!(codes(
            "def run():\n    import os  # colint: ignore[API-005]\n",
            "app.py"
        )
        .contains(&"STY-001".to_string()));
        assert!(!codes("assert value  # noqa\n", "app.py").contains(&"API-005".to_string()));
    }

    #[test]
    fn ignore_accepts_rule_names_alongside_codes() {
        assert!(!codes(
            "def run():\n    import os  # colint: ignore[nested-import]\n",
            "app.py"
        )
        .contains(&"STY-001".to_string()));
        assert!(!codes(
            "def run():\n    import os  # colint: ignore[nested-import,API-005]\n",
            "app.py"
        )
        .contains(&"STY-001".to_string()));
    }

    #[test]
    fn nested_function_raises_do_not_justify_outer_raises_docstring() {
        let source = "def outer():\n    \"\"\"Outer.\n\n    Raises:\n        ValueError: Never directly raised.\n    \"\"\"\n    def inner():\n        raise ValueError()\n";
        assert!(codes(source, "app.py").contains(&"DOC-002".to_string()));
    }

    #[test]
    fn strict_reenables_disabled_warning_rules() {
        let config = Config {
            warnings_as_errors: false,
            rules: HashMap::from([("API-001".to_string(), false)]),
            api002_skip_private_definitions: default_api002_skip_private_definitions(),
            import_paths: Vec::new(),
            docstring_variable_markup: VariableMarkup::default(),
        };
        let source = "def run(value):\n    if not value:\n        return\n    work()\n";
        assert!(!analyze(Path::new("app.py"), source, &config, false)
            .iter()
            .any(|finding| finding.code == "API-001"));
        assert!(analyze(Path::new("app.py"), source, &config, true)
            .iter()
            .any(|finding| finding.code == "API-001"));
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
        assert_eq!(codes.iter().filter(|code| *code == "STY-001").count(), 2);
        assert!(codes.contains(&"STY-003".to_string()));
    }
}
