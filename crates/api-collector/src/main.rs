use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    process::ExitCode,
};

use api_collector::{
    collect_empty_contract, discover_workspace, scan_effect_usages, write_contract_json,
    write_effect_usage_index, EffectUsageIndex, EndpointUsage, TypeScriptSourceFile,
};
use api_gen_effect_v4::{render_generated_package, render_symbol_graph, GeneratedPackage};
use api_ir::{ApiContract, Field, SourceRange, SourceSpan, TypeShape};
use syn::spanned::Spanned;

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

    if options.empty {
        let out = options
            .out
            .clone()
            .ok_or_else(|| "api collect --empty requires --out <path>".to_owned())?;
        let package_name = options.package_name.clone().ok_or_else(|| {
            "api collect --empty requires --package-name <ts-package-name>".to_owned()
        })?;
        let contract = collect_empty_contract(package_name);
        return write_contract_json(&contract, out).map_err(|error| error.to_string());
    }

    if options.workspace {
        let graph_path = collect_workspace_contracts(&options)?;
        println!("wrote workspace contract graph {}", graph_path.display());
        return Ok(());
    }

    let out = options
        .out
        .clone()
        .ok_or_else(|| "api collect requires --out <path>".to_owned())?;
    let contract = collect_cargo_contract(&options)?;
    write_contract_json(&contract, out).map_err(|error| error.to_string())
}

fn gen_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args = args.collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--workspace-contract") {
        return gen_workspace_command(args.into_iter());
    }
    let options = ContractTargetOptions::parse(args.into_iter(), "api gen")?;
    let contract = read_contract(&options.contract)?;
    let package = render_generated_package(&contract, &options.target_dir);
    write_generated_package(&package)?;
    write_symbol_graph_json(
        &render_symbol_graph(&contract, &package),
        &options.target_dir,
    )?;
    println!("generated {}", package.package_dir.display());
    Ok(())
}

fn gen_workspace_command(args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut args = args;
    let mut workspace_contract = None;
    let mut target_dir = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--workspace-contract" => workspace_contract = args.next().map(PathBuf::from),
            "--target-dir" | "--out-dir" => target_dir = args.next().map(PathBuf::from),
            other => return Err(format!("unknown api gen argument `{other}`")),
        }
    }

    let workspace_contract = workspace_contract
        .ok_or_else(|| "api gen requires --workspace-contract <path>".to_owned())?;
    let target_dir = target_dir.unwrap_or_else(|| PathBuf::from("target"));
    let graph = read_workspace_contract_graph(&workspace_contract)?;

    let mut symbol_graphs = Vec::new();
    for package_info in workspace_generation_order(&graph)? {
        let contract = read_contract(Path::new(&package_info.contract_path))?;
        let package = render_generated_package(&contract, &target_dir);
        write_generated_package(&package)?;
        symbol_graphs.push(render_symbol_graph(&contract, &package));
        println!("generated {}", package.package_dir.display());
    }
    write_workspace_tsconfig_paths(&graph, &target_dir)?;
    write_symbol_graph_json(&merge_symbol_graphs(&symbol_graphs)?, &target_dir)?;
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
    let mut symbol_graph = None;
    let mut explicit_symbol_graph = false;
    let mut ts_files = Vec::new();
    let mut ts_dirs = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--contract" => contract = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--symbol-graph" => {
                symbol_graph = args.next().map(PathBuf::from);
                explicit_symbol_graph = true;
            }
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
    let symbol_graph = symbol_graph.or_else(|| default_symbol_graph_for_usage_out(&out));
    if let Some(symbol_graph) = symbol_graph {
        if symbol_graph.is_file() {
            update_symbol_graph_usage_locations(&symbol_graph, &index)?;
        } else if explicit_symbol_graph {
            return Err(format!(
                "api check-usages --symbol-graph path does not exist: {}",
                symbol_graph.display()
            ));
        }
    }
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

#[derive(Debug, Default, serde::Deserialize)]
struct RustTsPackageMetadata {
    ts_package: Option<String>,
    api_root: Option<String>,
    features: Option<Vec<String>>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct WorkspaceContractGraph {
    packages: Vec<WorkspaceContractPackage>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct WorkspaceContractPackage {
    cargo_package: String,
    ts_package: String,
    contract_path: String,
    dependencies: Vec<String>,
}

#[derive(Debug)]
struct ResolvedCollectTarget {
    ts_package_name: String,
    api_root: Vec<String>,
    features: Vec<String>,
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
    workspace: bool,
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
            workspace: false,
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
                "--workspace" => options.workspace = true,
                other => return Err(format!("unknown api collect argument `{other}`")),
            }
        }

        options.features.sort();
        options.features.dedup();
        Ok(options)
    }
}

fn collect_cargo_contract(options: &CollectOptions) -> Result<ApiContract, String> {
    let metadata = load_cargo_metadata(&options.manifest_path)?;
    let package = resolve_collect_package(&metadata, options.cargo_package.as_deref())?;
    let (_, contract) = collect_package_contract(options, &metadata, package)?;
    Ok(contract)
}

fn collect_package_contract(
    options: &CollectOptions,
    metadata: &cargo_metadata::Metadata,
    package: &cargo_metadata::Package,
) -> Result<(ResolvedCollectTarget, ApiContract), String> {
    let package_metadata = package_rust_ts_metadata(package)?;
    let lib_crate_name = package_lib_crate_name(package)?;
    let resolved = resolve_collect_target(options, package, package_metadata, &lib_crate_name)?;
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
        &resolved,
        options.no_default_features,
        &metadata,
    )?;
    let mut contract = run_temp_collector(&collector_dir)?;
    refine_contract_source_ranges(&mut contract, metadata, package, &lib_crate_name)?;
    Ok((resolved, contract))
}

