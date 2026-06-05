//! Build-script helpers for API usage lints.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde::Deserialize;

/// Unused endpoint build lint policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsageLintLevel {
    Off,
    #[default]
    Warn,
    Deny,
}

/// Paths and policy used by the build usage lint bridge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageLintConfig {
    pub usage_index: PathBuf,
    pub symbol_graph: PathBuf,
    pub lint_level: UsageLintLevel,
    pub generated_lint_path: PathBuf,
}

impl UsageLintConfig {
    #[must_use]
    pub fn from_env() -> Self {
        let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let out_dir = env::var_os("OUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| manifest_dir.join("target/api-contract/build"));

        Self {
            usage_index: env_path("API_USAGE_INDEX").unwrap_or_else(|| {
                manifest_dir.join("target/api-contract/graph/effect-usage-index.json")
            }),
            symbol_graph: env_path("API_SYMBOL_GRAPH")
                .unwrap_or_else(|| manifest_dir.join("target/api-contract/rust-ts-symbols.json")),
            lint_level: env::var("API_UNUSED_ENDPOINTS")
                .ok()
                .and_then(|value| UsageLintLevel::parse(&value))
                .unwrap_or_default(),
            generated_lint_path: out_dir.join("api_usage_lints.rs"),
        }
    }
}

impl UsageLintLevel {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" | "Off" | "OFF" => Some(Self::Off),
            "warn" | "Warn" | "WARN" => Some(Self::Warn),
            "deny" | "Deny" | "DENY" => Some(Self::Deny),
            _ => None,
        }
    }
}

/// Result of evaluating usage lints.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsageLintReport {
    pub diagnostics: Vec<UsageLintDiagnostic>,
    pub stale: bool,
}

/// One build-facing usage lint diagnostic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsageLintDiagnostic {
    pub endpoint_id: Option<String>,
    pub route: Option<String>,
    pub accessor: Option<String>,
    pub message: String,
}

/// Error returned when usage lint inputs cannot be read or parsed.
#[derive(Debug)]
pub struct UsageLintError {
    message: String,
}

impl UsageLintError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UsageLintError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for UsageLintError {}

#[must_use]
pub const fn usage_lints_available() -> bool {
    true
}

/// Emits Cargo build warnings and generated lint glue using environment-derived paths.
pub fn emit_usage_lints() {
    let config = UsageLintConfig::from_env();
    if let Err(error) = emit_usage_lints_with_config(&config) {
        println!("cargo:warning=api usage lint bridge failed: {error}");
        let _ = write_lint_glue(
            &config.generated_lint_path,
            UsageLintLevel::Warn,
            &[UsageLintDiagnostic {
                endpoint_id: None,
                route: None,
                accessor: None,
                message: format!("api usage lint bridge failed: {error}"),
            }],
        );
    }
}

/// Evaluates usage lints, prints build warnings, and writes generated lint glue.
pub fn emit_usage_lints_with_config(
    config: &UsageLintConfig,
) -> Result<UsageLintReport, UsageLintError> {
    let report = check_usage_lints(config)?;

    for diagnostic in &report.diagnostics {
        if matches!(
            config.lint_level,
            UsageLintLevel::Warn | UsageLintLevel::Deny
        ) {
            println!("cargo:warning={}", diagnostic.message);
        }
    }
    write_lint_glue(
        &config.generated_lint_path,
        config.lint_level,
        &report.diagnostics,
    )?;

    Ok(report)
}

/// Reads the usage index and symbol graph and returns build-facing diagnostics.
pub fn check_usage_lints(config: &UsageLintConfig) -> Result<UsageLintReport, UsageLintError> {
    if matches!(config.lint_level, UsageLintLevel::Off) {
        return Ok(UsageLintReport::default());
    }

    if !config.usage_index.is_file() {
        return Ok(UsageLintReport {
            diagnostics: vec![UsageLintDiagnostic {
                endpoint_id: None,
                route: None,
                accessor: None,
                message: format!(
                    "missing Effect usage index `{}`; run `api check-usages` or start api-ls to generate target/api-contract/graph/effect-usage-index.json",
                    config.usage_index.display()
                ),
            }],
            stale: false,
        });
    }
    if !config.symbol_graph.is_file() {
        return Ok(UsageLintReport {
            diagnostics: vec![UsageLintDiagnostic {
                endpoint_id: None,
                route: None,
                accessor: None,
                message: format!(
                    "missing API symbol graph `{}`; regenerate the API contract before checking endpoint usage",
                    config.symbol_graph.display()
                ),
            }],
            stale: false,
        });
    }

    let usage_index = read_json::<EffectUsageIndex>(&config.usage_index, "usage index")?;
    let symbol_graph = read_json::<SymbolGraph>(&config.symbol_graph, "symbol graph")?;
    let stale = is_stale(&config.usage_index, &config.symbol_graph);
    let mut diagnostics = Vec::new();

    if stale {
        diagnostics.push(UsageLintDiagnostic {
            endpoint_id: None,
            route: None,
            accessor: None,
            message: format!(
                "stale Effect usage index `{}`; regenerate it after the newer symbol graph `{}`",
                config.usage_index.display(),
                config.symbol_graph.display()
            ),
        });
    }

    for symbol in symbol_graph.symbols {
        if symbol.kind != "endpoint" || symbol.metadata.allow_unused {
            continue;
        }
        if usage_index.strong_usage_count(&symbol.id) > 0 {
            continue;
        }

        let route = symbol.metadata.route_label();
        let accessor = symbol.metadata.accessor_label();
        diagnostics.push(UsageLintDiagnostic {
            endpoint_id: Some(symbol.id),
            route: Some(route.clone()),
            accessor: Some(accessor.clone()),
            message: format!(
                "warning[api::unused_endpoint]: endpoint is exported but has no strong TypeScript usages; route: {route}; generated accessor: {accessor}; help: call it from Effect code, mark #[api(allow_unused)], or remove it"
            ),
        });
    }

    diagnostics.sort_by(|left, right| {
        left.route
            .cmp(&right.route)
            .then_with(|| left.accessor.cmp(&right.accessor))
            .then_with(|| left.message.cmp(&right.message))
    });

    Ok(UsageLintReport { diagnostics, stale })
}

