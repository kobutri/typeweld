use std::{env, path::PathBuf, process::ExitCode};

use api_collector::{collect_empty_contract, discover_workspace, write_contract_json};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err("usage: api collect --package-name <name> --out <path>".to_owned());
    };

    if command != "collect" {
        return Err(format!("unknown api command `{command}`"));
    }

    let mut package_name = None;
    let mut out = None;
    let mut manifest_path = None;

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
