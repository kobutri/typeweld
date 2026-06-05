use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    process::ExitCode,
};

use api_collector::{
    collect_empty_contract, discover_workspace, scan_effect_usages, write_contract_json,
    write_effect_usage_index, TypeScriptSourceFile,
};
use api_gen_effect_v4::{render_generated_package, GeneratedPackage};
use api_ir::ApiContract;

fn main() -> ExitCode {
    match run_with_args(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_with_args(args: impl IntoIterator<Item = String>) -> Result<(), String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(usage());
    };

    match command.as_str() {
        "collect" => collect_command(args),
        "gen" => gen_command(args),
        "watch" => watch_command(args),
        "check" => check_command(args),
        "check-usages" => check_usages_command(args),
        "doctor" => doctor_command(args),
        "--help" | "-h" | "help" => {
            println!("{}", usage());
            Ok(())
        }
        _ => Err(format!("unknown api command `{command}`\n\n{}", usage())),
    }
}

fn collect_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let options = CollectOptions::parse(args)?;

    let package_name = options
        .package_name
        .clone()
        .ok_or_else(|| "api collect requires --package-name <ts-package-name>".to_owned())?;
    let out = options
        .out
        .clone()
        .ok_or_else(|| "api collect requires --out <path>".to_owned())?;

    let contract = if options.empty {
        collect_empty_contract(package_name)
    } else {
        collect_cargo_contract(&options, &package_name)?
    };

    write_contract_json(&contract, out).map_err(|error| error.to_string())
}

fn gen_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let options = ContractTargetOptions::parse(args, "api gen")?;
    let contract = read_contract(&options.contract)?;
    let package = render_generated_package(&contract, &options.target_dir);
    write_generated_package(&package)?;
    println!("generated {}", package.package_dir.display());
    Ok(())
}

fn watch_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut options = ContractTargetOptions::parse(args, "api watch")?;
    options.once = true;
    let contract = read_contract(&options.contract)?;
    let package = render_generated_package(&contract, &options.target_dir);
    write_generated_package(&package)?;
    println!(
        "watch regenerated {} once; rerun this command after contract changes",
        package.package_dir.display()
    );
    Ok(())
}

fn check_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let options = ContractTargetOptions::parse(args, "api check")?;
    let contract = read_contract(&options.contract)?;
    let expected = render_generated_package(&contract, &options.target_dir);
    let stale = stale_generated_files(&expected)?;

    if stale.is_empty() {
        println!(
            "generated package is current: {}",
            expected.package_dir.display()
        );
        Ok(())
    } else {
        Err(format!(
            "generated package is stale; run `api gen --contract {} --target-dir {}`\n{}",
            options.contract.display(),
            options.target_dir.display(),
            stale.join("\n")
        ))
    }
}

fn check_usages_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut args = args;
    let mut contract = None;
    let mut out = None;
    let mut ts_files = Vec::new();
    let mut ts_dirs = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--contract" => contract = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--ts" => ts_files.push(required_path(&mut args, "--ts")?),
            "--ts-dir" => ts_dirs.push(required_path(&mut args, "--ts-dir")?),
            other => return Err(format!("unknown api check-usages argument `{other}`")),
        }
    }

    let contract_path =
        contract.ok_or_else(|| "api check-usages requires --contract <path>".to_owned())?;
    let out = out.ok_or_else(|| "api check-usages requires --out <path>".to_owned())?;
    for dir in ts_dirs {
        collect_ts_files(&dir, &mut ts_files)?;
    }

    let contract = read_contract(&contract_path)?;
    let sources = read_ts_sources(&ts_files)?;
    let index = scan_effect_usages(&contract, &sources);
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create usage index directory `{}`: {error}",
                parent.display()
            )
        })?;
    }
    write_effect_usage_index(&index, &out).map_err(|error| error.to_string())?;
    println!(
        "wrote usage index {} with {} usage(s)",
        out.display(),
        index.usages.len()
    );
    Ok(())
}

