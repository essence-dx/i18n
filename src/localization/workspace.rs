use crate::error::{I18nError, Result};
use crate::localization::provider::{LocalizationProvider, LocalizationResponse};
use crate::localization::{
    BucketConfig, BucketPattern, DeltaKind, JsonLocalizationDocument, LocalizationConfig,
    MarkdownLocalizationDocument, SourceStringFingerprint, TranslationUnit, ensure_json_object,
    preserves_protected_tokens,
};
use crate::localization::{Lockfile, lingo_key_hash};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug)]
pub struct LocalizationWorkspace {
    root: PathBuf,
    config: LocalizationConfig,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceStatus {
    pub requires_cloud_auth: bool,
    pub source_file_count: usize,
    pub total_units: usize,
    pub pending_units: usize,
    pub pending_keys: Vec<String>,
    pub pending_files: Vec<WorkspaceStatusFile>,
    pub target_drift_files: Vec<PathBuf>,
    pub target_locales: Vec<String>,
    pub unsupported_bucket_types: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceStatusFile {
    pub relative_path: PathBuf,
    pub pending_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalizedJsonFile {
    pub relative_path: PathBuf,
    pub value: Value,
    pub provider_response: Option<LocalizationResponse>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalizedMarkdownFile {
    pub relative_path: PathBuf,
    pub contents: String,
    pub provider_response: Option<LocalizationResponse>,
}

struct SourceScan {
    file_count: usize,
    units: Vec<TranslationUnit>,
    force_pending_keys: BTreeSet<String>,
    files: Vec<SourceFileScan>,
}

struct SourceFileScan {
    relative_path: PathBuf,
    units: Vec<TranslationUnit>,
    force_pending_keys: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceFilters {
    pub bucket_types: Vec<String>,
    pub file_substrings: Vec<String>,
    pub key_prefixes: Vec<String>,
}

impl WorkspaceFilters {
    pub fn try_new(
        bucket_types: Vec<String>,
        file_substrings: Vec<String>,
        key_prefixes: Vec<String>,
    ) -> Result<Self> {
        Ok(Self {
            bucket_types: normalize_filter_values(bucket_types, "bucket")?,
            file_substrings: normalize_file_filters(file_substrings)?,
            key_prefixes: normalize_key_filters(key_prefixes)?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.bucket_types.is_empty()
            && self.file_substrings.is_empty()
            && self.key_prefixes.is_empty()
    }

    pub fn has_bucket_filter(&self) -> bool {
        !self.bucket_types.is_empty()
    }

    fn includes_bucket(&self, bucket_type: &str) -> bool {
        self.bucket_types.is_empty()
            || self
                .bucket_types
                .iter()
                .any(|candidate| candidate == bucket_type)
    }

    fn includes_file(
        &self,
        pattern: &BucketPattern,
        source_path: &Path,
        target_path: &Path,
    ) -> bool {
        if self.file_substrings.is_empty() {
            return true;
        }

        let pattern_path = pattern.path().replace('\\', "/");
        let source_path = path_to_slash(source_path);
        let target_path = path_to_slash(target_path);

        self.file_substrings.iter().any(|needle| {
            pattern_path.contains(needle)
                || source_path.contains(needle)
                || target_path.contains(needle)
        })
    }

    fn includes_key(&self, key: &str) -> bool {
        if self.key_prefixes.is_empty() {
            return true;
        }

        key_candidates(key).iter().any(|candidate| {
            self.key_prefixes
                .iter()
                .any(|prefix| key_prefix_matches(prefix, candidate))
        })
    }

    fn can_affect_key_path(&self, key: &str) -> bool {
        if self.key_prefixes.is_empty() || key.is_empty() {
            return true;
        }

        key_candidates(key).iter().any(|candidate| {
            self.key_prefixes.iter().any(|prefix| {
                key_prefix_matches(prefix, candidate)
                    || prefix
                        .strip_prefix(candidate)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
    }

    fn includes_source_file(&self, pattern: &BucketPattern, source_path: &Path) -> bool {
        if self.file_substrings.is_empty() {
            return true;
        }

        let pattern_path = pattern.path().replace('\\', "/");
        let source_path = path_to_slash(source_path);
        self.file_substrings
            .iter()
            .any(|needle| pattern_path.contains(needle) || source_path.contains(needle))
    }

    fn includes_status_file(
        &self,
        pattern: &BucketPattern,
        source_path: &Path,
        source_locale: &str,
        target_locales: &[String],
    ) -> Result<bool> {
        if self.file_substrings.is_empty() || self.includes_source_file(pattern, source_path) {
            return Ok(true);
        }

        for target_locale in target_locales {
            let target_path =
                target_path_for_source_pattern(pattern, source_path, source_locale, target_locale)?;
            if self.includes_file(pattern, source_path, &target_path) {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

impl LocalizationWorkspace {
    pub fn load(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let config_path = root.join("i18n.json");
        let config = LocalizationConfig::from_json_str(&fs::read_to_string(&config_path)?)?;
        let issues = config.validate_local();
        if let Some(issue) = issues.first() {
            return Err(I18nError::ConfigError(format!(
                "{}: {}",
                issue.code, issue.message
            )));
        }

        Ok(Self { root, config })
    }

    pub fn new(root: impl Into<PathBuf>, config: LocalizationConfig) -> Self {
        Self {
            root: root.into(),
            config,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> &LocalizationConfig {
        &self.config
    }

    pub fn requires_cloud_auth_for_local_mode(&self) -> bool {
        false
    }

    pub fn build_lockfile(&self) -> Result<Lockfile> {
        self.build_lockfile_filtered(&WorkspaceFilters::default())
    }

    pub fn build_lockfile_filtered(&self, filters: &WorkspaceFilters) -> Result<Lockfile> {
        self.build_lockfile_filtered_for_target_locales(filters, self.config.target_locales())
    }

    pub fn build_lockfile_filtered_for_target_locales(
        &self,
        filters: &WorkspaceFilters,
        target_locales: &[String],
    ) -> Result<Lockfile> {
        self.validate_filters(filters)?;
        self.validate_target_locales(target_locales)?;
        let scan = self.scan_source_units_for_target_locales(filters, target_locales)?;
        let mut lockfile = Lockfile::new();
        for unit in scan.units {
            lockfile.record_lingo_key(unit.key(), unit.text());
        }
        Ok(lockfile)
    }

    pub fn status_against(&self, lockfile: &Lockfile) -> Result<WorkspaceStatus> {
        self.status_against_filtered(lockfile, &WorkspaceFilters::default())
    }

    pub fn status_against_filtered(
        &self,
        lockfile: &Lockfile,
        filters: &WorkspaceFilters,
    ) -> Result<WorkspaceStatus> {
        self.status_against_filtered_for_target_locales(
            lockfile,
            filters,
            self.config.target_locales(),
        )
    }

    pub fn status_against_filtered_for_target_locales(
        &self,
        lockfile: &Lockfile,
        filters: &WorkspaceFilters,
        target_locales: &[String],
    ) -> Result<WorkspaceStatus> {
        self.status_against_filtered_for_target_locales_with_force(
            lockfile,
            filters,
            target_locales,
            false,
        )
    }

    pub fn status_against_filtered_for_target_locales_with_force(
        &self,
        lockfile: &Lockfile,
        filters: &WorkspaceFilters,
        target_locales: &[String],
        force: bool,
    ) -> Result<WorkspaceStatus> {
        self.validate_filters(filters)?;
        self.validate_target_locales(target_locales)?;
        let scan = self.scan_source_units_for_target_locales(filters, target_locales)?;
        let pending_keys = scan
            .units
            .iter()
            .filter(|unit| status_unit_is_pending(unit, lockfile, force, &scan.force_pending_keys))
            .map(|unit| unit.key().to_string())
            .collect::<Vec<_>>();
        let pending_files = scan
            .files
            .iter()
            .filter_map(|file| {
                let pending_keys = file
                    .units
                    .iter()
                    .filter(|unit| {
                        status_unit_is_pending(unit, lockfile, force, &file.force_pending_keys)
                    })
                    .map(|unit| unit.key().to_string())
                    .collect::<Vec<_>>();
                (!pending_keys.is_empty()).then(|| WorkspaceStatusFile {
                    relative_path: file.relative_path.clone(),
                    pending_keys,
                })
            })
            .collect::<Vec<_>>();
        let target_drift_files =
            self.target_drift_files_for_target_locales(filters, target_locales)?;

        Ok(WorkspaceStatus {
            requires_cloud_auth: self.requires_cloud_auth_for_local_mode(),
            source_file_count: scan.file_count,
            total_units: scan.units.len(),
            pending_units: pending_keys.len(),
            pending_keys,
            pending_files,
            target_drift_files,
            target_locales: target_locales.to_vec(),
            unsupported_bucket_types: self.unsupported_bucket_types_filtered(filters),
        })
    }

    pub fn render_local_json(&self, target_locale: &str) -> Result<Vec<LocalizedJsonFile>> {
        self.render_local_json_filtered(target_locale, &WorkspaceFilters::default())
    }

    pub fn render_local_json_filtered(
        &self,
        target_locale: &str,
        filters: &WorkspaceFilters,
    ) -> Result<Vec<LocalizedJsonFile>> {
        self.validate_filters(filters)?;
        let source_locale = self.config.source_locale();
        let target_locale = validate_target_locale(target_locale)?;
        if !self
            .config
            .target_locales()
            .iter()
            .any(|locale| locale == target_locale)
        {
            return Err(I18nError::ConfigError(format!(
                "target locale '{target_locale}' is not configured"
            )));
        }

        let mut outputs = Vec::new();
        for (bucket_type, bucket) in &self.config.buckets {
            if bucket_type != "json" || !filters.includes_bucket(bucket_type) {
                continue;
            }

            for pattern in &bucket.include {
                let source_files = self.expand_pattern(pattern, source_locale)?;
                for source_path in source_files {
                    if source_path_is_excluded(bucket, source_locale, &source_path)? {
                        continue;
                    }
                    let target_path = target_path_for_source_pattern(
                        pattern,
                        &source_path,
                        source_locale,
                        target_locale,
                    )?;
                    if !filters.includes_file(pattern, &source_path, &target_path) {
                        continue;
                    }
                    let source_value = read_json_file(&self.root.join(&source_path))?;
                    let target_value = read_optional_json_file(&self.root.join(&target_path))?;
                    let value = render_json_value(
                        &source_value,
                        target_value.as_ref(),
                        "",
                        bucket,
                        target_locale,
                        filters,
                    )
                    .unwrap_or(Value::Object(Map::new()));

                    outputs.push(LocalizedJsonFile {
                        relative_path: target_path,
                        value,
                        provider_response: None,
                    });
                }
            }
        }

        outputs.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(outputs)
    }

    pub fn render_local_markdown(&self, target_locale: &str) -> Result<Vec<LocalizedMarkdownFile>> {
        self.render_local_markdown_filtered(target_locale, &WorkspaceFilters::default())
    }

    pub fn render_local_markdown_filtered(
        &self,
        target_locale: &str,
        filters: &WorkspaceFilters,
    ) -> Result<Vec<LocalizedMarkdownFile>> {
        self.validate_filters(filters)?;
        let source_locale = self.config.source_locale();
        let target_locale = validate_target_locale(target_locale)?;
        if !self
            .config
            .target_locales()
            .iter()
            .any(|locale| locale == target_locale)
        {
            return Err(I18nError::ConfigError(format!(
                "target locale '{target_locale}' is not configured"
            )));
        }

        let mut outputs = Vec::new();
        for (bucket_type, bucket) in &self.config.buckets {
            if bucket_type != "markdown" || !filters.includes_bucket(bucket_type) {
                continue;
            }

            for pattern in &bucket.include {
                for source_path in self.expand_pattern(pattern, source_locale)? {
                    if source_path_is_excluded(bucket, source_locale, &source_path)? {
                        continue;
                    }
                    let target_path = target_path_for_source_pattern(
                        pattern,
                        &source_path,
                        source_locale,
                        target_locale,
                    )?;
                    if !filters.includes_file(pattern, &source_path, &target_path) {
                        continue;
                    }
                    let source = fs::read_to_string(self.root.join(&source_path))?;
                    let target = read_optional_string_file(&self.root.join(&target_path))?;
                    let source_canonical_path =
                        canonical_path_for_pattern(pattern, &source_path, source_locale)?;
                    let source_document =
                        MarkdownLocalizationDocument::new(&source, source_canonical_path);
                    let target_canonical_path =
                        canonical_path_for_pattern(pattern, &target_path, target_locale)?;
                    let translations = local_markdown_translations(
                        &source_document,
                        target.as_deref(),
                        target_canonical_path,
                        bucket,
                        target_locale,
                        filters,
                    );

                    outputs.push(LocalizedMarkdownFile {
                        relative_path: target_path,
                        contents: source_document.apply_translations(&translations),
                        provider_response: None,
                    });
                }
            }
        }

        outputs.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(outputs)
    }

    pub async fn render_provider_json<P>(
        &self,
        provider: &P,
        target_locale: &str,
    ) -> Result<Vec<LocalizedJsonFile>>
    where
        P: LocalizationProvider + ?Sized,
    {
        self.render_provider_json_filtered(provider, target_locale, &WorkspaceFilters::default())
            .await
    }

    pub async fn render_provider_json_filtered<P>(
        &self,
        provider: &P,
        target_locale: &str,
        filters: &WorkspaceFilters,
    ) -> Result<Vec<LocalizedJsonFile>>
    where
        P: LocalizationProvider + ?Sized,
    {
        let lockfile = Lockfile::new();
        self.render_provider_json_delta_filtered(provider, target_locale, filters, &lockfile, true)
            .await
    }

    pub async fn render_provider_json_delta_filtered<P>(
        &self,
        provider: &P,
        target_locale: &str,
        filters: &WorkspaceFilters,
        lockfile: &Lockfile,
        force: bool,
    ) -> Result<Vec<LocalizedJsonFile>>
    where
        P: LocalizationProvider + ?Sized,
    {
        self.validate_filters(filters)?;
        let source_locale = self.config.source_locale();
        let target_locale = validate_target_locale(target_locale)?;
        if !self
            .config
            .target_locales()
            .iter()
            .any(|locale| locale == target_locale)
        {
            return Err(I18nError::ConfigError(format!(
                "target locale '{target_locale}' is not configured"
            )));
        }

        let mut outputs = Vec::new();
        for (bucket_type, bucket) in &self.config.buckets {
            if bucket_type != "json" || !filters.includes_bucket(bucket_type) {
                continue;
            }

            for pattern in &bucket.include {
                for source_path in self.expand_pattern(pattern, source_locale)? {
                    if source_path_is_excluded(bucket, source_locale, &source_path)? {
                        continue;
                    }
                    let target_path = target_path_for_source_pattern(
                        pattern,
                        &source_path,
                        source_locale,
                        target_locale,
                    )?;
                    if !filters.includes_file(pattern, &source_path, &target_path) {
                        continue;
                    }
                    let source_value = read_json_file(&self.root.join(&source_path))?;
                    let target_value = read_optional_json_file(&self.root.join(&target_path))?;
                    let document = JsonLocalizationDocument::new(source_value.clone());
                    let units = document
                        .source_units()
                        .into_iter()
                        .filter(|unit| {
                            filters.includes_key(unit.key())
                                && bucket_sends_key_to_provider(bucket, unit.key())
                                && provider_unit_should_send(
                                    unit,
                                    lockfile,
                                    force,
                                    target_value
                                        .as_ref()
                                        .and_then(|target| json_string_at(target, unit.key())),
                                )
                        })
                        .collect::<Vec<_>>();
                    let (translations, provider_response) = if units.is_empty() {
                        (BTreeMap::new(), None)
                    } else {
                        let response = provider
                            .localize_response(source_locale, target_locale, &units)
                            .await?;
                        (response.translations.clone(), Some(response))
                    };
                    let value = render_provider_json_value(
                        &source_value,
                        &translations,
                        target_value.as_ref(),
                        "",
                        bucket,
                        target_locale,
                        filters,
                    )
                    .unwrap_or(Value::Object(Map::new()));
                    let provider_response = provider_response.map(|mut response| {
                        response.translations =
                            accepted_provider_json_translations(&source_value, &value, &units);
                        response
                    });

                    outputs.push(LocalizedJsonFile {
                        relative_path: target_path,
                        value,
                        provider_response,
                    });
                }
            }
        }

        outputs.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(outputs)
    }

    pub async fn render_provider_markdown<P>(
        &self,
        provider: &P,
        target_locale: &str,
    ) -> Result<Vec<LocalizedMarkdownFile>>
    where
        P: LocalizationProvider + ?Sized,
    {
        self.render_provider_markdown_filtered(
            provider,
            target_locale,
            &WorkspaceFilters::default(),
        )
        .await
    }

    pub async fn render_provider_markdown_filtered<P>(
        &self,
        provider: &P,
        target_locale: &str,
        filters: &WorkspaceFilters,
    ) -> Result<Vec<LocalizedMarkdownFile>>
    where
        P: LocalizationProvider + ?Sized,
    {
        let lockfile = Lockfile::new();
        self.render_provider_markdown_delta_filtered(
            provider,
            target_locale,
            filters,
            &lockfile,
            true,
        )
        .await
    }

    pub async fn render_provider_markdown_delta_filtered<P>(
        &self,
        provider: &P,
        target_locale: &str,
        filters: &WorkspaceFilters,
        lockfile: &Lockfile,
        force: bool,
    ) -> Result<Vec<LocalizedMarkdownFile>>
    where
        P: LocalizationProvider + ?Sized,
    {
        self.validate_filters(filters)?;
        let source_locale = self.config.source_locale();
        let target_locale = validate_target_locale(target_locale)?;
        if !self
            .config
            .target_locales()
            .iter()
            .any(|locale| locale == target_locale)
        {
            return Err(I18nError::ConfigError(format!(
                "target locale '{target_locale}' is not configured"
            )));
        }

        let mut outputs = Vec::new();
        for (bucket_type, bucket) in &self.config.buckets {
            if bucket_type != "markdown" || !filters.includes_bucket(bucket_type) {
                continue;
            }

            for pattern in &bucket.include {
                for source_path in self.expand_pattern(pattern, source_locale)? {
                    if source_path_is_excluded(bucket, source_locale, &source_path)? {
                        continue;
                    }
                    let target_path = target_path_for_source_pattern(
                        pattern,
                        &source_path,
                        source_locale,
                        target_locale,
                    )?;
                    if !filters.includes_file(pattern, &source_path, &target_path) {
                        continue;
                    }
                    let source = fs::read_to_string(self.root.join(&source_path))?;
                    let target = read_optional_string_file(&self.root.join(&target_path))?;
                    let source_canonical_path =
                        canonical_path_for_pattern(pattern, &source_path, source_locale)?;
                    let source_document =
                        MarkdownLocalizationDocument::new(&source, source_canonical_path);
                    let target_canonical_path =
                        canonical_path_for_pattern(pattern, &target_path, target_locale)?;
                    let source_units = source_document.source_units();
                    let source_unit_keys = source_units
                        .iter()
                        .map(|unit| unit.key().to_string())
                        .collect::<Vec<_>>();
                    let source_unit_map = source_units
                        .iter()
                        .map(|unit| (unit.key().to_string(), unit.text().to_string()))
                        .collect::<BTreeMap<_, _>>();
                    let target_units = markdown_target_units(
                        &source_unit_keys,
                        &source_unit_map,
                        target.as_deref(),
                        target_canonical_path.clone(),
                    );
                    let units = source_units
                        .into_iter()
                        .filter(|unit| {
                            filters.includes_key(unit.key())
                                && bucket_sends_key_to_provider(bucket, unit.key())
                                && provider_markdown_unit_should_send(
                                    unit,
                                    lockfile,
                                    force,
                                    target_units.get(unit.key()).map(String::as_str),
                                )
                        })
                        .collect::<Vec<_>>();
                    let (translations, provider_response) = if units.is_empty() {
                        (BTreeMap::new(), None)
                    } else {
                        let response = provider
                            .localize_response(source_locale, target_locale, &units)
                            .await?;
                        (response.translations.clone(), Some(response))
                    };
                    let provider_translations =
                        safe_provider_markdown_translations(&units, translations);
                    let translations = local_markdown_translations(
                        &source_document,
                        target.as_deref(),
                        target_canonical_path,
                        bucket,
                        target_locale,
                        filters,
                    )
                    .into_iter()
                    .chain(provider_translations.clone())
                    .collect::<BTreeMap<_, _>>();
                    let provider_response = provider_response.map(|mut response| {
                        response.translations = provider_translations;
                        response
                    });

                    outputs.push(LocalizedMarkdownFile {
                        relative_path: target_path,
                        contents: source_document.apply_translations(&translations),
                        provider_response,
                    });
                }
            }
        }

        outputs.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(outputs)
    }

    fn scan_source_units_for_target_locales(
        &self,
        filters: &WorkspaceFilters,
        target_locales: &[String],
    ) -> Result<SourceScan> {
        let mut scan = SourceScan {
            file_count: 0,
            units: Vec::new(),
            force_pending_keys: BTreeSet::new(),
            files: Vec::new(),
        };
        let source_locale = self.config.source_locale();

        for (bucket_type, bucket) in &self.config.buckets {
            if !workspace_supports_bucket(bucket_type) || !filters.includes_bucket(bucket_type) {
                continue;
            }

            for pattern in &bucket.include {
                for relative_path in self.expand_pattern(pattern, source_locale)? {
                    if source_path_is_excluded(bucket, source_locale, &relative_path)? {
                        continue;
                    }
                    if !filters.includes_status_file(
                        pattern,
                        &relative_path,
                        source_locale,
                        target_locales,
                    )? {
                        continue;
                    }
                    match bucket_type.as_str() {
                        "json" => {
                            let value = read_json_file(&self.root.join(&relative_path))?;
                            let value = ensure_json_object(value)?;
                            let mut units = JsonLocalizationDocument::new(value)
                                .source_units()
                                .into_iter()
                                .filter(|unit| {
                                    filters.includes_key(unit.key())
                                        && !bucket_ignores_key(bucket, unit.key())
                                        && !bucket_injects_locale(bucket, unit.key())
                                })
                                .collect::<Vec<_>>();
                            units.sort_by(|left, right| left.key().cmp(right.key()));
                            let force_pending_keys = units
                                .iter()
                                .filter(|unit| bucket_sends_key_to_provider(bucket, unit.key()))
                                .map(|unit| unit.key().to_string())
                                .collect::<BTreeSet<_>>();
                            scan.force_pending_keys
                                .extend(force_pending_keys.iter().cloned());
                            scan.units.extend(units.clone());
                            scan.files.push(SourceFileScan {
                                relative_path,
                                units,
                                force_pending_keys,
                            });
                            scan.file_count += 1;
                        }
                        "markdown" => {
                            let source = fs::read_to_string(self.root.join(&relative_path))?;
                            let canonical_path =
                                canonical_path_for_pattern(pattern, &relative_path, source_locale)?;
                            let mut units =
                                MarkdownLocalizationDocument::new(source, canonical_path)
                                    .source_units()
                                    .into_iter()
                                    .filter(|unit| {
                                        filters.includes_key(unit.key())
                                            && !bucket_ignores_key(bucket, unit.key())
                                            && !bucket_injects_locale(bucket, unit.key())
                                    })
                                    .collect::<Vec<_>>();
                            units.sort_by(|left, right| left.key().cmp(right.key()));
                            let force_pending_keys = units
                                .iter()
                                .filter(|unit| bucket_sends_key_to_provider(bucket, unit.key()))
                                .map(|unit| unit.key().to_string())
                                .collect::<BTreeSet<_>>();
                            scan.force_pending_keys
                                .extend(force_pending_keys.iter().cloned());
                            scan.units.extend(units.clone());
                            scan.files.push(SourceFileScan {
                                relative_path,
                                units,
                                force_pending_keys,
                            });
                            scan.file_count += 1;
                        }
                        _ => {}
                    }
                }
            }
        }

        scan.units
            .sort_by(|left, right| left.key().cmp(right.key()));
        scan.files
            .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(scan)
    }

    pub fn unsupported_bucket_types(&self) -> Vec<String> {
        self.unsupported_bucket_types_filtered(&WorkspaceFilters::default())
    }

    pub fn unsupported_bucket_types_filtered(&self, filters: &WorkspaceFilters) -> Vec<String> {
        self.config
            .buckets
            .keys()
            .filter(|bucket_type| filters.includes_bucket(bucket_type))
            .filter(|bucket_type| !workspace_supports_bucket(bucket_type))
            .cloned()
            .collect()
    }

    fn validate_filters(&self, filters: &WorkspaceFilters) -> Result<()> {
        for bucket_type in &filters.bucket_types {
            if !self.config.buckets.contains_key(bucket_type) {
                return Err(I18nError::ConfigError(format!(
                    "bucket '{bucket_type}' is not configured"
                )));
            }
        }

        Ok(())
    }

    fn validate_target_locales(&self, target_locales: &[String]) -> Result<()> {
        for target_locale in target_locales {
            let target_locale = validate_target_locale(target_locale)?;
            if !self
                .config
                .target_locales()
                .iter()
                .any(|locale| locale == target_locale)
            {
                return Err(I18nError::ConfigError(format!(
                    "target locale '{target_locale}' is not configured"
                )));
            }
        }

        Ok(())
    }

    fn target_drift_files_for_target_locales(
        &self,
        filters: &WorkspaceFilters,
        target_locales: &[String],
    ) -> Result<Vec<PathBuf>> {
        let mut drift_files = BTreeSet::new();

        for target_locale in target_locales {
            for output in self.render_local_json_filtered(target_locale, filters)? {
                let rendered = serde_json::to_string_pretty(&output.value)?;
                if output_file_has_drift(&self.root, &output.relative_path, rendered.as_bytes())? {
                    drift_files.insert(output.relative_path);
                }
            }

            for output in self.render_local_markdown_filtered(target_locale, filters)? {
                if output_file_has_drift(
                    &self.root,
                    &output.relative_path,
                    output.contents.as_bytes(),
                )? {
                    drift_files.insert(output.relative_path);
                }
            }
        }

        Ok(drift_files.into_iter().collect())
    }

    fn expand_pattern(&self, pattern: &BucketPattern, locale: &str) -> Result<Vec<PathBuf>> {
        let localized = pattern.path_for_locale(locale);
        if !localized.contains('*') {
            let path = normalize_relative_path(&localized)?;
            return path_exists(&self.root.join(&path))
                .then_some(vec![path])
                .ok_or_else(|| {
                    I18nError::ConfigError(format!(
                        "configured localization source file '{}' does not exist",
                        localized
                    ))
                });
        }

        expand_single_wildcard(&self.root, &localized)
    }
}

fn output_file_has_drift(root: &Path, relative_path: &Path, rendered: &[u8]) -> Result<bool> {
    match fs::read(root.join(relative_path)) {
        Ok(current) => Ok(current != rendered),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn workspace_supports_bucket(bucket_type: &str) -> bool {
    matches!(bucket_type, "json" | "markdown")
}

fn status_unit_is_pending(
    unit: &TranslationUnit,
    lockfile: &Lockfile,
    force: bool,
    force_pending_keys: &BTreeSet<String>,
) -> bool {
    if force {
        return force_pending_keys.contains(unit.key());
    }

    !matches!(
        lockfile.classify_lingo_key(unit.key(), unit.text()),
        DeltaKind::Current | DeltaKind::RenamedKey { .. }
    )
}

fn provider_unit_should_send(
    unit: &TranslationUnit,
    lockfile: &Lockfile,
    force: bool,
    existing_target: Option<&str>,
) -> bool {
    if force {
        return true;
    }

    match lockfile.classify_lingo_key(unit.key(), unit.text()) {
        DeltaKind::Current | DeltaKind::RenamedKey { .. } => {
            existing_target.is_none_or(|target| !preserves_protected_tokens(unit.text(), target))
        }
        DeltaKind::ChangedContent { .. } | DeltaKind::New => true,
    }
}

fn provider_markdown_unit_should_send(
    unit: &TranslationUnit,
    lockfile: &Lockfile,
    force: bool,
    existing_target: Option<&str>,
) -> bool {
    if force {
        return true;
    }

    match lockfile.classify_lingo_key(unit.key(), unit.text()) {
        DeltaKind::Current | DeltaKind::RenamedKey { .. } => existing_target.is_none_or(|target| {
            !markdown_unit_text_shape_is_safe(unit.text(), target)
                || !preserves_protected_tokens(unit.text(), target)
        }),
        DeltaKind::ChangedContent { .. } | DeltaKind::New => true,
    }
}

fn normalize_filter_values(values: Vec<String>, label: &str) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim();
        if value.is_empty() {
            return Err(I18nError::ConfigError(format!(
                "{label} filter cannot be empty"
            )));
        }
        if !normalized.iter().any(|candidate| candidate == value) {
            normalized.push(value.to_string());
        }
    }
    Ok(normalized)
}

fn normalize_file_filters(values: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for value in values {
        let value = value.trim().replace('\\', "/");
        if value.is_empty() {
            return Err(I18nError::ConfigError(
                "file filter cannot be empty".to_string(),
            ));
        }
        let _ = normalize_relative_path(&value)?;
        if !normalized.iter().any(|candidate| candidate == &value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

fn normalize_key_filters(values: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = Vec::new();
    for value in values {
        let value = normalize_key_filter(&value);
        if value.is_empty() {
            return Err(I18nError::ConfigError(
                "key filter cannot be empty".to_string(),
            ));
        }
        if !normalized.iter().any(|candidate| candidate == &value) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

fn normalize_key_filter(value: &str) -> String {
    let value = value.trim().replace('\\', "/");
    if let Some((path, section)) = value.split_once('#') {
        let path = path.trim_matches('/');
        let section = section.trim_matches('/').replace('.', "/");
        return format!("{path}#{section}");
    }

    value.replace('.', "/").trim_matches('/').to_string()
}

fn key_candidates(key: &str) -> Vec<String> {
    let slash = normalize_lingo_key(key);
    let mut candidates = vec![slash.clone()];
    if let Some((path, section)) = slash.split_once('#') {
        candidates.push(format!("{path}/{}", section.replace('/', ".")));
        candidates.push(section.to_string());
        candidates.push(section.replace('/', "."));
    }
    candidates
}

fn key_prefix_matches(prefix: &str, key: &str) -> bool {
    prefix == key
        || key
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

impl Lockfile {
    pub fn record_lingo_key(&mut self, key_path: &str, source_value: &str) {
        let key_path = normalize_lingo_key(key_path);
        let fingerprint = SourceStringFingerprint::for_lingo_key(&key_path, source_value);
        self.checksums
            .entry(fingerprint.content_hash)
            .or_default()
            .insert(key_path, fingerprint.key_hash);
    }

    pub fn classify_lingo_key(&self, key_path: &str, source_value: &str) -> DeltaKind {
        let key_path = normalize_lingo_key(key_path);
        let content_hash = crate::localization::content_hash(source_value);
        let key_hash = lingo_key_hash(&key_path);

        if let Some(labels) = self.checksums.get(&content_hash) {
            if labels.get(&key_path) == Some(&key_hash) {
                return DeltaKind::Current;
            }

            if let Some(previous_label) = labels
                .iter()
                .find_map(|(label, existing_hash)| (existing_hash != &key_hash).then(|| label))
            {
                return DeltaKind::RenamedKey {
                    previous_label: previous_label.clone(),
                };
            }
        }

        if let Some((previous_content_hash, _)) = self
            .checksums
            .iter()
            .find(|(_, labels)| labels.values().any(|candidate| candidate == &key_hash))
        {
            return DeltaKind::ChangedContent {
                previous_content_hash: previous_content_hash.clone(),
            };
        }

        DeltaKind::New
    }
}

impl SourceStringFingerprint {
    pub fn for_lingo_key(key_path: impl Into<String>, source_value: impl AsRef<str>) -> Self {
        let key_path = normalize_lingo_key(&key_path.into());
        Self {
            bucket_type: "lingo".to_string(),
            source_locale: String::new(),
            source_path: String::new(),
            content_hash: crate::localization::content_hash(source_value.as_ref()),
            key_hash: lingo_key_hash(&key_path),
            key_path,
        }
    }
}

fn read_json_file(path: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn read_optional_json_file(path: &Path) -> Result<Option<Value>> {
    if !path_exists(path) {
        return Ok(None);
    }
    Ok(Some(read_json_file(path)?))
}

fn read_optional_string_file(path: &Path) -> Result<Option<String>> {
    if !path_exists(path) {
        return Ok(None);
    }
    Ok(Some(fs::read_to_string(path)?))
}

fn path_exists(path: &Path) -> bool {
    fs::metadata(path).is_ok()
}

fn render_json_value(
    source: &Value,
    target: Option<&Value>,
    path: &str,
    bucket: &BucketConfig,
    target_locale: &str,
    filters: &WorkspaceFilters,
) -> Option<Value> {
    if bucket_ignores_key(bucket, path) {
        return None;
    }

    if bucket_injects_locale(bucket, path) {
        return Some(Value::String(target_locale.to_string()));
    }

    match source {
        Value::String(text) => {
            if !filters.includes_key(path) {
                return Some(Value::String(
                    target.and_then(Value::as_str).unwrap_or(text).to_string(),
                ));
            }

            if bucket_locks_key(bucket, path) {
                return Some(Value::String(text.clone()));
            }

            if bucket_preserves_key(bucket, path) {
                return Some(Value::String(safe_target_text(
                    text,
                    target.and_then(Value::as_str),
                )));
            }

            Some(Value::String(safe_target_text(
                text,
                target.and_then(Value::as_str),
            )))
        }
        Value::Array(items) => {
            let rendered = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let child_path = join_key_path(path, &index.to_string());
                    if bucket_ignores_key(bucket, &child_path) {
                        return item.clone();
                    }

                    let target_child = target.and_then(|value| value.get(index));
                    render_json_value(
                        item,
                        target_child,
                        &child_path,
                        bucket,
                        target_locale,
                        filters,
                    )
                    .unwrap_or_else(|| target_child.cloned().unwrap_or_else(|| item.clone()))
                })
                .collect::<Vec<_>>();
            Some(Value::Array(rendered))
        }
        Value::Object(object) => {
            if object.is_empty() {
                return Some(Value::Object(Map::new()));
            }

            let mut rendered = Map::new();
            for (key, item) in object {
                let child_path = join_key_path(path, &escape_json_pointer_segment(key));
                let target_child = target.and_then(|value| value.get(key));
                if !filters.can_affect_key_path(&child_path) {
                    if let Some(target_child) = target_child {
                        rendered.insert(key.clone(), target_child.clone());
                    } else {
                        rendered.insert(key.clone(), item.clone());
                    }
                    continue;
                }

                if let Some(value) = render_json_value(
                    item,
                    target_child,
                    &child_path,
                    bucket,
                    target_locale,
                    filters,
                ) {
                    rendered.insert(key.clone(), value);
                }
            }
            if rendered.is_empty() {
                return None;
            }
            Some(Value::Object(rendered))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(source.clone()),
    }
}

fn render_provider_json_value(
    source: &Value,
    provider_translations: &BTreeMap<String, String>,
    existing_target: Option<&Value>,
    path: &str,
    bucket: &BucketConfig,
    target_locale: &str,
    filters: &WorkspaceFilters,
) -> Option<Value> {
    if bucket_ignores_key(bucket, path) {
        return None;
    }

    if bucket_injects_locale(bucket, path) {
        return Some(Value::String(target_locale.to_string()));
    }

    match source {
        Value::String(text) => {
            if !filters.includes_key(path) {
                return Some(Value::String(
                    existing_target
                        .and_then(Value::as_str)
                        .unwrap_or(text)
                        .to_string(),
                ));
            }

            if bucket_locks_key(bucket, path) {
                return Some(Value::String(text.clone()));
            }

            if bucket_preserves_key(bucket, path) {
                return Some(Value::String(safe_target_text(
                    text,
                    existing_target.and_then(Value::as_str),
                )));
            }

            Some(Value::String(safe_target_text(
                text,
                provider_translations
                    .get(path)
                    .map(String::as_str)
                    .or_else(|| existing_target.and_then(Value::as_str)),
            )))
        }
        Value::Array(items) => {
            let rendered = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let child_path = join_key_path(path, &index.to_string());
                    if bucket_ignores_key(bucket, &child_path) {
                        return item.clone();
                    }

                    let existing_child = existing_target.and_then(|value| value.get(index));
                    render_provider_json_value(
                        item,
                        provider_translations,
                        existing_child,
                        &child_path,
                        bucket,
                        target_locale,
                        filters,
                    )
                    .unwrap_or_else(|| existing_child.cloned().unwrap_or_else(|| item.clone()))
                })
                .collect::<Vec<_>>();
            Some(Value::Array(rendered))
        }
        Value::Object(object) => {
            if object.is_empty() {
                return Some(Value::Object(Map::new()));
            }

            let mut rendered = Map::new();
            for (key, item) in object {
                let child_path = join_key_path(path, &escape_json_pointer_segment(key));
                let existing_child = existing_target.and_then(|value| value.get(key));
                if !filters.can_affect_key_path(&child_path) {
                    if let Some(existing_child) = existing_child {
                        rendered.insert(key.clone(), existing_child.clone());
                    } else {
                        rendered.insert(key.clone(), item.clone());
                    }
                    continue;
                }

                if let Some(value) = render_provider_json_value(
                    item,
                    provider_translations,
                    existing_child,
                    &child_path,
                    bucket,
                    target_locale,
                    filters,
                ) {
                    rendered.insert(key.clone(), value);
                }
            }
            if rendered.is_empty() {
                return None;
            }
            Some(Value::Object(rendered))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => Some(source.clone()),
    }
}

fn expand_single_wildcard(root: &Path, localized_pattern: &str) -> Result<Vec<PathBuf>> {
    let path = Path::new(localized_pattern);
    let safe_pattern = normalize_relative_path(localized_pattern)?;
    let file_pattern = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            I18nError::ConfigError(format!("invalid include pattern '{localized_pattern}'"))
        })?;
    let safe_parent = safe_pattern.parent().unwrap_or_else(|| Path::new(""));
    let (prefix, suffix) = file_pattern.split_once('*').ok_or_else(|| {
        I18nError::ConfigError(format!("unsupported include pattern '{localized_pattern}'"))
    })?;
    if suffix.contains('*') {
        return Err(I18nError::ConfigError(format!(
            "include pattern '{localized_pattern}' contains more than one wildcard"
        )));
    }

    let absolute_parent = root.join(safe_parent);
    let mut matches = Vec::new();
    for entry in fs::read_dir(&absolute_parent)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if file_name.starts_with(prefix) && file_name.ends_with(suffix) {
            matches.push(normalize_relative_path(
                &safe_parent.join(file_name.as_ref()).to_string_lossy(),
            )?);
        }
    }

    matches.sort();
    Ok(matches)
}

fn target_path_for_source_pattern(
    pattern: &BucketPattern,
    source_path: &Path,
    source_locale: &str,
    target_locale: &str,
) -> Result<PathBuf> {
    let source_pattern = normalize_pattern_path(&pattern.path_for_locale(source_locale))?;
    let target_pattern = normalize_pattern_path(&pattern.path_for_locale(target_locale))?;
    if !source_pattern.contains('*') {
        return normalize_relative_path(&target_pattern);
    }

    let (source_prefix, source_suffix) = source_pattern.split_once('*').ok_or_else(|| {
        I18nError::ConfigError(format!("unsupported include pattern '{}'", pattern.path()))
    })?;
    let (target_prefix, target_suffix) = target_pattern.split_once('*').ok_or_else(|| {
        I18nError::ConfigError(format!("unsupported include pattern '{}'", pattern.path()))
    })?;
    let source_path = path_to_slash(source_path);
    let capture = source_path
        .strip_prefix(source_prefix)
        .and_then(|remaining| remaining.strip_suffix(source_suffix))
        .ok_or_else(|| {
            I18nError::ConfigError(format!(
                "source path '{}' does not match include pattern '{}'",
                source_path,
                pattern.path()
            ))
        })?;

    normalize_relative_path(&format!("{target_prefix}{capture}{target_suffix}"))
}

fn source_path_is_excluded(
    bucket: &BucketConfig,
    source_locale: &str,
    source_path: &Path,
) -> Result<bool> {
    for pattern in &bucket.exclude {
        if pattern_matches_path(pattern, source_locale, source_path)? {
            return Ok(true);
        }
    }

    Ok(false)
}

fn pattern_matches_path(pattern: &BucketPattern, locale: &str, path: &Path) -> Result<bool> {
    let pattern = normalize_pattern_path(&pattern.path_for_locale(locale))?;
    let path = path_to_slash(path);
    if !pattern.contains('*') {
        return Ok(pattern == path);
    }

    let (prefix, suffix) = pattern.split_once('*').ok_or_else(|| {
        I18nError::ConfigError(format!("unsupported exclude pattern '{pattern}'"))
    })?;
    if suffix.contains('*') {
        return Err(I18nError::ConfigError(format!(
            "exclude pattern '{pattern}' contains more than one wildcard"
        )));
    }

    Ok(path.starts_with(prefix) && path.ends_with(suffix))
}

fn canonical_path_for_pattern(
    pattern: &BucketPattern,
    localized_path: &Path,
    locale: &str,
) -> Result<String> {
    let localized_pattern = normalize_pattern_path(&pattern.path_for_locale(locale))?;
    let canonical_pattern = path_to_slash(&canonical_pattern_path(pattern.path())?);
    let localized_path = path_to_slash(localized_path);

    if !localized_pattern.contains('*') {
        if localized_pattern != localized_path {
            return Err(I18nError::ConfigError(format!(
                "source path '{}' does not match include pattern '{}'",
                localized_path,
                pattern.path()
            )));
        }

        return Ok(canonical_pattern);
    }

    let (localized_prefix, localized_suffix) =
        localized_pattern.split_once('*').ok_or_else(|| {
            I18nError::ConfigError(format!("unsupported include pattern '{}'", pattern.path()))
        })?;
    let (canonical_prefix, canonical_suffix) =
        canonical_pattern.split_once('*').ok_or_else(|| {
            I18nError::ConfigError(format!("unsupported include pattern '{}'", pattern.path()))
        })?;
    let capture = localized_path
        .strip_prefix(localized_prefix)
        .and_then(|remaining| remaining.strip_suffix(localized_suffix))
        .ok_or_else(|| {
            I18nError::ConfigError(format!(
                "source path '{}' does not match include pattern '{}'",
                localized_path,
                pattern.path()
            ))
        })?;

    Ok(path_to_slash(&normalize_relative_path(&format!(
        "{canonical_prefix}{capture}{canonical_suffix}"
    ))?))
}

fn canonical_pattern_path(path: &str) -> Result<PathBuf> {
    let normalized = path.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(I18nError::ConfigError(format!(
            "localization path '{normalized}' must stay inside workspace"
        )));
    }

    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy().replace("[locale]", "");
                if !part.is_empty() {
                    safe.push(part);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(I18nError::ConfigError(format!(
                    "localization path '{normalized}' must stay inside workspace"
                )));
            }
        }
    }

    Ok(safe)
}

fn normalize_pattern_path(path: &str) -> Result<String> {
    Ok(path_to_slash(&normalize_relative_path(path)?))
}

fn path_to_slash(path: &Path) -> String {
    path.iter()
        .filter_map(|part| part.to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn local_markdown_translations(
    source_document: &MarkdownLocalizationDocument,
    target: Option<&str>,
    target_canonical_path: String,
    bucket: &BucketConfig,
    target_locale: &str,
    filters: &WorkspaceFilters,
) -> BTreeMap<String, String> {
    let source_units = source_document.source_units();
    let source_unit_keys = source_units
        .iter()
        .map(|unit| unit.key().to_string())
        .collect::<Vec<_>>();
    let source_unit_map = source_units
        .iter()
        .map(|unit| (unit.key().to_string(), unit.text().to_string()))
        .collect::<BTreeMap<_, _>>();
    let target_units = matching_markdown_target_units(
        &source_unit_keys,
        &source_unit_map,
        target,
        target_canonical_path,
    );

    source_units
        .iter()
        .map(|unit| {
            let text = local_markdown_unit_text(
                bucket,
                target_locale,
                filters,
                unit.key(),
                unit.text(),
                target_units.get(unit.key()).map(String::as_str),
            );
            (unit.key().to_string(), text)
        })
        .collect()
}

fn local_markdown_unit_text(
    bucket: &BucketConfig,
    target_locale: &str,
    filters: &WorkspaceFilters,
    key: &str,
    source_text: &str,
    target_text: Option<&str>,
) -> String {
    if bucket_ignores_key(bucket, key) {
        return source_text.to_string();
    }

    if bucket_injects_locale(bucket, key) {
        return target_locale.to_string();
    }

    if !filters.includes_key(key) {
        return target_text.unwrap_or(source_text).to_string();
    }

    if bucket_locks_key(bucket, key) {
        return source_text.to_string();
    }

    safe_markdown_target_text(source_text, target_text)
}

fn markdown_target_units(
    source_unit_keys: &[String],
    source_units: &BTreeMap<String, String>,
    target: Option<&str>,
    target_canonical_path: String,
) -> BTreeMap<String, String> {
    matching_markdown_target_units(
        source_unit_keys,
        source_units,
        target,
        target_canonical_path,
    )
}

fn matching_markdown_target_units(
    source_unit_keys: &[String],
    source_units: &BTreeMap<String, String>,
    target: Option<&str>,
    target_canonical_path: String,
) -> BTreeMap<String, String> {
    let Some(target) = target else {
        return BTreeMap::new();
    };

    let target_units =
        MarkdownLocalizationDocument::new(target, target_canonical_path).source_units();
    let target_unit_keys = target_units
        .iter()
        .map(|unit| unit.key().to_string())
        .collect::<Vec<_>>();
    let target_units = target_units
        .into_iter()
        .map(|unit| (unit.key().to_string(), unit.text().to_string()))
        .collect::<BTreeMap<_, _>>();

    if markdown_target_has_extra_section_units(source_units, &target_units)
        || !markdown_target_section_sequence_is_compatible(
            &markdown_section_sequence(source_unit_keys.iter()),
            &markdown_section_sequence(target_unit_keys.iter()),
        )
        || markdown_target_has_ambiguous_repeated_sections(
            source_unit_keys,
            source_units,
            &target_unit_keys,
        )
    {
        return BTreeMap::new();
    }

    target_units
        .into_iter()
        .filter(|(key, _)| source_units.contains_key(key))
        .collect()
}

fn markdown_target_has_extra_section_units(
    source_units: &BTreeMap<String, String>,
    target_units: &BTreeMap<String, String>,
) -> bool {
    let source_counts = markdown_section_counts(source_units.keys());
    let target_counts = markdown_section_counts(target_units.keys());
    target_counts
        .into_iter()
        .any(|(section, count)| count > source_counts.get(&section).copied().unwrap_or_default())
}

fn markdown_section_counts<'a>(keys: impl Iterator<Item = &'a String>) -> BTreeMap<String, usize> {
    keys.fold(BTreeMap::new(), |mut counts, key| {
        let section = key
            .rsplit_once('/')
            .map(|(section, _)| section)
            .unwrap_or(key);
        *counts.entry(section.to_string()).or_default() += 1;
        counts
    })
}

fn markdown_section_sequence<'a>(keys: impl Iterator<Item = &'a String>) -> Vec<String> {
    keys.map(|key| markdown_key_section(key).to_string())
        .collect()
}

fn markdown_target_section_sequence_is_compatible(source: &[String], target: &[String]) -> bool {
    target.len() <= source.len()
        && target
            .iter()
            .zip(source)
            .all(|(target, source)| target == source)
}

fn markdown_target_has_ambiguous_repeated_sections(
    source_unit_keys: &[String],
    source_units: &BTreeMap<String, String>,
    target_unit_keys: &[String],
) -> bool {
    let target_counts = markdown_section_counts(target_unit_keys.iter());
    let mut source_sections = BTreeMap::<String, Vec<&String>>::new();

    for key in source_unit_keys {
        source_sections
            .entry(markdown_key_section(key).to_string())
            .or_default()
            .push(key);
    }

    source_sections.into_iter().any(|(section, keys)| {
        keys.len() > 1
            && target_counts.get(&section).copied().unwrap_or_default() > 0
            && !markdown_repeated_units_have_stable_identity(
                keys.into_iter().filter_map(|key| source_units.get(key)),
            )
    })
}

fn markdown_repeated_units_have_stable_identity<'a>(
    texts: impl Iterator<Item = &'a String>,
) -> bool {
    let mut seen = BTreeSet::new();

    for text in texts {
        let signature = markdown_unit_identity_signature(text);
        if signature.is_empty() || !seen.insert(signature) {
            return false;
        }
    }

    true
}

fn markdown_unit_identity_signature(text: &str) -> String {
    let mut anchors = Vec::new();
    collect_brace_anchors(text, &mut anchors);
    collect_inline_code_anchors(text, &mut anchors);
    collect_markdown_link_anchors(text, &mut anchors);
    collect_html_tag_anchors(text, &mut anchors);
    anchors.sort();
    anchors.dedup();
    anchors.join("|")
}

fn collect_brace_anchors(text: &str, anchors: &mut Vec<String>) {
    let mut cursor = 0;

    while cursor < text.len() {
        let rest = &text[cursor..];
        let Some(open) = rest.find('{') else {
            break;
        };
        let open = cursor + open;
        let after_open = &text[open..];

        let (close_pattern, close_start) = if after_open.starts_with("{{") {
            ("}}", open + 2)
        } else {
            ("}", open + 1)
        };

        let Some(close_offset) = text[close_start..].find(close_pattern) else {
            cursor = open + 1;
            continue;
        };
        let close = close_start + close_offset + close_pattern.len();
        anchors.push(format!("brace:{}", &text[open..close]));
        cursor = close;
    }
}

fn collect_inline_code_anchors(text: &str, anchors: &mut Vec<String>) {
    let mut cursor = 0;

    while cursor < text.len() {
        let Some(open_offset) = text[cursor..].find('`') else {
            break;
        };
        let open = cursor + open_offset;
        let marker_len = repeated_markdown_marker_count(text, open, '`');
        let marker = "`".repeat(marker_len);
        let close_start = open + marker_len;
        let Some(close_offset) = text[close_start..].find(&marker) else {
            break;
        };
        let close = close_start + close_offset + marker_len;
        anchors.push(format!("code:{}", &text[open..close]));
        cursor = close;
    }
}

fn repeated_markdown_marker_count(text: &str, start: usize, marker: char) -> usize {
    text[start..]
        .chars()
        .take_while(|character| *character == marker)
        .count()
}

fn collect_markdown_link_anchors(text: &str, anchors: &mut Vec<String>) {
    let mut cursor = 0;

    while cursor < text.len() {
        let Some(link_offset) = text[cursor..].find("](") else {
            break;
        };
        let destination_start = cursor + link_offset + 2;
        let Some(destination_end_offset) = text[destination_start..].find(')') else {
            break;
        };
        let destination_end = destination_start + destination_end_offset;
        anchors.push(format!(
            "link:{}",
            &text[destination_start..destination_end]
        ));
        cursor = destination_end + 1;
    }
}

fn collect_html_tag_anchors(text: &str, anchors: &mut Vec<String>) {
    let mut cursor = 0;

    while cursor < text.len() {
        let Some(open_offset) = text[cursor..].find('<') else {
            break;
        };
        let open = cursor + open_offset;
        let Some(close_offset) = text[open + 1..].find('>') else {
            break;
        };
        let close = open + 1 + close_offset + 1;
        anchors.push(format!("html:{}", &text[open..close]));
        cursor = close;
    }
}

fn markdown_key_section(key: &str) -> &str {
    key.rsplit_once('/')
        .map(|(section, _)| section)
        .unwrap_or(key)
}

fn safe_target_text(source: &str, target: Option<&str>) -> String {
    let Some(target) = target else {
        return source.to_string();
    };

    if preserves_protected_tokens(source, target) {
        target.to_string()
    } else {
        source.to_string()
    }
}

fn safe_markdown_target_text(source: &str, target: Option<&str>) -> String {
    let Some(target) = target else {
        return source.to_string();
    };

    if markdown_unit_text_shape_is_safe(source, target)
        && preserves_protected_tokens(source, target)
    {
        target.to_string()
    } else {
        source.to_string()
    }
}

fn markdown_unit_text_shape_is_safe(source: &str, candidate: &str) -> bool {
    let line_shape_matches = source.contains('\n')
        || source.contains('\r')
        || (!candidate.contains('\n') && !candidate.contains('\r'));
    line_shape_matches
        && markdown_block_marker_kind(source) == markdown_block_marker_kind(candidate)
}

fn markdown_block_marker_kind(text: &str) -> Option<&'static str> {
    let trimmed = text.trim_start();
    if trimmed.is_empty() {
        return None;
    }

    if markdown_heading_marker_len(trimmed).is_some() {
        return Some("heading");
    }
    if trimmed.starts_with('>') {
        return Some("quote");
    }
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return Some("fence");
    }
    if markdown_unordered_list_marker(trimmed) || markdown_ordered_list_marker(trimmed) {
        return Some("list");
    }
    if markdown_thematic_break(trimmed) {
        return Some("thematic_break");
    }

    None
}

fn markdown_heading_marker_len(trimmed: &str) -> Option<usize> {
    let hashes = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if (1..=6).contains(&hashes)
        && trimmed[hashes..]
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace())
    {
        Some(hashes)
    } else {
        None
    }
}

fn markdown_unordered_list_marker(trimmed: &str) -> bool {
    let mut chars = trimmed.chars();
    matches!(chars.next(), Some('-' | '*' | '+'))
        && chars
            .next()
            .is_some_and(|character| character.is_whitespace())
}

fn markdown_ordered_list_marker(trimmed: &str) -> bool {
    let Some((digits, rest)) = trimmed.split_once('.') else {
        return false;
    };
    !digits.is_empty()
        && digits.chars().all(|character| character.is_ascii_digit())
        && rest
            .chars()
            .next()
            .is_some_and(|character| character.is_whitespace())
}

fn markdown_thematic_break(trimmed: &str) -> bool {
    let marker = trimmed.chars().next();
    matches!(marker, Some('-' | '*' | '_'))
        && trimmed
            .chars()
            .all(|character| Some(character) == marker || character.is_whitespace())
        && trimmed
            .chars()
            .filter(|character| Some(*character) == marker)
            .count()
            >= 3
}

fn safe_provider_markdown_translations(
    units: &[TranslationUnit],
    translations: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    units
        .iter()
        .filter_map(|unit| {
            let translated = translations.get(unit.key())?;
            (markdown_unit_text_shape_is_safe(unit.text(), translated)
                && preserves_protected_tokens(unit.text(), translated))
            .then(|| (unit.key().to_string(), translated.clone()))
        })
        .collect()
}

fn accepted_provider_json_translations(
    source: &Value,
    rendered: &Value,
    units: &[TranslationUnit],
) -> BTreeMap<String, String> {
    units
        .iter()
        .filter_map(|unit| {
            let source_text = json_string_at(source, unit.key())?;
            let rendered_text = json_string_at(rendered, unit.key())?;
            (source_text != rendered_text && preserves_protected_tokens(source_text, rendered_text))
                .then(|| (unit.key().to_string(), rendered_text.to_string()))
        })
        .collect()
}

fn json_string_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    let mut current = value;
    for segment in key.split('/') {
        current = match current {
            Value::Array(items) => items.get(segment.parse::<usize>().ok()?)?,
            Value::Object(object) => object.get(&unescape_json_pointer_segment(segment))?,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => return None,
        };
    }
    current.as_str()
}

fn unescape_json_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn normalize_relative_path(path: &str) -> Result<PathBuf> {
    let normalized = path.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(I18nError::ConfigError(format!(
            "localization path '{normalized}' must stay inside workspace"
        )));
    }

    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(I18nError::ConfigError(format!(
                    "localization path '{normalized}' must stay inside workspace"
                )));
            }
        }
    }

    Ok(safe)
}

fn validate_target_locale(locale: &str) -> Result<&str> {
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

fn join_key_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}/{child}")
    }
}

fn escape_json_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn bucket_ignores_key(bucket: &BucketConfig, key: &str) -> bool {
    matches_any_key_pattern(&bucket.ignored_keys, key)
}

fn bucket_locks_key(bucket: &BucketConfig, key: &str) -> bool {
    matches_any_key_pattern(&bucket.locked_keys, key)
}

fn bucket_preserves_key(bucket: &BucketConfig, key: &str) -> bool {
    matches_any_key_pattern(&bucket.preserved_keys, key)
}

fn bucket_injects_locale(bucket: &BucketConfig, key: &str) -> bool {
    matches_any_key_pattern(&bucket.inject_locale, key)
}

fn bucket_sends_key_to_provider(bucket: &BucketConfig, key: &str) -> bool {
    !bucket_ignores_key(bucket, key)
        && !bucket_locks_key(bucket, key)
        && !bucket_preserves_key(bucket, key)
        && !bucket_injects_locale(bucket, key)
}

fn matches_any_key_pattern(patterns: &[String], key: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| key_pattern_matches(pattern, key))
}

fn key_pattern_matches(pattern: &str, key: &str) -> bool {
    let pattern = pattern.trim();
    if pattern == key {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix("/*") {
        return key
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'));
    }

    false
}

fn normalize_lingo_key(key_path: &str) -> String {
    key_path.trim_matches('/').replace('\\', "/")
}
