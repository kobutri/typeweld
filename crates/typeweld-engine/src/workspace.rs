//! Cargo workspace discovery without invoking cargo.
//!
//! Generation must stay fast enough for the language server's edit loop, so
//! the workspace layout is read directly from `Cargo.toml` files: member
//! globs are expanded, package names and crate roots resolved, and path
//! dependencies between members recorded. External (registry) dependencies
//! are irrelevant here — external API types come from a fixed registry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diag::Diagnostic;
use crate::vfs::FileProvider;

/// A discovered cargo workspace (or single package).
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    /// Directory containing the root `Cargo.toml`.
    pub root: PathBuf,
    /// Member packages, sorted by name.
    pub packages: Vec<Package>,
}

/// One workspace member package.
#[derive(Clone, Debug)]
pub struct Package {
    /// The cargo package name, e.g. `my-server`.
    pub name: String,
    /// The crate identifier, e.g. `my_server`.
    pub ident: String,
    /// Directory containing the package `Cargo.toml`.
    pub manifest_dir: PathBuf,
    /// The crate root source file (lib target preferred over bin).
    pub crate_root: PathBuf,
    /// Names of other workspace members this package depends on by path.
    pub workspace_deps: Vec<String>,
}

impl Workspace {
    /// Looks up a member package by cargo name.
    #[must_use]
    pub fn package(&self, name: &str) -> Option<&Package> {
        self.packages.iter().find(|package| package.name == name)
    }

    /// The packages reachable from `name` through path dependencies,
    /// including `name` itself, in deterministic order.
    #[must_use]
    pub fn reachable_packages(&self, name: &str) -> Vec<&Package> {
        let mut seen = Vec::new();
        let mut queue = vec![name.to_owned()];
        while let Some(current) = queue.pop() {
            if seen.contains(&current) {
                continue;
            }
            if let Some(package) = self.package(&current) {
                queue.extend(package.workspace_deps.iter().cloned());
                seen.push(current);
            }
        }
        seen.sort();
        let mut packages: Vec<&Package> = seen
            .iter()
            .filter_map(|current| self.package(current))
            .collect();
        packages.sort_by(|a, b| a.name.cmp(&b.name));
        packages
    }
}