fn doctor_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut manifest_path = None;
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest-path" => manifest_path = args.next().map(PathBuf::from),
            other => return Err(format!("unknown api doctor argument `{other}`")),
        }
    }

    let manifest_path = manifest_path.unwrap_or_else(|| PathBuf::from("Cargo.toml"));
    let workspace = discover_workspace(&manifest_path).map_err(|error| {
        format!(
            "api doctor could not inspect `{}`: {error}\nhelp: pass --manifest-path <Cargo.toml>",
            manifest_path.display()
        )
    })?;

    println!("workspace root: {}", workspace.root);
    println!("workspace packages: {}", workspace.packages.join(", "));
    println!(
        "setup: run `api collect`, `api gen`, and `api check-usages` before enabling deny lints"
    );
    Ok(())
}

#[derive(Debug)]
struct CollectOptions {
    package_name: Option<String>,
    cargo_package: Option<String>,
    api_root: Option<String>,
    out: Option<PathBuf>,
    manifest_path: PathBuf,
    target_dir: Option<PathBuf>,
    features: Vec<String>,
    all_features: bool,
    no_default_features: bool,
    empty: bool,
}

impl CollectOptions {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = args;
        let mut options = Self {
            package_name: None,
            cargo_package: None,
            api_root: None,
            out: None,
            manifest_path: PathBuf::from("Cargo.toml"),
            target_dir: None,
            features: Vec::new(),
            all_features: false,
            no_default_features: false,
            empty: false,
        };

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--package-name" => options.package_name = args.next(),
                "--package" | "-p" => options.cargo_package = args.next(),
                "--api-root" => options.api_root = args.next(),
                "--out" => options.out = args.next().map(PathBuf::from),
                "--manifest-path" => {
                    options.manifest_path = required_path(&mut args, "--manifest-path")?;
                }
                "--target-dir" => {
                    options.target_dir = Some(required_path(&mut args, "--target-dir")?);
                }
                "--features" => {
                    let value = args
                        .next()
                        .ok_or_else(|| "--features requires <features>".to_owned())?;
                    options.features.extend(parse_features(&value));
                }
                "--all-features" => options.all_features = true,
                "--no-default-features" => options.no_default_features = true,
                "--empty" => options.empty = true,
                other => return Err(format!("unknown api collect argument `{other}`")),
            }
        }

        options.features.sort();
        options.features.dedup();
        Ok(options)
    }
}

fn collect_cargo_contract(
    options: &CollectOptions,
    ts_package_name: &str,
) -> Result<ApiContract, String> {
    let cargo_package = options.cargo_package.as_deref().ok_or_else(|| {
        "api collect requires --package <cargo-package> unless --empty is set".to_owned()
    })?;
    let api_root = options.api_root.as_deref().ok_or_else(|| {
        "api collect requires --api-root <path::to::api> unless --empty is set".to_owned()
    })?;

    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(&options.manifest_path)
        .exec()
        .map_err(|error| {
            format!(
                "api collect could not inspect `{}`: {error}",
                options.manifest_path.display()
            )
        })?;
    let package = resolve_metadata_package(&metadata, cargo_package)?;
    let lib_crate_name = package_lib_crate_name(package)?;
    let api_root = normalize_api_root(api_root, &lib_crate_name)?;
    let target_dir = options
        .target_dir
        .clone()
        .unwrap_or_else(|| metadata.target_directory.clone().into_std_path_buf());
    let collector_dir = target_dir
        .join("api-contract")
        .join("collector")
        .join(sanitize_package_dir_name(&package.name));

    write_temp_collector_crate(
        &collector_dir,
        package,
        &api_root,
        ts_package_name,
        options,
        &metadata,
    )?;
    run_temp_collector(&collector_dir)
}

fn resolve_metadata_package<'a>(
    metadata: &'a cargo_metadata::Metadata,
    package_name: &str,
) -> Result<&'a cargo_metadata::Package, String> {
    metadata
        .workspace_packages()
        .into_iter()
        .find(|package| package.name == package_name)
        .ok_or_else(|| format!("api collect could not find workspace package `{package_name}`"))
}

fn package_lib_crate_name(package: &cargo_metadata::Package) -> Result<String, String> {
    package
        .targets
        .iter()
        .find(|target| {
            target.kind.iter().any(|kind| {
                matches!(
                    kind,
                    cargo_metadata::TargetKind::Lib | cargo_metadata::TargetKind::RLib
                )
            })
        })
        .map(|target| target.name.replace('-', "_"))
        .ok_or_else(|| {
            format!(
                "api collect package `{}` must expose a library target so the temporary collector can depend on it",
                package.name
            )
        })
}

