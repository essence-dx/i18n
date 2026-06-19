use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lingo_compatible")
}

fn read_fixture(path: impl AsRef<Path>) -> String {
    fs::read_to_string(fixture_root().join(path)).expect("fixture file should be readable")
}

fn read_json(path: impl AsRef<Path>) -> Value {
    serde_json::from_str(&read_fixture(path)).expect("fixture JSON should parse")
}

fn collect_fixture_files(
    root: &Path,
    json_files: &mut Vec<PathBuf>,
    text_files: &mut Vec<PathBuf>,
) {
    for entry in fs::read_dir(root).expect("fixture directory should be readable") {
        let entry = entry.expect("fixture entry should be readable");
        let path = entry.path();
        if entry
            .file_type()
            .expect("fixture file type should be readable")
            .is_dir()
        {
            collect_fixture_files(&path, json_files, text_files);
            continue;
        }

        text_files.push(path.clone());
        if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            json_files.push(path);
        }
    }
}

#[test]
fn lingo_fixture_tree_has_parseable_json_lockfile_and_no_conflict_markers() {
    let root = fixture_root();
    let mut json_files = Vec::new();
    let mut text_files = Vec::new();
    collect_fixture_files(&root, &mut json_files, &mut text_files);

    for path in &json_files {
        let contents = fs::read_to_string(path).expect("JSON fixture should be readable");
        serde_json::from_str::<Value>(&contents).expect("JSON fixture should parse");
    }

    let lockfile = dx_i18n::localization::I18nLock::from_lingo_yaml(&read_fixture("i18n.lock"))
        .expect("fixture lockfile should parse");
    assert_eq!(lockfile.version, 1);

    for path in &text_files {
        let contents = fs::read_to_string(path).expect("text fixture should be readable");
        assert!(
            !contents.contains("<<<<<<<")
                && !contents.contains("=======")
                && !contents.contains(">>>>>>>"),
            "fixture contains a conflict marker: {}",
            path.display()
        );
    }

    assert_eq!(json_files.len(), 3);
}

#[test]
fn lingo_config_is_local_first_and_bucket_compatible() {
    let config = read_json("i18n.json");

    assert_eq!(config["$schema"], "https://lingo.dev/schema/i18n.json");
    assert_eq!(config["version"], "1.15");
    assert_eq!(config["locale"]["source"], "en");
    assert_eq!(string_array(&config["locale"]["targets"]), vec!["es"]);
    assert_eq!(
        string_array(&config["buckets"]["json"]["include"]),
        vec!["locales/[locale]/common.json"]
    );
    assert_eq!(
        string_array(&config["buckets"]["markdown"]["include"]),
        vec!["docs/[locale]/*.md"]
    );
    assert_eq!(
        string_array(&config["buckets"]["json"]["lockedKeys"]),
        vec!["brand/name", "config/*"]
    );
    assert_eq!(
        string_array(&config["buckets"]["json"]["ignoredKeys"]),
        vec!["internal/*"]
    );
    assert_eq!(
        string_array(&config["buckets"]["json"]["preservedKeys"]),
        vec!["releaseNotes/manualOverride"]
    );

    assert!(
        config.get("engineId").is_none(),
        "fixture should not depend on a remote Lingo engine"
    );
    assert_eq!(config["provider"]["id"], "ollama");
    let base_url = config["provider"]["baseUrl"]
        .as_str()
        .expect("local provider should expose baseUrl");
    assert!(
        base_url.starts_with("http://127.0.0.1") || base_url.starts_with("http://localhost"),
        "local-first fixture provider must stay loopback-only"
    );
    assert_no_secret_fields(&config, "$");
}