/// Discovers the workspace rooted at `root` (the directory containing the
/// top-level `Cargo.toml`).
///
/// # Errors
/// Returns diagnostics for a missing or unparsable root manifest. Problems in
/// individual members are reported as diagnostics alongside a partial result.
pub fn discover(
    root: &Path,
    files: &dyn FileProvider,
) -> Result<(Workspace, Vec<Diagnostic>), Diagnostic> {
    let manifest_path = root.join("Cargo.toml");
    let Some(manifest_source) = files.read(&manifest_path) else {
        return Err(Diagnostic::error(
            "missing-manifest",
            format!("no Cargo.toml found at {}", manifest_path.display()),
            None,
        ));
    };
    let manifest: toml::Value = toml::from_str(&manifest_source).map_err(|error| {
        Diagnostic::error(
            "invalid-manifest",
            format!("failed to parse {}: {error}", manifest_path.display()),
            None,
        )
    })?;

    let mut diagnostics = Vec::new();
    let mut member_dirs: Vec<PathBuf> = Vec::new();

    if let Some(workspace) = manifest.get("workspace") {
        let members = workspace
            .get("members")
            .and_then(toml::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let excludes: Vec<String> = workspace
            .get("exclude")
            .and_then(toml::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(toml::Value::as_str)
            .map(str::to_owned)
            .collect();

        for member in members.iter().filter_map(toml::Value::as_str) {
            for dir in expand_member_glob(root, member, &mut diagnostics) {
                let relative = dir.strip_prefix(root).unwrap_or(&dir);
                let relative = relative.to_string_lossy().replace('\\', "/");
                if excludes.contains(&relative) {
                    continue;
                }
                member_dirs.push(dir);
            }
        }
    }
    if manifest.get("package").is_some() {
        member_dirs.push(root.to_path_buf());
    }

    member_dirs.sort();
    member_dirs.dedup();

    let mut packages = BTreeMap::new();
    for dir in member_dirs {
        match read_package(&dir, files) {
            Ok(Some(package)) => {
                packages.insert(package.name.clone(), package);
            }
            Ok(None) => {}
            Err(diagnostic) => diagnostics.push(diagnostic),
        }
    }

    // Resolve path deps to member names now that all members are known.
    let dir_to_name: BTreeMap<PathBuf, String> = packages
        .values()
        .map(|package| (package.manifest_dir.clone(), package.name.clone()))
        .collect();
    let mut resolved: Vec<Package> = packages.into_values().collect();
    for package in &mut resolved {
        package.workspace_deps = package
            .workspace_deps
            .iter()
            .filter_map(|dep_path| {
                let absolute = normalize(&package.manifest_dir.join(dep_path));
                dir_to_name.get(&absolute).cloned()
            })
            .collect();
        package.workspace_deps.sort();
        package.workspace_deps.dedup();
    }

    Ok((
        Workspace {
            root: root.to_path_buf(),
            packages: resolved,
        },
        diagnostics,
    ))
}

fn expand_member_glob(
    root: &Path,
    member: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<PathBuf> {
    if !member.contains('*') {
        return vec![normalize(&root.join(member))];
    }

    let pattern = root.join(member).to_string_lossy().into_owned();
    match glob::glob(&pattern) {
        Ok(paths) => paths
            .filter_map(Result::ok)
            .filter(|path| path.is_dir())
            .map(|path| normalize(&path))
            .collect(),
        Err(error) => {
            diagnostics.push(Diagnostic::warning(
                "invalid-member-glob",
                format!("invalid workspace member pattern `{member}`: {error}"),
                None,
            ));
            Vec::new()
        }
    }
}

fn read_package(dir: &Path, files: &dyn FileProvider) -> Result<Option<Package>, Diagnostic> {
    let manifest_path = dir.join("Cargo.toml");
    let Some(source) = files.read(&manifest_path) else {
        return Err(Diagnostic::warning(
            "missing-member-manifest",
            format!(
                "workspace member has no manifest: {}",
                manifest_path.display()
            ),
            None,
        ));
    };
    let manifest: toml::Value = toml::from_str(&source).map_err(|error| {
        Diagnostic::error(
            "invalid-manifest",
            format!("failed to parse {}: {error}", manifest_path.display()),
            None,
        )
    })?;

    let Some(package) = manifest.get("package") else {
        // A virtual manifest (workspace root without a package) is fine.
        return Ok(None);
    };
    let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
        return Err(Diagnostic::error(
            "invalid-manifest",
            format!("missing package.name in {}", manifest_path.display()),
            None,
        ));
    };

    let lib_path = manifest
        .get("lib")
        .and_then(|lib| lib.get("path"))
        .and_then(toml::Value::as_str)
        .map(|path| dir.join(path));
    let crate_root = lib_path
        .filter(|path| files.exists(path))
        .or_else(|| Some(dir.join("src/lib.rs")).filter(|path| files.exists(path)))
        .or_else(|| Some(dir.join("src/main.rs")).filter(|path| files.exists(path)));
    let Some(crate_root) = crate_root else {
        return Err(Diagnostic::warning(
            "missing-crate-root",
            format!("no src/lib.rs or src/main.rs found for package `{name}`"),
            None,
        ));
    };

    let mut path_deps = Vec::new();
    for table in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(deps) = manifest.get(table).and_then(toml::Value::as_table) {
            for value in deps.values() {
                if let Some(path) = value.get("path").and_then(toml::Value::as_str) {
                    path_deps.push(path.to_owned());
                }
            }
        }
    }

    Ok(Some(Package {
        name: name.to_owned(),
        ident: name.replace('-', "_"),
        manifest_dir: dir.to_path_buf(),
        crate_root,
        // Holds raw path strings until discover() maps them to member names.
        workspace_deps: path_deps,
    }))
}

/// Lexically normalizes `.` and `..` segments without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::MemoryFileProvider;

    fn provider(files: &[(&str, &str)]) -> MemoryFileProvider {
        let mut memory = MemoryFileProvider::default();
        for (path, contents) in files {
            memory.insert(PathBuf::from(path), (*contents).to_owned());
        }
        memory
    }

    #[test]
    fn discovers_members_and_path_deps() {
        let files = provider(&[
            (
                "/ws/Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\"]\n",
            ),
            (
                "/ws/crates/api/Cargo.toml",
                "[package]\nname = \"api\"\n[dependencies]\nshared = { path = \"../shared\" }\nserde = \"1\"\n",
            ),
            ("/ws/crates/api/src/lib.rs", ""),
            (
                "/ws/crates/shared/Cargo.toml",
                "[package]\nname = \"shared\"\n",
            ),
            ("/ws/crates/shared/src/lib.rs", ""),
        ]);
        // Member globs need a real filesystem; emulate by listing explicitly.
        let files_explicit = provider(&[
            (
                "/ws/Cargo.toml",
                "[workspace]\nmembers = [\"crates/api\", \"crates/shared\"]\n",
            ),
            (
                "/ws/crates/api/Cargo.toml",
                "[package]\nname = \"api\"\n[dependencies]\nshared = { path = \"../shared\" }\nserde = \"1\"\n",
            ),
            ("/ws/crates/api/src/lib.rs", ""),
            (
                "/ws/crates/shared/Cargo.toml",
                "[package]\nname = \"shared\"\n",
            ),
            ("/ws/crates/shared/src/lib.rs", ""),
        ]);
        drop(files);

        let (workspace, diagnostics) =
            discover(Path::new("/ws"), &files_explicit).expect("discover");
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        assert_eq!(workspace.packages.len(), 2);

        let api = workspace.package("api").expect("api");
        assert_eq!(api.ident, "api");
        assert_eq!(api.workspace_deps, ["shared"]);
        assert_eq!(api.crate_root, PathBuf::from("/ws/crates/api/src/lib.rs"));

        let reachable = workspace.reachable_packages("api");
        let names: Vec<&str> = reachable.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["api", "shared"]);
    }

    #[test]
    fn single_package_without_workspace_section() {
        let files = provider(&[
            ("/pkg/Cargo.toml", "[package]\nname = \"solo\"\n"),
            ("/pkg/src/main.rs", "fn main() {}"),
        ]);
        let (workspace, diagnostics) = discover(Path::new("/pkg"), &files).expect("discover");
        assert!(diagnostics.is_empty());
        assert_eq!(workspace.packages.len(), 1);
        assert_eq!(
            workspace.packages[0].crate_root,
            PathBuf::from("/pkg/src/main.rs")
        );
    }
}