fn normalize_api_root(api_root: &str, lib_crate_name: &str) -> Result<Vec<String>, String> {
    let mut parts = api_root
        .split("::")
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return Err("api collect --api-root must name a Rust function path".to_owned());
    }
    if parts.first().is_some_and(|part| part == lib_crate_name) {
        parts.remove(0);
    }
    if parts.iter().any(|part| !is_rust_identifier(part)) {
        return Err(format!(
            "api collect --api-root `{api_root}` contains a non-identifier path segment"
        ));
    }
    Ok(parts)
}

fn write_temp_collector_crate(
    collector_dir: &Path,
    package: &cargo_metadata::Package,
    api_root: &[String],
    ts_package_name: &str,
    options: &CollectOptions,
    metadata: &cargo_metadata::Metadata,
) -> Result<(), String> {
    let src_dir = collector_dir.join("src");
    fs::create_dir_all(&src_dir).map_err(|error| {
        format!(
            "failed to create temporary collector directory `{}`: {error}",
            src_dir.display()
        )
    })?;

    let api_collector_path = tool_package_manifest_dir(metadata, "api-collector")?;
    let api_core_path = tool_package_manifest_dir(metadata, "api-core")?;
    let package_dir = package
        .manifest_path
        .parent()
        .ok_or_else(|| {
            format!(
                "package `{}` manifest has no parent directory",
                package.name
            )
        })?
        .to_path_buf()
        .into_std_path_buf();
    let features = selected_dependency_features(package, options);
    let default_features = if options.no_default_features {
        "default-features = false\n"
    } else {
        ""
    };
    let features = if features.is_empty() {
        String::new()
    } else {
        format!(
            "features = [{}]\n",
            features
                .iter()
                .map(|feature| toml_string(feature))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let manifest = format!(
        r#"[package]
name = "api-contract-collector-{collector_name}"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[dependencies]
api-collector = {{ path = {api_collector_path} }}
api-core = {{ path = {api_core_path} }}

[dependencies.target-api-package]
package = {package_name}
path = {package_dir}
{default_features}{features}
"#,
        collector_name = sanitize_package_dir_name(&package.name),
        api_collector_path = toml_string(&api_collector_path.display().to_string()),
        api_core_path = toml_string(&api_core_path.display().to_string()),
        package_name = toml_string(package.name.as_str()),
        package_dir = toml_string(&package_dir.display().to_string()),
    );
    let root_call = format!("target_api_package::{}()", api_root.join("::"));
    let main_rs = format!(
        r#"fn main() {{
    let root_module: api_core::ApiModule = {root_call};
    let contract = api_collector::collect_contract(api_collector::CollectorInput {{
        package_name: {ts_package_name}.to_owned(),
        root_module,
        types: Vec::new(),
        errors: Vec::new(),
    }});
    println!(
        "{{}}",
        api_collector::contract_to_json(&contract).expect("collected contract should serialize")
    );
}}
"#,
        ts_package_name = rust_string(ts_package_name),
    );

    fs::write(collector_dir.join("Cargo.toml"), manifest).map_err(|error| {
        format!(
            "failed to write temporary collector manifest `{}`: {error}",
            collector_dir.join("Cargo.toml").display()
        )
    })?;
    fs::write(src_dir.join("main.rs"), main_rs).map_err(|error| {
        format!(
            "failed to write temporary collector main `{}`: {error}",
            src_dir.join("main.rs").display()
        )
    })
}

