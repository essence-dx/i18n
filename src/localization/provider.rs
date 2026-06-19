use crate::Result;
use crate::localization::lingo::{LingoApiProvider, LingoLocalizeResult, LingoUsage};
use crate::localization::local_first::{LocalCatalog, LocalFirstLocalizer, TranslationUnit};
use async_trait::async_trait;
use std::collections::BTreeMap;

pub type LocalizationOutput = BTreeMap<String, String>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalizationResponse {
    pub source_locale: Option<String>,
    pub target_locale: Option<String>,
    pub translations: LocalizationOutput,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub usage: Option<LocalizationUsage>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LocalizationUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub llm_cost: Option<f64>,
    pub localization_cost: Option<f64>,
    pub cost: Option<f64>,
}

impl LocalizationResponse {
    pub fn local(
        source_locale: impl Into<String>,
        target_locale: impl Into<String>,
        translations: LocalizationOutput,
    ) -> Self {
        Self {
            source_locale: Some(source_locale.into()),
            target_locale: Some(target_locale.into()),
            translations,
            provider: Some("local".to_string()),
            model: None,
            usage: None,
        }
    }
}

impl From<LingoLocalizeResult> for LocalizationResponse {
    fn from(result: LingoLocalizeResult) -> Self {
        Self {
            source_locale: result.source_locale,
            target_locale: result.target_locale,
            translations: result.translations,
            provider: Some("lingo.dev".to_string()),
            model: result.model,
            usage: result.usage.map(LocalizationUsage::from),
        }
    }
}

impl From<LingoUsage> for LocalizationUsage {
    fn from(usage: LingoUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            llm_cost: usage.llm_cost,
            localization_cost: usage.localization_cost,
            cost: usage.cost,
        }
    }
}

#[async_trait]
pub trait LocalizationProvider: Send + Sync {
    fn requires_cloud_auth(&self) -> bool;

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizationResponse>;

    async fn localize_units(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizationOutput> {
        Ok(self
            .localize_response(source_locale, target_locale, units)
            .await?
            .translations)
    }
}

#[derive(Clone, Debug)]
pub enum LocalizationBackend {
    Local(LocalFirstLocalizer),
    Lingo(LingoApiProvider),
}

impl LocalizationBackend {
    pub fn local_only(catalog: LocalCatalog) -> Self {
        Self::Local(LocalFirstLocalizer::local_only(catalog))
    }

    pub fn lingo(provider: LingoApiProvider) -> Self {
        Self::Lingo(provider)
    }
}

#[async_trait]
impl LocalizationProvider for LocalizationBackend {
    fn requires_cloud_auth(&self) -> bool {
        match self {
            Self::Local(provider) => provider.requires_cloud_auth(),
            Self::Lingo(_) => true,
        }
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizationResponse> {
        match self {
            Self::Local(provider) => provider
                .localize_units(source_locale, target_locale, units)
                .await
                .map(|translations| {
                    LocalizationResponse::local(source_locale, target_locale, translations)
                })
                .map_err(|error| crate::I18nError::ConfigError(error.to_string())),
            Self::Lingo(provider) => provider
                .localize_response(source_locale, target_locale, units)
                .await
                .map(LocalizationResponse::from),
        }
    }
}

#[async_trait]
impl LocalizationProvider for LocalFirstLocalizer {
    fn requires_cloud_auth(&self) -> bool {
        LocalFirstLocalizer::requires_cloud_auth(self)
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizationResponse> {
        LocalFirstLocalizer::localize_units(self, source_locale, target_locale, units)
            .await
            .map(|translations| {
                LocalizationResponse::local(source_locale, target_locale, translations)
            })
            .map_err(|error| crate::I18nError::ConfigError(error.to_string()))
    }
}

#[async_trait]
impl LocalizationProvider for LingoApiProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizationResponse> {
        LingoApiProvider::localize_response(self, source_locale, target_locale, units)
            .await
            .map(LocalizationResponse::from)
    }
}
