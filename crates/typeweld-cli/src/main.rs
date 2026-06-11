//! The typeweld CLI.

mod scaffold;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use typeweld_engine::config::{Config, LintLevel};
use typeweld_engine::diag::{Diagnostic, Severity};
use typeweld_engine::extract::extract;
use typeweld_engine::gen::{generate, write_package};
use typeweld_engine::usage;
use typeweld_engine::vfs::DiskFileProvider;
use typeweld_engine::workspace;

#[derive(Parser)]
#[command(
    name = "typeweld",
    version,
    about = "Rust-to-TypeScript API tooling for Effect clients"
)]
struct Cli {
    /// Workspace root (directory containing Cargo.toml and typeweld.toml).
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold a new typeweld project.
    New {
        /// Project name (also the directory to create).
        name: String,
    },
    /// Generate TypeScript bindings for every configured package.
    Generate {
        /// Keep watching the workspace and regenerate on change.
        #[arg(long)]
        watch: bool,
        /// Also write the extracted contract JSON next to each package.
        #[arg(long)]
        contract_json: bool,
    },
    /// Check the workspace: extraction diagnostics and optional usage lints.
    Check {
        /// Lint for endpoints with no TypeScript usage.
        #[arg(long)]
        unused: bool,
    },
    /// Run the typeweld language server on stdio.
    Lsp,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = cli
        .root
        .or_else(find_workspace_root)
        .unwrap_or_else(|| PathBuf::from("."));

    let result = match cli.command {
        Command::New { name } => scaffold::new_project(&name).map(|()| false),
        Command::Generate {
            watch,
            contract_json,
        } => {
            if watch {
                run_watch(&root, contract_json).map(|()| false)
            } else {
                run_generate(&root, contract_json)
            }
        }
        Command::Check { unused } => run_check(&root, unused),
        Command::Lsp => typeweld_ls::run_stdio()
            .map(|()| false)
            .map_err(anyhow::Error::msg),
    };

