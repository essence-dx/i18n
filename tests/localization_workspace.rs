use async_trait::async_trait;
use dx_i18n::localization::{
    BucketPattern, I18nLock, LocalizationProvider, LocalizationResponse, LocalizationUsage,
    LocalizationWorkspace, TranslationUnit, WorkspaceFilters,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lingo_compatible")
}

fn read_fixture(path: impl AsRef<Path>) -> String {
    fs::read_to_string(fixture_root().join(path)).expect("fixture should be readable")
}

fn read_json_fixture(path: impl AsRef<Path>) -> Value {
    serde_json::from_str(&read_fixture(path)).expect("fixture JSON should parse")
}

#[test]
fn workspace_builds_lingo_lockfile_from_configured_source_buckets() {
    let workspace = LocalizationWorkspace::load(fixture_root()).expect("workspace should load");
    let expected = I18nLock::from_lingo_yaml(&read_fixture("i18n.lock"))
        .expect("fixture lockfile should parse");

    let generated = workspace
        .build_lockfile()
        .expect("lockfile should build from source buckets");

    assert_eq!(generated.checksums, expected.checksums);
}

#[test]
fn workspace_renders_local_json_outputs_without_cloud_auth() {
    let workspace = LocalizationWorkspace::load(fixture_root()).expect("workspace should load");

    let outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");

    assert!(!workspace.requires_cloud_auth_for_local_mode());
    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("locales/es/common.json")
    );
    assert_eq!(
        outputs[0].value,
        read_json_fixture("locales/es/common.json")
    );
}

#[test]
fn workspace_status_uses_lockfile_without_requiring_cloud_auth() {
    let workspace = LocalizationWorkspace::load(fixture_root()).expect("workspace should load");
    let lock = I18nLock::from_lingo_yaml(&read_fixture("i18n.lock"))
        .expect("fixture lockfile should parse");

    let status = workspace
        .status_against(&lock)
        .expect("status should compare source buckets to lockfile");

    assert!(!status.requires_cloud_auth);
    assert_eq!(status.source_file_count, 2);
    assert_eq!(status.total_units, 14);
    assert_eq!(status.pending_units, 0);
    assert_eq!(
        status.target_drift_files,
        vec![PathBuf::from("locales/es/common.json")]
    );
    assert_eq!(status.target_locales, vec!["es"]);
    assert!(status.unsupported_bucket_types.is_empty());
}