#[derive(Debug, Default)]
struct RustSourceIndex {
    functions: BTreeMap<Vec<String>, SourceRange>,
    endpoint_params: BTreeMap<Vec<String>, SourceRange>,
    types: BTreeMap<Vec<String>, SourceRange>,
    fields: BTreeMap<Vec<String>, SourceRange>,
    variants: BTreeMap<Vec<String>, SourceRange>,
}

fn refine_contract_source_ranges(
    contract: &mut ApiContract,
    metadata: &cargo_metadata::Metadata,
    package: &cargo_metadata::Package,
    lib_crate_name: &str,
) -> Result<(), String> {
    let source_index = RustSourceIndex::for_package(metadata, package, lib_crate_name)?;

    for endpoint in &mut contract.endpoints {
        if let Some(source) = source_index.functions.get(&endpoint.rust_path) {
            endpoint.source = source.clone();
        }
        let endpoint_path = endpoint.rust_path.clone();
        for field in endpoint
            .request
            .path_params
            .iter_mut()
            .chain(endpoint.request.query_params.iter_mut())
        {
            refine_field_source(
                field,
                &symbol_path(&endpoint_path, &[&field.rust_name]),
                &source_index,
            );
        }
    }

    for type_def in &mut contract.types {
        if let Some(source) = source_index.types.get(&type_def.rust_path) {
            type_def.source = source.clone();
        }
        let type_path = type_def.rust_path.clone();
        match &mut type_def.shape {
            TypeShape::Struct(shape) => {
                for field in &mut shape.fields {
                    refine_field_source(
                        field,
                        &symbol_path(&type_path, &[&field.rust_name]),
                        &source_index,
                    );
                }
            }
            TypeShape::Enum(shape) => {
                for variant in &mut shape.variants {
                    let variant_path = symbol_path(&type_path, &[&variant.rust_name]);
                    if let Some(source) = source_index.variants.get(&variant_path) {
                        variant.source = source.clone();
                    }
                    for field in &mut variant.fields {
                        refine_field_source(
                            field,
                            &symbol_path(&variant_path, &[&field.rust_name]),
                            &source_index,
                        );
                    }
                }
            }
            TypeShape::Primitive(_)
            | TypeShape::Newtype(_)
            | TypeShape::Tuple(_)
            | TypeShape::List(_)
            | TypeShape::Map { .. }
            | TypeShape::Option(_)
            | TypeShape::External(_) => {}
        }
    }

    for error in &mut contract.errors {
        if let Some(source) = source_index.types.get(&error.rust_path) {
            error.source = source.clone();
        }
        let error_path = error.rust_path.clone();
        for variant in &mut error.variants {
            let variant_path = symbol_path(&error_path, &[&variant.rust_name]);
            if let Some(source) = source_index.variants.get(&variant_path) {
                variant.source = source.clone();
            }
            for field in &mut variant.fields {
                refine_field_source(
                    field,
                    &symbol_path(&variant_path, &[&field.rust_name]),
                    &source_index,
                );
            }
        }
    }

    Ok(())
}

fn refine_field_source(field: &mut Field, path: &[String], source_index: &RustSourceIndex) {
    if let Some(source) = source_index
        .fields
        .get(path)
        .or_else(|| source_index.endpoint_params.get(path))
    {
        field.source = source.clone();
    }
}

impl RustSourceIndex {
    fn for_package(
        metadata: &cargo_metadata::Metadata,
        package: &cargo_metadata::Package,
        lib_crate_name: &str,
    ) -> Result<Self, String> {
        let lib_target = package_lib_target(package)?;
        let root = lib_target.src_path.clone().into_std_path_buf();
        let workspace_root = metadata.workspace_root.clone().into_std_path_buf();
        let mut index = Self::default();
        let mut visited = BTreeSet::new();
        index.index_file(
            &root,
            &[lib_crate_name.to_owned()],
            &workspace_root,
            &mut visited,
        )?;
        Ok(index)
    }

    fn index_file(
        &mut self,
        file: &Path,
        module_path: &[String],
        workspace_root: &Path,
        visited: &mut BTreeSet<PathBuf>,
    ) -> Result<(), String> {
        let canonical = fs::canonicalize(file).map_err(|error| {
            format!(
                "failed to resolve Rust source file `{}`: {error}",
                file.display()
            )
        })?;
        if !visited.insert(canonical) {
            return Ok(());
        }

        let contents = fs::read_to_string(file).map_err(|error| {
            format!(
                "failed to read Rust source file `{}`: {error}",
                file.display()
            )
        })?;
        let parsed = syn::parse_file(&contents).map_err(|error| {
            format!(
                "failed to parse Rust source file `{}`: {error}",
                file.display()
            )
        })?;
        let source_file = source_file_label(file, workspace_root);
        self.index_items(
            &parsed.items,
            module_path,
            file,
            &contents,
            &source_file,
            workspace_root,
            visited,
        )
    }

