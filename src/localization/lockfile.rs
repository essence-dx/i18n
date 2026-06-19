use crate::error::{I18nError, Result};
use crate::localization::TranslationUnit;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const LOCKFILE_VERSION: u32 = 1;
pub const HASH_ALGORITHM: &str = "sha256";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub version: u32,
    pub checksums: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStringFingerprint {
    pub bucket_type: String,
    pub source_locale: String,
    pub source_path: String,
    pub key_path: String,
    pub content_hash: String,
    pub key_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaKind {
    Current,
    RenamedKey { previous_label: String },
    ChangedContent { previous_content_hash: String },
    New,
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            checksums: BTreeMap::new(),
        }
    }

    pub fn record(&mut self, fingerprint: &SourceStringFingerprint) {
        self.checksums
            .entry(fingerprint.content_hash.clone())
            .or_default()
            .insert(fingerprint.lock_label(), fingerprint.key_hash.clone());
    }

    pub fn from_source_units(units: &[TranslationUnit]) -> Self {
        let mut lockfile = Self::new();
        for unit in units {
            lockfile.record(&SourceStringFingerprint::from_unit(unit));
        }
        lockfile
    }

    pub fn needs_translation(&self, unit: &TranslationUnit) -> bool {
        !matches!(
            self.classify(&SourceStringFingerprint::from_unit(unit)),
            DeltaKind::Current | DeltaKind::RenamedKey { .. }
        )
    }

    pub fn has_same_content_with_different_key(&self, unit: &TranslationUnit) -> bool {
        matches!(
            self.classify(&SourceStringFingerprint::from_unit(unit)),
            DeltaKind::RenamedKey { .. }
        )
    }

    pub fn classify(&self, fingerprint: &SourceStringFingerprint) -> DeltaKind {
        if let Some(labels) = self.checksums.get(&fingerprint.content_hash) {
            let label = fingerprint.lock_label();
            if labels.get(&label) == Some(&fingerprint.key_hash) {
                return DeltaKind::Current;
            }

            if let Some(previous_label) = labels
                .iter()
                .find_map(|(label, key_hash)| (key_hash != &fingerprint.key_hash).then(|| label))
            {
                return DeltaKind::RenamedKey {
                    previous_label: previous_label.clone(),
                };
            }
        }

        if let Some(previous_content_hash) = self.find_content_hash_for_key(&fingerprint.key_hash) {
            return DeltaKind::ChangedContent {
                previous_content_hash,
            };
        }

        DeltaKind::New
    }

    pub fn to_lingo_yaml(&self) -> String {
        let mut yaml = String::from("version: 1\nchecksums:\n");

        for (content_hash, keys) in &self.checksums {
            yaml.push_str("  ");
            yaml.push_str(content_hash);
            yaml.push_str(":\n");

            for (label, key_hash) in keys {
                yaml.push_str("    ");
                yaml.push_str(&yaml_key(label));
                yaml.push_str(": ");
                yaml.push_str(key_hash);
                yaml.push('\n');
            }
        }

        yaml
    }

    pub fn from_lingo_yaml(contents: &str) -> Result<Self> {
        let mut version = None;
        let mut checksums: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        let mut in_checksums = false;
        let mut current_content_hash: Option<String> = None;

        for (line_index, raw_line) in contents.lines().enumerate() {
            let line_number = line_index + 1;
            let line = raw_line.trim_end();
            let trimmed = line.trim();

            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            if !line.starts_with(' ') {
                if let Some(raw_version) = trimmed.strip_prefix("version:") {
                    let parsed_version = raw_version.trim().parse::<u32>().map_err(|error| {
                        I18nError::ConfigError(format!(
                            "invalid i18n.lock version on line {line_number}: {error}"
                        ))
                    })?;
                    version = Some(parsed_version);
                    continue;
                }

                if trimmed == "checksums:" {
                    in_checksums = true;
                    continue;
                }
            }

            if !in_checksums {
                return Err(I18nError::ConfigError(format!(
                    "unexpected i18n.lock line outside checksums on line {line_number}"
                )));
            }

            if line.starts_with("  ") && !line.starts_with("    ") {
                let hash = trimmed.trim_end_matches(':');
                validate_sha256_hex(hash, "content checksum", line_number)?;
                checksums.entry(hash.to_string()).or_default();
                current_content_hash = Some(hash.to_string());
                continue;
            }

            if line.starts_with("    ") {
                let (raw_label, key_hash) =
                    split_lingo_lock_key_checksum(trimmed).ok_or_else(|| {
                        I18nError::ConfigError(format!(
                            "invalid i18n.lock key checksum on line {line_number}"
                        ))
                    })?;
                validate_sha256_hex(key_hash, "key checksum", line_number)?;
                let label = unquote_yaml_key(raw_label)?;
                let content_hash = current_content_hash.as_ref().ok_or_else(|| {
                    I18nError::ConfigError(format!(
                        "i18n.lock key checksum has no parent content checksum on line {line_number}"
                    ))
                })?;

                checksums
                    .get_mut(content_hash)
                    .expect("parent checksum was inserted")
                    .insert(label, key_hash.to_string());
                continue;
            }

            return Err(I18nError::ConfigError(format!(
                "unsupported i18n.lock indentation on line {line_number}"
            )));
        }

        let version = version
            .ok_or_else(|| I18nError::ConfigError("i18n.lock must declare version".to_string()))?;
        if version != LOCKFILE_VERSION {
            return Err(I18nError::ConfigError(format!(
                "unsupported i18n.lock version {version}; expected {LOCKFILE_VERSION}"
            )));
        }

        Ok(Self { version, checksums })
    }

    fn find_content_hash_for_key(&self, key_hash: &str) -> Option<String> {
        self.checksums
            .iter()
            .find(|(_, keys)| keys.values().any(|candidate| candidate == key_hash))
            .map(|(content_hash, _)| content_hash.clone())
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceStringFingerprint {
    pub fn new(
        bucket_type: impl Into<String>,
        source_locale: impl Into<String>,
        source_path: impl Into<String>,
        key_path: impl Into<String>,
        source_value: impl AsRef<str>,
    ) -> Self {
        let bucket_type = bucket_type.into();
        let source_locale = source_locale.into();
        let source_path = normalize_path(&source_path.into());
        let key_path = normalize_key_path(&key_path.into());

        Self {
            content_hash: content_hash(source_value.as_ref()),
            key_hash: key_hash(&bucket_type, &source_locale, &source_path, &key_path),
            bucket_type,
            source_locale,
            source_path,
            key_path,
        }
    }

    pub fn lock_label(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.bucket_type, self.source_locale, self.source_path, self.key_path
        )
    }

    fn from_unit(unit: &TranslationUnit) -> Self {
        Self {
            bucket_type: "lingo".to_string(),
            source_locale: String::new(),
            source_path: String::new(),
            content_hash: content_hash(unit.text()),
            key_hash: lingo_key_hash(unit.key()),
            key_path: normalize_key_path(unit.key()),
        }
    }
}