#[test]
fn locale_fixtures_preserve_placeholders_and_locked_json_keys() {
    let source = read_json("locales/en/common.json");
    let target = read_json("locales/es/common.json");
    let source_strings = flatten_json_strings(&source);
    let target_strings = flatten_json_strings(&target);
    let expected_target_keys = source_strings
        .keys()
        .filter(|key| !key.starts_with("internal/"))
        .cloned()
        .collect::<BTreeSet<_>>();
    let actual_target_keys = target_strings.keys().cloned().collect::<BTreeSet<_>>();

    assert!(
        !target.get("internal").is_some(),
        "ignored internal keys should not be emitted to target locale files"
    );
    assert_eq!(
        expected_target_keys, actual_target_keys,
        "target locale JSON keys should match source keys minus ignored keys"
    );
    assert_eq!(
        value_at_path(&source, "brand/name"),
        value_at_path(&target, "brand/name"),
        "locked brand key should be copied without translation"
    );
    assert_eq!(
        value_at_path(&source, "config/apiUrl"),
        value_at_path(&target, "config/apiUrl"),
        "locked config URL should be copied without translation"
    );
    assert_eq!(
        value_at_path(&source, "config/build"),
        value_at_path(&target, "config/build"),
        "locked config build should be copied without translation"
    );
    assert_ne!(
        value_at_path(&source, "releaseNotes/manualOverride"),
        value_at_path(&target, "releaseNotes/manualOverride"),
        "preserved keys should allow target-authored text to survive source changes"
    );

    for (key, target_text) in &target_strings {
        let Some(source_text) = source_strings.get(key) else {
            continue;
        };

        assert_eq!(
            structural_tokens(source_text),
            structural_tokens(target_text),
            "structural tokens changed for key {key}"
        );
    }

    assert_icu_plural_shape(value_at_path(&source, "icu"));
    assert_icu_plural_shape(value_at_path(&target, "icu"));
}

#[test]
fn markdown_fixture_preserves_frontmatter_code_and_inline_tokens() {
    let source = read_fixture("docs/en/product.md");
    let target = read_fixture("docs/es/product.md");
    let (source_frontmatter, source_body) = split_frontmatter(&source);
    let (target_frontmatter, target_body) = split_frontmatter(&target);

    assert_eq!(
        source_frontmatter.keys().collect::<Vec<_>>(),
        target_frontmatter.keys().collect::<Vec<_>>(),
        "frontmatter key set should be stable across locales"
    );
    assert_eq!(source_frontmatter["status"], target_frontmatter["status"]);
    assert_eq!(source_frontmatter["slug"], target_frontmatter["slug"]);
    assert_ne!(
        source_frontmatter["title"], target_frontmatter["title"],
        "title is translatable while routing metadata stays locked"
    );
    assert_eq!(
        fenced_code_blocks(source_body),
        fenced_code_blocks(target_body),
        "code fences should be copied exactly"
    );
    assert_eq!(
        structural_tokens(source_body),
        structural_tokens(target_body),
        "markdown placeholders, HTML tags, and interpolation tokens should be stable"
    );
}

#[test]
fn lock_fixture_tracks_source_content_and_key_hashes() {
    let source_json = read_json("locales/en/common.json");
    let mut expected = flatten_json_strings(&source_json);
    expected.remove("internal/trace");
    expected.insert(
        "docs/product.md#frontmatter/title".to_string(),
        "DX launch notes".to_string(),
    );
    expected.insert(
        "docs/product.md#heading/1".to_string(),
        "Hello, {name}".to_string(),
    );
    expected.insert(
        "docs/product.md#paragraph/1".to_string(),
        "Visit [dashboard]({dashboard_url}) and keep `dx run --locale {locale}` unchanged."
            .to_string(),
    );
    expected.insert(
        "docs/product.md#quote/1".to_string(),
        "Preserve **Markdown** around {{productName}}.".to_string(),
    );

    let lock_contents = read_fixture("i18n.lock");
    let lockfile = parse_lockfile(&lock_contents);
    let production_lockfile = dx_i18n::localization::Lockfile::from_lingo_yaml(&lock_contents)
        .expect("production lockfile parser should accept fixture");
    assert_eq!(lockfile.version, 1);
    assert_eq!(production_lockfile.version, 1);
    assert_eq!(production_lockfile.checksums, lockfile.checksums);

    for (key, source_text) in expected {
        let content_hash = sha256_hex(&source_text);
        let key_hash = sha256_hex(&key);
        assert_eq!(
            lockfile
                .checksums
                .get(&content_hash)
                .and_then(|entries| entries.get(&key)),
            Some(&key_hash),
            "lockfile missing source/key hash pair for {key}"
        );
    }

    let duplicate_content = lockfile
        .checksums
        .get(&sha256_hex("Built local-first."))
        .expect("duplicate source content should share one content fingerprint");
    assert!(duplicate_content.contains_key("support/tagline"));
    assert!(duplicate_content.contains_key("footer/tagline"));
}

