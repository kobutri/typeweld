use std::{
    env, fs,
    path::{Path, PathBuf},
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
    let mut package_name = None;
    let mut out = None;
    let mut manifest_path = None;
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--package-name" => package_name = args.next(),
            "--out" => out = args.next().map(PathBuf::from),
            "--manifest-path" => manifest_path = args.next().map(PathBuf::from),
            other => return Err(format!("unknown api collect argument `{other}`")),
        }
    }

    if let Some(manifest_path) = manifest_path {
        let _workspace = discover_workspace(manifest_path).map_err(|error| error.to_string())?;
    }

    let package_name =
        package_name.ok_or_else(|| "api collect requires --package-name <name>".to_owned())?;
    let out = out.ok_or_else(|| "api collect requires --out <path>".to_owned())?;
    let contract = collect_empty_contract(package_name);

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
        "  api collect --package-name <name> --out <path> [--manifest-path <Cargo.toml>]",
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

    fn test_root(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("api-cli-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }
}
