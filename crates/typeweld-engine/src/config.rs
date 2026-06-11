//! `typeweld.toml` project configuration.
//!
//! Lives next to the workspace root `Cargo.toml`:
//!
//! ```toml
//! [[package]]
//! cargo = "server"
//! ts = "@workspace/server-api"
//!
//! [generate]
//! out-dir = "target/typeweld/packages"   # default
//!
//! [app]
//! src = ["app/src/**/*.ts"]              # scanned for usage lints
//!
//! [lint]
//! unused-endpoints = "warn"              # off | warn | deny
//! ```

use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::vfs::FileProvider;

/// One generated package: a cargo package and its TS package name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageConfig {
    pub cargo: String,
    pub ts: String,
}

/// Unused-endpoint lint level.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LintLevel {
    Off,
    #[default]
    Warn,
    Deny,
}

/// Parsed `typeweld.toml`.
#[derive(Clone, Debug)]
pub struct Config {
    pub packages: Vec<PackageConfig>,
    /// Workspace-root-relative output directory for generated packages.
    pub out_dir: PathBuf,
    /// Globs (workspace-root-relative) of app TS sources for usage lints.
    pub app_src: Vec<String>,
    pub unused_endpoints: LintLevel,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            packages: Vec::new(),
            out_dir: PathBuf::from("target/typeweld/packages"),
            app_src: Vec::new(),
            unused_endpoints: LintLevel::default(),
        }
    }
}

impl Config {
    /// The directory a package's generated files are written to,
    /// relative to the workspace root.
    #[must_use]
    pub fn package_dir(&self, ts_package: &str) -> PathBuf {
        let mut dir = self.out_dir.clone();
        for segment in ts_package.split('/') {
            dir.push(segment);
        }
        dir
    }

    /// Loads `typeweld.toml` from the workspace root.
    ///
    /// # Errors
    /// Returns a diagnostic when the file is missing or invalid.
    pub fn load(root: &Path, files: &dyn FileProvider) -> Result<Self, Diagnostic> {
        let path = root.join("typeweld.toml");
        let Some(source) = files.read(&path) else {
            return Err(Diagnostic::error(
                "missing-config",
                format!(
                    "no typeweld.toml found at {}; run `typeweld new` or create one with a \
                     [[package]] entry",
                    path.display()
                ),
                None,
            ));
        };
        Self::parse(&source)
    }

    /// Parses `typeweld.toml` contents.
    ///
    /// # Errors
    /// Returns a diagnostic for malformed or incomplete configuration.
    pub fn parse(source: &str) -> Result<Self, Diagnostic> {
        let value: toml::Value = toml::from_str(source).map_err(|error| {
            Diagnostic::error(
                "invalid-config",
                format!("invalid typeweld.toml: {error}"),
                None,
            )
        })?;

        let mut config = Self::default();

        if let Some(packages) = value.get("package").and_then(toml::Value::as_array) {
            for entry in packages {
                let cargo = entry.get("cargo").and_then(toml::Value::as_str);
                let ts = entry.get("ts").and_then(toml::Value::as_str);
                match (cargo, ts) {
                    (Some(cargo), Some(ts)) => config.packages.push(PackageConfig {
                        cargo: cargo.to_owned(),
                        ts: ts.to_owned(),
                    }),
                    _ => {
                        return Err(Diagnostic::error(
                            "invalid-config",
                            "each [[package]] needs `cargo = \"...\"` and `ts = \"...\"`",
                            None,
                        ));
                    }
                }
            }
        }
        if config.packages.is_empty() {
            return Err(Diagnostic::error(
                "invalid-config",
                "typeweld.toml declares no [[package]] entries",
                None,
            ));
        }

        if let Some(out_dir) = value
            .get("generate")
            .and_then(|section| section.get("out-dir"))
            .and_then(toml::Value::as_str)
        {
            config.out_dir = PathBuf::from(out_dir);
        }

        if let Some(src) = value
            .get("app")
            .and_then(|section| section.get("src"))
            .and_then(toml::Value::as_array)
        {
            config.app_src = src
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_owned)
                .collect();
        }

        if let Some(level) = value
            .get("lint")
            .and_then(|section| section.get("unused-endpoints"))
            .and_then(toml::Value::as_str)
        {
            config.unused_endpoints = match level {
                "off" => LintLevel::Off,
                "warn" => LintLevel::Warn,
                "deny" => LintLevel::Deny,
                other => {
                    return Err(Diagnostic::error(
                        "invalid-config",
                        format!("unknown lint level `{other}`; expected off, warn, or deny"),
                        None,
                    ));
                }
            };
        }

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_config() {
        let config = Config::parse(
            r#"
[[package]]
cargo = "server"
ts = "@workspace/server-api"

[generate]
out-dir = "generated"

[app]
src = ["app/src/**/*.ts"]

[lint]
unused-endpoints = "deny"
"#,
        )
        .expect("parse");
        assert_eq!(config.packages.len(), 1);
        assert_eq!(config.out_dir, PathBuf::from("generated"));
        assert_eq!(config.unused_endpoints, LintLevel::Deny);
        assert_eq!(
            config.package_dir("@workspace/server-api"),
            PathBuf::from("generated/@workspace/server-api")
        );
    }

    #[test]
    fn rejects_empty_and_malformed_configs() {
        assert!(Config::parse("").is_err());
        assert!(Config::parse("[[package]]\ncargo = \"x\"\n").is_err());
        assert!(Config::parse(
            "[[package]]\ncargo = \"x\"\nts = \"y\"\n[lint]\nunused-endpoints = \"loud\"\n"
        )
        .is_err());
    }
}