fn run_temp_collector(collector_dir: &Path) -> Result<ApiContract, String> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(collector_dir.join("Cargo.toml"))
        .output()
        .map_err(|error| {
            format!(
                "failed to run temporary collector `{}`: {error}",
                collector_dir.display()
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "api collect failed while compiling or running the temporary collector `{}`\n{}",
            collector_dir.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "temporary collector `{}` did not emit a valid API contract: {error}\nstdout:\n{}",
            collector_dir.display(),
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn tool_package_manifest_dir(
    metadata: &cargo_metadata::Metadata,
    package_name: &str,
) -> Result<PathBuf, String> {
    if let Some(package) = metadata
        .workspace_packages()
        .into_iter()
        .find(|package| package.name == package_name)
    {
        return package
            .manifest_path
            .parent()
            .ok_or_else(|| format!("package `{package_name}` manifest has no parent directory"))
            .map(|path| path.to_path_buf().into_std_path_buf());
    }

    let collector_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match package_name {
        "api-collector" => Ok(collector_dir),
        "api-core" => collector_dir
            .parent()
            .map(|crates_dir| crates_dir.join("api-core"))
            .filter(|path| path.join("Cargo.toml").is_file())
            .ok_or_else(|| {
                "api collect could not locate api-core next to the running api-collector source"
                    .to_owned()
            }),
        _ => Err(format!(
            "api collect could not locate tool package `{package_name}`"
        )),
    }
}

fn selected_dependency_features(
    package: &cargo_metadata::Package,
    options: &CollectOptions,
) -> Vec<String> {
    if options.all_features {
        let mut features = package.features.keys().cloned().collect::<Vec<_>>();
        features.sort();
        return features;
    }
    options.features.clone()
}

fn parse_features(value: &str) -> Vec<String> {
    value
        .split([',', ' '])
        .map(str::trim)
        .filter(|feature| !feature.is_empty())
        .map(str::to_owned)
        .collect()
}

fn sanitize_package_dir_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect()
}

fn is_rust_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization should not fail")
}

fn rust_string(value: &str) -> String {
    toml_string(value)
}

#[derive(Debug)]
struct ContractTargetOptions {
    contract: PathBuf,
    target_dir: PathBuf,
    once: bool,
}

impl ContractTargetOptions {
    fn parse(args: impl Iterator<Item = String>, command: &str) -> Result<Self, String> {
        let mut args = args;
        let mut contract = None;
        let mut target_dir = None;
        let mut once = false;

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--contract" => contract = args.next().map(PathBuf::from),
                "--target-dir" | "--out-dir" => target_dir = args.next().map(PathBuf::from),
                "--once" => once = true,
                other => return Err(format!("unknown {command} argument `{other}`")),
            }
        }

        Ok(Self {
            contract: contract.ok_or_else(|| format!("{command} requires --contract <path>"))?,
            target_dir: target_dir.unwrap_or_else(|| PathBuf::from("target")),
            once,
        })
    }
}

fn read_contract(path: &Path) -> Result<ApiContract, String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read contract `{}`: {error}", path.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("failed to parse contract `{}`: {error}", path.display()))
}

fn write_generated_package(package: &GeneratedPackage) -> Result<(), String> {
    for file in &package.files {
        let path = package.package_dir.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed to create generated package directory `{}`: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&path, &file.contents).map_err(|error| {
            format!(
                "failed to write generated package file `{}`: {error}",
                path.display()
            )
        })?;
    }
    Ok(())
}

fn stale_generated_files(package: &GeneratedPackage) -> Result<Vec<String>, String> {
    let mut stale = Vec::new();

    for file in &package.files {
        let path = package.package_dir.join(&file.path);
        match fs::read_to_string(&path) {
            Ok(contents) if contents == file.contents => {}
            Ok(_) => stale.push(format!("stale: {}", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                stale.push(format!("missing: {}", path.display()));
            }
            Err(error) => {
                return Err(format!(
                    "failed to read generated package file `{}`: {error}",
                    path.display()
                ));
            }
        }
    }

    Ok(stale)
}

fn required_path(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<PathBuf, String> {
    args.next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{flag} requires <path>"))
}

fn collect_ts_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|error| {
        format!(
            "failed to read TypeScript directory `{}`: {error}",
            dir.display()
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "failed to read TypeScript directory entry in `{}`: {error}",
                dir.display()
            )
        })?;
        let path = entry.path();
        if path.is_dir() {
            collect_ts_files(&path, files)?;
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "ts" | "tsx"))
        {
            files.push(path);
        }
    }
    files.sort();
    files.dedup();
    Ok(())
}

fn read_ts_sources(paths: &[PathBuf]) -> Result<Vec<TypeScriptSourceFile>, String> {
    paths
        .iter()
        .map(|path| {
            let contents = fs::read_to_string(path).map_err(|error| {
                format!(
                    "failed to read TypeScript source `{}`: {error}",
                    path.display()
                )
            })?;
            Ok(TypeScriptSourceFile {
                path: path.to_string_lossy().into_owned(),
                contents,
            })
        })
        .collect()
}

