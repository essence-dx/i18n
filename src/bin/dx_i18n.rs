use dx_i18n::I18nError;
use dx_i18n::localization::{
    DX_LINGO_API_KEY_ENV, I18nLock, LINGO_API_KEY_ENV, LINGODOTDEV_API_KEY_ENV, LingoApiConfig,
    LingoApiProvider, LocalizationWorkspace, WorkspaceFilters, is_supported_raw_provider_id,
};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = dx_i18n::dx_config::I18nDxConfig::load();
    std::fs::create_dir_all(&cfg.sr_dir)?;
    std::fs::create_dir_all(&cfg.receipts_dir)?;
    cfg.write_sr("i18n", &[("tool", "i18n"), ("action", "run"), ("status", "ok")])?;
    cfg.write_global_sr("i18n", &[("tool", "i18n"), ("action", "run"), ("status", "ok")])?;
    if let Some(status) = cfg.read_status("i18n") {
        eprintln!("[i18n] sr cache verified: {} entries", status.len());
    }

    let args = Args::parse(env::args().skip(1).collect())
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
    let workspace = LocalizationWorkspace::load(&args.root)?;

    match args.command {
        Command::ShowConfig => {
            println!("{}", serde_json::to_string_pretty(workspace.config())?);
        }
        Command::Status {
            target_locales,
            force,
            verbose,
            filters,
        } => {
            let workspace_filters = filters.to_workspace_filters()?;
            let target_locales = resolve_target_locales(&workspace, &target_locales)?;
            let lockfile = load_lockfile(&args.root)?;
            let status = workspace.status_against_filtered_for_target_locales_with_force(
                &lockfile,
                &workspace_filters,
                &target_locales,
                force,
            )?;
            println!("source_files={}", status.source_file_count);
            println!("target_locales={}", target_locales.join(","));
            println!("total_units={}", status.total_units);
            println!("pending_units={}", status.pending_units);
            println!("target_drift_files={}", status.target_drift_files.len());
            println!("requires_cloud_auth={}", status.requires_cloud_auth);
            if verbose {
                println!("verbose=true");
                for file in &status.pending_files {
                    let relative_path = path_to_cli(&file.relative_path);
                    println!("pending_file={relative_path}");
                    for key in &file.pending_keys {
                        println!("pending_file_key={relative_path}::{key}");
                    }
                }
                for file in &status.target_drift_files {
                    println!("target_drift_file={}", path_to_cli(file));
                }
            }
            for bucket_type in status.unsupported_bucket_types {
                println!("unsupported_bucket={bucket_type}");
            }
            for key in status.pending_keys {
                println!("pending={key}");
            }
        }
        Command::Lockfile { mode } => {
            ensure_no_unsupported_buckets(&workspace, &WorkspaceFilters::default())?;
            let lockfile = workspace.build_lockfile()?;
            let rendered = lockfile.to_lingo_yaml();
            let path = args.root.join("i18n.lock");
            match mode {
                LockfileMode::Check => {
                    if !lockfile_matches(&path, &lockfile)? {
                        return Err(I18nError::ConfigError(
                            "i18n.lock is out of date; run dx-i18n lockfile --force".to_string(),
                        )
                        .into());
                    }
                    println!("i18n.lock is current");
                }
                LockfileMode::Write { force } => {
                    if !force && path.exists() {
                        if lockfile_matches(&path, &lockfile)? {
                            println!("i18n.lock is current");
                            return Ok(());
                        }

                        return Err(I18nError::ConfigError(
                            "i18n.lock already exists; use dx-i18n lockfile --force to rewrite"
                                .to_string(),
                        )
                        .into());
                    }

                    fs::write(path, rendered)?;
                    if force {
                        println!("rewrote i18n.lock");
                    } else {
                        println!("wrote i18n.lock");
                    }
                }
            }
        }
        Command::Run {
            target_locales,
            mode,
            frozen,
            force,
            api_key_override,
            filters,
        } => {
            let workspace_filters = filters.to_workspace_filters()?;
            match mode {
                RunMode::Local => {
                    ensure_no_unsupported_buckets(&workspace, &workspace_filters)?;
                    let target_locales = resolve_target_locales(&workspace, &target_locales)?;
                    ensure_frozen_if_requested(
                        frozen,
                        &args.root,
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                    ensure_run_filters_select_units(
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                    for target_locale in &target_locales {
                        for output in workspace
                            .render_local_json_filtered(&target_locale, &workspace_filters)?
                        {
                            write_or_verify_output(
                                &args.root,
                                &output.relative_path,
                                serde_json::to_string_pretty(&output.value)?,
                                frozen,
                            )?;
                        }
                        for output in workspace
                            .render_local_markdown_filtered(&target_locale, &workspace_filters)?
                        {
                            write_or_verify_output(
                                &args.root,
                                &output.relative_path,
                                output.contents,
                                frozen,
                            )?;
                        }
                    }
                    write_lockfile_after_successful_run(
                        frozen,
                        &args.root,
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                }
                RunMode::Lingo => {
                    ensure_no_unsupported_buckets(&workspace, &workspace_filters)?;
                    let target_locales = resolve_target_locales(&workspace, &target_locales)?;
                    ensure_frozen_if_requested(
                        frozen,
                        &args.root,
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                    ensure_run_filters_select_units(
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                    let lockfile = load_lockfile(&args.root)?;
                    let provider =
                        lingo_provider_from_env(&workspace, api_key_override.as_deref())?;
                    let runtime = tokio::runtime::Runtime::new()?;
                    let mut accepted_provider_files = Vec::new();
                    for target_locale in &target_locales {
                        let (json_outputs, markdown_outputs) = runtime.block_on(async {
                            let json_outputs = workspace
                                .render_provider_json_delta_filtered(
                                    &provider,
                                    &target_locale,
                                    &workspace_filters,
                                    &lockfile,
                                    force,
                                )
                                .await?;
                            let markdown_outputs = workspace
                                .render_provider_markdown_delta_filtered(
                                    &provider,
                                    &target_locale,
                                    &workspace_filters,
                                    &lockfile,
                                    force,
                                )
                                .await?;
                            dx_i18n::Result::Ok((json_outputs, markdown_outputs))
                        })?;

                        for output in json_outputs {
                            let accepted_keys = accepted_provider_translation_keys(
                                output.provider_response.as_ref(),
                            );
                            if accepted_keys.is_empty() {
                                continue;
                            }
                            accepted_provider_files.push(AcceptedProviderFile {
                                bucket_type: "json".to_string(),
                                target_locale: target_locale.clone(),
                                relative_path: output.relative_path.clone(),
                                keys: accepted_keys,
                            });
                            write_or_verify_output(
                                &args.root,
                                &output.relative_path,
                                serde_json::to_string_pretty(&output.value)?,
                                frozen,
                            )?;
                        }
                        for output in markdown_outputs {
                            let accepted_keys = accepted_provider_translation_keys(
                                output.provider_response.as_ref(),
                            );
                            if accepted_keys.is_empty() {
                                continue;
                            }
                            accepted_provider_files.push(AcceptedProviderFile {
                                bucket_type: "markdown".to_string(),
                                target_locale: target_locale.clone(),
                                relative_path: output.relative_path.clone(),
                                keys: accepted_keys,
                            });
                            write_or_verify_output(
                                &args.root,
                                &output.relative_path,
                                output.contents,
                                frozen,
                            )?;
                        }
                    }
                    write_lockfile_after_provider_run(
                        frozen,
                        &args.root,
                        &workspace,
                        &accepted_provider_files,
                    )?;
                }
                RunMode::RawProvider { id } => {
                    ensure_no_unsupported_buckets(&workspace, &workspace_filters)?;
                    let target_locales = resolve_target_locales(&workspace, &target_locales)?;
                    ensure_frozen_if_requested(
                        frozen,
                        &args.root,
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                    ensure_run_filters_select_units(
                        &workspace,
                        &workspace_filters,
                        &target_locales,
                    )?;
                    return Err(raw_provider_boundary_error(&id).into());
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AcceptedProviderFile {
    bucket_type: String,
    target_locale: String,
    relative_path: PathBuf,
    keys: Vec<String>,
}

fn accepted_provider_translation_keys(
    response: Option<&dx_i18n::localization::LocalizationResponse>,
) -> Vec<String> {
    response
        .map(|response| response.translations.keys().cloned().collect())
        .unwrap_or_default()
}

fn raw_provider_boundary_error(provider_id: &str) -> I18nError {
    I18nError::ConfigError(format!(
        "direct raw provider execution for '{provider_id}' is not implemented in the Rust CLI; use local mode for auth-free DX localization or --provider lingo for the Lingo.dev-compatible engine adapter"
    ))
}

fn lingo_provider_from_env(
    workspace: &LocalizationWorkspace,
    api_key_override: Option<&str>,
) -> Result<LingoApiProvider, Box<dyn std::error::Error>> {
    let mut config = lingo_config_from_env(api_key_override)?.ok_or_else(|| {
        I18nError::ApiKeyRequired(
            "Lingo.dev".to_string(),
            format!("{DX_LINGO_API_KEY_ENV}, {LINGO_API_KEY_ENV}, or {LINGODOTDEV_API_KEY_ENV}"),
        )
    })?;

    config.set_engine_id_if_missing(workspace.config().engine_id.as_deref());

    Ok(LingoApiProvider::new(config))
}

fn lingo_config_from_env(
    api_key_override: Option<&str>,
) -> Result<Option<LingoApiConfig>, Box<dyn std::error::Error>> {
    let Some(api_key_override) = api_key_override else {
        return Ok(LingoApiConfig::from_env()?);
    };
    if api_key_override.trim().is_empty() {
        return Err(
            I18nError::ApiKeyRequired("Lingo.dev".to_string(), "--api-key".to_string()).into(),
        );
    }

    Ok(LingoApiConfig::from_env_values(|key| {
        if key == DX_LINGO_API_KEY_ENV {
            Some(api_key_override.to_string())
        } else {
            env::var(key).ok()
        }
    })?)
}

fn ensure_no_unsupported_buckets(
    workspace: &LocalizationWorkspace,
    filters: &WorkspaceFilters,
) -> Result<(), Box<dyn std::error::Error>> {
    let unsupported = workspace.unsupported_bucket_types_filtered(filters);
    if unsupported.is_empty() {
        return Ok(());
    }

    Err(I18nError::ConfigError(format!(
        "unsupported localization bucket(s) for the Rust workspace: {}",
        unsupported.join(", ")
    ))
    .into())
}

fn ensure_frozen_if_requested(
    frozen: bool,
    root: &PathBuf,
    workspace: &LocalizationWorkspace,
    filters: &WorkspaceFilters,
    target_locales: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if !frozen {
        return Ok(());
    }

    let lockfile = load_lockfile(root)?;
    let status =
        workspace.status_against_filtered_for_target_locales(&lockfile, filters, target_locales)?;
    if !status.target_drift_files.is_empty() {
        return Err(I18nError::ConfigError(format!(
            "target output drift: {} file(s) out of date",
            status.target_drift_files.len()
        ))
        .into());
    }
    if status.pending_units == 0 {
        return Ok(());
    }

    Err(I18nError::ConfigError(format!(
        "i18n.lock is out of date: {} pending localization unit(s)",
        status.pending_units
    ))
    .into())
}

fn ensure_run_filters_select_units(
    workspace: &LocalizationWorkspace,
    filters: &WorkspaceFilters,
    target_locales: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if filters.is_empty() {
        return Ok(());
    }

    let selected = workspace.status_against_filtered_for_target_locales(
        &I18nLock::new(),
        filters,
        target_locales,
    )?;
    if selected.source_file_count == 0 {
        return Err(I18nError::ConfigError(
            "localization filters selected no source files".to_string(),
        )
        .into());
    }
    if !filters.key_prefixes.is_empty() && selected.total_units == 0 {
        return Err(I18nError::ConfigError(
            "localization key filters selected no localization units".to_string(),
        )
        .into());
    }

    Ok(())
}

fn resolve_target_locales(
    workspace: &LocalizationWorkspace,
    requested: &[String],
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let requested = if requested.is_empty() {
        workspace.config().target_locales()
    } else {
        requested
    };
    let mut seen = BTreeSet::new();
    let mut resolved = Vec::new();

    for target_locale in requested {
        let target_locale = validate_cli_target_locale(target_locale)?;
        if !workspace
            .config()
            .target_locales()
            .iter()
            .any(|locale| locale == target_locale)
        {
            return Err(I18nError::ConfigError(format!(
                "target locale '{target_locale}' is not configured"
            ))
            .into());
        }

        if seen.insert(target_locale.to_string()) {
            resolved.push(target_locale.to_string());
        }
    }

    Ok(resolved)
}

fn validate_cli_target_locale(locale: &str) -> Result<&str, I18nError> {
    let locale = locale.trim();
    if locale.is_empty()
        || !locale
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(I18nError::ConfigError(format!(
            "invalid target locale '{locale}'"
        )));
    }

    Ok(locale)
}

fn lockfile_matches(path: &Path, generated: &I18nLock) -> Result<bool, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(false);
    }

    let existing = I18nLock::from_lingo_yaml(&fs::read_to_string(path)?)?;
    Ok(&existing == generated)
}

fn write_lockfile_after_successful_run(
    frozen: bool,
    root: &Path,
    workspace: &LocalizationWorkspace,
    filters: &WorkspaceFilters,
    target_locales: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    if frozen {
        return Ok(());
    }

    let lockfile = if filters.is_empty() {
        workspace.build_lockfile()?
    } else {
        let selected =
            workspace.build_lockfile_filtered_for_target_locales(filters, target_locales)?;
        let current_reference_filters = WorkspaceFilters {
            bucket_types: filters.bucket_types.clone(),
            file_substrings: Vec::new(),
            key_prefixes: Vec::new(),
        };
        let current_reference = workspace.build_lockfile_filtered_for_target_locales(
            &current_reference_filters,
            target_locales,
        )?;
        merge_lockfiles(
            load_lockfile(&root.to_path_buf())?,
            selected,
            current_reference,
        )
    };
    fs::write(root.join("i18n.lock"), lockfile.to_lingo_yaml())?;
    Ok(())
}

fn write_lockfile_after_provider_run(
    frozen: bool,
    root: &Path,
    workspace: &LocalizationWorkspace,
    accepted_files: &[AcceptedProviderFile],
) -> Result<(), Box<dyn std::error::Error>> {
    if frozen || accepted_files.is_empty() {
        return Ok(());
    }

    let mut selected = I18nLock::new();
    let mut current_reference = I18nLock::new();
    for accepted in accepted_files {
        let filters = WorkspaceFilters::try_new(
            vec![accepted.bucket_type.clone()],
            vec![path_to_cli(&accepted.relative_path)],
            accepted.keys.clone(),
        )?;
        let target_locales = [accepted.target_locale.clone()];
        append_lockfile_entries(
            &mut selected,
            workspace.build_lockfile_filtered_for_target_locales(&filters, &target_locales)?,
        );

        let reference_filters = WorkspaceFilters::try_new(
            vec![accepted.bucket_type.clone()],
            Vec::new(),
            accepted.keys.clone(),
        )?;
        current_reference = merge_lockfiles(
            current_reference,
            workspace
                .build_lockfile_filtered_for_target_locales(&reference_filters, &target_locales)?,
            I18nLock::new(),
        );
    }

    let existing = load_lockfile(&root.to_path_buf())?;
    let lockfile = merge_lockfiles(existing, selected, current_reference);
    fs::write(root.join("i18n.lock"), lockfile.to_lingo_yaml())?;
    Ok(())
}

fn append_lockfile_entries(target: &mut I18nLock, source: I18nLock) {
    for (content_hash, keys) in source.checksums {
        target
            .checksums
            .entry(content_hash)
            .or_default()
            .extend(keys);
    }
}

fn merge_lockfiles(
    mut existing: I18nLock,
    selected: I18nLock,
    current_reference: I18nLock,
) -> I18nLock {
    let selected_keys = selected
        .checksums
        .values()
        .flat_map(|keys| keys.keys().cloned())
        .collect::<BTreeSet<_>>();
    existing.checksums.retain(|content_hash, keys| {
        for key in &selected_keys {
            let key_is_current = current_reference
                .checksums
                .get(content_hash)
                .is_some_and(|reference_keys| reference_keys.contains_key(key));
            if !key_is_current {
                keys.remove(key);
            }
        }
        !keys.is_empty()
    });

    for (content_hash, keys) in selected.checksums {
        existing
            .checksums
            .entry(content_hash)
            .or_default()
            .extend(keys);
    }
    existing
}

fn write_or_verify_output(
    root: &PathBuf,
    relative_path: &Path,
    contents: impl AsRef<[u8]>,
    frozen: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if frozen {
        return verify_output_current(root, relative_path, contents);
    }

    write_output(root, relative_path, contents)
}

fn verify_output_current(
    root: &PathBuf,
    relative_path: &Path,
    contents: impl AsRef<[u8]>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(relative_path);
    let expected = contents.as_ref();
    let current = fs::read(&path).unwrap_or_default();
    if current == expected {
        println!("current {}", relative_path.display());
        return Ok(());
    }

    Err(I18nError::ConfigError(format!(
        "target output '{}' is out of date; run dx-i18n run without --frozen",
        relative_path.display()
    ))
    .into())
}

fn write_output(
    root: &PathBuf,
    relative_path: &Path,
    contents: impl AsRef<[u8]>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = root.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, contents)?;
    println!("wrote {}", relative_path.display());

    Ok(())
}

#[derive(Debug)]
struct Args {
    root: PathBuf,
    command: Command,
}

#[derive(Debug)]
enum Command {
    ShowConfig,
    Status {
        target_locales: Vec<String>,
        force: bool,
        verbose: bool,
        filters: CliFilters,
    },
    Lockfile {
        mode: LockfileMode,
    },
    Run {
        target_locales: Vec<String>,
        mode: RunMode,
        frozen: bool,
        force: bool,
        api_key_override: Option<String>,
        filters: CliFilters,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum LockfileMode {
    Check,
    Write { force: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RunMode {
    Local,
    Lingo,
    RawProvider { id: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct CliFilters {
    bucket_types: Vec<String>,
    file_substrings: Vec<String>,
    key_prefixes: Vec<String>,
}

impl CliFilters {
    fn to_workspace_filters(&self) -> dx_i18n::Result<WorkspaceFilters> {
        WorkspaceFilters::try_new(
            self.bucket_types.clone(),
            self.file_substrings.clone(),
            self.key_prefixes.clone(),
        )
    }
}

impl Args {
    fn parse(raw_args: Vec<String>) -> Result<Self, String> {
        let mut root = env::current_dir().map_err(|error| error.to_string())?;
        let mut args = raw_args.into_iter().peekable();

        while args.peek().is_some_and(|arg| arg == "--root") {
            args.next();
            let Some(value) = args.next() else {
                return Err("--root requires a path".to_string());
            };
            root = PathBuf::from(value);
        }

        let Some(command) = args.next() else {
            return Err(usage());
        };

        let command = match command.as_str() {
            "show" => match args.next().as_deref() {
                Some("config") => Command::ShowConfig,
                _ => return Err(usage()),
            },
            "status" => {
                let mut target_locales = Vec::new();
                let mut force = false;
                let mut verbose = false;
                let mut filters = CliFilters::default();
                while let Some(arg) = args.next() {
                    match arg.as_str() {
                        "--locale" => {
                            target_locales.push(
                                args.next()
                                    .ok_or_else(|| "--locale requires a locale".to_string())?,
                            );
                        }
                        "--bucket" => {
                            filters.bucket_types.push(
                                args.next()
                                    .ok_or_else(|| "--bucket requires a value".to_string())?,
                            );
                        }
                        "--file" => {
                            filters.file_substrings.push(
                                args.next()
                                    .ok_or_else(|| "--file requires a value".to_string())?,
                            );
                        }
                        "--force" => {
                            force = true;
                        }
                        "--verbose" => {
                            verbose = true;
                        }
                        _ => return Err(format!("unknown status option '{arg}'\n{}", usage())),
                    }
                }

                Command::Status {
                    target_locales,
                    force,
                    verbose,
                    filters,
                }
            }
            "lockfile" => {
                let mut mode = LockfileMode::Write { force: false };
                while let Some(arg) = args.next() {
                    match arg.as_str() {
                        "--check" | "--frozen" => mode = LockfileMode::Check,
                        "--force" => mode = LockfileMode::Write { force: true },
                        _ => return Err(format!("unknown lockfile option '{arg}'\n{}", usage())),
                    }
                }
                Command::Lockfile { mode }
            }
            "run" => {
                let mut target_locales = Vec::new();
                let mut mode = RunMode::Local;
                let mut frozen = false;
                let mut force = false;
                let mut api_key_override = None;
                let mut filters = CliFilters::default();
                while let Some(arg) = args.next() {
                    match arg.as_str() {
                        "--target" | "--target-locale" => {
                            target_locales.push(
                                args.next()
                                    .ok_or_else(|| format!("{arg} requires a locale"))?,
                            );
                        }
                        "--bucket" => {
                            filters.bucket_types.push(
                                args.next()
                                    .ok_or_else(|| "--bucket requires a value".to_string())?,
                            );
                        }
                        "--file" => {
                            filters.file_substrings.push(
                                args.next()
                                    .ok_or_else(|| "--file requires a value".to_string())?,
                            );
                        }
                        "--key" => {
                            filters.key_prefixes.push(
                                args.next()
                                    .ok_or_else(|| "--key requires a value".to_string())?,
                            );
                        }
                        "--local" => {
                            mode = RunMode::Local;
                        }
                        "--lingo" => {
                            mode = RunMode::Lingo;
                        }
                        "--frozen" => {
                            frozen = true;
                        }
                        "--force" => {
                            force = true;
                        }
                        "--api-key" => {
                            let api_key = args
                                .next()
                                .ok_or_else(|| "--api-key requires a value".to_string())?;
                            let api_key = api_key.trim();
                            if api_key.is_empty() {
                                return Err("--api-key requires a non-empty value".to_string());
                            }
                            api_key_override = Some(api_key.to_string());
                        }
                        "--mode" => {
                            mode =
                                parse_run_mode(&args.next().ok_or_else(|| {
                                    "--mode requires local or lingo".to_string()
                                })?)?;
                        }
                        "--provider" => {
                            let provider = args
                                .next()
                                .ok_or_else(|| "--provider requires a provider id".to_string())?;
                            mode = parse_provider_mode(&provider)?;
                        }
                        _ => return Err(format!("unknown run option '{arg}'\n{}", usage())),
                    }
                }

                if force && frozen {
                    return Err("cannot combine run --force with --frozen".to_string());
                }

                Command::Run {
                    target_locales,
                    mode,
                    frozen,
                    force,
                    api_key_override,
                    filters,
                }
            }
            _ => return Err(usage()),
        };

        if args.next().is_some() {
            return Err(usage());
        }

        Ok(Self { root, command })
    }
}

fn parse_run_mode(mode: &str) -> Result<RunMode, String> {
    match mode {
        "local" => Ok(RunMode::Local),
        "lingo" => Ok(RunMode::Lingo),
        _ => Err(format!("unsupported run mode '{mode}'\n{}", usage())),
    }
}

fn parse_provider_mode(provider: &str) -> Result<RunMode, String> {
    if provider == "lingo" {
        return Ok(RunMode::Lingo);
    }

    if is_supported_raw_provider_id(provider) {
        return Ok(RunMode::RawProvider {
            id: provider.to_string(),
        });
    }

    Err(format!(
        "unsupported run provider '{provider}'\n{}",
        usage()
    ))
}

fn load_lockfile(root: &PathBuf) -> Result<I18nLock, Box<dyn std::error::Error>> {
    let path = root.join("i18n.lock");
    if !path.exists() {
        return Ok(I18nLock::new());
    }

    Ok(I18nLock::from_lingo_yaml(&fs::read_to_string(path)?)?)
}

fn path_to_cli(path: &Path) -> String {
    path.iter()
        .filter_map(|part| part.to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn usage() -> String {
    "usage: dx-i18n [--root <path>] <show config|status [--locale <locale>...] [--bucket <bucket>...] [--file <substring>...] [--force] [--verbose]|lockfile [--check|--force]|run [--target-locale <locale>...] [--bucket <bucket>...] [--file <substring>...] [--key <prefix>...] [--frozen] [--force] [--local|--lingo|--mode local|--mode lingo|--provider lingo|openai|anthropic|google|mistral|openrouter|ollama] [--api-key <api-key>]>".to_string()
}

#[cfg(test)]
mod tests {
    use super::{Args, CliFilters, Command, LockfileMode, RunMode};

    #[test]
    fn run_defaults_to_local_mode() {
        let args = Args::parse(vec!["run".into(), "--target".into(), "es".into()])
            .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::Local,
                frozen: false,
                ..
            } if target_locales == ["es"]
        ));
    }

    #[test]
    fn run_accepts_explicit_lingo_mode() {
        let args = Args::parse(vec![
            "run".into(),
            "--target".into(),
            "es".into(),
            "--provider".into(),
            "lingo".into(),
        ])
        .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::Lingo,
                frozen: false,
                ..
            } if target_locales == ["es"]
        ));
    }

    #[test]
    fn run_accepts_lingo_api_key_override() {
        let args = Args::parse(vec![
            "run".into(),
            "--target".into(),
            "es".into(),
            "--lingo".into(),
            "--api-key".into(),
            "secret".into(),
        ])
        .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::Lingo,
                frozen: false,
                api_key_override: Some(ref api_key),
                ..
            } if target_locales == ["es"] && api_key == "secret"
        ));
    }

    #[test]
    fn run_rejects_blank_lingo_api_key_override() {
        let error = Args::parse(vec![
            "run".into(),
            "--target".into(),
            "es".into(),
            "--lingo".into(),
            "--api-key".into(),
            "   ".into(),
        ])
        .expect_err("blank explicit API key should not fall back to ambient env");

        assert!(error.contains("--api-key requires a non-empty value"));
    }

    #[test]
    fn run_accepts_lingo_compatible_filters() {
        let args = Args::parse(vec![
            "run".into(),
            "--bucket".into(),
            "json".into(),
            "--bucket".into(),
            "markdown".into(),
            "--file".into(),
            "common.json".into(),
            "--file".into(),
            "docs/".into(),
            "--key".into(),
            "auth.login".into(),
            "--key".into(),
            "frontmatter.title".into(),
        ])
        .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                filters: CliFilters {
                    bucket_types,
                    file_substrings,
                    key_prefixes,
                },
                ..
            } if bucket_types == ["json", "markdown"]
                && file_substrings == ["common.json", "docs/"]
                && key_prefixes == ["auth.login", "frontmatter.title"]
        ));
    }

    #[test]
    fn status_accepts_lingo_compatible_filters() {
        let args = Args::parse(vec![
            "status".into(),
            "--verbose".into(),
            "--locale".into(),
            "es".into(),
            "--locale".into(),
            "fr".into(),
            "--bucket".into(),
            "json".into(),
            "--bucket".into(),
            "markdown".into(),
            "--file".into(),
            "common.json".into(),
            "--file".into(),
            "docs/".into(),
            "--force".into(),
        ])
        .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Status {
                target_locales,
                verbose: true,
                force: true,
                filters: CliFilters {
                    bucket_types,
                    file_substrings,
                    key_prefixes,
                }
            } if target_locales == ["es", "fr"]
                && bucket_types == ["json", "markdown"]
                && file_substrings == ["common.json", "docs/"]
                && key_prefixes.is_empty()
        ));
    }

    #[test]
    fn status_defaults_verbose_to_false() {
        let args = Args::parse(vec!["status".into()]).expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Status {
                verbose: false,
                force: false,
                ..
            }
        ));
    }

    #[test]
    fn status_does_not_accept_key_filter() {
        let error = Args::parse(vec!["status".into(), "--key".into(), "auth.login".into()])
            .expect_err("status --key should not parse");

        assert!(error.contains("unknown status option '--key'"));
    }

    #[test]
    fn status_rejects_remote_only_or_unimplemented_options() {
        for option in [
            "--api-key",
            "--mode",
            "--provider",
            "--lingo",
            "--local",
            "--watch",
            "--debug",
            "--concurrency",
            "--source-locale",
        ] {
            let error = Args::parse(vec!["status".into(), option.into(), "value".into()])
                .expect_err("status option should not parse");

            assert!(error.contains(&format!("unknown status option '{option}'")));
        }
    }

    #[test]
    fn run_accepts_raw_provider_boundary() {
        let args = Args::parse(vec![
            "run".into(),
            "--target".into(),
            "fr".into(),
            "--provider".into(),
            "openai".into(),
        ])
        .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::RawProvider { ref id },
                frozen: false,
                ..
            } if target_locales == ["fr"] && id == "openai"
        ));
    }

    #[test]
    fn run_defaults_to_all_configured_targets_when_target_is_omitted() {
        let args = Args::parse(vec!["run".into()]).expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::Local,
                frozen: false,
                ..
            } if target_locales.is_empty()
        ));
    }

    #[test]
    fn run_accepts_repeated_target_locale_and_frozen_flags() {
        let args = Args::parse(vec![
            "run".into(),
            "--target-locale".into(),
            "es".into(),
            "--target-locale".into(),
            "fr".into(),
            "--frozen".into(),
        ])
        .expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::Local,
                frozen: true,
                ..
            } if target_locales == ["es", "fr"]
        ));
    }

    #[test]
    fn run_accepts_force_without_switching_from_local_mode() {
        let args = Args::parse(vec!["run".into(), "--force".into()]).expect("args should parse");

        assert!(matches!(
            args.command,
            Command::Run {
                target_locales,
                mode: RunMode::Local,
                frozen: false,
                force: true,
                ..
            } if target_locales.is_empty()
        ));
    }

    #[test]
    fn run_rejects_force_with_frozen() {
        let error = Args::parse(vec!["run".into(), "--force".into(), "--frozen".into()])
            .expect_err("force and frozen should be rejected together");

        assert!(error.contains("cannot combine run --force with --frozen"));
    }

    #[test]
    fn lockfile_accepts_check_and_force_modes() {
        let check = Args::parse(vec!["lockfile".into(), "--check".into()])
            .expect("lockfile check should parse");
        let force = Args::parse(vec!["lockfile".into(), "--force".into()])
            .expect("lockfile force should parse");

        assert!(matches!(
            check.command,
            Command::Lockfile {
                mode: LockfileMode::Check
            }
        ));
        assert!(matches!(
            force.command,
            Command::Lockfile {
                mode: LockfileMode::Write { force: true }
            }
        ));
    }
}