    match result {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// Walks up from the current directory to the first typeweld.toml.
fn find_workspace_root() -> Option<PathBuf> {
    let mut current = std::env::current_dir().ok()?;
    loop {
        if current.join("typeweld.toml").is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Generates all configured packages. Returns whether any errors occurred.
fn run_generate(root: &Path, contract_json: bool) -> anyhow::Result<bool> {
    let files = DiskFileProvider;
    let config =
        Config::load(root, &files).map_err(|diagnostic| anyhow::anyhow!("{diagnostic}"))?;
    let (workspace, workspace_diagnostics) =
        workspace::discover(root, &files).map_err(|diagnostic| anyhow::anyhow!("{diagnostic}"))?;
    let mut failed = report(root, &workspace_diagnostics);

    for package in &config.packages {
        let extraction = extract(&workspace, &package.cargo, &package.ts, &files)
            .map_err(|diagnostic| anyhow::anyhow!("{diagnostic}"))?;
        failed |= report(root, &extraction.diagnostics);
        if extraction.has_errors() {
            eprintln!("skipping generation for `{}` due to errors", package.ts);
            continue;
        }

        let (generated, generation_diagnostics) = generate(&extraction.contract);
        failed |= report(root, &generation_diagnostics);
        if generation_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
        {
            eprintln!("skipping write for `{}` due to errors", package.ts);
            continue;
        }

        let dir = root.join(config.package_dir(&package.ts));
        let changed =
            write_package(&dir, &generated).map_err(|message| anyhow::anyhow!(message))?;
        if contract_json {
            let json = serde_json::to_string_pretty(&extraction.contract)?;
            std::fs::write(dir.join("contract.json"), json)?;
        }
        println!(
            "generated {} ({} endpoints, {} types, {} errors){}",
            package.ts,
            extraction.contract.endpoints.len(),
            extraction.contract.types.len(),
            extraction.contract.errors.len(),
            if changed { "" } else { " [unchanged]" },
        );
    }

    Ok(failed)
}

/// Checks extraction (and optionally usage lints). Returns failure.
fn run_check(root: &Path, unused: bool) -> anyhow::Result<bool> {
    let files = DiskFileProvider;
    let config =
        Config::load(root, &files).map_err(|diagnostic| anyhow::anyhow!("{diagnostic}"))?;
    let (workspace, workspace_diagnostics) =
        workspace::discover(root, &files).map_err(|diagnostic| anyhow::anyhow!("{diagnostic}"))?;
    let mut failed = report(root, &workspace_diagnostics);

    for package in &config.packages {
        let extraction = extract(&workspace, &package.cargo, &package.ts, &files)
            .map_err(|diagnostic| anyhow::anyhow!("{diagnostic}"))?;
        failed |= report(root, &extraction.diagnostics);

        if unused && config.unused_endpoints != LintLevel::Off {
            let lints = unused_endpoint_lints(root, &config, &extraction.contract)?;
            failed |= report(root, &lints);
        }

        println!(
            "checked {} ({} endpoints)",
            package.ts,
            extraction.contract.endpoints.len()
        );
    }

    Ok(failed)
}

fn unused_endpoint_lints(
    root: &Path,
    config: &Config,
    contract: &typeweld_engine::ir::Contract,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut sources = Vec::new();
    let mut seen = HashSet::new();
    for pattern in &config.app_src {
        let absolute = root.join(pattern);
        let pattern = absolute.to_string_lossy().into_owned();
        for path in (glob::glob(&pattern)?).flatten() {
            if !seen.insert(path.clone()) {
                continue;
            }
            if let Ok(contents) = std::fs::read_to_string(&path) {
                sources.push((path.to_string_lossy().into_owned(), contents));
            }
        }
    }

    let index = usage::scan_files(contract, &sources);
    let used = index.strongly_used_endpoints();

    let level = config.unused_endpoints;
    let mut lints = Vec::new();
    for endpoint in &contract.endpoints {
        if endpoint.allow_unused || used.contains(&endpoint.id) {
            continue;
        }
        let message = format!(
            "endpoint `{}` ({} {}) has no TypeScript usage; remove it or mark it \
             #[api(..., allow_unused)]",
            endpoint.rust_name,
            endpoint.method.as_str(),
            endpoint.route,
        );
        let span = Some(
            endpoint
                .spans
                .attr
                .clone()
                .unwrap_or_else(|| endpoint.spans.name.clone()),
        );
        lints.push(match level {
            LintLevel::Deny => Diagnostic::error("unused-endpoint", message, span),
            _ => Diagnostic::warning("unused-endpoint", message, span),
        });
    }
    Ok(lints)
}

/// Prints diagnostics with file positions. Returns whether any were errors.
fn report(root: &Path, diagnostics: &[Diagnostic]) -> bool {
    let mut failed = false;
    for diagnostic in diagnostics {
        let source = diagnostic
            .span
            .as_ref()
            .and_then(|span| std::fs::read_to_string(root.join(&span.file)).ok());
        eprintln!("{}", diagnostic.render(source.as_deref()));
        failed |= diagnostic.severity == Severity::Error;
    }
    failed
}

/// Watch mode: regenerate on every Rust/manifest change, debounced.
fn run_watch(root: &Path, contract_json: bool) -> anyhow::Result<()> {
    use notify::Watcher as _;
    use std::sync::mpsc;
    use std::time::Duration;

    let _ = run_generate(root, contract_json);

    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let relevant = event.paths.iter().any(|path| {
                let in_build_dir = path.components().any(|component| {
                    component.as_os_str() == "target" || component.as_os_str() == "node_modules"
                });
                !in_build_dir
                    && (path.extension().is_some_and(|ext| ext == "rs")
                        || path
                            .file_name()
                            .is_some_and(|name| name == "Cargo.toml" || name == "typeweld.toml"))
            });
            if relevant {
                let _ = sender.send(());
            }
        }
    })?;
    watcher.watch(root, notify::RecursiveMode::Recursive)?;
    println!("watching {} for changes...", root.display());

    loop {
        // Block for the first event, then debounce trailing ones.
        if receiver.recv().is_err() {
            return Ok(());
        }
        while receiver.recv_timeout(Duration::from_millis(150)).is_ok() {}
        let _ = run_generate(root, contract_json);
    }
}