fn usage() -> String {
    [
        "usage:",
        "  api collect --package <cargo-package> --api-root <path::to::api> --package-name <ts-package> --out <path> [--manifest-path <Cargo.toml>] [--target-dir <dir>] [--features <list>] [--all-features] [--no-default-features]",
        "  api collect --empty --package-name <ts-package> --out <path>",
        "  api gen --contract <path> [--target-dir <dir>]",
        "  api watch --contract <path> [--target-dir <dir>] [--once]",
        "  api check --contract <path> [--target-dir <dir>]",
        "  api check-usages --contract <path> --out <path> [--ts <file>] [--ts-dir <dir>]",
        "  api doctor [--manifest-path <Cargo.toml>]",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn gen_and_check_hidden_package() {
        let root = test_root("gen-check");
        fs::create_dir_all(&root).expect("create root");
        let contract_path = root.join("contract.json");
        let target_dir = root.join("target");
        fs::write(
            &contract_path,
            serde_json::to_string_pretty(&api_test_fixtures::basic_contract())
                .expect("serialize contract"),
        )
        .expect("write contract");

        run_with_args(vec![
            "gen".to_owned(),
            "--contract".to_owned(),
            contract_path.display().to_string(),
            "--target-dir".to_owned(),
            target_dir.display().to_string(),
        ])
        .expect("generate package");
        run_with_args(vec![
            "check".to_owned(),
            "--contract".to_owned(),
            contract_path.display().to_string(),
            "--target-dir".to_owned(),
            target_dir.display().to_string(),
        ])
        .expect("check generated package");

        let package_dir = target_dir
            .join("api-contract/effect-v4/packages")
            .join("_workspace_server-api");
        assert!(package_dir.join("index.ts").is_file());
        assert!(package_dir.join("endpoints.ts").is_file());
    }

    #[test]
    fn check_usages_writes_usage_index() {
        let root = test_root("check-usages");
        fs::create_dir_all(root.join("src")).expect("create src");
        let contract_path = root.join("contract.json");
        let usage_path = root.join("target/api-contract/graph/effect-usage-index.json");
        let ts_path = root.join("src/client.ts");
        fs::write(
            &contract_path,
            serde_json::to_string_pretty(&api_test_fixtures::basic_contract())
                .expect("serialize contract"),
        )
        .expect("write contract");
        fs::write(
            &ts_path,
            r#"import { users } from "@workspace/server-api"

export const program = Effect.gen(function* () {
  yield* users.getUser({ id: 1 })
})
"#,
        )
        .expect("write ts");

        run_with_args(vec![
            "check-usages".to_owned(),
            "--contract".to_owned(),
            contract_path.display().to_string(),
            "--out".to_owned(),
            usage_path.display().to_string(),
            "--ts".to_owned(),
            ts_path.display().to_string(),
        ])
        .expect("write usage index");

        let usage_json = fs::read_to_string(usage_path).expect("read usage index");
        assert!(usage_json.contains("\"strong\": 1"));
        assert!(usage_json.contains("\"accessor_path\": ["));
    }

    #[test]
    fn check_reports_stale_generated_package() {
        let root = test_root("stale");
        fs::create_dir_all(&root).expect("create root");
        let contract_path = root.join("contract.json");
        let target_dir = root.join("target");
        fs::write(
            &contract_path,
            serde_json::to_string_pretty(&api_test_fixtures::basic_contract())
                .expect("serialize contract"),
        )
        .expect("write contract");

        let error = run_with_args(vec![
            "check".to_owned(),
            "--contract".to_owned(),
            contract_path.display().to_string(),
            "--target-dir".to_owned(),
            target_dir.display().to_string(),
        ])
        .expect_err("missing generated package should be stale");

        assert!(error.contains("generated package is stale"));
        assert!(error.contains("api gen --contract"));
        assert!(error.contains("missing:"));
    }

    #[test]
    fn collect_requires_explicit_empty_debug_flag() {
        let root = test_root("collect-empty");
        fs::create_dir_all(&root).expect("create root");
        let contract_path = root.join("contract.json");

        let error = run_with_args(vec![
            "collect".to_owned(),
            "--package-name".to_owned(),
            "@workspace/server-api".to_owned(),
            "--out".to_owned(),
            contract_path.display().to_string(),
        ])
        .expect_err("collect without package/root should not write an empty contract");

        assert!(error.contains("requires --package"));
        assert!(!contract_path.exists());

        run_with_args(vec![
            "collect".to_owned(),
            "--empty".to_owned(),
            "--package-name".to_owned(),
            "@workspace/server-api".to_owned(),
            "--out".to_owned(),
            contract_path.display().to_string(),
        ])
        .expect("explicit empty collection");

        let contract: ApiContract =
            serde_json::from_str(&fs::read_to_string(contract_path).expect("read contract"))
                .expect("parse contract");
        assert!(contract.endpoints.is_empty());
    }

    #[test]
    fn collect_builds_temp_crate_and_calls_api_root_with_features() {
        let root = test_root("collect-real");
        let server_dir = root.join("server");
        fs::create_dir_all(server_dir.join("src")).expect("create server src");
        fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["server"]
resolver = "2"
"#,
        )
        .expect("write workspace manifest");
        fs::write(
            server_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "server"
version = "0.0.0"
edition = "2021"

[features]
extra = []

[dependencies]
api-core = {{ path = {api_core_path} }}
api-macros = {{ path = {api_macros_path} }}
"#,
                api_core_path =
                    toml_string(&repo_root().join("crates/api-core").display().to_string()),
                api_macros_path =
                    toml_string(&repo_root().join("crates/api-macros").display().to_string()),
            ),
        )
        .expect("write server manifest");
        fs::write(
            server_dir.join("src/lib.rs"),
            r#"use api_core::{api_module, ApiModule, ApiType, Json, Path};

#[derive(api_macros::ApiType)]
pub struct User {
    id: i64,
}

#[cfg(feature = "extra")]
#[derive(api_macros::ApiType)]
pub struct AuditEvent {
    id: i64,
}

#[derive(api_macros::ApiError)]
pub enum GetUserError {
    #[api_error(status = 404)]
    NotFound,
}

#[api_macros::api(method = "GET", path = "/users/{id}")]
pub async fn get_user(id: Path<i64>) -> Result<Json<User>, GetUserError> {
    let _ = id;
    todo!()
}

#[cfg(feature = "extra")]
#[api_macros::api(method = "GET", path = "/audit/{id}")]
pub async fn get_audit_event(id: Path<i64>) -> Result<Json<AuditEvent>, GetUserError> {
    let _ = id;
    todo!()
}

#[cfg(feature = "extra")]
pub fn api() -> ApiModule {
    api_module!(name = "server", endpoints = [get_user, get_audit_event])
}

#[cfg(not(feature = "extra"))]
pub fn api() -> ApiModule {
    api_module!(name = "server", endpoints = [get_user])
}
"#,
        )
        .expect("write server lib");

        let contract_path = root.join("contract.json");
        run_with_args(vec![
            "collect".to_owned(),
            "--manifest-path".to_owned(),
            root.join("Cargo.toml").display().to_string(),
            "--target-dir".to_owned(),
            root.join("target").display().to_string(),
            "--package".to_owned(),
            "server".to_owned(),
            "--api-root".to_owned(),
            "server::api".to_owned(),
            "--features".to_owned(),
            "extra".to_owned(),
            "--package-name".to_owned(),
            "@workspace/server-api".to_owned(),
            "--out".to_owned(),
            contract_path.display().to_string(),
        ])
        .expect("collect real contract");

        let contract: ApiContract =
            serde_json::from_str(&fs::read_to_string(contract_path).expect("read contract"))
                .expect("parse contract");
        let endpoint_names = contract
            .endpoints
            .iter()
            .map(|endpoint| endpoint.rust_name.as_str())
            .collect::<Vec<_>>();
        let type_names = contract
            .types
            .iter()
            .map(|type_def| type_def.rust_name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(endpoint_names, ["get_user", "get_audit_event"]);
        assert!(type_names.contains(&"User"));
        assert!(type_names.contains(&"AuditEvent"));
        assert_eq!(contract.errors[0].rust_name, "GetUserError");
    }

    fn test_root(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("api-cli-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn repo_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("api-collector is under crates/")
            .to_path_buf()
    }
}
