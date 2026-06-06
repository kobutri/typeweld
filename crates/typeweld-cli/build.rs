use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, SystemTime},
};

fn main() {
    println!("cargo:rerun-if-env-changed=TYPEWELD_TEMPLATE_SOURCE");
    println!("cargo:rerun-if-env-changed=TYPEWELD_TEMPLATE_VERSION");
    println!("cargo:rerun-if-env-changed=TYPEWELD_TEMPLATE_GITHUB_REPO");
    println!("cargo:rerun-if-env-changed=TYPEWELD_TEMPLATE_NPM_TARBALL_BASE");

    if env::var_os("CARGO_FEATURE_EMBEDDED_SEMANTIC_SCANNER").is_none() {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir.join("../..");
    let npm_dir = repo_root.join("npm");
    let scanner_dir = npm_dir.join("semantic-usage-scanner");
    let out_file =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR")).join("semantic_usage_scanner.js");

    if !scanner_dir.join("package.json").is_file() {
        write_scanner_stub(&out_file);
        println!(
            "cargo:warning=semantic usage scanner sources were not found at {}; embedded scanner will report a setup error",
            scanner_dir.display()
        );
        return;
    }

    println!(
        "cargo:rerun-if-changed={}",
        npm_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        npm_dir.join("package-lock.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        scanner_dir.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        scanner_dir.join("vite.config.mts").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        scanner_dir.join("src").join("index.ts").display()
    );

    let _lock = NpmBuildLock::acquire(npm_dir.join(".semantic-usage-scanner-build.lock"));
    run(&npm_dir, npm_command(), &["ci"], None);
    run(
        &npm_dir,
        npm_command(),
        &[
            "run",
            "build",
            "--workspace",
            "@typeweld/semantic-usage-scanner",
        ],
        Some(("SEMANTIC_USAGE_SCANNER_OUT", out_file.as_os_str())),
    );
}

fn write_scanner_stub(out_file: &Path) {
    fs::write(
        out_file,
        r#"globalThis.__semanticUsageScanner = {
  scan() {
    throw new Error("semantic TypeScript usage scanning is unavailable in this build");
  },
};
"#,
    )
    .unwrap_or_else(|error| {
        panic!(
            "failed to write semantic usage scanner stub `{}`: {error}",
            out_file.display()
        )
    });
}

struct NpmBuildLock {
    path: PathBuf,
}

impl NpmBuildLock {
    fn acquire(path: PathBuf) -> Self {
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).expect("write npm build lock");
                    return Self { path };
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    remove_stale_lock(&path);
                    thread::sleep(Duration::from_millis(250));
                }
                Err(error) => panic!(
                    "failed to create npm build lock `{}`: {error}",
                    path.display()
                ),
            }
        }
    }
}

impl Drop for NpmBuildLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn remove_stale_lock(path: &Path) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    let Ok(modified) = metadata.modified() else {
        return;
    };
    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return;
    };
    if age > Duration::from_secs(15 * 60) {
        let _ = fs::remove_file(path);
    }
}

fn npm_command() -> &'static str {
    if cfg!(windows) {
        "npm.cmd"
    } else {
        "npm"
    }
}

fn run(
    current_dir: &Path,
    command: &str,
    args: &[&str],
    env_var: Option<(&str, &std::ffi::OsStr)>,
) {
    let mut child = Command::new(command);
    child.args(args).current_dir(current_dir);
    if let Some((key, value)) = env_var {
        child.env(key, value);
    }

    let status = child
        .status()
        .unwrap_or_else(|error| panic!("failed to run `{command} {}`: {error}", args.join(" ")));
    assert!(
        status.success(),
        "`{command} {}` failed with {status}",
        args.join(" ")
    );
}