pub fn content_hash(source_value: &str) -> String {
    sha256_hex(source_value.as_bytes())
}

pub fn key_hash(
    bucket_type: &str,
    source_locale: &str,
    source_path: &str,
    key_path: &str,
) -> String {
    let identity = [
        bucket_type,
        source_locale,
        &normalize_path(source_path),
        &normalize_key_path(key_path),
    ]
    .join("\u{0}");

    sha256_hex(identity.as_bytes())
}

pub fn lingo_key_hash(key_path: &str) -> String {
    sha256_hex(normalize_key_path(key_path).as_bytes())
}

pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    let digest = Sha256::digest(bytes.as_ref());
    format!("{digest:x}")
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/")
}

fn normalize_key_path(path: &str) -> String {
    path.trim_matches('/').replace('\\', "/")
}

fn yaml_key(key: &str) -> String {
    if key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.' | ':' | '#'))
    {
        return key.to_string();
    }

    format!("\"{}\"", key.replace('\\', "\\\\").replace('"', "\\\""))
}

fn unquote_yaml_key(key: &str) -> Result<String> {
    if !(key.starts_with('"') && key.ends_with('"')) {
        return Ok(key.to_string());
    }

    let mut unquoted = String::new();
    let mut escaped = false;
    for character in key[1..key.len() - 1].chars() {
        if escaped {
            unquoted.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            unquoted.push(character);
        }
    }

    if escaped {
        return Err(I18nError::ConfigError(
            "quoted i18n.lock key ends with an unfinished escape".to_string(),
        ));
    }

    Ok(unquoted)
}

fn split_lingo_lock_key_checksum(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut escaped = false;
    let mut cursor = 0usize;

    while cursor + 1 < bytes.len() {
        let character = line[cursor..].chars().next()?;
        if escaped {
            escaped = false;
            cursor += character.len_utf8();
            continue;
        }

        match character {
            '\\' if in_quotes => escaped = true,
            '"' => in_quotes = !in_quotes,
            ':' if !in_quotes && bytes.get(cursor + 1) == Some(&b' ') => {
                return Some((&line[..cursor], &line[cursor + 2..]));
            }
            _ => {}
        }

        cursor += character.len_utf8();
    }

    None
}

