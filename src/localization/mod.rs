//! Local-first localization workflow with Lingo.dev-compatible boundaries.

pub mod config;
pub mod json_document;
pub mod lingo;
pub mod local_first;
pub mod lockfile;
pub mod markdown_document;
pub mod protection;
pub mod provider;
pub mod workspace;

pub use config::{
    AuthRequirement, BucketConfig, BucketPattern, ConfigIssue, LocaleConfig, LocalizationCommand,
    LocalizationConfig, ProviderConfig, RAW_PROVIDER_IDS, TranslationBackend,
    is_supported_raw_provider_id,
};
pub use json_document::{JsonLocalizationDocument, ensure_json_object};
pub use lingo::{
    DEFAULT_LINGO_API_BASE_URL, DX_LINGO_API_KEY_ENV, DX_LINGO_ENGINE_ID_ENV,
    LINGO_API_BASE_URL_ENV, LINGO_API_KEY_ENV, LINGO_API_URL_ENV, LINGO_ENGINE_ID_ENV,
    LINGODOTDEV_API_KEY_ENV, LingoApiConfig, LingoApiProvider, LingoHttpTimeouts,
    LingoLocalizeResult, LingoUsage,
};
pub use local_first::{
    LocalCatalog, LocalFirstLocalizer, LocalizeError, TranslationPathSegment, TranslationUnit,
};
pub use lockfile::{
    DeltaKind, Lockfile, SourceStringFingerprint, content_hash, key_hash, lingo_key_hash,
    sha256_hex,
};
pub use markdown_document::MarkdownLocalizationDocument;
pub use protection::{
    LocaleKeyDiff, LocaleKeyStructure, ProtectedText, ProtectionError, preserves_protected_tokens,
};
pub use provider::{
    LocalizationBackend, LocalizationOutput, LocalizationProvider, LocalizationResponse,
    LocalizationUsage,
};
pub use workspace::{
    LocalizationWorkspace, LocalizedJsonFile, LocalizedMarkdownFile, WorkspaceFilters,
    WorkspaceStatus,
};

pub type DxI18nConfig = LocalizationConfig;
pub type I18nLock = Lockfile;