struct FixtureLockfile {
    version: u64,
    checksums: BTreeMap<String, BTreeMap<String, String>>,
}

fn parse_lockfile(contents: &str) -> FixtureLockfile {
    let mut version = None;
    let mut checksums: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut in_checksums = false;
    let mut current_hash: Option<String> = None;

    for raw_line in contents.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(raw_version) = trimmed.strip_prefix("version:") {
            version = Some(
                raw_version
                    .trim()
                    .parse()
                    .expect("lockfile version should be numeric"),
            );
            continue;
        }

        if trimmed == "checksums:" {
            in_checksums = true;
            continue;
        }

        if !in_checksums {
            panic!("unexpected lockfile line outside checksums: {line}");
        }

        if raw_line.starts_with("  ") && !raw_line.starts_with("    ") {
            let hash = trimmed.trim_end_matches(':').to_string();
            assert!(
                is_sha256_hex(&hash),
                "content checksum should be SHA-256 hex: {hash}"
            );
            checksums.entry(hash.clone()).or_default();
            current_hash = Some(hash);
            continue;
        }

        if raw_line.starts_with("    ") {
            let (key, hash) = trimmed
                .split_once(": ")
                .expect("checksum entries should be key: hash pairs");
            assert!(
                is_sha256_hex(hash),
                "key checksum should be SHA-256 hex: {hash}"
            );
            let parent = current_hash
                .as_ref()
                .expect("checksum entry should have a parent content hash");
            checksums
                .get_mut(parent)
                .expect("parent checksum should exist")
                .insert(key.to_string(), hash.to_string());
            continue;
        }

        panic!("unexpected lockfile indentation: {line}");
    }

    FixtureLockfile {
        version: version.expect("lockfile should declare a version"),
        checksums,
    }
}

fn flatten_json_strings(value: &Value) -> BTreeMap<String, String> {
    let mut strings = BTreeMap::new();
    flatten_json_strings_into(value, "", &mut strings);
    strings
}

fn flatten_json_strings_into(value: &Value, prefix: &str, strings: &mut BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            strings.insert(prefix.to_string(), text.to_string());
        }
        Value::Object(object) => {
            for (key, child) in object {
                let child_prefix = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}/{key}")
                };
                flatten_json_strings_into(child, &child_prefix, strings);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let child_prefix = if prefix.is_empty() {
                    index.to_string()
                } else {
                    format!("{prefix}/{index}")
                };
                flatten_json_strings_into(child, &child_prefix, strings);
            }
        }
        _ => {}
    }
}

fn string_array(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .expect("value should be an array")
        .iter()
        .map(|item| item.as_str().expect("array items should be strings"))
        .collect()
}

fn value_at_path<'a>(value: &'a Value, path: &str) -> &'a str {
    let mut current = value;
    for part in path.split('/') {
        current = current
            .get(part)
            .unwrap_or_else(|| panic!("missing JSON fixture path {path}"));
    }
    current
        .as_str()
        .unwrap_or_else(|| panic!("fixture path {path} should be a string"))
}

fn structural_tokens(text: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    collect_brace_tokens(text, &mut tokens);
    collect_html_tags(text, &mut tokens);
    tokens
}

