use dx_i18n::localization::I18nLock;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

const LINGO_ENV_KEYS: &[&str] = &[
    "DX_I18N_LINGO_API_KEY",
    "LINGO_API_KEY",
    "LINGODOTDEV_API_KEY",
    "DX_I18N_LINGO_ENGINE_ID",
    "LINGO_ENGINE_ID",
    "LINGO_API_URL",
    "LINGO_API_BASE_URL",
];

#[test]
fn cli_status_is_auth_free_and_no_write() {
    let root = copy_fixture("dx_i18n_cli_status");
    let lock_path = root.join("i18n.lock");
    let before = fs::read(&lock_path).expect("lockfile should be readable");

    let output = dx_i18n(&root, ["status"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("source_files=2"));
    assert!(stdout.contains("pending_units=0"));
    assert!(stdout.contains("target_drift_files=1"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_ignores_poisoned_lingo_env_without_writes() {
    let root = copy_fixture("dx_i18n_cli_status_poisoned_lingo_env");
    let lock_path = root.join("i18n.lock");
    let before = fs::read(&lock_path).expect("lockfile should be readable");

    let output = dx_i18n_with_env(
        &root,
        ["status"],
        [
            ("LINGO_API_KEY", "poison-key"),
            ("LINGO_API_URL", "http://example.com"),
        ],
    );

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_reports_target_drift_without_writes() {
    let root = copy_fixture("dx_i18n_cli_status_target_drift");
    let lock_path = root.join("i18n.lock");
    let target_path = root.join("locales/es/common.json");
    let before_lock = fs::read(&lock_path).expect("lockfile should be readable");
    let before_target = br#"{"welcome":"Hola"}"#.to_vec();
    fs::write(&target_path, &before_target).expect("target JSON should be made stale");

    let output = dx_i18n(&root, ["status", "--verbose"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("pending_units=0"));
    assert!(stdout.contains("target_drift_files=1"));
    assert!(stdout.contains("target_drift_file=locales/es/common.json"));
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before_lock
    );
    assert_eq!(
        fs::read(&target_path).expect("target JSON should be readable"),
        before_target
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_bucket_filter_skips_unselected_unsupported_buckets() {
    let root = unique_temp_project("dx_i18n_cli_status_bucket_filter");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] },
            "yaml": { "include": ["messages/[locale].yaml"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let output = dx_i18n(&root, ["status", "--bucket", "json"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("source_files=1"));
    assert!(stdout.contains("pending_units=1"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(!stdout.contains("unsupported_bucket=yaml"));
    assert!(!root.join("locales/es.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_verbose_bucket_filter_skips_unselected_unsupported_buckets_without_writes() {
    let root = unique_temp_project("dx_i18n_cli_status_verbose_bucket_filter");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] },
            "yaml": { "include": ["messages/[locale].yaml"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let output = dx_i18n(&root, ["status", "--verbose", "--bucket", "json"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("verbose=true"));
    assert!(stdout.contains("source_files=1"));
    assert!(stdout.contains("target_locales=es"));
    assert!(stdout.contains("pending_units=1"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(stdout.contains("pending_file=locales/en.json"));
    assert!(stdout.contains("pending_file_key=locales/en.json::cta"));
    assert!(!stdout.contains("unsupported_bucket=yaml"));
    assert!(!root.join("locales/es.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_file_filter_reports_only_matching_source_files() {
    let root = unique_temp_project("dx_i18n_cli_status_file_filter");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"admin":"Settings"}"#,
    )
    .expect("admin source JSON should be written");

    let output = dx_i18n(&root, ["status", "--file", "common.json"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("source_files=1"));
    assert!(stdout.contains("pending_units=1"));
    assert!(stdout.contains("pending=cta"));
    assert!(!stdout.contains("pending=admin"));
    assert!(!root.join("locales/es/common.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_verbose_reports_pending_keys_by_source_file_without_auth_or_writes() {
    let root = unique_temp_project("dx_i18n_cli_status_verbose");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"admin":"Settings"}"#,
    )
    .expect("admin source JSON should be written");

    let output = dx_i18n(&root, ["status", "--verbose", "--file", "common.json"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("verbose=true"));
    assert!(stdout.contains("source_files=1"));
    assert!(stdout.contains("pending_units=1"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(stdout.contains("pending_file=locales/en/common.json"));
    assert!(stdout.contains("pending_file_key=locales/en/common.json::cta"));
    assert!(!stdout.contains("pending_file=locales/en/admin.json"));
    assert!(!stdout.contains("pending_file_key=locales/en/admin.json::admin"));
    assert!(!root.join("locales/es/common.json").exists());
    assert!(!root.join("locales/es/admin.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_file_filter_matches_target_locale_paths_like_run() {
    let root = unique_temp_project("dx_i18n_cli_status_target_file_filter");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");

    let output = dx_i18n(&root, ["status", "--file", "locales/es/common.json"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("source_files=1"));
    assert!(stdout.contains("pending=cta"));
    assert!(!root.join("locales/es/common.json").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_locale_filter_reports_only_requested_target_locales() {
    let root = unique_temp_project("dx_i18n_cli_status_locale_filter");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es", "fr"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");

    let output = dx_i18n(&root, ["status", "--locale", "fr"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("target_locales=fr"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(!stdout.contains("target_locales=es,fr"));
    assert!(stdout.contains("pending=cta"));
    assert!(!root.join("locales/fr/common.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_locale_filter_scopes_target_path_file_matching() {
    let root = unique_temp_project("dx_i18n_cli_status_locale_file_scope");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es", "fr"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");

    let output = dx_i18n(
        &root,
        [
            "status",
            "--locale",
            "fr",
            "--file",
            "locales/es/common.json",
        ],
    );

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("target_locales=fr"));
    assert!(stdout.contains("source_files=0"));
    assert!(stdout.contains("pending_units=0"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(!stdout.contains("pending=cta"));

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_verbose_locale_filter_scopes_target_path_file_matching() {
    let root = unique_temp_project("dx_i18n_cli_status_verbose_locale_file_scope");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es", "fr"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");

    let output = dx_i18n(
        &root,
        [
            "status",
            "--verbose",
            "--locale",
            "fr",
            "--file",
            "locales/es/common.json",
        ],
    );

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("verbose=true"));
    assert!(stdout.contains("target_locales=fr"));
    assert!(stdout.contains("source_files=0"));
    assert!(stdout.contains("pending_units=0"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(!stdout.contains("target_locales=es,fr"));
    assert!(!stdout.contains("pending=cta"));
    assert!(!stdout.contains("pending_file=locales/en/common.json"));
    assert!(!stdout.contains("pending_file_key=locales/en/common.json::cta"));
    assert!(!root.join("locales/fr/common.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_locale_filter_rejects_unconfigured_target_before_writes() {
    let root = unique_temp_project("dx_i18n_cli_status_locale_filter_invalid");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let output = dx_i18n(&root, ["status", "--locale", "fr"]);

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("target locale 'fr' is not configured"));
    assert!(!root.join("locales/fr.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_force_counts_current_units_as_pending_without_writes() {
    let root = copy_fixture("dx_i18n_cli_status_force");
    let lock_path = root.join("i18n.lock");
    let target_json_path = root.join("locales/es/common.json");
    let target_markdown_path = root.join("docs/es/product.md");
    let before = fs::read(&lock_path).expect("lockfile should be readable");
    let target_json_before = fs::read(&target_json_path).expect("target JSON should be readable");
    let target_markdown_before =
        fs::read(&target_markdown_path).expect("target Markdown should be readable");

    let output = dx_i18n(&root, ["status", "--force"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("total_units=14"));
    assert!(stdout.contains("pending_units=10"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(stdout.contains("pending=cta"));
    assert!(stdout.contains("pending=docs/product.md#frontmatter/title"));
    assert!(stdout.contains("pending=docs/product.md#heading/1"));
    assert!(stdout.contains("pending=docs/product.md#paragraph/1"));
    assert!(stdout.contains("pending=docs/product.md#quote/1"));
    assert!(!stdout.contains("pending=brand/name"));
    assert!(!stdout.contains("pending=config/apiUrl"));
    assert!(!stdout.contains("pending=config/build"));
    assert!(!stdout.contains("pending=releaseNotes/manualOverride"));
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before
    );
    assert_eq!(
        fs::read(&target_json_path).expect("target JSON should be readable"),
        target_json_before
    );
    assert_eq!(
        fs::read(&target_markdown_path).expect("target Markdown should be readable"),
        target_markdown_before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_verbose_force_reports_safe_pending_files_without_writes() {
    let root = copy_fixture("dx_i18n_cli_status_verbose_force");
    let lock_path = root.join("i18n.lock");
    let target_json_path = root.join("locales/es/common.json");
    let target_markdown_path = root.join("docs/es/product.md");
    let before = fs::read(&lock_path).expect("lockfile should be readable");
    let target_json_before = fs::read(&target_json_path).expect("target JSON should be readable");
    let target_markdown_before =
        fs::read(&target_markdown_path).expect("target Markdown should be readable");

    let output = dx_i18n(&root, ["status", "--verbose", "--force"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("verbose=true"));
    assert!(stdout.contains("total_units=14"));
    assert!(stdout.contains("pending_units=10"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(stdout.contains("pending_file=docs/en/product.md"));
    assert!(
        stdout.contains("pending_file_key=docs/en/product.md::docs/product.md#frontmatter/title")
    );
    assert!(stdout.contains("pending_file_key=docs/en/product.md::docs/product.md#heading/1"));
    assert!(stdout.contains("pending_file=locales/en/common.json"));
    assert!(stdout.contains("pending_file_key=locales/en/common.json::cta"));
    assert!(stdout.contains("pending_file_key=locales/en/common.json::welcome"));
    assert!(!stdout.contains("pending_file_key=locales/en/common.json::brand/name"));
    assert!(!stdout.contains("pending_file_key=locales/en/common.json::config/apiUrl"));
    assert!(
        !stdout.contains("pending_file_key=locales/en/common.json::releaseNotes/manualOverride")
    );
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before
    );
    assert_eq!(
        fs::read(&target_json_path).expect("target JSON should be readable"),
        target_json_before
    );
    assert_eq!(
        fs::read(&target_markdown_path).expect("target Markdown should be readable"),
        target_markdown_before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_status_locale_force_stays_scoped_auth_free_and_no_write() {
    let root = unique_temp_project("dx_i18n_cli_status_locale_force");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::create_dir_all(root.join("locales/es")).expect("first target dir should be created");
    fs::create_dir_all(root.join("locales/fr")).expect("second target dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es", "fr"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale]/*.json"],
              "lockedKeys": ["brand/name"]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en/common.json"),
        r#"{"brand":{"name":"DX"},"cta":"Open"}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/fr/common.json"),
        r#"{"brand":{"name":"DX"},"cta":"Ouvrir"}"#,
    )
    .expect("target JSON should be written");
    let target_before =
        fs::read(root.join("locales/fr/common.json")).expect("target JSON should be readable");

    let output = dx_i18n(&root, ["status", "--locale", "fr", "--force"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("target_locales=fr"));
    assert!(stdout.contains("total_units=2"));
    assert!(stdout.contains("pending_units=1"));
    assert!(stdout.contains("requires_cloud_auth=false"));
    assert!(stdout.contains("pending=cta"));
    assert!(!stdout.contains("pending=brand/name"));
    assert_eq!(
        fs::read(root.join("locales/fr/common.json")).expect("target JSON should be readable"),
        target_before
    );
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_lockfile_check_accepts_semantically_current_lock_without_rewrite() {
    let root = copy_fixture("dx_i18n_cli_lock_check");
    let lock_path = root.join("i18n.lock");
    let before = fs::read(&lock_path).expect("lockfile should be readable");

    let output = dx_i18n(&root, ["lockfile", "--check"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("i18n.lock is current"));
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_lockfile_check_ignores_poisoned_lingo_env_without_writes() {
    let root = copy_fixture("dx_i18n_cli_lockfile_poisoned_lingo_env");
    let lock_path = root.join("i18n.lock");
    let before = fs::read(&lock_path).expect("lockfile should be readable");

    let output = dx_i18n_with_env(
        &root,
        ["lockfile", "--check"],
        [
            ("LINGO_API_KEY", "poison-key"),
            ("LINGO_API_URL", "http://example.com"),
        ],
    );

    assert_success(&output);
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_lockfile_refuses_plain_overwrite_and_force_rewrites() {
    let root = copy_fixture("dx_i18n_cli_lock_force");
    fs::write(
        root.join("locales/en/common.json"),
        fs::read_to_string(root.join("locales/en/common.json"))
            .expect("source JSON should be readable")
            .replace("Open [dashboard]({url})", "Launch [dashboard]({url})"),
    )
    .expect("source JSON should be mutated");
    let stale = fs::read(root.join("i18n.lock")).expect("lockfile should be readable");

    let plain = dx_i18n(&root, ["lockfile"]);
    assert_failure(&plain);
    assert_eq!(
        fs::read(root.join("i18n.lock")).expect("lockfile should be readable"),
        stale
    );

    let force = dx_i18n(&root, ["lockfile", "--force"]);
    assert_success(&force);
    assert_ne!(
        fs::read(root.join("i18n.lock")).expect("lockfile should be readable"),
        stale
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_frozen_rejects_stale_targets_without_writes() {
    let root = copy_fixture("dx_i18n_cli_frozen");
    let target_json = root.join("locales/es/common.json");
    fs::write(&target_json, r#"{"cta":"stale"}"#).expect("target JSON should be mutated");
    let before = fs::read(&target_json).expect("target JSON should be readable");

    let output = dx_i18n(&root, ["run", "--target", "es", "--frozen"]);

    assert_failure(&output);
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_frozen_rejects_stale_source_lock_without_writes() {
    let root = copy_fixture("dx_i18n_cli_frozen_source_stale");
    let source_json = root.join("locales/en/common.json");
    let target_json = root.join("locales/es/common.json");
    let lock_path = root.join("i18n.lock");
    fs::write(
        &source_json,
        fs::read_to_string(&source_json)
            .expect("source JSON should be readable")
            .replace("Open [dashboard]({url})", "Launch [dashboard]({url})"),
    )
    .expect("source JSON should be mutated");
    let target_before = fs::read(&target_json).expect("target JSON should be readable");
    let lock_before = fs::read(&lock_path).expect("lockfile should be readable");

    let output = dx_i18n(&root, ["run", "--target", "es", "--frozen"]);

    assert_failure(&output);
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        target_before
    );
    assert_eq!(
        fs::read(&lock_path).expect("lockfile should be readable"),
        lock_before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_validates_all_requested_targets_before_first_write() {
    let root = copy_fixture("dx_i18n_cli_prevalidate_targets");
    let target_json = root.join("locales/es/common.json");
    fs::write(&target_json, r#"{"cta":"custom"}"#).expect("target JSON should be mutated");
    let before = fs::read(&target_json).expect("target JSON should be readable");

    let output = dx_i18n(&root, ["run", "--target", "es", "--target", "../bad"]);

    assert_failure(&output);
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_local_preserves_existing_localizations_without_cloud_auth() {
    let root = copy_fixture("dx_i18n_cli_local_run");

    let output = dx_i18n(&root, ["run", "--target", "es"]);

    assert_success(&output);
    let json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    let markdown = fs::read_to_string(root.join("docs/es/product.md"))
        .expect("target Markdown should be readable");
    assert!(json.contains("Abre [panel]({url})"));
    assert!(json.contains("Hola, {name}"));
    assert!(markdown.contains("Hola, {name}"));
    assert!(markdown.contains("{{productName}}"));

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_local_ignores_lingo_env_and_api_key_without_lingo_mode() {
    let root = copy_fixture("dx_i18n_cli_local_run_ignores_lingo_env");
    let server = LingoCliMockServer::start(0);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--api-key", "test-key"],
        [
            ("LINGO_API_KEY", "env-test-key"),
            ("LINGO_API_URL", server.base_url.as_str()),
        ],
    );

    assert_success(&output);
    assert!(server.join().is_empty());
    let json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    assert!(json.contains("Abre [panel]({url})"));
    assert!(json.contains("Hola, {name}"));

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_local_force_does_not_require_lingo_credentials() {
    let root = copy_fixture("dx_i18n_cli_local_force");

    let output = dx_i18n(&root, ["run", "--target", "es", "--force"]);

    assert_success(&output);
    let json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    assert!(json.contains("Abre [panel]({url})"));
    assert!(json.contains("Hola, {name}"));

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_local_updates_missing_lockfile_after_success() {
    let root = copy_fixture("dx_i18n_cli_run_writes_lock");
    let lock_path = root.join("i18n.lock");
    fs::remove_file(&lock_path).expect("lockfile should be removed");

    let output = dx_i18n(&root, ["run", "--target", "es"]);

    assert_success(&output);
    let lock = fs::read_to_string(&lock_path).expect("lockfile should be written");
    I18nLock::from_lingo_yaml(&lock).expect("written lockfile should parse");

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_missing_credentials_exits_before_writes() {
    let root = copy_fixture("dx_i18n_cli_lingo_auth");
    let target_json = root.join("locales/es/common.json");
    let before = fs::read(&target_json).expect("target JSON should be readable");

    let output = dx_i18n(&root, ["run", "--target", "es", "--lingo"]);

    assert_failure(&output);
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_blank_api_key_override_exits_before_network_or_writes() {
    let root = copy_fixture("dx_i18n_cli_lingo_auth");
    let target_json = root.join("locales/es/common.json");
    let before = fs::read(&target_json).expect("target JSON should be readable");
    let server = LingoCliMockServer::start(0);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "   "],
        [
            ("LINGO_API_KEY", "ambient-key"),
            ("LINGO_API_URL", server.base_url.as_str()),
        ],
    );

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("--api-key requires a non-empty value"));
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before
    );
    assert!(server.join().is_empty());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_api_key_override_posts_to_mock_endpoint() {
    let root = copy_fixture("dx_i18n_cli_lingo_success");
    let config_path = root.join("i18n.json");
    fs::write(
        &config_path,
        fs::read_to_string(&config_path)
            .expect("config should be readable")
            .replace(
                "\"provider\":",
                "\"engineId\": \"eng_mock\",\n  \"provider\":",
            ),
    )
    .expect("config should be updated");
    let server = LingoCliMockServer::start_translating(2);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--lingo",
            "--force",
            "--api-key",
            "test-key",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    let markdown = fs::read_to_string(root.join("docs/es/product.md"))
        .expect("target Markdown should be readable");
    let lock = fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be readable");
    assert!(json.contains("Abre [panel]({url})"));
    assert!(markdown.contains("# Hola, {name}"));
    I18nLock::from_lingo_yaml(&lock).expect("written lockfile should parse");

    let requests = server.join();
    assert_eq!(requests.len(), 2);
    let json_data = &requests[0].body["data"];
    assert!(json_data.get("welcome").is_some());
    assert!(json_data.get("cta").is_some());
    assert!(json_data.get("icu").is_some());
    assert!(json_data.get("html").is_some());
    assert!(json_data["support"].get("tagline").is_some());
    assert!(json_data["footer"].get("tagline").is_some());
    assert!(json_data.get("brand").is_none());
    assert!(json_data.get("config").is_none());
    assert!(json_data.get("releaseNotes").is_none());

    let markdown_data = &requests[1].body["data"]["docs"];
    assert!(
        markdown_data["product.md#frontmatter"]
            .get("title")
            .is_some()
    );
    assert!(markdown_data["product.md#heading"].get("1").is_some());
    assert!(markdown_data["product.md#paragraph"].get("1").is_some());
    assert!(markdown_data["product.md#quote"].get("1").is_some());

    for request in requests {
        assert!(request.headers.starts_with("POST /process/localize "));
        assert!(
            request
                .headers
                .lines()
                .any(|line| { line.eq_ignore_ascii_case("x-api-key: test-key") })
        );
        assert_eq!(request.body["engineId"], "eng_mock");
        assert_eq!(request.body["sourceLocale"], "en");
        assert_eq!(request.body["targetLocale"], "es");
        assert!(!request.body.to_string().contains("test-key"));
    }

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_mode_lingo_posts_to_mock_endpoint() {
    let root = copy_fixture("dx_i18n_cli_mode_lingo_success");
    let server = LingoCliMockServer::start(2);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--mode",
            "lingo",
            "--force",
            "--api-key",
            "test-key",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body["sourceLocale"], "en");
    assert_eq!(requests[0].body["targetLocale"], "es");
    assert_eq!(requests[1].body["sourceLocale"], "en");
    assert_eq!(requests[1].body["targetLocale"], "es");

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_current_lockfile_skips_provider_requests_and_preserves_targets() {
    let root = copy_fixture("dx_i18n_cli_lingo_current_skip");
    let target_json = root.join("locales/es/common.json");
    let target_markdown = root.join("docs/es/product.md");
    let lockfile = root.join("i18n.lock");
    let before_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&target_json).expect("target JSON should be readable"))
            .expect("target JSON should parse");
    let before_markdown = fs::read(&target_markdown).expect("target Markdown should be readable");
    let before_lockfile = I18nLock::from_lingo_yaml(
        &fs::read_to_string(&lockfile).expect("lockfile should be readable"),
    )
    .expect("lockfile should parse");
    let server = LingoCliMockServer::start(0);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "test-key"],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let after_json: serde_json::Value =
        serde_json::from_slice(&fs::read(&target_json).expect("target JSON should be readable"))
            .expect("target JSON should parse");
    assert_eq!(after_json, before_json);
    assert_eq!(
        fs::read(&target_markdown).expect("target Markdown should be readable"),
        before_markdown
    );
    assert_eq!(
        I18nLock::from_lingo_yaml(
            &fs::read_to_string(&lockfile).expect("lockfile should be readable")
        )
        .expect("lockfile should parse"),
        before_lockfile
    );
    assert!(server.join().is_empty());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_force_frozen_rejects_before_network_or_writes() {
    let root = copy_fixture("dx_i18n_cli_lingo_force_frozen");
    let target_json = root.join("locales/es/common.json");
    let before_json = fs::read(&target_json).expect("target JSON should be readable");
    let server = LingoCliMockServer::start(0);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--lingo",
            "--force",
            "--frozen",
            "--api-key",
            "test-key",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("cannot combine run --force with --frozen"));
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before_json
    );
    assert!(server.join().is_empty());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_frozen_target_drift_rejects_before_network_or_writes() {
    let root = copy_fixture("dx_i18n_cli_lingo_frozen_target_drift");
    let target_json = root.join("locales/es/common.json");
    let lockfile = root.join("i18n.lock");
    let before_lock = fs::read(&lockfile).expect("lockfile should be readable");
    let before_json = br#"{"welcome":"Hola"}"#.to_vec();
    fs::write(&target_json, &before_json).expect("target JSON should be made stale");
    let server = LingoCliMockServer::start(0);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--lingo",
            "--frozen",
            "--api-key",
            "test-key",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("target output drift"));
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before_json
    );
    assert_eq!(
        fs::read(&lockfile).expect("lockfile should be readable"),
        before_lock
    );
    assert!(server.join().is_empty());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_sends_only_changed_json_unit_by_default() {
    let root = copy_fixture("dx_i18n_cli_lingo_delta_json");
    fs::write(
        root.join("locales/en/common.json"),
        fs::read_to_string(root.join("locales/en/common.json"))
            .expect("source JSON should be readable")
            .replace("Open [dashboard]({url})", "Launch [dashboard]({url})"),
    )
    .expect("source JSON should be updated");
    let before_markdown =
        fs::read(root.join("docs/es/product.md")).expect("target Markdown should be readable");
    let server = LingoCliMockServer::start_translating(1);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "test-key"],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 1);
    let cta_payload = requests[0].body["data"]["cta"]
        .as_str()
        .expect("cta payload should be a string");
    assert!(cta_payload.starts_with("Launch [dashboard]("));
    assert!(cta_payload.contains("DX_I18N_PROTECTED"));
    assert!(requests[0].body["data"].get("welcome").is_none());
    assert!(requests[0].body["data"].get("icu").is_none());
    assert!(requests[0].body["data"].get("html").is_none());
    assert!(requests[0].body["data"].get("support").is_none());
    assert!(requests[0].body["data"].get("footer").is_none());

    let json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    assert!(json.contains("Lanza [panel]({url})"));
    assert!(json.contains("Hola, {name}"));
    assert_eq!(
        fs::read(root.join("docs/es/product.md")).expect("target Markdown should be readable"),
        before_markdown
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_skips_target_and_lockfile_write_when_zero_translations_are_accepted() {
    let root = copy_fixture("dx_i18n_cli_lingo_zero_accepted");
    fs::write(
        root.join("locales/en/common.json"),
        fs::read_to_string(root.join("locales/en/common.json"))
            .expect("source JSON should be readable")
            .replace("Open [dashboard]({url})", "Launch [dashboard]({url})"),
    )
    .expect("source JSON should be updated");
    let target_json = root.join("locales/es/common.json");
    let lockfile = root.join("i18n.lock");
    let before_json = fs::read(&target_json).expect("target JSON should be readable");
    let before_lock = fs::read(&lockfile).expect("lockfile should be readable");
    let server = LingoCliMockServer::start(1);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "test-key"],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before_json
    );
    assert_eq!(
        fs::read(&lockfile).expect("lockfile should be readable"),
        before_lock
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_key_filter_updates_lock_for_accepted_key_only() {
    let root = copy_fixture("dx_i18n_cli_lingo_key_filter_lock");
    let before_lock_contents =
        fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be readable");
    let before_lock =
        I18nLock::from_lingo_yaml(&before_lock_contents).expect("lockfile should parse");
    let old_cta_hash = lock_content_hash_for_key(&before_lock, "cta");
    let welcome_hash = lock_content_hash_for_key(&before_lock, "welcome");

    fs::write(
        root.join("locales/en/common.json"),
        fs::read_to_string(root.join("locales/en/common.json"))
            .expect("source JSON should be readable")
            .replace("Open [dashboard]({url})", "Launch [dashboard]({url})"),
    )
    .expect("source JSON should be updated");
    let server = LingoCliMockServer::start_translating(1);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--lingo",
            "--api-key",
            "test-key",
            "--key",
            "cta",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body["data"].get("cta").is_some());
    assert!(requests[0].body["data"].get("welcome").is_none());

    let target_json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    assert!(target_json.contains("Lanza [panel]({url})"));
    assert!(target_json.contains("Hola, {name}"));

    let after_lock = I18nLock::from_lingo_yaml(
        &fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be readable"),
    )
    .expect("lockfile should parse");
    assert_ne!(lock_content_hash_for_key(&after_lock, "cta"), old_cta_hash);
    assert!(
        !after_lock
            .checksums
            .get(&old_cta_hash)
            .is_some_and(|keys| keys.contains_key("cta"))
    );
    assert_eq!(
        lock_content_hash_for_key(&after_lock, "welcome"),
        welcome_hash
    );
    assert_eq!(lock_entry_count_for_key(&after_lock, "cta"), 1);

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_key_filter_preserves_multiple_accepted_files_with_same_key() {
    let root = unique_temp_project("dx_i18n_cli_lingo_key_filter_same_key_files");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en/common.json"),
        r#"{"cta":"Open dashboard"}"#,
    )
    .expect("common source should be written");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"cta":"Open admin"}"#,
    )
    .expect("admin source should be written");
    assert_success(&dx_i18n(&root, ["run", "--target", "es"]));

    fs::write(
        root.join("locales/en/common.json"),
        r#"{"cta":"Launch dashboard"}"#,
    )
    .expect("common source should be updated");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"cta":"Launch admin"}"#,
    )
    .expect("admin source should be updated");
    let server = LingoCliMockServer::start_translating(2);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--lingo",
            "--api-key",
            "test-key",
            "--key",
            "cta",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.body["data"].get("cta").is_some())
    );

    let lock = I18nLock::from_lingo_yaml(
        &fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be readable"),
    )
    .expect("lockfile should parse");
    assert_eq!(lock_entry_count_for_key(&lock, "cta"), 2);
    assert!(
        fs::read_to_string(root.join("locales/es/common.json"))
            .expect("common target should be readable")
            .contains("Lanza dashboard")
    );
    assert!(
        fs::read_to_string(root.join("locales/es/admin.json"))
            .expect("admin target should be readable")
            .contains("Lanza admin")
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn cli_run_lingo_key_filter_records_only_safe_same_key_files() {
    let root = unique_temp_project("dx_i18n_cli_lingo_key_filter_same_key_partial_accept");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en/common.json"),
        r#"{"cta":"Open dashboard {url}"}"#,
    )
    .expect("common source should be written");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"cta":"Open admin [settings](/admin)"}"#,
    )
    .expect("admin source should be written");
    assert_success(&dx_i18n(&root, ["run", "--target", "es"]));

    fs::write(
        root.join("locales/en/common.json"),
        r#"{"cta":"Launch dashboard {url}"}"#,
    )
    .expect("common source should be updated");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"cta":"Launch admin [settings](/admin)"}"#,
    )
    .expect("admin source should be updated");
    let server = LingoCliMockServer::start_translating_with_one_unsafe_cta(2);

    let output = dx_i18n_with_env(
        &root,
        [
            "run",
            "--target",
            "es",
            "--lingo",
            "--api-key",
            "test-key",
            "--key",
            "cta",
        ],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.body["data"].get("cta").is_some())
    );

    let common_target = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("common target should be readable");
    let admin_target = fs::read_to_string(root.join("locales/es/admin.json"))
        .expect("admin target should be readable");
    assert!(common_target.contains("Lanza dashboard {url}"));
    assert!(admin_target.contains("Open admin [settings](/admin)"));
    assert!(!admin_target.contains("Launch admin"));

    let lock = I18nLock::from_lingo_yaml(
        &fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be readable"),
    )
    .expect("lockfile should parse");
    assert_eq!(lock_entry_count_for_key(&lock, "cta"), 1);

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn cli_run_lingo_sends_only_changed_markdown_unit_by_default() {
    let root = copy_fixture("dx_i18n_cli_lingo_delta_markdown");
    fs::write(
        root.join("docs/en/product.md"),
        fs::read_to_string(root.join("docs/en/product.md"))
            .expect("source Markdown should be readable")
            .replace("# Hello, {name}", "# Welcome, {name}"),
    )
    .expect("source Markdown should be updated");
    let before_json =
        fs::read(root.join("locales/es/common.json")).expect("target JSON should be readable");
    let server = LingoCliMockServer::start_translating(1);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "test-key"],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 1);
    let markdown_payload = &requests[0].body["data"]["docs"]["product.md#heading"];
    let heading_payload = markdown_payload["1"]
        .as_str()
        .expect("heading payload should be a string");
    assert!(heading_payload.starts_with("Welcome, "));
    assert!(heading_payload.contains("DX_I18N_PROTECTED"));
    assert!(
        requests[0].body["data"]["docs"]["product.md#frontmatter"]
            .as_object()
            .is_none()
    );
    assert!(
        requests[0].body["data"]["docs"]["product.md#paragraph"]
            .as_object()
            .is_none()
    );
    assert!(
        requests[0].body["data"]["docs"]["product.md#quote"]
            .as_object()
            .is_none()
    );

    let markdown = fs::read_to_string(root.join("docs/es/product.md"))
        .expect("target Markdown should be readable");
    assert!(markdown.contains("# Bienvenido, {name}"));
    assert!(markdown.contains("Visita [panel]({dashboard_url})"));
    assert_eq!(
        fs::read(root.join("locales/es/common.json")).expect("target JSON should be readable"),
        before_json
    );

    let lock = fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be readable");
    I18nLock::from_lingo_yaml(&lock).expect("updated lockfile should parse");

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_sends_only_added_markdown_unit_when_target_lacks_new_key() {
    let root = copy_fixture("dx_i18n_cli_lingo_delta_added_markdown");
    fs::write(
        root.join("docs/en/product.md"),
        format!(
            "{}\n\nNew rollout note for {{name}}.\n",
            fs::read_to_string(root.join("docs/en/product.md"))
                .expect("source Markdown should be readable")
                .trim_end()
        ),
    )
    .expect("source Markdown should be updated");
    let before_json =
        fs::read(root.join("locales/es/common.json")).expect("target JSON should be readable");
    let server = LingoCliMockServer::start_translating(1);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "test-key"],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 1);
    let markdown_docs = &requests[0].body["data"]["docs"];
    assert!(markdown_docs.get("product.md#frontmatter").is_none());
    assert!(markdown_docs.get("product.md#heading").is_none());
    assert!(markdown_docs["product.md#paragraph"].get("1").is_none());
    assert!(markdown_docs.get("product.md#quote").is_none());
    assert!(
        markdown_docs["product.md#paragraph"]["2"]
            .as_str()
            .expect("new paragraph payload should be a string")
            .contains("DX_I18N_PROTECTED")
    );

    let markdown = fs::read_to_string(root.join("docs/es/product.md"))
        .expect("target Markdown should be readable");
    assert!(markdown.contains("# Hola, {name}"));
    assert!(markdown.contains("Visita [panel]({dashboard_url})"));
    assert!(markdown.contains("Nueva nota de despliegue para {name}."));
    assert_eq!(
        fs::read(root.join("locales/es/common.json")).expect("target JSON should be readable"),
        before_json
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_lingo_repairs_current_unsafe_json_and_markdown_targets() {
    let root = copy_fixture("dx_i18n_cli_lingo_repairs_unsafe_current_targets");
    fs::write(
        root.join("locales/es/common.json"),
        fs::read_to_string(root.join("locales/es/common.json"))
            .expect("target JSON should be readable")
            .replace("Hola, {name}. Tienes {{count}} tareas nuevas.", "Hola."),
    )
    .expect("target JSON should be made unsafe");
    fs::write(
        root.join("docs/es/product.md"),
        fs::read_to_string(root.join("docs/es/product.md"))
            .expect("target Markdown should be readable")
            .replace("# Hola, {name}", "# Hola"),
    )
    .expect("target Markdown should be made unsafe");
    let server = LingoCliMockServer::start_translating(2);

    let output = dx_i18n_with_env(
        &root,
        ["run", "--target", "es", "--lingo", "--api-key", "test-key"],
        [("LINGO_API_URL", server.base_url.as_str())],
    );

    assert_success(&output);
    let requests = server.join();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body["data"]["welcome"]
            .as_str()
            .expect("welcome payload should be a string")
            .matches("DX_I18N_PROTECTED")
            .count(),
        2
    );
    assert!(
        requests[1].body["data"]["docs"]["product.md#heading"]["1"]
            .as_str()
            .expect("heading payload should be a string")
            .contains("DX_I18N_PROTECTED")
    );

    let json = fs::read_to_string(root.join("locales/es/common.json"))
        .expect("target JSON should be readable");
    let markdown = fs::read_to_string(root.join("docs/es/product.md"))
        .expect("target Markdown should be readable");
    assert!(json.contains("Hola, {name}. Tienes {{count}} tareas nuevas."));
    assert!(markdown.contains("# Hola, {name}"));
    assert!(markdown.contains("Visita [panel]({dashboard_url})"));

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_raw_provider_reports_boundary_before_writes() {
    let root = copy_fixture("dx_i18n_cli_raw_provider_boundary");
    let target_json = root.join("locales/es/common.json");
    let before = fs::read(&target_json).expect("target JSON should be readable");

    let output = dx_i18n(&root, ["run", "--target", "es", "--provider", "openai"]);

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("direct raw provider execution"));
    assert_eq!(
        fs::read(&target_json).expect("target JSON should be readable"),
        before
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_blocks_unsupported_buckets_before_writes() {
    let root = unique_temp_project("dx_i18n_cli_unsupported_bucket");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] },
            "yaml": { "include": ["messages/[locale].yaml"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let output = dx_i18n(&root, ["run", "--target", "es"]);

    assert_failure(&output);
    assert!(!root.join("locales/es.json").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_bucket_filter_skips_unselected_unsupported_buckets() {
    let root = unique_temp_project("dx_i18n_cli_bucket_filter");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] },
            "yaml": { "include": ["messages/[locale].yaml"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let output = dx_i18n(&root, ["run", "--target", "es", "--bucket", "json"]);

    assert_success(&output);
    assert!(root.join("locales/es.json").exists());
    assert!(!root.join("messages/es.yaml").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_file_filter_writes_only_matching_files() {
    let root = unique_temp_project("dx_i18n_cli_file_filter");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");
    fs::write(root.join("locales/en/admin.json"), r#"{"cta":"Admin"}"#)
        .expect("admin source JSON should be written");

    let output = dx_i18n(&root, ["run", "--target", "es", "--file", "common.json"]);

    assert_success(&output);
    assert!(root.join("locales/es/common.json").exists());
    assert!(!root.join("locales/es/admin.json").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_file_filter_is_scoped_to_requested_target_locale_before_writes() {
    let root = unique_temp_project("dx_i18n_cli_file_filter_target_scope");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::create_dir_all(root.join("locales/es")).expect("es locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es", "fr"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");
    fs::write(root.join("locales/es/common.json"), r#"{"cta":"Abrir"}"#)
        .expect("es JSON should be written");

    let output = dx_i18n(
        &root,
        ["run", "--target", "fr", "--file", "locales/es/common.json"],
    );

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("localization filters selected no source files"));
    assert!(!root.join("locales/fr/common.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_key_filter_preserves_unselected_target_strings() {
    let root = unique_temp_project("dx_i18n_cli_key_filter");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"auth":{"login":{"title":"Sign in {name}"}},"marketing":{"title":"Welcome {name}"}}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"auth":{"login":{"title":"Inicio anterior"}},"marketing":{"title":"Bienvenido"}}"#,
    )
    .expect("target JSON should be written");

    let output = dx_i18n(&root, ["run", "--target", "es", "--key", "auth.login"]);

    assert_success(&output);
    let rendered: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(root.join("locales/es.json")).expect("target JSON should be readable"),
    )
    .expect("target JSON should parse");
    assert_eq!(rendered["auth"]["login"]["title"], "Sign in {name}");
    assert_eq!(rendered["marketing"]["title"], "Bienvenido");
    I18nLock::from_lingo_yaml(
        &fs::read_to_string(root.join("i18n.lock")).expect("lockfile should be written"),
    )
    .expect("filtered run lockfile should parse");

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_key_filter_rejects_no_matching_units_before_writes() {
    let root = unique_temp_project("dx_i18n_cli_key_filter_no_match");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"auth":{"login":"Sign in"}}"#,
    )
    .expect("source JSON should be written");

    let output = dx_i18n(&root, ["run", "--target", "es", "--key", "auth.typo"]);

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("selected no localization units"));
    assert!(!root.join("locales/es.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_run_file_filter_rejects_no_matching_source_files_before_writes() {
    let root = unique_temp_project("dx_i18n_cli_file_filter_no_match");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let output = dx_i18n(&root, ["run", "--target", "es", "--file", "missing.json"]);

    assert_failure(&output);
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");
    assert!(stderr.contains("selected no source files"));
    assert!(!root.join("locales/es/common.json").exists());
    assert!(!root.join("i18n.lock").exists());

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_filtered_run_replaces_stale_lockfile_entries_for_selected_keys() {
    let root = unique_temp_project("dx_i18n_cli_filtered_lock_replace");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"auth":{"login":"Old text"}}"#,
    )
    .expect("source JSON should be written");

    assert_success(&dx_i18n(
        &root,
        ["run", "--target", "es", "--key", "auth.login"],
    ));
    fs::write(
        root.join("locales/en.json"),
        r#"{"auth":{"login":"New text"}}"#,
    )
    .expect("source JSON should be updated");
    assert_success(&dx_i18n(
        &root,
        ["run", "--target", "es", "--key", "auth.login"],
    ));
    fs::write(
        root.join("locales/en.json"),
        r#"{"auth":{"login":"New text"},"reuse":"Old text"}"#,
    )
    .expect("source JSON should add a key reusing the old text");

    let output = dx_i18n(&root, ["status"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("pending=reuse"));

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

#[test]
fn cli_filtered_run_preserves_unselected_same_key_lockfile_entries() {
    let root = unique_temp_project("dx_i18n_cli_filtered_lock_same_key");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::create_dir_all(root.join("locales/es")).expect("target locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Open"}"#)
        .expect("common source JSON should be written");
    fs::write(root.join("locales/en/admin.json"), r#"{"cta":"Approve"}"#)
        .expect("admin source JSON should be written");
    assert_success(&dx_i18n(&root, ["run", "--target", "es"]));

    fs::write(root.join("locales/en/common.json"), r#"{"cta":"Launch"}"#)
        .expect("common source JSON should be updated");
    assert_success(&dx_i18n(
        &root,
        ["run", "--target", "es", "--file", "common.json"],
    ));

    let output = dx_i18n(&root, ["status", "--verbose"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("pending_units=0"), "{stdout}");
    assert!(
        !stdout.contains("pending_file=locales/en/admin.json"),
        "{stdout}"
    );

    fs::write(
        root.join("locales/en/common.json"),
        r#"{"cta":"Launch","reuse":"Open"}"#,
    )
    .expect("common source JSON should reuse stale selected text");

    let output = dx_i18n(&root, ["status", "--verbose"]);

    assert_success(&output);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    assert!(stdout.contains("pending=reuse"), "{stdout}");
    assert!(
        !stdout.contains("pending_file=locales/en/admin.json"),
        "{stdout}"
    );

    fs::remove_dir_all(root).expect("temp fixture should be removed");
}

fn dx_i18n<const N: usize>(root: &Path, args: [&str; N]) -> Output {
    dx_i18n_with_env(root, args, [])
}

fn dx_i18n_with_env<const N: usize, const M: usize>(
    root: &Path,
    args: [&str; N],
    envs: [(&str, &str); M],
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dx-i18n"));
    command.arg("--root").arg(root);
    for arg in args {
        command.arg(arg);
    }
    for key in LINGO_ENV_KEYS {
        command.env_remove(key);
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("dx-i18n command should run")
}

#[derive(Debug)]
struct CapturedCliLingoRequest {
    headers: String,
    body: serde_json::Value,
}

struct LingoCliMockServer {
    base_url: String,
    handle: thread::JoinHandle<Vec<CapturedCliLingoRequest>>,
}

#[derive(Clone, Copy)]
enum LingoCliMockMode {
    Echo,
    Translate,
    TranslateWithOneUnsafeCta,
}

impl LingoCliMockServer {
    fn start(expected_requests: usize) -> Self {
        Self::start_with_mode(expected_requests, LingoCliMockMode::Echo)
    }

    fn start_translating(expected_requests: usize) -> Self {
        Self::start_with_mode(expected_requests, LingoCliMockMode::Translate)
    }

    fn start_translating_with_one_unsafe_cta(expected_requests: usize) -> Self {
        Self::start_with_mode(
            expected_requests,
            LingoCliMockMode::TranslateWithOneUnsafeCta,
        )
    }

    fn start_with_mode(expected_requests: usize, mode: LingoCliMockMode) -> Self {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("mock Lingo server should bind to loopback");
        let base_url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("mock Lingo server should expose address")
        );
        listener
            .set_nonblocking(true)
            .expect("mock Lingo server should become nonblocking");
        let handle = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut requests = Vec::new();
            while requests.len() < expected_requests && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("mock Lingo stream should become blocking");
                        requests.push(capture_cli_lingo_request(&mut stream, mode));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("mock Lingo server should accept requests: {error}"),
                }
            }
            requests
        });

        Self { base_url, handle }
    }

    fn join(self) -> Vec<CapturedCliLingoRequest> {
        self.handle
            .join()
            .expect("mock Lingo server thread should finish")
    }
}

fn capture_cli_lingo_request(
    stream: &mut std::net::TcpStream,
    mode: LingoCliMockMode,
) -> CapturedCliLingoRequest {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .expect("mock Lingo server should read request");
        assert!(read > 0, "request should not close before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
            break index;
        }
    };

    let headers = String::from_utf8(bytes[..header_end].to_vec()).expect("headers should be UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .expect("request should include content-length");
    let body_start = header_end + b"\r\n\r\n".len();
    while bytes.len() < body_start + content_length {
        let read = stream
            .read(&mut chunk)
            .expect("mock Lingo server should read request body");
        assert!(read > 0, "request should not close before body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body: serde_json::Value =
        serde_json::from_slice(&bytes[body_start..body_start + content_length])
            .expect("request body should be JSON");
    respond_with_lingo_data(stream, &body, mode);

    CapturedCliLingoRequest { headers, body }
}

fn respond_with_lingo_data(
    stream: &mut std::net::TcpStream,
    request: &serde_json::Value,
    mode: LingoCliMockMode,
) {
    let data = match mode {
        LingoCliMockMode::Echo => request["data"].clone(),
        LingoCliMockMode::Translate => translate_lingo_mock_data(&request["data"]),
        LingoCliMockMode::TranslateWithOneUnsafeCta => {
            translate_lingo_mock_data_with_one_unsafe_cta(&request["data"])
        }
    };
    let response = serde_json::json!({
        "sourceLocale": request["sourceLocale"],
        "targetLocale": request["targetLocale"],
        "data": data,
        "model": "mock/model",
        "usage": { "inputTokens": 1, "outputTokens": 1, "cost": 0.0 }
    })
    .to_string();
    let response_headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.len()
    );
    stream
        .write_all(response_headers.as_bytes())
        .expect("mock Lingo server should write response headers");
    stream
        .write_all(response.as_bytes())
        .expect("mock Lingo server should write response body");
}

fn translate_lingo_mock_data(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(translate_lingo_mock_text(text))
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(translate_lingo_mock_data).collect())
        }
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| (key.clone(), translate_lingo_mock_data(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn translate_lingo_mock_data_with_one_unsafe_cta(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) if text.contains("Launch admin") => {
            serde_json::Value::String("Lanza admin [settings](/wrong)".to_string())
        }
        serde_json::Value::String(text) => {
            serde_json::Value::String(translate_lingo_mock_text(text))
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(translate_lingo_mock_data_with_one_unsafe_cta)
                .collect(),
        ),
        serde_json::Value::Object(object) => serde_json::Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        translate_lingo_mock_data_with_one_unsafe_cta(value),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn translate_lingo_mock_text(text: &str) -> String {
    text.replace("Launch ", "Lanza ")
        .replace("Open ", "Abre ")
        .replace("[dashboard]", "[panel]")
        .replace("Welcome, ", "Bienvenido, ")
        .replace("Hello, ", "Hola, ")
        .replace("You have ", "Tienes ")
        .replace("new tasks", "tareas nuevas")
        .replace("New rollout note for ", "Nueva nota de despliegue para ")
}

fn lock_content_hash_for_key(lockfile: &I18nLock, key: &str) -> String {
    lockfile
        .checksums
        .iter()
        .find_map(|(content_hash, keys)| keys.contains_key(key).then(|| content_hash.clone()))
        .unwrap_or_else(|| panic!("lockfile should contain key {key}"))
}

fn lock_entry_count_for_key(lockfile: &I18nLock, key: &str) -> usize {
    lockfile
        .checksums
        .values()
        .filter(|keys| keys.contains_key(key))
        .count()
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "expected success\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "expected failure\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn copy_fixture(prefix: &str) -> PathBuf {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lingo_compatible");
    let target = unique_temp_project(prefix);
    copy_dir(&source, &target);
    target
}

fn copy_dir(source: &Path, target: &Path) {
    fs::create_dir_all(target).expect("target dir should be created");
    for entry in fs::read_dir(source).expect("source dir should be readable") {
        let entry = entry.expect("source entry should be readable");
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        if entry
            .file_type()
            .expect("file type should be readable")
            .is_dir()
        {
            copy_dir(&source_path, &target_path);
        } else {
            fs::copy(&source_path, &target_path).expect("fixture file should be copied");
        }
    }
}

fn unique_temp_project(prefix: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), suffix))
}
