use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

pub type LocalizedUnits = BTreeMap<String, String>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslationUnit {
    key: String,
    text: String,
    path_segments: Option<Vec<TranslationPathSegment>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranslationPathSegment {
    ObjectKey(String),
    ArrayIndex(usize),
}

impl TranslationUnit {
    pub fn new(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            text: text.into(),
            path_segments: None,
        }
    }

    pub fn with_path_segments(
        key: impl Into<String>,
        text: impl Into<String>,
        path_segments: Vec<TranslationPathSegment>,
    ) -> Self {
        Self {
            key: key.into(),
            text: text.into(),
            path_segments: Some(path_segments),
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn path_segments(&self) -> Option<&[TranslationPathSegment]> {
        self.path_segments.as_deref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalCatalog {
    source_locale: String,
    translations: BTreeMap<String, BTreeMap<String, String>>,
}

impl LocalCatalog {
    pub fn new(source_locale: impl Into<String>) -> Self {
        Self {
            source_locale: source_locale.into().trim().to_string(),
            translations: BTreeMap::new(),
        }
    }

    pub fn source_locale(&self) -> &str {
        &self.source_locale
    }

    pub fn insert(
        &mut self,
        target_locale: impl Into<String>,
        key: impl Into<String>,
        translation: impl Into<String>,
    ) -> Option<String> {
        let target_locale = target_locale.into().trim().to_string();
        let key = key.into().trim().to_string();

        if target_locale.is_empty() || key.is_empty() {
            return None;
        }

        self.translations
            .entry(target_locale)
            .or_default()
            .insert(key, translation.into())
    }

    pub fn get(&self, target_locale: &str, key: &str) -> Option<&str> {
        self.translations
            .get(target_locale.trim())
            .and_then(|locale| locale.get(key))
            .map(String::as_str)
    }

    pub fn target_locales(&self) -> Vec<&str> {
        self.translations.keys().map(String::as_str).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.translations.values().all(BTreeMap::is_empty)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalizeError {
    EmptyLocale { field: &'static str },
    EmptyKey,
    DuplicateKey { key: String },
    SourceLocaleMismatch { catalog: String, requested: String },
}

impl fmt::Display for LocalizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLocale { field } => write!(formatter, "{field} locale cannot be empty"),
            Self::EmptyKey => formatter.write_str("translation unit key cannot be empty"),
            Self::DuplicateKey { key } => {
                write!(formatter, "duplicate translation unit key '{key}'")
            }
            Self::SourceLocaleMismatch { catalog, requested } => write!(
                formatter,
                "catalog source locale '{catalog}' does not match requested source locale '{requested}'"
            ),
        }
    }
}

impl Error for LocalizeError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalFirstLocalizer {
    catalog: LocalCatalog,
}

impl LocalFirstLocalizer {
    pub fn local_only(catalog: LocalCatalog) -> Self {
        Self { catalog }
    }

    pub fn catalog(&self) -> &LocalCatalog {
        &self.catalog
    }

    pub fn requires_cloud_auth(&self) -> bool {
        false
    }

    pub async fn localize_units(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizedUnits, LocalizeError> {
        self.localize_units_sync(source_locale, target_locale, units)
    }

    pub fn localize_units_sync(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LocalizedUnits, LocalizeError> {
        let source_locale = normalize_locale("source", source_locale)?;
        let target_locale = normalize_locale("target", target_locale)?;
        let catalog_source = normalize_locale("catalog source", self.catalog.source_locale())?;

        if source_locale != catalog_source {
            return Err(LocalizeError::SourceLocaleMismatch {
                catalog: catalog_source.to_string(),
                requested: source_locale.to_string(),
            });
        }

        let mut translated = BTreeMap::new();
        let mut seen_keys = BTreeSet::new();
        for unit in units {
            let key = unit.key();
            let trimmed_key = key.trim();
            if trimmed_key.is_empty() {
                return Err(LocalizeError::EmptyKey);
            }

            if !seen_keys.insert(key.to_string()) {
                return Err(LocalizeError::DuplicateKey {
                    key: key.to_string(),
                });
            }

            let value = if source_locale == target_locale {
                unit.text().to_string()
            } else {
                self.catalog
                    .get(target_locale, key)
                    .or_else(|| self.catalog.get(target_locale, trimmed_key))
                    .unwrap_or_else(|| unit.text())
                    .to_string()
            };
            translated.insert(key.to_string(), value);
        }

        Ok(translated)
    }
}

fn normalize_locale<'a>(field: &'static str, locale: &'a str) -> Result<&'a str, LocalizeError> {
    let locale = locale.trim();
    if locale.is_empty() {
        return Err(LocalizeError::EmptyLocale { field });
    }
    Ok(locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_uses_catalog_before_source_fallback() {
        let mut catalog = LocalCatalog::new("en");
        catalog.insert("es", "nav/title", "Panel");

        let localizer = LocalFirstLocalizer::local_only(catalog);
        let units = vec![
            TranslationUnit::new("nav/title", "Dashboard"),
            TranslationUnit::new("nav/subtitle", "Welcome {name}"),
        ];

        let translated = localizer
            .localize_units_sync("en", "es", &units)
            .expect("local-only localization should not require cloud auth");

        assert_eq!(translated.get("nav/title").unwrap(), "Panel");
        assert_eq!(translated.get("nav/subtitle").unwrap(), "Welcome {name}");
        assert!(!localizer.requires_cloud_auth());
    }

    #[test]
    fn local_only_rejects_blank_unit_keys() {
        let localizer = LocalFirstLocalizer::local_only(LocalCatalog::new("en"));
        let error = localizer
            .localize_units_sync("en", "es", &[TranslationUnit::new("   ", "Untitled")])
            .expect_err("blank keys should not be localized");

        assert_eq!(error, LocalizeError::EmptyKey);
    }

    #[test]
    fn catalog_insert_ignores_blank_locale_or_key() {
        let mut catalog = LocalCatalog::new("en");

        assert_eq!(catalog.insert("", "nav/title", "Panel"), None);
        assert_eq!(catalog.insert("es", "   ", "Panel"), None);
        assert!(catalog.is_empty());
    }

    #[test]
    fn catalog_insert_reports_duplicate_replacement() {
        let mut catalog = LocalCatalog::new("en");

        assert_eq!(catalog.insert("es", "nav/title", "Panel"), None);
        assert_eq!(
            catalog.insert("es", " nav/title ", "Inicio"),
            Some("Panel".to_string())
        );
        assert_eq!(catalog.get("es", "nav/title"), Some("Inicio"));
    }

    #[test]
    fn local_only_rejects_duplicate_unit_keys() {
        let localizer = LocalFirstLocalizer::local_only(LocalCatalog::new("en"));
        let error = localizer
            .localize_units_sync(
                "en",
                "es",
                &[
                    TranslationUnit::new("nav/title", "Dashboard"),
                    TranslationUnit::new("nav/title", "Home"),
                ],
            )
            .expect_err("duplicate keys should not overwrite earlier values");

        assert_eq!(
            error,
            LocalizeError::DuplicateKey {
                key: "nav/title".to_string()
            }
        );
    }

    #[test]
    fn local_only_preserves_exact_unit_keys_with_spaces() {
        let localizer = LocalFirstLocalizer::local_only(LocalCatalog::new("en"));
        let translated = localizer
            .localize_units_sync(
                "en",
                "es",
                &[
                    TranslationUnit::new(" nav/title ", "Dashboard"),
                    TranslationUnit::new("nav/title", "Home"),
                ],
            )
            .expect("exact keys should not be collapsed");

        assert_eq!(translated.get(" nav/title ").unwrap(), "Dashboard");
        assert_eq!(translated.get("nav/title").unwrap(), "Home");
    }
}