fn collect_brace_tokens(text: &str, tokens: &mut BTreeSet<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '{' {
            index += 1;
            continue;
        }

        if chars.get(index + 1) == Some(&'{') {
            if let Some(end) = find_pair(&chars, index + 2, "}}") {
                let inner: String = chars[index + 2..end].iter().collect();
                if is_identifier(&inner) {
                    tokens.insert(format!("{{{{{inner}}}}}"));
                }
                index = end + 2;
                continue;
            }
        }

        if let Some(end) = chars[index + 1..].iter().position(|ch| *ch == '}') {
            let end = index + 1 + end;
            let inner: String = chars[index + 1..end].iter().collect();
            if is_identifier(&inner) {
                tokens.insert(format!("{{{inner}}}"));
            }
            index = end + 1;
            continue;
        }

        index += 1;
    }
}

fn collect_html_tags(text: &str, tokens: &mut BTreeSet<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;

    while index < chars.len() {
        if chars[index] != '<' {
            index += 1;
            continue;
        }

        if let Some(end) = chars[index + 1..].iter().position(|ch| *ch == '>') {
            let end = index + 1 + end;
            let token: String = chars[index..=end].iter().collect();
            if token
                .chars()
                .nth(1)
                .is_some_and(|ch| ch == '/' || ch.is_ascii_alphabetic())
            {
                tokens.insert(token);
            }
            index = end + 1;
            continue;
        }

        index += 1;
    }
}

fn find_pair(chars: &[char], start: usize, pair: &str) -> Option<usize> {
    let pair: Vec<char> = pair.chars().collect();
    chars[start..]
        .windows(pair.len())
        .position(|window| window == pair.as_slice())
        .map(|offset| start + offset)
}

fn is_identifier(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn assert_icu_plural_shape(text: &str) {
    assert!(
        text.starts_with("{count, plural,"),
        "ICU plural should keep count selector"
    );
    assert!(
        text.contains("one {# "),
        "ICU plural should keep one branch marker"
    );
    assert!(
        text.contains("other {# "),
        "ICU plural should keep other branch marker"
    );
    assert!(
        text.ends_with('}'),
        "ICU plural should remain a braced message"
    );
}

fn split_frontmatter(markdown: &str) -> (BTreeMap<String, String>, &str) {
    let rest = markdown
        .strip_prefix("---\n")
        .expect("markdown fixture should start with frontmatter");
    let (frontmatter_text, body) = rest
        .split_once("\n---\n")
        .expect("markdown fixture should close frontmatter");

    let mut frontmatter = BTreeMap::new();

    for line in frontmatter_text.lines() {
        let (key, value) = line
            .split_once(": ")
            .expect("frontmatter lines should be key/value pairs");
        frontmatter.insert(key.to_string(), value.trim_matches('"').to_string());
    }

    (frontmatter, body)
}

fn fenced_code_blocks(markdown: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current = Vec::new();
    let mut in_fence = false;

    for line in markdown.lines() {
        if line.starts_with("```") {
            if in_fence {
                current.push(line.to_string());
                blocks.push(current.join("\n"));
                current.clear();
                in_fence = false;
            } else {
                current.push(line.to_string());
                in_fence = true;
            }
            continue;
        }

        if in_fence {
            current.push(line.to_string());
        }
    }

    blocks
}

fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn assert_no_secret_fields(value: &Value, path: &str) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let lower = key.to_ascii_lowercase();
                assert!(
                    !lower.contains("apikey")
                        && !lower.contains("api_key")
                        && !lower.contains("secret")
                        && !lower.contains("token"),
                    "fixture config should not carry secret-like field {path}.{key}"
                );
                assert_no_secret_fields(child, &format!("{path}.{key}"));
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_no_secret_fields(item, &format!("{path}[{index}]"));
            }
        }
        Value::String(text) => {
            assert!(
                !text.contains("sk-") && !text.contains("Bearer "),
                "fixture config should not carry secret-like value at {path}"
            );
        }
        _ => {}
    }
}