/// Includes generated deny-mode lint glue emitted by [`emit_usage_lints`].
#[macro_export]
macro_rules! include_usage_lints {
    () => {
        include!(concat!(env!("OUT_DIR"), "/api_usage_lints.rs"));
    };
}

fn write_lint_glue(
    path: &Path,
    lint_level: UsageLintLevel,
    diagnostics: &[UsageLintDiagnostic],
) -> Result<(), UsageLintError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            UsageLintError::new(format!(
                "failed to create generated lint directory `{}`: {error}",
                parent.display()
            ))
        })?;
    }

    let mut output = String::new();
    output.push_str("// Generated by api-build. Do not edit.\n");
    if matches!(lint_level, UsageLintLevel::Deny) {
        for diagnostic in diagnostics {
            output.push_str("compile_error!(");
            output.push_str(&rust_string(&diagnostic.message));
            output.push_str(");\n");
        }
    }

    fs::write(path, output).map_err(|error| {
        UsageLintError::new(format!(
            "failed to write generated lint glue `{}`: {error}",
            path.display()
        ))
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path, label: &str) -> Result<T, UsageLintError> {
    let contents = fs::read_to_string(path).map_err(|error| {
        UsageLintError::new(format!(
            "failed to read {label} `{}`: {error}",
            path.display()
        ))
    })?;
    serde_json::from_str(&contents).map_err(|error| {
        UsageLintError::new(format!(
            "failed to parse {label} `{}`: {error}",
            path.display()
        ))
    })
}

fn is_stale(usage_index: &Path, symbol_graph: &Path) -> bool {
    let usage_modified = modified_at(usage_index);
    let graph_modified = modified_at(symbol_graph);

    matches!((usage_modified, graph_modified), (Some(usage), Some(graph)) if usage < graph)
}

fn modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn rust_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serializes")
}

#[derive(Clone, Debug, Deserialize)]
struct EffectUsageIndex {
    #[serde(default)]
    endpoints: Vec<EndpointUsageSummary>,
}