#[test]
fn workspace_status_reports_target_drift_without_marking_source_pending() {
    let root = unique_temp_project("dx_i18n_status_target_drift");
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
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open {url}"}"#)
        .expect("source JSON should be written");
    fs::write(root.join("locales/es.json"), r#"{"cta":"Abrir"}"#)
        .expect("unsafe target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lock = workspace.build_lockfile().expect("lockfile should build");
    let status = workspace
        .status_against(&lock)
        .expect("status should compare target outputs without writing");

    assert_eq!(status.pending_units, 0);
    assert_eq!(
        status.target_drift_files,
        vec![PathBuf::from("locales/es.json")]
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_status_reports_pending_keys_grouped_by_source_file() {
    let workspace = LocalizationWorkspace::load(fixture_root()).expect("workspace should load");
    let status = workspace
        .status_against_filtered_for_target_locales_with_force(
            &I18nLock::new(),
            &WorkspaceFilters::default(),
            &["es".to_string()],
            true,
        )
        .expect("forced status should report pending source-file detail");

    assert_eq!(status.pending_units, 10);
    assert_eq!(status.pending_files.len(), 2);
    assert_eq!(
        status.pending_files[0].relative_path,
        PathBuf::from("docs/en/product.md")
    );
    assert!(
        status.pending_files[0]
            .pending_keys
            .contains(&"docs/product.md#heading/1".to_string())
    );
    assert_eq!(
        status.pending_files[1].relative_path,
        PathBuf::from("locales/en/common.json")
    );
    assert!(
        status.pending_files[1]
            .pending_keys
            .contains(&"cta".to_string())
    );
    assert!(
        !status.pending_files[1]
            .pending_keys
            .contains(&"brand/name".to_string())
    );
    assert!(
        !status.pending_files[1]
            .pending_keys
            .contains(&"config/apiUrl".to_string())
    );
    assert!(
        !status.pending_files[1]
            .pending_keys
            .contains(&"releaseNotes/manualOverride".to_string())
    );
}

#[test]
fn workspace_status_reports_unsupported_buckets_without_blocking_local_json() {
    let root = unique_temp_project("dx_i18n_unsupported_bucket_status");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] },
            "yaml": { "include": ["translations/[locale].yaml"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Get started"}"#)
        .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let status = workspace
        .status_against(&I18nLock::new())
        .expect("status should report supported buckets and not require remote auth");
    let outputs = workspace
        .render_local_json("es")
        .expect("supported local JSON should still render");

    assert_eq!(status.source_file_count, 1);
    assert_eq!(status.total_units, 1);
    assert_eq!(status.unsupported_bucket_types, vec!["yaml"]);
    assert_eq!(outputs[0].value["cta"], "Get started");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_rejects_unconfigured_or_path_like_target_locales() {
    let workspace = LocalizationWorkspace::load(fixture_root()).expect("workspace should load");

    let unknown = workspace
        .render_local_json("fr")
        .expect_err("unconfigured target locales should fail");
    assert!(unknown.to_string().contains("not configured"));

    let path_like = workspace
        .render_local_json("../es")
        .expect_err("path-like target locales should fail");
    assert!(path_like.to_string().contains("invalid target locale"));
}

#[test]
fn workspace_load_rejects_configured_path_like_target_locales() {
    let root = unique_temp_project("dx_i18n_bad_configured_locale");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "version": "1.15",
          "locale": { "source": "en", "targets": ["../es"] },
          "buckets": { "json": { "include": ["locales/[locale].json"] } }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Open"}"#)
        .expect("source JSON should be written");

    let error = LocalizationWorkspace::load(&root)
        .expect_err("path-like configured target locale should fail at load");

    assert!(error.to_string().contains("locale"));
    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_rejects_include_patterns_that_escape_root() {
    let base = LocalizationWorkspace::load(fixture_root()).expect("workspace should load");
    let mut config = base.config().clone();
    config
        .buckets
        .get_mut("json")
        .expect("json bucket should exist")
        .include = vec![BucketPattern::Path("../outside/[locale].json".to_string())];
    let workspace = LocalizationWorkspace::new(fixture_root(), config);

    let error = workspace
        .build_lockfile()
        .expect_err("escaping include pattern should fail");

    assert!(error.to_string().contains("must stay inside workspace"));
}

#[test]
fn workspace_maps_wildcard_json_sources_to_target_locale_paths() {
    let root = unique_temp_project("dx_i18n_wildcard_json");
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
        r#"{"cta":"Get started"}"#,
    )
    .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");

    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("locales/es/common.json")
    );
    assert_eq!(outputs[0].value["cta"], "Get started");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_honors_exclude_patterns_when_rendering_local_json() {
    let root = unique_temp_project("dx_i18n_exclude_json");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale]/*.json"],
              "exclude": ["locales/[locale]/internal.json"]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en/common.json"),
        r#"{"cta":"Get started"}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/en/internal.json"),
        r#"{"secret":"Do not ship"}"#,
    )
    .expect("excluded JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");

    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("locales/es/common.json")
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_applies_bucket_pattern_locale_delimiter() {
    let root = unique_temp_project("dx_i18n_locale_delimiter");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en-US", "targets": ["es-MX"] },
          "buckets": {
            "json": {
              "include": [{ "path": "locales/[locale].json", "delimiter": "_" }]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en_US.json"), r#"{"cta":"Get started"}"#)
        .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_json("es-MX")
        .expect("local JSON render should work");

    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("locales/es_MX.json")
    );
    assert_eq!(outputs[0].value["cta"], "Get started");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_injects_target_locale_for_configured_json_keys() {
    let root = unique_temp_project("dx_i18n_inject_locale");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale].json"],
              "injectLocale": ["meta/locale"]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"meta":{"locale":"en","title":"Dashboard"}}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"meta":{"locale":"wrong","title":"Panel"}}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");
    let lock = workspace
        .build_lockfile()
        .expect("lockfile should build from source buckets");
    let status = workspace
        .status_against(&lock)
        .expect("status should compare source buckets to lockfile");

    assert_eq!(outputs[0].value["meta"]["locale"], "es");
    assert_eq!(outputs[0].value["meta"]["title"], "Panel");
    assert_eq!(status.total_units, 1);

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_load_keeps_local_commands_auth_free_with_incomplete_remote_provider() {
    let root = unique_temp_project("dx_i18n_local_provider_metadata");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] }
          },
          "provider": { "id": "future-provider", "model": "" }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Get started"}"#)
        .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lock = workspace
        .build_lockfile()
        .expect("lockfile should build without remote provider validation");
    let status = workspace
        .status_against(&lock)
        .expect("status should stay local-only");

    assert!(!workspace.requires_cloud_auth_for_local_mode());
    assert!(!status.requires_cloud_auth);

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_load_keeps_local_commands_auth_free_with_partial_remote_provider() {
    let root = unique_temp_project("dx_i18n_partial_provider_metadata");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale].json"] }
          },
          "provider": { "id": "future-provider" }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("locales/en.json"), r#"{"cta":"Get started"}"#)
        .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lock = workspace
        .build_lockfile()
        .expect("lockfile should build without remote provider validation");
    let status = workspace
        .status_against(&lock)
        .expect("status should stay local-only");

    assert!(!workspace.requires_cloud_auth_for_local_mode());
    assert!(!status.requires_cloud_auth);
    assert!(
        workspace
            .config()
            .validate()
            .iter()
            .any(|issue| issue.code == "provider.model.empty")
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_falls_back_to_source_when_target_json_loses_structural_tokens() {
    let root = unique_temp_project("dx_i18n_json_token_safety");
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
        r#"{"welcome":"Hello {name}","safe":"Run `dx build`"}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"welcome":"Hola","safe":"Ejecuta `dx build`"}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");

    assert_eq!(outputs[0].value["welcome"], "Hello {name}");
    assert_eq!(outputs[0].value["safe"], "Ejecuta `dx build`");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_does_not_compact_arrays_when_ignoring_json_array_elements() {
    let root = unique_temp_project("dx_i18n_json_array_ignore");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale].json"],
              "ignoredKeys": ["items/1"]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"items":["Open","Internal","Close"]}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"items":["Abrir","Privado","Cerrar"]}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");

    assert_eq!(
        outputs[0].value["items"],
        serde_json::json!(["Abrir", "Internal", "Cerrar"])
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_preserves_empty_json_objects_in_local_and_provider_outputs() {
    let root = unique_temp_project("dx_i18n_json_empty_objects");
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
        r#"{"meta":{},"nested":{"empty":{},"title":"Hello {name}"}}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"nested":{"title":"Hola {name}"}}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let local_outputs = workspace
        .render_local_json("es")
        .expect("local JSON render should work");
    let provider_outputs = workspace
        .render_provider_json(&MockProvider, "es")
        .await
        .expect("provider JSON render should work");

    assert_eq!(
        local_outputs[0].value,
        serde_json::json!({
            "meta": {},
            "nested": {
                "empty": {},
                "title": "Hola {name}"
            }
        })
    );
    assert_eq!(
        provider_outputs[0].value,
        serde_json::json!({
            "meta": {},
            "nested": {
                "empty": {},
                "title": "[es] Hello {name}"
            }
        })
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_renders_local_markdown_from_source_structure_and_target_text() {
    let root = unique_temp_project("dx_i18n_markdown_render");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "---\ntitle: \"DX launch notes\"\nstatus: \"stable\"\nslug: \"dx-launch\"\n---\n\n# Hello, {name}\n\nVisit [dashboard]({dashboard_url}) and keep `dx run --locale {locale}` unchanged.\n\n```tsx\n<Button label=\"{cta_label}\" />\n```\n\n> Preserve **Markdown** around {{productName}}.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "---\ntitle: \"Notas de lanzamiento de DX\"\nstatus: \"draft\"\nslug: \"wrong-slug\"\n---\n\n# Hola, {name}\n\nVisita [panel]({dashboard_url}) y conserva `dx run --locale {locale}` sin cambios.\n\n```tsx\n<Wrong label=\"changed\" />\n```\n\n> Conserva **Markdown** alrededor de {{productName}}.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("docs/es/product.md")
    );
    assert_eq!(
        outputs[0].contents,
        "---\ntitle: \"Notas de lanzamiento de DX\"\nstatus: \"stable\"\nslug: \"dx-launch\"\n---\n\n# Hola, {name}\n\nVisita [panel]({dashboard_url}) y conserva `dx run --locale {locale}` sin cambios.\n\n```tsx\n<Button label=\"{cta_label}\" />\n```\n\n> Conserva **Markdown** alrededor de {{productName}}.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_falls_back_to_source_markdown_unit_when_target_loses_tokens() {
    let root = unique_temp_project("dx_i18n_markdown_token_safety");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "# Hello, {name}\n\n> Preserve {{productName}}.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "# Hola\n\n> Conserva {{productName}}.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "# Hello, {name}\n\n> Conserva {{productName}}.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_ignores_markdown_target_text_when_unit_structure_drifts() {
    let root = unique_temp_project("dx_i18n_markdown_target_structure_drift");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "# Hello\n\nFirst paragraph.\n\nSecond paragraph for {name}.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "# Hola\n\nExtra target paragraph.\n\nPrimer parrafo.\n\nSegundo parrafo para {name}.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "# Hello\n\nFirst paragraph.\n\nSecond paragraph for {name}.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_ignores_markdown_target_text_when_unit_order_drifts() {
    let root = unique_temp_project("dx_i18n_markdown_target_order_drift");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "# Hello {name}\n\nRun {name}.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "Ejecuta {name}.\n\n# Hola {name}\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(outputs[0].contents, "# Hello {name}\n\nRun {name}.\n");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_ignores_ambiguous_same_kind_markdown_target_reorder() {
    let root = unique_temp_project("dx_i18n_markdown_same_kind_target_reorder");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "First plain paragraph.\n\nSecond plain paragraph.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "Segundo parrafo.\n\nPrimer parrafo.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "First plain paragraph.\n\nSecond plain paragraph.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_ignores_ambiguous_same_kind_markdown_target_prefix() {
    let root = unique_temp_project("dx_i18n_markdown_same_kind_target_prefix");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "First plain paragraph.\n\nSecond plain paragraph.\n",
    )
    .expect("source markdown should be written");
    fs::write(root.join("docs/es/product.md"), "Segundo parrafo.\n")
        .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "First plain paragraph.\n\nSecond plain paragraph.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_uses_repeated_markdown_targets_when_anchors_are_stable() {
    let root = unique_temp_project("dx_i18n_markdown_same_kind_stable_anchors");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "First {first} paragraph.\n\nSecond {second} paragraph.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "Primer {first} parrafo.\n\nSegundo {second} parrafo.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "Primer {first} parrafo.\n\nSegundo {second} parrafo.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_uses_repeated_markdown_targets_when_code_anchors_are_stable() {
    let root = unique_temp_project("dx_i18n_markdown_same_kind_code_anchors");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "Use ``alpha`` now.\n\nUse ``beta`` now.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "Usa ``alpha`` ahora.\n\nUsa ``beta`` ahora.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "Usa ``alpha`` ahora.\n\nUsa ``beta`` ahora.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_preserves_nested_blockquote_markers_in_local_markdown() {
    let root = unique_temp_project("dx_i18n_markdown_nested_blockquotes");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "> > Keep {name}\n> Plain {{product}}\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "> > Conserva {name}\n> Simple {{product}}\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(
        outputs[0].contents,
        "> > Conserva {name}\n> Simple {{product}}\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_renders_json_with_lingo_compatible_provider_without_using_local_target_text() {
    let root = unique_temp_project("dx_i18n_lingo_provider_json");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale].json"],
              "lockedKeys": ["brand/name"],
              "preservedKeys": ["manual"],
              "injectLocale": ["meta/locale"]
            }
          },
          "engineId": "eng_test"
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"brand":{"name":"DX"},"cta":"Get started","manual":"Editor approved","meta":{"locale":"en"}}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"cta":"Stale local","manual":"Manual local","meta":{"locale":"wrong"}}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_json(&MockProvider, "es")
        .await
        .expect("provider JSON render should work");

    assert_eq!(outputs[0].value["brand"]["name"], "DX");
    assert_eq!(outputs[0].value["cta"], "[es] Get started");
    assert_eq!(outputs[0].value["manual"], "Manual local");
    assert_eq!(outputs[0].value["meta"]["locale"], "es");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_json_preserves_response_metadata() {
    let root = unique_temp_project("dx_i18n_lingo_provider_json_metadata");
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
    fs::write(root.join("locales/en.json"), r#"{"cta":"Get started"}"#)
        .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_json(&MetadataProvider, "es")
        .await
        .expect("provider JSON render should work");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider metadata should be retained");

    assert_eq!(outputs[0].value["cta"], "[es] Get started");
    assert_eq!(response.provider.as_deref(), Some("mock-lingo"));
    assert_eq!(response.model.as_deref(), Some("mock/model"));
    assert_eq!(response.usage.as_ref().unwrap().input_tokens, Some(11));

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_renders_markdown_with_lingo_compatible_provider() {
    let root = unique_temp_project("dx_i18n_lingo_provider_markdown");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          },
          "engineId": "eng_test"
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "---\ntitle: \"Launch notes\"\nslug: \"launch\"\n---\n\n# Hello, {name}\n\nRun `dx build`.\n",
    )
    .expect("source markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_markdown(&MockProvider, "es")
        .await
        .expect("provider Markdown render should work");

    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("docs/es/product.md")
    );
    assert_eq!(
        outputs[0].contents,
        "---\ntitle: \"[es] Launch notes\"\nslug: \"launch\"\n---\n\n# [es] Hello, {name}\n\n[es] Run `dx build`.\n"
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_markdown_preserves_response_metadata() {
    let root = unique_temp_project("dx_i18n_lingo_provider_markdown_metadata");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("docs/en/product.md"), "# Hello, {name}\n")
        .expect("source markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_markdown(&MetadataProvider, "es")
        .await
        .expect("provider Markdown render should work");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider metadata should be retained");

    assert_eq!(outputs[0].contents, "# [es] Hello, {name}\n");
    assert_eq!(response.provider.as_deref(), Some("mock-lingo"));
    assert_eq!(response.model.as_deref(), Some("mock/model"));
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, Some(7));

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_markdown_falls_back_when_provider_loses_tokens() {
    let root = unique_temp_project("dx_i18n_lingo_provider_markdown_safety");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("docs/en/product.md"), "# Hello, {name}\n")
        .expect("source markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_markdown(&UnsafeMarkdownProvider, "es")
        .await
        .expect("provider Markdown render should work");

    assert_eq!(outputs[0].contents, "# Hello, {name}\n");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_markdown_rejects_multiline_structure_injection() {
    let root = unique_temp_project("dx_i18n_lingo_provider_markdown_multiline_injection");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "# Hello, {name}\n\nRun `dx build`.\n",
    )
    .expect("source markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_markdown(&MultilineMarkdownProvider, "es")
        .await
        .expect("provider Markdown render should work");

    assert_eq!(outputs[0].contents, "# Hello, {name}\n\nRun `dx build`.\n");
    assert!(!outputs[0].contents.contains("Injected"));

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_markdown_rejects_single_line_block_marker_injection() {
    let root = unique_temp_project("dx_i18n_lingo_provider_markdown_block_marker_injection");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("docs/en/product.md"), "Plain {name}.\n")
        .expect("source markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_markdown(&SingleLineBlockMarkerProvider, "es")
        .await
        .expect("provider Markdown render should work");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider response should be present");

    assert_eq!(outputs[0].contents, "Plain {name}.\n");
    assert!(response.translations.is_empty());
    assert!(!outputs[0].contents.contains("Injected"));

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_markdown_keys_keep_non_locale_segments_named_like_source_locale() {
    let root = unique_temp_project("dx_i18n_locale_segment_collision");
    fs::create_dir_all(root.join("docs/en/guides/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es/guides/en")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": { "markdown": { "include": ["docs/[locale]/guides/en/*.md"] } }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/guides/en/product.md"),
        "# Hello {name}\n",
    )
    .expect("source markdown should be written");
    fs::write(root.join("docs/es/guides/en/product.md"), "# Hola {name}\n")
        .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_local_markdown("es")
        .expect("local Markdown render should work");

    assert_eq!(outputs[0].contents, "# Hola {name}\n");
    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_json_skips_provider_when_no_translatable_units_remain() {
    let root = unique_temp_project("dx_i18n_lingo_provider_empty_units");
    fs::create_dir_all(root.join("locales")).expect("locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale].json"],
              "lockedKeys": ["brand/name"],
              "injectLocale": ["meta/locale"]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en.json"),
        r#"{"brand":{"name":"DX"},"meta":{"locale":"en"}}"#,
    )
    .expect("source JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let outputs = workspace
        .render_provider_json(&PanicOnUseProvider, "es")
        .await
        .expect("provider JSON render should work without calling provider");

    assert_eq!(outputs[0].value["brand"]["name"], "DX");
    assert_eq!(outputs[0].value["meta"]["locale"], "es");
    assert!(outputs[0].provider_response.is_none());

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_filters_local_json_by_bucket_file_and_dot_key_prefix() {
    let root = unique_temp_project("dx_i18n_workspace_local_filters");
    fs::create_dir_all(root.join("locales/en")).expect("source locale dir should be created");
    fs::create_dir_all(root.join("locales/es")).expect("target locale dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "json": { "include": ["locales/[locale]/*.json"] },
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("locales/en/common.json"),
        r#"{"auth":{"login":{"title":"Sign in {name}"}},"marketing":{"title":"Welcome {name}"}}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/en/admin.json"),
        r#"{"auth":{"login":{"title":"Admin sign in {name}"}}}"#,
    )
    .expect("other source JSON should be written");
    fs::write(
        root.join("locales/es/common.json"),
        r#"{"auth":{"login":{"title":"Iniciar sesion"}},"marketing":{"title":"Bienvenido"}}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let filters = WorkspaceFilters::try_new(
        vec!["json".to_string()],
        vec!["common.json".to_string()],
        vec!["auth.login".to_string()],
    )
    .expect("filters should parse");
    let outputs = workspace
        .render_local_json_filtered("es", &filters)
        .expect("filtered local JSON render should work");

    assert_eq!(outputs.len(), 1);
    assert_eq!(
        outputs[0].relative_path,
        PathBuf::from("locales/es/common.json")
    );
    assert_eq!(outputs[0].value["auth"]["login"]["title"], "Sign in {name}");
    assert_eq!(outputs[0].value["marketing"]["title"], "Bienvenido");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[test]
fn workspace_filters_markdown_by_full_canonical_key_with_file_extension() {
    let root = unique_temp_project("dx_i18n_workspace_markdown_full_key_filter");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "# Hello {name}\n\nBody text.\n",
    )
    .expect("source markdown should be written");
    fs::write(root.join("docs/es/product.md"), "# Hola {name}\n\nTexto.\n")
        .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let filters = WorkspaceFilters::try_new(
        vec!["markdown".to_string()],
        Vec::new(),
        vec!["docs/product.md#heading/1".to_string()],
    )
    .expect("filters should parse");
    let status = workspace
        .status_against_filtered_for_target_locales_with_force(
            &I18nLock::new(),
            &filters,
            &["es".to_string()],
            true,
        )
        .expect("forced status should honor full Markdown keys");

    assert_eq!(status.total_units, 1);
    assert_eq!(status.pending_units, 1);
    assert_eq!(
        status.pending_keys,
        vec!["docs/product.md#heading/1".to_string()]
    );

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_filters_provider_json_units_by_dot_key_prefix() {
    let root = unique_temp_project("dx_i18n_workspace_provider_filters");
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
        r#"{"auth":{"login":{"title":"Sign in {name}"},"logout":"Sign out"},"marketing":{"title":"Welcome"}}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"auth":{"login":{"title":"Inicio anterior {name}"},"logout":"Salir"},"marketing":{"title":"Bienvenido"}}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let filters = WorkspaceFilters::try_new(
        vec!["json".to_string()],
        Vec::new(),
        vec!["auth.login".to_string()],
    )
    .expect("filters should parse");
    let outputs = workspace
        .render_provider_json_filtered(&MockProvider, "es", &filters)
        .await
        .expect("filtered provider JSON render should work");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider should be called for selected key");

    assert_eq!(
        response.translations.keys().cloned().collect::<Vec<_>>(),
        vec!["auth/login/title".to_string()]
    );
    assert_eq!(
        outputs[0].value["auth"]["login"]["title"],
        "[es] Sign in {name}"
    );
    assert_eq!(outputs[0].value["auth"]["logout"], "Salir");
    assert_eq!(outputs[0].value["marketing"]["title"], "Bienvenido");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_markdown_key_controls_match_json_provider_eligibility() {
    let root = unique_temp_project("dx_i18n_workspace_markdown_key_controls");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": {
              "include": ["docs/[locale]/*.md"],
              "lockedKeys": ["docs/product.md#heading/1"],
              "preservedKeys": ["docs/product.md#paragraph/1"],
              "injectLocale": ["docs/product.md#frontmatter/title"],
              "ignoredKeys": ["docs/product.md#quote/1"]
            }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "---\ntitle: Release notes\n---\n# Hello {name}\n\nKeep [dashboard]({url}).\n\n> Internal {{productName}}.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "---\ntitle: Notas\n---\n# Hola {name}\n\nConserva [dashboard]({url}).\n\n> Interno {{productName}}.\n",
    )
    .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let status = workspace
        .status_against_filtered_for_target_locales_with_force(
            &I18nLock::new(),
            &WorkspaceFilters::default(),
            &["es".to_string()],
            true,
        )
        .expect("forced status should exclude provider-ineligible Markdown keys");
    let outputs = workspace
        .render_provider_markdown(&PanicOnUseProvider, "es")
        .await
        .expect("provider Markdown render should work without translatable keys");

    assert_eq!(status.pending_units, 0);
    assert!(status.pending_keys.is_empty());
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0].provider_response.is_none());
    assert!(outputs[0].contents.contains("title: es"));
    assert!(outputs[0].contents.contains("# Hello {name}"));
    assert!(outputs[0].contents.contains("Conserva [dashboard]({url})."));
    assert!(outputs[0].contents.contains("> Internal {{productName}}."));

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_json_delta_preserves_current_safe_target_units() {
    let root = unique_temp_project("dx_i18n_workspace_provider_json_delta");
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
        r#"{"welcome":"Hello {name}","cta":"Open {url}"}"#,
    )
    .expect("source JSON should be written");
    fs::write(
        root.join("locales/es.json"),
        r#"{"welcome":"Hola {name}","cta":"Abre {url}"}"#,
    )
    .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lockfile = workspace.build_lockfile().expect("lockfile should build");
    fs::write(
        root.join("locales/en.json"),
        r#"{"welcome":"Hello {name}","cta":"Launch {url}"}"#,
    )
    .expect("source JSON should be updated");
    let workspace = LocalizationWorkspace::load(&root).expect("workspace should reload");
    let outputs = workspace
        .render_provider_json_delta_filtered(
            &MockProvider,
            "es",
            &WorkspaceFilters::default(),
            &lockfile,
            false,
        )
        .await
        .expect("delta provider JSON render should work");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider should be called for changed unit only");

    assert_eq!(
        response.translations.keys().cloned().collect::<Vec<_>>(),
        vec!["cta".to_string()]
    );
    assert_eq!(outputs[0].value["welcome"], "Hola {name}");
    assert_eq!(outputs[0].value["cta"], "[es] Launch {url}");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_json_delta_resends_current_unit_when_target_is_unsafe() {
    let root = unique_temp_project("dx_i18n_workspace_provider_json_unsafe_target");
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
        r#"{"welcome":"Hello {name}"}"#,
    )
    .expect("source JSON should be written");
    fs::write(root.join("locales/es.json"), r#"{"welcome":"Hola"}"#)
        .expect("target JSON should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lockfile = workspace.build_lockfile().expect("lockfile should build");
    let outputs = workspace
        .render_provider_json_delta_filtered(
            &MockProvider,
            "es",
            &WorkspaceFilters::default(),
            &lockfile,
            false,
        )
        .await
        .expect("unsafe current target should be sent to provider");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider should repair unsafe target unit");

    assert_eq!(
        response.translations.keys().cloned().collect::<Vec<_>>(),
        vec!["welcome".to_string()]
    );
    assert_eq!(outputs[0].value["welcome"], "[es] Hello {name}");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_markdown_delta_resends_current_unit_when_target_is_unsafe() {
    let root = unique_temp_project("dx_i18n_workspace_provider_markdown_unsafe_target");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(
        root.join("docs/en/product.md"),
        "# Hello, {name}\n\nKeep `dx run` stable.\n",
    )
    .expect("source markdown should be written");
    fs::write(
        root.join("docs/es/product.md"),
        "# Hola\n\nMantener `dx run` estable.\n",
    )
    .expect("unsafe target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lockfile = workspace.build_lockfile().expect("lockfile should build");
    let outputs = workspace
        .render_provider_markdown_delta_filtered(
            &MockProvider,
            "es",
            &WorkspaceFilters::default(),
            &lockfile,
            false,
        )
        .await
        .expect("unsafe current target Markdown should be sent to provider");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider should repair unsafe Markdown unit");

    assert_eq!(
        response.translations.keys().cloned().collect::<Vec<_>>(),
        vec!["docs/product.md#heading/1".to_string()]
    );
    assert!(outputs[0].contents.contains("# [es] Hello, {name}"));
    assert!(outputs[0].contents.contains("Mantener `dx run` estable."));

    fs::remove_dir_all(root).expect("temp project should be removed");
}

#[tokio::test]
async fn workspace_provider_markdown_delta_resends_current_unit_when_target_has_block_marker_injection()
 {
    let root = unique_temp_project("dx_i18n_workspace_provider_markdown_block_marker_target");
    fs::create_dir_all(root.join("docs/en")).expect("source docs dir should be created");
    fs::create_dir_all(root.join("docs/es")).expect("target docs dir should be created");
    fs::write(
        root.join("i18n.json"),
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es"] },
          "buckets": {
            "markdown": { "include": ["docs/[locale]/*.md"] }
          }
        }"#,
    )
    .expect("config should be written");
    fs::write(root.join("docs/en/product.md"), "# Hello {name}\n")
        .expect("source markdown should be written");
    fs::write(root.join("docs/es/product.md"), "# > Hola {name}\n")
        .expect("target markdown should be written");

    let workspace = LocalizationWorkspace::load(&root).expect("workspace should load");
    let lockfile = workspace.build_lockfile().expect("lockfile should build");
    let outputs = workspace
        .render_provider_markdown_delta_filtered(
            &MockProvider,
            "es",
            &WorkspaceFilters::default(),
            &lockfile,
            false,
        )
        .await
        .expect("unsafe Markdown shape should be sent to provider");
    let response = outputs[0]
        .provider_response
        .as_ref()
        .expect("provider should repair unsafe Markdown shape");

    assert_eq!(
        response.translations.keys().cloned().collect::<Vec<_>>(),
        vec!["docs/product.md#heading/1".to_string()]
    );
    assert_eq!(outputs[0].contents, "# [es] Hello {name}\n");

    fs::remove_dir_all(root).expect("temp project should be removed");
}

struct MockProvider;

#[async_trait]
impl LocalizationProvider for MockProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> dx_i18n::Result<LocalizationResponse> {
        assert_eq!(source_locale, "en");
        let translations = units
            .iter()
            .map(|unit| {
                (
                    unit.key().to_string(),
                    format!("[{target_locale}] {}", unit.text()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(LocalizationResponse::local(
            source_locale,
            target_locale,
            translations,
        ))
    }
}

struct UnsafeMarkdownProvider;

#[async_trait]
impl LocalizationProvider for UnsafeMarkdownProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> dx_i18n::Result<LocalizationResponse> {
        assert_eq!(source_locale, "en");
        assert_eq!(target_locale, "es");
        let translations = units
            .iter()
            .map(|unit| (unit.key().to_string(), "Hola".to_string()))
            .collect::<BTreeMap<_, _>>();

        Ok(LocalizationResponse::local(
            source_locale,
            target_locale,
            translations,
        ))
    }
}

struct MultilineMarkdownProvider;

#[async_trait]
impl LocalizationProvider for MultilineMarkdownProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> dx_i18n::Result<LocalizationResponse> {
        assert_eq!(source_locale, "en");
        assert_eq!(target_locale, "es");
        let translations = units
            .iter()
            .map(|unit| {
                (
                    unit.key().to_string(),
                    format!("[es] {}\n\n## Injected", unit.text()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(LocalizationResponse::local(
            source_locale,
            target_locale,
            translations,
        ))
    }
}

struct SingleLineBlockMarkerProvider;

#[async_trait]
impl LocalizationProvider for SingleLineBlockMarkerProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> dx_i18n::Result<LocalizationResponse> {
        assert_eq!(source_locale, "en");
        assert_eq!(target_locale, "es");
        let translations = units
            .iter()
            .map(|unit| {
                (
                    unit.key().to_string(),
                    format!("## Injected {}", unit.text()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(LocalizationResponse::local(
            source_locale,
            target_locale,
            translations,
        ))
    }
}

struct MetadataProvider;

#[async_trait]
impl LocalizationProvider for MetadataProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> dx_i18n::Result<LocalizationResponse> {
        let translations = units
            .iter()
            .map(|unit| {
                (
                    unit.key().to_string(),
                    format!("[{target_locale}] {}", unit.text()),
                )
            })
            .collect::<BTreeMap<_, _>>();

        Ok(LocalizationResponse {
            source_locale: Some(source_locale.to_string()),
            target_locale: Some(target_locale.to_string()),
            translations,
            provider: Some("mock-lingo".to_string()),
            model: Some("mock/model".to_string()),
            usage: Some(LocalizationUsage {
                input_tokens: Some(11),
                output_tokens: Some(7),
                llm_cost: None,
                localization_cost: None,
                cost: Some(0.001),
            }),
        })
    }
}

struct PanicOnUseProvider;

#[async_trait]
impl LocalizationProvider for PanicOnUseProvider {
    fn requires_cloud_auth(&self) -> bool {
        true
    }

    async fn localize_response(
        &self,
        _source_locale: &str,
        _target_locale: &str,
        _units: &[TranslationUnit],
    ) -> dx_i18n::Result<LocalizationResponse> {
        panic!("provider should not be called when no translatable units remain");
    }
}

fn unique_temp_project(prefix: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be valid")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), suffix))
}