fn validate_sha256_hex(value: &str, label: &str, line_number: usize) -> Result<()> {
    if matches!(value.len(), 32 | 64)
        && value.chars().all(|character| character.is_ascii_hexdigit())
    {
        return Ok(());
    }

    Err(I18nError::ConfigError(format!(
        "invalid {label} on line {line_number}: expected 32 or 64 hex characters"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_source_values_with_sha256() {
        assert_eq!(
            content_hash("Hello"),
            "185f8db32271fe25f561a6fc938b2e264306ec304eda518007d1764826381969"
        );
    }

    #[test]
    fn classifies_current_entries() {
        let fingerprint =
            SourceStringFingerprint::new("json", "en", "locales/en.json", "title", "Hello");
        let mut lockfile = Lockfile::new();
        lockfile.record(&fingerprint);

        assert_eq!(lockfile.classify(&fingerprint), DeltaKind::Current);
    }

    #[test]
    fn classifies_same_key_with_changed_content() {
        let original =
            SourceStringFingerprint::new("json", "en", "locales/en.json", "title", "Hello");
        let changed =
            SourceStringFingerprint::new("json", "en", "locales/en.json", "title", "Hello!");
        let mut lockfile = Lockfile::new();
        lockfile.record(&original);

        assert_eq!(
            lockfile.classify(&changed),
            DeltaKind::ChangedContent {
                previous_content_hash: original.content_hash
            }
        );
    }

    #[test]
    fn classifies_same_content_with_new_key_as_rename() {
        let original =
            SourceStringFingerprint::new("json", "en", "locales/en.json", "title", "Hello");
        let renamed =
            SourceStringFingerprint::new("json", "en", "locales/en.json", "hero/title", "Hello");
        let previous_label = original.lock_label();
        let mut lockfile = Lockfile::new();
        lockfile.record(&original);

        assert_eq!(
            lockfile.classify(&renamed),
            DeltaKind::RenamedKey { previous_label }
        );
    }

    #[test]
    fn renders_lingo_shaped_yaml_lockfile() {
        let fingerprint =
            SourceStringFingerprint::new("json", "en", "locales/en.json", "title", "Hello");
        let mut lockfile = Lockfile::new();
        lockfile.record(&fingerprint);

        let yaml = lockfile.to_lingo_yaml();

        assert!(yaml.starts_with("version: 1\nchecksums:\n"));
        assert!(yaml.contains(&fingerprint.content_hash));
        assert!(yaml.contains(&fingerprint.key_hash));
    }

    #[test]
    fn parses_lingo_yaml_and_keeps_last_duplicate_key_hash() {
        let content_hash = content_hash("Hello");
        let first_key_hash = sha256_hex("first");
        let second_key_hash = sha256_hex("second");
        let yaml = format!(
            "version: 1\nchecksums:\n  {content_hash}:\n    greeting: {first_key_hash}\n    greeting: {second_key_hash}\n"
        );

        let lockfile = Lockfile::from_lingo_yaml(&yaml).expect("lockfile should parse");

        assert_eq!(
            lockfile
                .checksums
                .get(&content_hash)
                .and_then(|entries| entries.get("greeting")),
            Some(&second_key_hash)
        );
    }

    #[test]
    fn lingo_yaml_roundtrips_reserved_and_colon_space_key_labels() {
        let mut lockfile = Lockfile::new();
        lockfile.record_lingo_key("version", "Version label");
        lockfile.record_lingo_key("title: subtitle", "Colon label");

        let parsed = Lockfile::from_lingo_yaml(&lockfile.to_lingo_yaml())
            .expect("rendered lockfile should parse");

        assert_eq!(parsed, lockfile);
    }

    #[test]
    fn rejects_malformed_lingo_yaml_hashes() {
        let error = Lockfile::from_lingo_yaml(
            "version: 1\nchecksums:\n  not-a-sha:\n    greeting: also-not-a-sha\n",
        )
        .expect_err("malformed hashes should fail");

        assert!(error.to_string().contains("invalid content checksum"));
    }

    #[test]
    fn parses_current_cli_md5_style_lingo_yaml_hashes() {
        let lockfile = Lockfile::from_lingo_yaml(
            "version: 1\nchecksums:\n  29ba5363b2b0f1cc53dd4a667d52f86e:\n    content/0: bd24fa10e2af7659360cf0a39f8865bd\n",
        )
        .expect("current CLI-style 32-char hashes should parse");

        assert_eq!(lockfile.checksums.len(), 1);
    }
}
