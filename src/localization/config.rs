use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

pub const LINGO_SCHEMA_URL: &str = "https://lingo.dev/schema/i18n.json";
pub const LINGO_SCHEMA_VERSION: &str = "1.15";
pub const RAW_PROVIDER_IDS: &[&str] = &[
    "openai",
    "anthropic",
    "google",
    "mistral",
    "openrouter",
    "ollama",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalizationConfig {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default = "default_schema_version")]
    pub version: String,
    pub locale: LocaleConfig,
    #[serde(default)]
    pub buckets: BTreeMap<String, BucketConfig>,
    #[serde(rename = "engineId", skip_serializing_if = "Option::is_none")]
    pub engine_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocaleConfig {
    pub source: String,
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketConfig {
    pub include: Vec<BucketPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<BucketPattern>,
    #[serde(default, rename = "lockedKeys", skip_serializing_if = "Vec::is_empty")]
    pub locked_keys: Vec<String>,
    #[serde(default, rename = "ignoredKeys", skip_serializing_if = "Vec::is_empty")]
    pub ignored_keys: Vec<String>,
    #[serde(
        default,
        rename = "preservedKeys",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub preserved_keys: Vec<String>,
    #[serde(
        default,
        rename = "injectLocale",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub inject_locale: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BucketPattern {
    Path(String),
    WithDelimiter { path: String, delimiter: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(rename = "baseUrl", skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalizationCommand {
    Init,
    ShowConfig,
    ShowFiles,
    Status,
    Lockfile,
    RunLocal,
    RunRemote,
    Run,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslationBackend {
    LocalOnly,
    LingoEngine { engine_id: Option<String> },
    RawProvider { id: String, model: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthRequirement {
    None,
    LingoApiKey,
    ProviderApiKey { env_var: &'static str },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigIssue {
    pub code: &'static str,
    pub message: String,
}

impl LocalizationConfig {
    pub fn from_json_str(input: &str) -> crate::Result<Self> {
        Ok(serde_json::from_str(input)?)
    }

    pub fn lingo_minimal(source: impl Into<String>, targets: Vec<String>) -> Self {
        Self {
            schema: Some(LINGO_SCHEMA_URL.to_string()),
            version: LINGO_SCHEMA_VERSION.to_string(),
            locale: LocaleConfig {
                source: source.into(),
                targets,
            },
            buckets: BTreeMap::new(),
            engine_id: None,
            provider: None,
        }
    }

    pub fn source_locale(&self) -> &str {
        &self.locale.source
    }

    pub fn target_locales(&self) -> &[String] {
        &self.locale.targets
    }

    pub fn bucket(&self, bucket_type: &str) -> Option<&BucketConfig> {
        self.buckets.get(bucket_type)
    }

    pub fn requires_cloud_auth_for_local_mode(&self) -> bool {
        false
    }

    pub fn backend(&self) -> TranslationBackend {
        if let Some(provider) = &self.provider {
            return TranslationBackend::RawProvider {
                id: provider.id.clone(),
                model: provider.model.clone(),
            };
        }

        if self.engine_id.is_some() {
            return TranslationBackend::LingoEngine {
                engine_id: self.engine_id.clone(),
            };
        }

        TranslationBackend::LocalOnly
    }

    pub fn auth_requirement(&self, command: LocalizationCommand) -> AuthRequirement {
        match command {
            LocalizationCommand::Init
            | LocalizationCommand::ShowConfig
            | LocalizationCommand::ShowFiles
            | LocalizationCommand::Status
            | LocalizationCommand::Lockfile
            | LocalizationCommand::RunLocal
            | LocalizationCommand::Run => AuthRequirement::None,
            LocalizationCommand::RunRemote => match self.backend() {
                TranslationBackend::LocalOnly => AuthRequirement::None,
                TranslationBackend::LingoEngine { .. } => AuthRequirement::LingoApiKey,
                TranslationBackend::RawProvider { ref id, .. } => provider_auth_requirement(id),
            },
        }
    }

    pub fn validate(&self) -> Vec<ConfigIssue> {
        self.validate_with_remote_provider(true)
    }

    pub fn validate_local(&self) -> Vec<ConfigIssue> {
        self.validate_with_remote_provider(false)
    }

    fn validate_with_remote_provider(&self, validate_remote_provider: bool) -> Vec<ConfigIssue> {
        let mut issues = Vec::new();

        if self.version.trim().is_empty() {
            issues.push(ConfigIssue {
                code: "version.empty",
                message: "version must not be empty".to_string(),
            });
        }

        if self.locale.source.trim().is_empty() {
            issues.push(ConfigIssue {
                code: "locale.source.empty",
                message: "locale.source must not be empty".to_string(),
            });
        }
        validate_locale_code("locale.source", &self.locale.source, &mut issues);

        if self.locale.targets.is_empty() {
            issues.push(ConfigIssue {
                code: "locale.targets.empty",
                message: "locale.targets must include at least one target locale".to_string(),
            });
        }
        for target in &self.locale.targets {
            validate_locale_code("locale.targets", target, &mut issues);
        }

        let mut target_set = BTreeSet::new();
        for target in &self.locale.targets {
            let target = target.trim();
            if target.is_empty() {
                issues.push(ConfigIssue {
                    code: "locale.targets.blank",
                    message: "locale.targets must not contain blank locale codes".to_string(),
                });
                continue;
            }

            if target == self.locale.source {
                issues.push(ConfigIssue {
                    code: "locale.targets.source",
                    message: format!("target locale '{target}' duplicates locale.source"),
                });
            }

            if !target_set.insert(target.to_string()) {
                issues.push(ConfigIssue {
                    code: "locale.targets.duplicate",
                    message: format!("target locale '{target}' appears more than once"),
                });
            }
        }

        if self.buckets.is_empty() {
            issues.push(ConfigIssue {
                code: "buckets.empty",
                message: "buckets must define at least one file format bucket".to_string(),
            });
        }

        for (bucket_type, bucket) in &self.buckets {
            validate_bucket(bucket_type, bucket, &mut issues);
        }

        if validate_remote_provider && let Some(provider) = &self.provider {
            validate_provider(provider, &mut issues);
        }

        issues
    }
}

impl BucketPattern {
    pub fn path(&self) -> &str {
        match self {
            Self::Path(path) => path,
            Self::WithDelimiter { path, .. } => path,
        }
    }

    pub fn path_for_locale(&self, locale: &str) -> String {
        self.path()
            .replace("[locale]", &self.locale_for_path(locale))
    }

    pub fn uses_locale_placeholder(&self) -> bool {
        self.path().contains("[locale]")
    }

    pub fn uses_recursive_glob(&self) -> bool {
        self.path().contains("**")
    }

    fn locale_for_path(&self, locale: &str) -> String {
        match self {
            Self::Path(_) => locale.to_string(),
            Self::WithDelimiter { delimiter, .. } => locale.replace('-', delimiter),
        }
    }
}

impl PartialEq<&str> for BucketPattern {
    fn eq(&self, other: &&str) -> bool {
        self.path() == *other
    }
}

impl PartialEq<BucketPattern> for &str {
    fn eq(&self, other: &BucketPattern) -> bool {
        *self == other.path()
    }
}

fn validate_bucket(bucket_type: &str, bucket: &BucketConfig, issues: &mut Vec<ConfigIssue>) {
    if bucket_type.trim().is_empty() {
        issues.push(ConfigIssue {
            code: "bucket.type.empty",
            message: "bucket type must not be empty".to_string(),
        });
    } else if !is_lingo_supported_bucket_type(bucket_type) {
        issues.push(ConfigIssue {
            code: "bucket.type.unsupported",
            message: format!(
                "bucket type '{bucket_type}' is not a Lingo.dev-supported format known to this DX adapter"
            ),
        });
    }

    if bucket.include.is_empty() {
        issues.push(ConfigIssue {
            code: "bucket.include.empty",
            message: format!("bucket '{bucket_type}' must include at least one pattern"),
        });
    }

    let requires_locale = bucket_requires_locale_placeholder(bucket_type);
    for pattern in bucket.include.iter().chain(bucket.exclude.iter()) {
        if pattern.path().trim().is_empty() {
            issues.push(ConfigIssue {
                code: "bucket.pattern.empty",
                message: format!("bucket '{bucket_type}' contains an empty path pattern"),
            });
        }

        if pattern.uses_recursive_glob() {
            issues.push(ConfigIssue {
                code: "bucket.pattern.recursive_glob",
                message: format!(
                    "bucket '{bucket_type}' pattern '{}' uses unsupported recursive glob '**'",
                    pattern.path()
                ),
            });
        }

        if let BucketPattern::WithDelimiter { delimiter, .. } = pattern {
            if delimiter.is_empty() || delimiter.contains('/') || delimiter.contains('\\') {
                issues.push(ConfigIssue {
                    code: "bucket.pattern.delimiter",
                    message: format!(
                        "bucket '{bucket_type}' delimiter must be a non-path separator"
                    ),
                });
            }
        }
    }

    for pattern in &bucket.include {
        if requires_locale && !pattern.uses_locale_placeholder() {
            issues.push(ConfigIssue {
                code: "bucket.include.missing_locale",
                message: format!(
                    "bucket '{bucket_type}' include pattern '{}' must contain [locale]",
                    pattern.path()
                ),
            });
        }
    }

    validate_key_paths(bucket_type, "lockedKeys", &bucket.locked_keys, issues);
    validate_key_paths(bucket_type, "ignoredKeys", &bucket.ignored_keys, issues);
    validate_key_paths(bucket_type, "preservedKeys", &bucket.preserved_keys, issues);
    validate_key_paths(bucket_type, "injectLocale", &bucket.inject_locale, issues);
}

fn validate_key_paths(
    bucket_type: &str,
    field: &'static str,
    paths: &[String],
    issues: &mut Vec<ConfigIssue>,
) {
    for path in paths {
        if path.trim().is_empty() || path.split('/').any(str::is_empty) {
            issues.push(ConfigIssue {
                code: "bucket.key_path.invalid",
                message: format!("bucket '{bucket_type}' {field} contains invalid path '{path}'"),
            });
        } else if path.contains('*') && !path.ends_with("/*") {
            issues.push(ConfigIssue {
                code: "bucket.key_path.unsupported_wildcard",
                message: format!(
                    "bucket '{bucket_type}' {field} path '{path}' only supports trailing /* wildcards"
                ),
            });
        }
    }
}

fn validate_locale_code(field: &'static str, locale: &str, issues: &mut Vec<ConfigIssue>) {
    let trimmed = locale.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        issues.push(ConfigIssue {
            code: "locale.code.invalid",
            message: format!("{field} contains invalid locale '{locale}'"),
        });
    }
}

fn bucket_requires_locale_placeholder(bucket_type: &str) -> bool {
    !matches!(bucket_type, "csv" | "xcode-xcstrings" | "yaml-root-key")
}

fn is_lingo_supported_bucket_type(bucket_type: &str) -> bool {
    matches!(
        bucket_type,
        "android"
            | "csv"
            | "csv-dictionary"
            | "csv-per-locale"
            | "flutter"
            | "flutter-arb"
            | "gettext"
            | "html"
            | "ios-strings"
            | "java-properties"
            | "json"
            | "json-dictionary"
            | "json5"
            | "jsonc"
            | "markdown"
            | "markdoc"
            | "md"
            | "mdx"
            | "mjml"
            | "php"
            | "po"
            | "properties"
            | "rails-yaml"
            | "strings"
            | "stringsdict"
            | "srt"
            | "txt"
            | "typescript"
            | "vtt"
            | "vue-json"
            | "xcode-strings"
            | "xcode-stringsdict"
            | "xcode-xcstrings"
            | "xcstrings"
            | "xliff"
            | "xml"
            | "yaml"
            | "yaml-root-key"
            | "yml"
    )
}

fn provider_auth_requirement(provider_id: &str) -> AuthRequirement {
    match provider_id {
        "ollama" => AuthRequirement::None,
        "openai" => AuthRequirement::ProviderApiKey {
            env_var: "OPENAI_API_KEY",
        },
        "anthropic" => AuthRequirement::ProviderApiKey {
            env_var: "ANTHROPIC_API_KEY",
        },
        "google" => AuthRequirement::ProviderApiKey {
            env_var: "GOOGLE_API_KEY",
        },
        "mistral" => AuthRequirement::ProviderApiKey {
            env_var: "MISTRAL_API_KEY",
        },
        "openrouter" => AuthRequirement::ProviderApiKey {
            env_var: "OPENROUTER_API_KEY",
        },
        _ => AuthRequirement::ProviderApiKey {
            env_var: "PROVIDER_API_KEY",
        },
    }
}

fn validate_provider(provider: &ProviderConfig, issues: &mut Vec<ConfigIssue>) {
    let id = provider.id.trim();
    if !is_supported_raw_provider_id(id) {
        issues.push(ConfigIssue {
            code: "provider.id.unsupported",
            message: format!(
                "provider.id '{id}' is not one of openai, anthropic, google, mistral, openrouter, or ollama"
            ),
        });
    }

    if provider.model.trim().is_empty() {
        issues.push(ConfigIssue {
            code: "provider.model.empty",
            message: "provider.model must not be empty".to_string(),
        });
    }

    if id == "ollama" {
        match provider.base_url.as_deref().map(str::trim) {
            None | Some("") => issues.push(ConfigIssue {
                code: "provider.base_url.required",
                message: "provider.baseUrl is required for local ollama providers".to_string(),
            }),
            Some(base_url) => validate_ollama_base_url(base_url, issues),
        }
    }
}

pub fn is_supported_raw_provider_id(provider_id: &str) -> bool {
    RAW_PROVIDER_IDS.contains(&provider_id)
}

fn validate_ollama_base_url(base_url: &str, issues: &mut Vec<ConfigIssue>) {
    let Ok(parsed) = Url::parse(base_url) else {
        issues.push(ConfigIssue {
            code: "provider.base_url.invalid",
            message: "provider.baseUrl must be a valid URL".to_string(),
        });
        return;
    };

    let is_local_http = matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some_and(is_loopback_host)
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query().is_none()
        && parsed.fragment().is_none();

    if !is_local_http {
        issues.push(ConfigIssue {
            code: "provider.base_url.loopback_required",
            message:
                "provider.baseUrl for auth-free ollama providers must target localhost or loopback without credentials, query, or fragment"
                    .to_string(),
        });
    }
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn default_schema_version() -> String {
    LINGO_SCHEMA_VERSION.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_and_lockfile_are_auth_free_for_local_configs() {
        let config = LocalizationConfig::lingo_minimal("en", vec!["es".to_string()]);

        assert_eq!(
            config.auth_requirement(LocalizationCommand::Status),
            AuthRequirement::None
        );
        assert_eq!(
            config.auth_requirement(LocalizationCommand::Lockfile),
            AuthRequirement::None
        );
        assert_eq!(config.backend(), TranslationBackend::LocalOnly);
    }

    #[test]
    fn local_minimal_run_stays_auth_free_by_default() {
        let config = LocalizationConfig::lingo_minimal("en", vec!["es".to_string()]);

        assert_eq!(config.backend(), TranslationBackend::LocalOnly);
        assert_eq!(
            config.auth_requirement(LocalizationCommand::Run),
            AuthRequirement::None
        );
    }

    #[test]
    fn lingo_engine_run_requires_lingo_api_key() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["de".to_string()]);
        config.engine_id = Some("eng_test".to_string());

        assert_eq!(
            config.backend(),
            TranslationBackend::LingoEngine {
                engine_id: Some("eng_test".to_string())
            }
        );
        assert_eq!(
            config.auth_requirement(LocalizationCommand::Status),
            AuthRequirement::None
        );
        assert_eq!(
            config.auth_requirement(LocalizationCommand::RunLocal),
            AuthRequirement::None
        );
        assert_eq!(
            config.auth_requirement(LocalizationCommand::RunRemote),
            AuthRequirement::LingoApiKey
        );
    }

    #[test]
    fn raw_provider_auth_is_only_required_for_run() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["fr".to_string()]);
        config.provider = Some(ProviderConfig {
            id: "openai".to_string(),
            model: "gpt-4o-mini".to_string(),
            prompt: None,
            base_url: None,
        });

        assert_eq!(
            config.auth_requirement(LocalizationCommand::Status),
            AuthRequirement::None
        );
        assert_eq!(
            config.auth_requirement(LocalizationCommand::RunLocal),
            AuthRequirement::None
        );
        assert_eq!(
            config.auth_requirement(LocalizationCommand::RunRemote),
            AuthRequirement::ProviderApiKey {
                env_var: "OPENAI_API_KEY"
            }
        );
    }

    #[test]
    fn validates_lingo_supported_provider_ids_and_ollama_base_url() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["fr".to_string()]);
        config.provider = Some(ProviderConfig {
            id: "ollama".to_string(),
            model: "llama3.1".to_string(),
            prompt: None,
            base_url: Some("http://127.0.0.1:11434".to_string()),
        });

        assert!(
            !config
                .validate()
                .iter()
                .any(|issue| issue.code.starts_with("provider."))
        );

        config.provider = Some(ProviderConfig {
            id: "unknown".to_string(),
            model: " ".to_string(),
            prompt: None,
            base_url: None,
        });
        let issues = config.validate();

        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "provider.id.unsupported")
        );
        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "provider.model.empty")
        );

        config.provider = Some(ProviderConfig {
            id: "ollama".to_string(),
            model: "llama3.1".to_string(),
            prompt: None,
            base_url: None,
        });
        assert!(
            config
                .validate()
                .iter()
                .any(|issue| issue.code == "provider.base_url.required")
        );

        config.provider = Some(ProviderConfig {
            id: "ollama".to_string(),
            model: "llama3.1".to_string(),
            prompt: None,
            base_url: Some("http://192.168.1.10:11434".to_string()),
        });
        assert!(
            config
                .validate()
                .iter()
                .any(|issue| issue.code == "provider.base_url.loopback_required")
        );
    }

    #[test]
    fn local_validation_does_not_reject_optional_remote_provider_metadata() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["fr".to_string()]);
        config.buckets.insert(
            "json".to_string(),
            BucketConfig {
                include: vec![BucketPattern::Path("locales/[locale].json".to_string())],
                exclude: Vec::new(),
                locked_keys: Vec::new(),
                ignored_keys: Vec::new(),
                preserved_keys: Vec::new(),
                inject_locale: Vec::new(),
            },
        );
        config.provider = Some(ProviderConfig {
            id: "future-provider".to_string(),
            model: " ".to_string(),
            prompt: None,
            base_url: None,
        });

        assert!(config.validate_local().is_empty());
        assert!(
            config
                .validate()
                .iter()
                .any(|issue| issue.code == "provider.id.unsupported")
        );
    }

    #[test]
    fn json_bucket_requires_locale_placeholder() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["ja".to_string()]);
        config.buckets.insert(
            "json".to_string(),
            BucketConfig {
                include: vec![BucketPattern::Path("locales/app.json".to_string())],
                exclude: Vec::new(),
                locked_keys: Vec::new(),
                ignored_keys: Vec::new(),
                preserved_keys: Vec::new(),
                inject_locale: Vec::new(),
            },
        );

        let issues = config.validate();

        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "bucket.include.missing_locale")
        );
    }

    #[test]
    fn validates_key_guard_wildcards_match_runtime_support() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["ja".to_string()]);
        config.buckets.insert(
            "json".to_string(),
            BucketConfig {
                include: vec![BucketPattern::Path("locales/[locale].json".to_string())],
                exclude: Vec::new(),
                locked_keys: vec!["config/*/url".to_string()],
                ignored_keys: vec!["internal*".to_string()],
                preserved_keys: vec!["manual/**".to_string()],
                inject_locale: Vec::new(),
            },
        );

        let issues = config.validate();

        assert!(
            issues
                .iter()
                .any(|issue| issue.code == "bucket.key_path.unsupported_wildcard")
        );
    }

    #[test]
    fn validates_locale_codes_for_path_safe_workspace_expansion() {
        let config = LocalizationConfig::lingo_minimal("../en", vec!["es".to_string()]);
        assert!(
            config
                .validate()
                .iter()
                .any(|issue| issue.code == "locale.code.invalid")
        );

        let config = LocalizationConfig::lingo_minimal("en", vec!["../es".to_string()]);
        assert!(
            config
                .validate()
                .iter()
                .any(|issue| issue.code == "locale.code.invalid")
        );
    }

    #[test]
    fn mutating_source_bucket_does_not_require_locale_placeholder() {
        let mut config = LocalizationConfig::lingo_minimal("en", vec!["de".to_string()]);
        config.buckets.insert(
            "xcode-xcstrings".to_string(),
            BucketConfig {
                include: vec![BucketPattern::Path("ios/Localizable.xcstrings".to_string())],
                exclude: Vec::new(),
                locked_keys: Vec::new(),
                ignored_keys: Vec::new(),
                preserved_keys: Vec::new(),
                inject_locale: Vec::new(),
            },
        );

        assert!(config.validate().is_empty());
    }

    #[test]
    fn validates_lingo_bucket_types_without_rejecting_unimplemented_formats() {
        for bucket_type in ["flutter", "markdoc", "srt", "txt", "vtt", "yaml"] {
            let mut config = LocalizationConfig::lingo_minimal("en", vec!["de".to_string()]);
            config.buckets.insert(
                bucket_type.to_string(),
                BucketConfig {
                    include: vec![BucketPattern::Path(format!(
                        "locales/[locale]/messages.{bucket_type}"
                    ))],
                    exclude: Vec::new(),
                    locked_keys: Vec::new(),
                    ignored_keys: Vec::new(),
                    preserved_keys: Vec::new(),
                    inject_locale: Vec::new(),
                },
            );

            assert!(
                !config
                    .validate()
                    .iter()
                    .any(|issue| issue.code == "bucket.type.unsupported"),
                "{bucket_type} should be treated as a known Lingo.dev bucket type"
            );
        }

        let mut config = LocalizationConfig::lingo_minimal("en", vec!["de".to_string()]);
        config.buckets.clear();
        config.buckets.insert(
            "future-bucket".to_string(),
            BucketConfig {
                include: vec![BucketPattern::Path("locales/[locale].future".to_string())],
                exclude: Vec::new(),
                locked_keys: Vec::new(),
                ignored_keys: Vec::new(),
                preserved_keys: Vec::new(),
                inject_locale: Vec::new(),
            },
        );

        assert!(
            config
                .validate()
                .iter()
                .any(|issue| issue.code == "bucket.type.unsupported")
        );
    }
}