impl EffectUsageIndex {
    fn strong_usage_count(&self, endpoint_id: &str) -> u64 {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .map_or(0, |endpoint| endpoint.strong)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct EndpointUsageSummary {
    endpoint_id: String,
    #[serde(default)]
    strong: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct SymbolGraph {
    #[serde(default)]
    symbols: Vec<LinkedSymbol>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkedSymbol {
    id: String,
    kind: String,
    #[serde(default)]
    metadata: SymbolMetadata,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SymbolMetadata {
    method: Option<String>,
    route: Option<String>,
    ts_path: Vec<String>,
    allow_unused: bool,
}

impl SymbolMetadata {
    fn route_label(&self) -> String {
        match (&self.method, &self.route) {
            (Some(method), Some(route)) => format!("{method} {route}"),
            (Some(method), None) => format!("{method} <unknown route>"),
            (None, Some(route)) => route.clone(),
            (None, None) => "<unknown route>".to_owned(),
        }
    }

    fn accessor_label(&self) -> String {
        if self.ts_path.is_empty() {
            "<unknown accessor>".to_owned()
        } else {
            self.ts_path.join(".")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        thread,
        time::Duration,
    };

    use serde_json::json;

    use super::*;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reports_unused_endpoints_and_skips_strong_or_allowed_endpoints() {
        let root = test_root("unused");
        let config = write_inputs(
            &root,
            UsageLintLevel::Warn,
            json!({
                "symbols": [
                    endpoint_symbol("endpoint:unused", "GET", "/unused", ["api", "unused"], false),
                    endpoint_symbol("endpoint:used", "GET", "/used", ["api", "used"], false),
                    endpoint_symbol("endpoint:allowed", "POST", "/admin/reindex", ["admin", "reindex"], true)
                ]
            }),
            json!({
                "endpoints": [
                    { "endpoint_id": "endpoint:unused", "accessor_path": ["api", "unused"], "strong": 0 },
                    { "endpoint_id": "endpoint:used", "accessor_path": ["api", "used"], "strong": 1 },
                    { "endpoint_id": "endpoint:allowed", "accessor_path": ["admin", "reindex"], "strong": 0 }
                ]
            }),
        );

        let report = check_usage_lints(&config).expect("check usage lints");

        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0].message.contains("GET /unused"));
        assert!(report.diagnostics[0].message.contains("api.unused"));
        assert!(!report.diagnostics[0].message.contains("/used"));
        assert!(!report.diagnostics[0].message.contains("/admin/reindex"));
    }

    #[test]
    fn writes_deny_mode_compile_error_glue() {
        let root = test_root("deny");
        let config = write_inputs(
            &root,
            UsageLintLevel::Deny,
            json!({
                "symbols": [
                    endpoint_symbol("endpoint:unused", "GET", "/unused", ["api", "unused"], false)
                ]
            }),
            json!({
                "endpoints": [
                    { "endpoint_id": "endpoint:unused", "accessor_path": ["api", "unused"], "strong": 0 }
                ]
            }),
        );

        let report = emit_usage_lints_with_config(&config).expect("emit usage lints");
        let glue = fs::read_to_string(&config.generated_lint_path).expect("read glue");

        assert_eq!(report.diagnostics.len(), 1);
        assert!(glue.contains("compile_error!"));
        assert!(glue.contains("GET /unused"));
        assert!(glue.contains("api.unused"));
    }

    #[test]
    fn missing_usage_index_reports_actionable_instruction() {
        let root = test_root("missing-index");
        fs::create_dir_all(&root).expect("create root");
        let config = UsageLintConfig {
            usage_index: root.join("missing/effect-usage-index.json"),
            symbol_graph: root.join("rust-ts-symbols.json"),
            lint_level: UsageLintLevel::Warn,
            generated_lint_path: root.join("out/api_usage_lints.rs"),
        };

        let report = check_usage_lints(&config).expect("check usage lints");

        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0]
            .message
            .contains("missing Effect usage index"));
        assert!(report.diagnostics[0].message.contains("api check-usages"));
    }

    #[test]
    fn detects_stale_usage_index() {
        let root = test_root("stale");
        let config = write_inputs(
            &root,
            UsageLintLevel::Warn,
            json!({ "symbols": [] }),
            json!({ "endpoints": [] }),
        );

        thread::sleep(Duration::from_millis(5));
        fs::write(&config.symbol_graph, json!({ "symbols": [] }).to_string())
            .expect("touch symbol graph");
        let report = check_usage_lints(&config).expect("check usage lints");

        assert!(report.stale);
        assert_eq!(report.diagnostics.len(), 1);
        assert!(report.diagnostics[0]
            .message
            .contains("stale Effect usage index"));
    }

    #[test]
    fn off_mode_suppresses_diagnostics() {
        let root = test_root("off");
        let config = write_inputs(
            &root,
            UsageLintLevel::Off,
            json!({
                "symbols": [
                    endpoint_symbol("endpoint:unused", "GET", "/unused", ["api", "unused"], false)
                ]
            }),
            json!({ "endpoints": [] }),
        );

        let report = check_usage_lints(&config).expect("check usage lints");

        assert!(report.diagnostics.is_empty());
    }

    fn write_inputs(
        root: &Path,
        lint_level: UsageLintLevel,
        symbol_graph: serde_json::Value,
        usage_index: serde_json::Value,
    ) -> UsageLintConfig {
        fs::create_dir_all(root.join("target/api-contract/graph")).expect("create graph dir");
        let symbol_graph_path = root.join("target/api-contract/rust-ts-symbols.json");
        let usage_index_path = root.join("target/api-contract/graph/effect-usage-index.json");
        fs::write(&symbol_graph_path, symbol_graph.to_string()).expect("write symbol graph");
        fs::write(&usage_index_path, usage_index.to_string()).expect("write usage index");

        UsageLintConfig {
            usage_index: usage_index_path,
            symbol_graph: symbol_graph_path,
            lint_level,
            generated_lint_path: root.join("out/api_usage_lints.rs"),
        }
    }

    fn endpoint_symbol(
        id: &str,
        method: &str,
        route: &str,
        ts_path: [&str; 2],
        allow_unused: bool,
    ) -> serde_json::Value {
        json!({
            "id": id,
            "kind": "endpoint",
            "metadata": {
                "method": method,
                "route": route,
                "tsPath": ts_path,
                "allowUnused": allow_unused
            }
        })
    }

    fn test_root(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("api-build-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }
}