    fn index_items(
        &mut self,
        items: &[syn::Item],
        module_path: &[String],
        file: &Path,
        contents: &str,
        source_file: &str,
        workspace_root: &Path,
        visited: &mut BTreeSet<PathBuf>,
    ) -> Result<(), String> {
        for item in items {
            match item {
                syn::Item::Fn(item_fn) => {
                    self.index_function(module_path, item_fn, contents, source_file);
                }
                syn::Item::Struct(item_struct) => {
                    self.index_struct(module_path, item_struct, contents, source_file);
                }
                syn::Item::Enum(item_enum) => {
                    self.index_enum(module_path, item_enum, contents, source_file);
                }
                syn::Item::Mod(item_mod) => {
                    let child_path = symbol_path(module_path, &[&item_mod.ident.to_string()]);
                    if let Some((_, items)) = &item_mod.content {
                        self.index_items(
                            items,
                            &child_path,
                            file,
                            contents,
                            source_file,
                            workspace_root,
                            visited,
                        )?;
                    } else if let Some(module_file) =
                        resolve_module_file(file, &item_mod.ident.to_string())
                    {
                        self.index_file(&module_file, &child_path, workspace_root, visited)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn index_function(
        &mut self,
        module_path: &[String],
        item_fn: &syn::ItemFn,
        contents: &str,
        source_file: &str,
    ) {
        let function_path = symbol_path(module_path, &[&item_fn.sig.ident.to_string()]);
        if let Some(source) = source_range_from_spans(
            source_file,
            contents,
            item_fn.sig.ident.span(),
            item_fn.span(),
        ) {
            self.functions.insert(function_path.clone(), source);
        }

        for input in &item_fn.sig.inputs {
            let syn::FnArg::Typed(input) = input else {
                continue;
            };
            let syn::Pat::Ident(pat_ident) = input.pat.as_ref() else {
                continue;
            };
            if let Some(source) =
                source_range_from_spans(source_file, contents, pat_ident.ident.span(), input.span())
            {
                self.endpoint_params.insert(
                    symbol_path(&function_path, &[&pat_ident.ident.to_string()]),
                    source,
                );
            }
        }
    }

    fn index_struct(
        &mut self,
        module_path: &[String],
        item_struct: &syn::ItemStruct,
        contents: &str,
        source_file: &str,
    ) {
        let type_path = symbol_path(module_path, &[&item_struct.ident.to_string()]);
        if let Some(source) = source_range_from_spans(
            source_file,
            contents,
            item_struct.ident.span(),
            item_struct.span(),
        ) {
            self.types.insert(type_path.clone(), source);
        }
        if let syn::Fields::Named(fields) = &item_struct.fields {
            for field in &fields.named {
                let Some(ident) = &field.ident else {
                    continue;
                };
                if let Some(source) =
                    source_range_from_spans(source_file, contents, ident.span(), field.span())
                {
                    self.fields
                        .insert(symbol_path(&type_path, &[&ident.to_string()]), source);
                }
            }
        }
    }

    fn index_enum(
        &mut self,
        module_path: &[String],
        item_enum: &syn::ItemEnum,
        contents: &str,
        source_file: &str,
    ) {
        let type_path = symbol_path(module_path, &[&item_enum.ident.to_string()]);
        if let Some(source) = source_range_from_spans(
            source_file,
            contents,
            item_enum.ident.span(),
            item_enum.span(),
        ) {
            self.types.insert(type_path.clone(), source);
        }

        for variant in &item_enum.variants {
            let variant_path = symbol_path(&type_path, &[&variant.ident.to_string()]);
            if let Some(source) =
                source_range_from_spans(source_file, contents, variant.ident.span(), variant.span())
            {
                self.variants.insert(variant_path.clone(), source);
            }
            if let syn::Fields::Named(fields) = &variant.fields {
                for field in &fields.named {
                    let Some(ident) = &field.ident else {
                        continue;
                    };
                    if let Some(source) =
                        source_range_from_spans(source_file, contents, ident.span(), field.span())
                    {
                        self.fields
                            .insert(symbol_path(&variant_path, &[&ident.to_string()]), source);
                    }
                }
            }
        }
    }
}

fn source_range_from_spans(
    file: &str,
    contents: &str,
    name_span: proc_macro2::Span,
    full_span: proc_macro2::Span,
) -> Option<SourceRange> {
    let name = source_span_from_span(contents, name_span)?;
    let full = source_span_from_span(contents, full_span).unwrap_or(name);
    Some(SourceRange {
        file: file.to_owned(),
        start_line: name.start_line,
        start_column: name.start_column,
        end_line: name.end_line,
        end_column: name.end_column,
        full_range: Some(full),
    })
}

fn source_span_from_span(contents: &str, span: proc_macro2::Span) -> Option<SourceSpan> {
    let start = span.start();
    let end = span.end();
    if start.line == 0 || end.line == 0 {
        return None;
    }
    Some(SourceSpan {
        start_line: u32::try_from(start.line).ok()?,
        start_column: utf16_column(contents, start.line, start.column)?,
        end_line: u32::try_from(end.line).ok()?,
        end_column: utf16_column(contents, end.line, end.column)?,
    })
}

fn utf16_column(contents: &str, line_number: usize, byte_column: usize) -> Option<u32> {
    let line = contents.lines().nth(line_number.checked_sub(1)?)?;
    let prefix = line.get(..byte_column).unwrap_or(line);
    let utf16_units = prefix
        .chars()
        .map(|character| u32::try_from(character.len_utf16()).unwrap_or(1))
        .sum::<u32>();
    Some(utf16_units + 1)
}

fn source_file_label(file: &Path, workspace_root: &Path) -> String {
    let path = file.strip_prefix(workspace_root).unwrap_or(file);
    normalize_path(path)
}

fn resolve_module_file(current_file: &Path, module_name: &str) -> Option<PathBuf> {
    let parent = current_file.parent()?;
    let stem = current_file.file_stem()?.to_string_lossy();
    let module_base = if stem == "lib" || stem == "mod" {
        parent.to_path_buf()
    } else {
        parent.join(stem.as_ref())
    };
    [
        module_base.join(format!("{module_name}.rs")),
        module_base.join(module_name).join("mod.rs"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn symbol_path(base: &[String], additions: &[&str]) -> Vec<String> {
    base.iter()
        .cloned()
        .chain(additions.iter().map(|addition| (*addition).to_owned()))
        .collect()
}

fn load_cargo_metadata(manifest_path: &Path) -> Result<cargo_metadata::Metadata, String> {
    cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest_path)
        .exec()
        .map_err(|error| {
            format!(
                "api collect could not inspect `{}`: {error}",
                manifest_path.display()
            )
        })
}

fn collect_workspace_contracts(options: &CollectOptions) -> Result<PathBuf, String> {
    let metadata = load_cargo_metadata(&options.manifest_path)?;
    let packages = api_enabled_packages(&metadata)?;
    if packages.is_empty() {
        return Err(
            "api collect --workspace found no packages with [package.metadata.rust_ts]".to_owned(),
        );
    }

    let target_dir = options
        .target_dir
        .clone()
        .unwrap_or_else(|| metadata.target_directory.clone().into_std_path_buf());
    let contract_dir = target_dir.join("api-contract").join("contracts");
    fs::create_dir_all(&contract_dir).map_err(|error| {
        format!(
            "failed to create workspace contract directory `{}`: {error}",
            contract_dir.display()
        )
    })?;

    let api_package_names = packages
        .iter()
        .map(|package| package.name.to_string())
        .collect::<BTreeSet<_>>();
    let mut graph_packages = Vec::new();

    for package in packages {
        let (resolved, contract) = collect_package_contract(options, &metadata, package)?;
        let contract_path =
            contract_dir.join(format!("{}.json", sanitize_package_dir_name(&package.name)));
        write_contract_json(&contract, &contract_path).map_err(|error| {
            format!(
                "failed to write workspace contract `{}`: {error}",
                contract_path.display()
            )
        })?;

        graph_packages.push(WorkspaceContractPackage {
            cargo_package: package.name.to_string(),
            ts_package: resolved.ts_package_name,
            contract_path: contract_path.display().to_string(),
            dependencies: api_dependency_edges(package, &api_package_names),
        });
    }

    graph_packages.sort_by(|left, right| left.cargo_package.cmp(&right.cargo_package));
    let graph = WorkspaceContractGraph {
        packages: graph_packages,
    };
    let graph_path = target_dir
        .join("api-contract")
        .join("workspace-contract.json");
    if let Some(parent) = graph_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create workspace graph directory `{}`: {error}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(&graph)
        .map_err(|error| format!("failed to serialize workspace contract graph: {error}"))?;
    fs::write(&graph_path, json).map_err(|error| {
        format!(
            "failed to write workspace contract graph `{}`: {error}",
            graph_path.display()
        )
    })?;
    Ok(graph_path)
}

fn api_dependency_edges(
    package: &cargo_metadata::Package,
    api_package_names: &BTreeSet<String>,
) -> Vec<String> {
    let mut dependencies = package
        .dependencies
        .iter()
        .filter_map(|dependency| {
            api_package_names
                .contains(dependency.name.as_str())
                .then(|| dependency.name.to_string())
        })
        .collect::<Vec<_>>();
    dependencies.sort();
    dependencies.dedup();
    dependencies
}

fn resolve_collect_package<'a>(
    metadata: &'a cargo_metadata::Metadata,
    package_name: Option<&str>,
) -> Result<&'a cargo_metadata::Package, String> {
    if let Some(package_name) = package_name {
        return metadata
            .workspace_packages()
            .into_iter()
            .find(|package| package.name == package_name)
            .ok_or_else(|| {
                format!("api collect could not find workspace package `{package_name}`")
            });
    }

    let api_packages = api_enabled_packages(metadata)?;
    match api_packages.as_slice() {
        [package] => Ok(*package),
        [] => Err("api collect requires --package <cargo-package> because no workspace package declares [package.metadata.rust_ts]".to_owned()),
        packages => Err(format!(
            "api collect found multiple API-enabled packages ({}) ; pass --package <cargo-package>",
            packages
                .iter()
                .map(|package| package.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn api_enabled_packages<'a>(
    metadata: &'a cargo_metadata::Metadata,
) -> Result<Vec<&'a cargo_metadata::Package>, String> {
    metadata
        .workspace_packages()
        .into_iter()
        .filter_map(|package| match package_rust_ts_metadata(package) {
            Ok(Some(_)) => Some(Ok(package)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn package_rust_ts_metadata(
    package: &cargo_metadata::Package,
) -> Result<Option<RustTsPackageMetadata>, String> {
    let Some(value) = package.metadata.get("rust_ts") else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|error| {
            format!(
                "invalid [package.metadata.rust_ts] for package `{}`: {error}",
                package.name
            )
        })
}

fn resolve_collect_target(
    options: &CollectOptions,
    package: &cargo_metadata::Package,
    package_metadata: Option<RustTsPackageMetadata>,
    lib_crate_name: &str,
) -> Result<ResolvedCollectTarget, String> {
    let package_metadata = package_metadata.unwrap_or_default();
    let ts_package_name = options
        .package_name
        .clone()
        .or(package_metadata.ts_package)
        .ok_or_else(|| {
            format!(
                "api collect package `{}` requires --package-name <ts-package-name> or package.metadata.rust_ts.ts_package",
                package.name
            )
        })?;
    let api_root = options
        .api_root
        .clone()
        .or(package_metadata.api_root)
        .ok_or_else(|| {
            format!(
                "api collect package `{}` requires --api-root <path::to::api> or package.metadata.rust_ts.api_root",
                package.name
            )
        })?;
    let features = if options.all_features {
        let mut features = package.features.keys().cloned().collect::<Vec<_>>();
        features.sort();
        features
    } else if options.features.is_empty() {
        package_metadata.features.unwrap_or_default()
    } else {
        options.features.clone()
    };

    Ok(ResolvedCollectTarget {
        ts_package_name,
        api_root: normalize_api_root(&api_root, lib_crate_name)?,
        features,
    })
}

fn package_lib_crate_name(package: &cargo_metadata::Package) -> Result<String, String> {
    package_lib_target(package).map(|target| target.name.replace('-', "_"))
}

fn package_lib_target(
    package: &cargo_metadata::Package,
) -> Result<&cargo_metadata::Target, String> {
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
    resolved: &ResolvedCollectTarget,
    no_default_features: bool,
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
    let default_features = if no_default_features {
        "default-features = false\n"
    } else {
        ""
    };
    let features = if resolved.features.is_empty() {
        String::new()
    } else {
        format!(
            "features = [{}]\n",
            resolved
                .features
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
    let root_call = format!("target_api_package::{}()", resolved.api_root.join("::"));
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
        ts_package_name = rust_string(&resolved.ts_package_name),
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

fn json_string(value: &str) -> String {
    toml_string(value)
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

fn read_workspace_contract_graph(path: &Path) -> Result<WorkspaceContractGraph, String> {
    let contents = fs::read_to_string(path).map_err(|error| {
        format!(
            "failed to read workspace contract graph `{}`: {error}",
            path.display()
        )
    })?;
    serde_json::from_str(&contents).map_err(|error| {
        format!(
            "failed to parse workspace contract graph `{}`: {error}",
            path.display()
        )
    })
}

fn workspace_generation_order(
    graph: &WorkspaceContractGraph,
) -> Result<Vec<&WorkspaceContractPackage>, String> {
    let by_package = graph
        .packages
        .iter()
        .map(|package| (package.cargo_package.as_str(), package))
        .collect::<BTreeMap<_, _>>();
    let mut temporary = BTreeSet::new();
    let mut permanent = BTreeSet::new();
    let mut ordered = Vec::new();

    for package in &graph.packages {
        visit_workspace_package(
            package.cargo_package.as_str(),
            &by_package,
            &mut temporary,
            &mut permanent,
            &mut ordered,
        )?;
    }

    Ok(ordered)
}

fn visit_workspace_package<'a>(
    package_name: &'a str,
    by_package: &BTreeMap<&'a str, &'a WorkspaceContractPackage>,
    temporary: &mut BTreeSet<&'a str>,
    permanent: &mut BTreeSet<&'a str>,
    ordered: &mut Vec<&'a WorkspaceContractPackage>,
) -> Result<(), String> {
    if permanent.contains(package_name) {
        return Ok(());
    }
    if !temporary.insert(package_name) {
        return Err(format!(
            "workspace contract graph contains a dependency cycle at `{package_name}`"
        ));
    }

    let package = by_package.get(package_name).ok_or_else(|| {
        format!("workspace contract graph references unknown package `{package_name}`")
    })?;
    for dependency in &package.dependencies {
        if by_package.contains_key(dependency.as_str()) {
            visit_workspace_package(dependency, by_package, temporary, permanent, ordered)?;
        }
    }

    temporary.remove(package_name);
    permanent.insert(package_name);
    ordered.push(*package);
    Ok(())
}

fn write_workspace_tsconfig_paths(
    graph: &WorkspaceContractGraph,
    target_dir: &Path,
) -> Result<(), String> {
    let path = target_dir
        .join("api-contract")
        .join("effect-v4")
        .join("tsconfig.paths.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create workspace TypeScript paths directory `{}`: {error}",
                parent.display()
            )
        })?;
    }

    let mut entries = Vec::new();
    for package in &graph.packages {
        let package_dir = api_gen_effect_v4::generated_package_dir(target_dir, &package.ts_package);
        let package_dir = normalize_path(&package_dir);
        entries.push(format!(
            "      {package_name}: [\n        {index_path}\n      ]",
            package_name = json_string(&package.ts_package),
            index_path = json_string(&format!("{package_dir}/index.ts")),
        ));
        entries.push(format!(
            "      {package_glob}: [\n        {package_dir_glob}\n      ]",
            package_glob = json_string(&format!("{}/*", package.ts_package)),
            package_dir_glob = json_string(&format!("{package_dir}/*")),
        ));
    }

    let contents = format!(
        "{{\n  \"compilerOptions\": {{\n    \"paths\": {{\n{}\n    }}\n  }}\n}}\n",
        entries.join(",\n")
    );
    fs::write(&path, contents).map_err(|error| {
        format!(
            "failed to write workspace TypeScript paths `{}`: {error}",
            path.display()
        )
    })
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

fn write_symbol_graph_json(symbol_graph: &str, target_dir: &Path) -> Result<(), String> {
    let path = target_dir.join("api-contract").join("rust-ts-symbols.json");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create symbol graph directory `{}`: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&path, symbol_graph)
        .map_err(|error| format!("failed to write symbol graph `{}`: {error}", path.display()))
}

fn default_symbol_graph_for_usage_out(out: &Path) -> Option<PathBuf> {
    let parent = out.parent()?;
    let graph_dir = parent.file_name()?.to_string_lossy();
    if graph_dir != "graph" {
        return None;
    }
    Some(parent.parent()?.join("rust-ts-symbols.json"))
}

fn update_symbol_graph_usage_locations(
    symbol_graph_path: &Path,
    index: &EffectUsageIndex,
) -> Result<(), String> {
    let contents = fs::read_to_string(symbol_graph_path).map_err(|error| {
        format!(
            "failed to read symbol graph `{}`: {error}",
            symbol_graph_path.display()
        )
    })?;
    let mut value = serde_json::from_str::<serde_json::Value>(&contents).map_err(|error| {
        format!(
            "failed to parse symbol graph `{}`: {error}",
            symbol_graph_path.display()
        )
    })?;
    let symbols = value
        .get_mut("symbols")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| {
            format!(
                "symbol graph `{}` is missing a symbols array",
                symbol_graph_path.display()
            )
        })?;

    let mut summaries = BTreeMap::<String, u64>::new();
    for endpoint in &index.endpoints {
        summaries.insert(endpoint.endpoint_id.as_str().to_owned(), endpoint.strong);
    }

    let mut usages = BTreeMap::<String, Vec<&EndpointUsage>>::new();
    for usage in &index.usages {
        usages
            .entry(usage.endpoint_id.as_str().to_owned())
            .or_default()
            .push(usage);
    }

    for symbol in symbols {
        let Some(symbol_object) = symbol.as_object_mut() else {
            continue;
        };
        if symbol_object
            .get("kind")
            .and_then(serde_json::Value::as_str)
            != Some("endpoint")
        {
            continue;
        }
        let Some(id) = symbol_object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };

        if let Some(strong) = summaries.get(&id) {
            let metadata = symbol_object
                .entry("metadata")
                .or_insert_with(|| serde_json::json!({}));
            let Some(metadata) = metadata.as_object_mut() else {
                continue;
            };
            metadata.insert("usageCount".to_owned(), serde_json::json!(strong));
        }

        let typescript = symbol_object
            .entry("typescript")
            .or_insert_with(|| serde_json::json!([]));
        let Some(typescript) = typescript.as_array_mut() else {
            continue;
        };
        typescript.retain(|location| {
            location
                .get("generated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        });
        if let Some(endpoint_usages) = usages.get(&id) {
            for usage in endpoint_usages {
                typescript.push(usage_location_json(usage));
            }
        }
    }

    let updated = serde_json::to_string_pretty(&value)
        .map_err(|error| format!("failed to serialize symbol graph: {error}"))?;
    fs::write(symbol_graph_path, updated).map_err(|error| {
        format!(
            "failed to write symbol graph `{}`: {error}",
            symbol_graph_path.display()
        )
    })
}

fn usage_location_json(usage: &EndpointUsage) -> serde_json::Value {
    let range = source_range_json(&usage.source);
    serde_json::json!({
        "file": usage.file.clone(),
        "range": range.clone(),
        "nameRange": range.clone(),
        "fullRange": range,
        "generated": false,
        "renamable": false
    })
}

fn source_range_json(source: &api_ir::SourceRange) -> serde_json::Value {
    serde_json::json!({
        "start": {
            "line": source.start_line.saturating_sub(1),
            "character": source.start_column.saturating_sub(1)
        },
        "end": {
            "line": source.end_line.saturating_sub(1),
            "character": source.end_column.saturating_sub(1)
        }
    })
}

fn merge_symbol_graphs(graphs: &[String]) -> Result<String, String> {
    let mut symbols = Vec::new();
    for graph in graphs {
        let value = serde_json::from_str::<serde_json::Value>(graph)
            .map_err(|error| format!("failed to parse generated symbol graph: {error}"))?;
        let graph_symbols = value
            .get("symbols")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| "generated symbol graph is missing a symbols array".to_owned())?;
        symbols.extend(graph_symbols.iter().cloned());
    }
    symbols.sort_by(|left, right| {
        let left_key = (
            left.get("kind").and_then(serde_json::Value::as_str),
            left.get("id").and_then(serde_json::Value::as_str),
        );
        let right_key = (
            right.get("kind").and_then(serde_json::Value::as_str),
            right.get("id").and_then(serde_json::Value::as_str),
        );
        left_key.cmp(&right_key)
    });
    serde_json::to_string_pretty(&serde_json::json!({ "symbols": symbols }))
        .map_err(|error| format!("failed to serialize merged symbol graph: {error}"))
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
        "  api collect --package <cargo-package> --out <path> [--api-root <path::to::api>] [--package-name <ts-package>] [--manifest-path <Cargo.toml>] [--target-dir <dir>] [--features <list>] [--all-features] [--no-default-features]",
        "  api collect --workspace [--manifest-path <Cargo.toml>] [--target-dir <dir>]",
        "  api collect --empty --package-name <ts-package> --out <path>",
        "  api gen --contract <path> [--target-dir <dir>]",
        "  api gen --workspace-contract <path> [--target-dir <dir>]",
        "  api watch --contract <path> [--target-dir <dir>] [--once]",
        "  api check --contract <path> [--target-dir <dir>]",
        "  api check-usages --contract <path> --out <path> [--symbol-graph <path>] [--ts <file>] [--ts-dir <dir>]",
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
        let symbol_graph = fs::read_to_string(target_dir.join("api-contract/rust-ts-symbols.json"))
            .expect("read symbol graph");
        assert!(symbol_graph.contains("\"kind\": \"endpoint\""));
        assert!(symbol_graph.contains("fixture:endpoint:getUser"));
        assert!(symbol_graph.contains("\"generated\": true"));
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
    fn check_usages_enriches_symbol_graph_with_user_locations() {
        let root = test_root("check-usages-symbol-graph");
        fs::create_dir_all(root.join("src")).expect("create src");
        let contract_path = root.join("contract.json");
        let target_dir = root.join("target");
        let usage_path = target_dir.join("api-contract/graph/effect-usage-index.json");
        let ts_path = root.join("src/client.ts");
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
        .expect("check usages");

        let symbol_graph = fs::read_to_string(target_dir.join("api-contract/rust-ts-symbols.json"))
            .expect("read symbol graph");
        assert!(symbol_graph.contains("\"usageCount\": 1"));
        assert!(symbol_graph.contains("src/client.ts"));
        assert!(symbol_graph.contains("\"generated\": false"));
        assert!(symbol_graph.contains("\"renamable\": false"));
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

[package.metadata.rust_ts]
ts_package = "@metadata/server-api"
api_root = "server::api"
features = ["extra"]

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
        assert_eq!(contract.package_name, "@metadata/server-api");
        assert!(type_names.contains(&"User"));
        assert!(type_names.contains(&"AuditEvent"));
        assert_eq!(contract.errors[0].rust_name, "GetUserError");

        let override_contract_path = root.join("override-contract.json");
        run_with_args(vec![
            "collect".to_owned(),
            "--manifest-path".to_owned(),
            root.join("Cargo.toml").display().to_string(),
            "--target-dir".to_owned(),
            root.join("target").display().to_string(),
            "--package".to_owned(),
            "server".to_owned(),
            "--api-root".to_owned(),
            "api".to_owned(),
            "--package-name".to_owned(),
            "@override/server-api".to_owned(),
            "--out".to_owned(),
            override_contract_path.display().to_string(),
        ])
        .expect("collect real contract with CLI overrides");

        let override_contract: ApiContract = serde_json::from_str(
            &fs::read_to_string(override_contract_path).expect("read override contract"),
        )
        .expect("parse override contract");
        assert_eq!(override_contract.package_name, "@override/server-api");
    }

    #[test]
    fn collect_refines_rust_identifier_ranges_for_symbol_graph() {
        let root = test_root("collect-ranges");
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

[package.metadata.rust_ts]
ts_package = "@workspace/server-api"
api_root = "server::api"

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
            r#"pub mod users;

pub fn api() -> api_core::ApiModule {
    users::api()
}
"#,
        )
        .expect("write lib");
        let users_rs = r#"use api_core::{api_module, ApiModule, ApiType, Json, Path};

#[derive(api_macros::ApiType)]
pub struct User {
    pub id: i64,
    pub display_name: String,
}

#[derive(api_macros::ApiError)]
pub enum GetUserError {
    #[api_error(status = 404)]
    UserNotFound { id: i64 },
}

#[api_macros::api(method = "GET", path = "/users/{id}")]
pub async fn get_user(id: Path<i64>) -> Result<Json<User>, GetUserError> {
    let _ = id;
    todo!()
}

pub fn api() -> ApiModule {
    api_module!(name = "server", endpoints = [get_user])
}
"#;
        fs::write(server_dir.join("src/users.rs"), users_rs).expect("write users module");

        let contract_path = root.join("contract.json");
        let target_dir = root.join("target");
        run_with_args(vec![
            "collect".to_owned(),
            "--manifest-path".to_owned(),
            root.join("Cargo.toml").display().to_string(),
            "--target-dir".to_owned(),
            target_dir.display().to_string(),
            "--package".to_owned(),
            "server".to_owned(),
            "--out".to_owned(),
            contract_path.display().to_string(),
        ])
        .expect("collect contract with refined ranges");
        run_with_args(vec![
            "gen".to_owned(),
            "--contract".to_owned(),
            contract_path.display().to_string(),
            "--target-dir".to_owned(),
            target_dir.display().to_string(),
        ])
        .expect("generate symbol graph");

        let graph: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(target_dir.join("api-contract/rust-ts-symbols.json"))
                .expect("read symbol graph"),
        )
        .expect("parse symbol graph");
        let display_name = find_symbol_by_rust_path(
            &graph,
            "field",
            &["server", "users", "User", "display_name"],
        );
        assert_eq!(display_name["rust"]["file"], "server/src/users.rs");
        assert_eq!(
            display_name["rust"]["range"],
            expected_lsp_range(users_rs, "display_name")
        );
        assert!(
            display_name["rust"]["fullRange"]["start"]["character"]
                .as_u64()
                .expect("full range start character")
                < display_name["rust"]["range"]["start"]["character"]
                    .as_u64()
                    .expect("name range start character")
        );

        let error_tag = find_symbol_by_rust_path(
            &graph,
            "errorTag",
            &["server", "users", "GetUserError", "UserNotFound"],
        );
        assert_eq!(
            error_tag["rust"]["range"],
            expected_lsp_range(users_rs, "UserNotFound")
        );
        assert!(
            error_tag["rust"]["fullRange"]["start"]["line"]
                .as_u64()
                .expect("full range start line")
                <= error_tag["rust"]["range"]["start"]["line"]
                    .as_u64()
                    .expect("name range start line")
        );
    }

    #[test]
    fn collect_reports_multiple_metadata_packages_when_package_is_omitted() {
        let root = test_root("collect-multiple-metadata");
        for package in ["server", "admin"] {
            let package_dir = root.join(package);
            fs::create_dir_all(package_dir.join("src")).expect("create package src");
            fs::write(
                package_dir.join("Cargo.toml"),
                format!(
                    r#"[package]
name = "{package}"
version = "0.0.0"
edition = "2021"

[package.metadata.rust_ts]
ts_package = "@workspace/{package}-api"
api_root = "{package}::api"
"#
                ),
            )
            .expect("write package manifest");
            fs::write(package_dir.join("src/lib.rs"), "").expect("write package lib");
        }
        fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["server", "admin"]
resolver = "2"
"#,
        )
        .expect("write workspace manifest");

        let error = run_with_args(vec![
            "collect".to_owned(),
            "--manifest-path".to_owned(),
            root.join("Cargo.toml").display().to_string(),
            "--out".to_owned(),
            root.join("contract.json").display().to_string(),
        ])
        .expect_err("ambiguous metadata packages should require --package");

        assert!(error.contains("multiple API-enabled packages"));
        assert!(error.contains("server"));
        assert!(error.contains("admin"));
    }

    #[test]
    fn workspace_collect_writes_graph_and_gen_uses_dependency_order() {
        let root = test_root("workspace-graph");
        fs::create_dir_all(&root).expect("create workspace root");
        fs::write(
            root.join("Cargo.toml"),
            r#"[workspace]
members = ["server", "admin"]
resolver = "2"
"#,
        )
        .expect("write workspace manifest");
        write_workspace_api_package(
            &root,
            "server",
            "@workspace/server-api",
            &[],
            "ServerUser",
            "get_server_user",
            "/server/users",
        );
        write_workspace_api_package(
            &root,
            "admin",
            "@workspace/admin-api",
            &[("server", "../server")],
            "AdminUser",
            "get_admin_user",
            "/admin/users",
        );

        run_with_args(vec![
            "collect".to_owned(),
            "--workspace".to_owned(),
            "--manifest-path".to_owned(),
            root.join("Cargo.toml").display().to_string(),
            "--target-dir".to_owned(),
            root.join("target").display().to_string(),
        ])
        .expect("collect workspace contracts");

        let graph_path = root.join("target/api-contract/workspace-contract.json");
        let graph: WorkspaceContractGraph =
            serde_json::from_str(&fs::read_to_string(&graph_path).expect("read graph"))
                .expect("parse graph");
        let admin = graph
            .packages
            .iter()
            .find(|package| package.cargo_package == "admin")
            .expect("admin graph package");
        assert_eq!(graph.packages.len(), 2);
        assert_eq!(admin.dependencies, ["server"]);
        for package in &graph.packages {
            assert!(Path::new(&package.contract_path).is_file());
        }

        run_with_args(vec![
            "gen".to_owned(),
            "--workspace-contract".to_owned(),
            graph_path.display().to_string(),
            "--target-dir".to_owned(),
            root.join("target").display().to_string(),
        ])
        .expect("generate workspace packages");

        assert!(api_gen_effect_v4::generated_package_dir(
            &root.join("target"),
            "@workspace/server-api"
        )
        .join("index.ts")
        .is_file());
        assert!(api_gen_effect_v4::generated_package_dir(
            &root.join("target"),
            "@workspace/admin-api"
        )
        .join("index.ts")
        .is_file());
        let paths =
            fs::read_to_string(root.join("target/api-contract/effect-v4/tsconfig.paths.json"))
                .expect("read workspace tsconfig paths");
        assert!(paths.contains("@workspace/server-api"));
        assert!(paths.contains("@workspace/admin-api"));
        let symbol_graph =
            fs::read_to_string(root.join("target/api-contract/rust-ts-symbols.json"))
                .expect("read workspace symbol graph");
        assert!(symbol_graph.contains("get_server_user"));
        assert!(symbol_graph.contains("get_admin_user"));
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

    fn write_workspace_api_package(
        root: &Path,
        package: &str,
        ts_package: &str,
        dependencies: &[(&str, &str)],
        dto: &str,
        endpoint: &str,
        route: &str,
    ) {
        let package_dir = root.join(package);
        fs::create_dir_all(package_dir.join("src")).expect("create package src");
        let dependency_lines = dependencies
            .iter()
            .map(|(name, path)| format!("{name} = {{ path = {path} }}", path = toml_string(path)))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(
            package_dir.join("Cargo.toml"),
            format!(
                r#"[package]
name = "{package}"
version = "0.0.0"
edition = "2021"

[package.metadata.rust_ts]
ts_package = "{ts_package}"
api_root = "{package}::api"

[dependencies]
api-core = {{ path = {api_core_path} }}
api-macros = {{ path = {api_macros_path} }}
{dependency_lines}
"#,
                api_core_path =
                    toml_string(&repo_root().join("crates/api-core").display().to_string()),
                api_macros_path =
                    toml_string(&repo_root().join("crates/api-macros").display().to_string()),
            ),
        )
        .expect("write package manifest");
        fs::write(
            package_dir.join("src/lib.rs"),
            format!(
                r#"use api_core::{{api_module, ApiModule, Json}};

#[derive(api_macros::ApiType)]
pub struct {dto} {{
    id: i64,
}}

#[api_macros::api(method = "GET", path = "{route}")]
pub async fn {endpoint}() -> Json<{dto}> {{
    todo!()
}}

pub fn api() -> ApiModule {{
    api_module!(name = "{package}", endpoints = [{endpoint}])
}}
"#
            ),
        )
        .expect("write package lib");
    }

    fn find_symbol_by_rust_path<'a>(
        graph: &'a serde_json::Value,
        kind: &str,
        rust_path: &[&str],
    ) -> &'a serde_json::Value {
        let expected_path = serde_json::json!(rust_path);
        graph["symbols"]
            .as_array()
            .expect("symbols array")
            .iter()
            .find(|symbol| {
                symbol["kind"] == kind && symbol["metadata"]["rustPath"] == expected_path
            })
            .unwrap_or_else(|| panic!("missing {kind} symbol for rust path {rust_path:?}"))
    }

    fn expected_lsp_range(contents: &str, needle: &str) -> serde_json::Value {
        let offset = contents.find(needle).expect("needle appears in source");
        let start = lsp_position(contents, offset);
        let end = lsp_position(contents, offset + needle.len());
        serde_json::json!({
            "start": start,
            "end": end
        })
    }

    fn lsp_position(contents: &str, offset: usize) -> serde_json::Value {
        let mut line = 0_u32;
        let mut character = 0_u32;
        for (index, ch) in contents.char_indices() {
            if index >= offset {
                break;
            }
            if ch == '\n' {
                line += 1;
                character = 0;
            } else {
                character += u32::try_from(ch.len_utf16()).unwrap_or(1);
            }
        }
        serde_json::json!({
            "line": line,
            "character": character
        })
    }
}
