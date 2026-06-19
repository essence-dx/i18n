use dx_i18n::localization::{
    DX_LINGO_API_KEY_ENV, DX_LINGO_ENGINE_ID_ENV, DxI18nConfig, I18nLock, JsonLocalizationDocument,
    LINGO_API_BASE_URL_ENV, LINGO_API_KEY_ENV, LINGO_API_URL_ENV, LINGO_ENGINE_ID_ENV,
    LINGODOTDEV_API_KEY_ENV, LingoApiConfig, LingoApiProvider, LingoHttpTimeouts, LocalCatalog,
    LocalFirstLocalizer, LocalizationResponse, ProtectedText, TranslationUnit,
};
use serde_json::json;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn test_lingo_config(
    api_key: &str,
    engine_id: Option<&str>,
    base_url: impl Into<String>,
) -> LingoApiConfig {
    LingoApiConfig::new(api_key, engine_id.map(str::to_string), base_url)
        .expect("test Lingo config should be valid")
}

#[test]
fn parses_lingo_style_config_without_requiring_cloud_auth() {
    let config = DxI18nConfig::from_json_str(
        r#"{
          "$schema": "https://lingo.dev/schema/i18n.json",
          "version": "1.15",
          "locale": { "source": "en", "targets": ["es", "fr"] },
          "buckets": {
            "json": {
              "include": ["locales/[locale].json"],
              "lockedKeys": ["brand/name"],
              "ignoredKeys": ["debug/*"]
            }
          },
          "engineId": "eng_test"
        }"#,
    )
    .expect("config should parse");

    assert_eq!(config.source_locale(), "en");
    assert_eq!(config.target_locales(), ["es", "fr"]);
    assert_eq!(
        config.bucket("json").unwrap().include,
        ["locales/[locale].json"]
    );
    assert_eq!(config.bucket("json").unwrap().locked_keys, ["brand/name"]);
    assert!(!config.requires_cloud_auth_for_local_mode());
}

#[test]
fn lockfile_tracks_source_hashes_and_detects_deltas() {
    let units = vec![
        TranslationUnit::new("nav/title", "Dashboard"),
        TranslationUnit::new("nav/subtitle", "Welcome {name}"),
    ];
    let lock = I18nLock::from_source_units(&units);

    assert!(!lock.needs_translation(&TranslationUnit::new("nav/title", "Dashboard")));
    assert!(lock.needs_translation(&TranslationUnit::new("nav/title", "Dashboards")));
    assert!(
        lock.has_same_content_with_different_key(
            &TranslationUnit::new("nav/heading", "Dashboard",)
        )
    );
}

#[test]
fn protected_text_restores_markdown_code_and_variables() {
    let original = "Hello {name}, run `dx build`, then:\n```rust\nfn main() {}\n```\n{count, plural, one {# file} other {# files}}";
    let protected = ProtectedText::protect(original).expect("text should be protected");

    assert!(!protected.translatable_text().contains("dx build"));
    assert!(!protected.translatable_text().contains("fn main"));
    assert!(!protected.translatable_text().contains("{count, plural"));

    let translated = protected
        .translatable_text()
        .replace("Hello", "Hola")
        .replace("run", "ejecuta")
        .replace("then", "luego");
    let restored = protected
        .restore(&translated)
        .expect("tokens should restore");

    assert!(restored.contains("Hola {name}"));
    assert!(restored.contains("`dx build`"));
    assert!(restored.contains("```rust\nfn main() {}\n```"));
    assert!(restored.contains("{count, plural, one {# file} other {# files}}"));
}

#[tokio::test]
async fn local_first_localizer_uses_existing_translations_without_auth() {
    let mut catalog = LocalCatalog::new("en");
    catalog.insert("es", "nav/title", "Panel");

    let localizer = LocalFirstLocalizer::local_only(catalog);
    let units = vec![
        TranslationUnit::new("nav/title", "Dashboard"),
        TranslationUnit::new("nav/subtitle", "Welcome {name}"),
    ];
    let translated = localizer
        .localize_units("en", "es", &units)
        .await
        .expect("local-only localization should work");

    assert_eq!(translated.get("nav/title").unwrap(), "Panel");
    assert_eq!(translated.get("nav/subtitle").unwrap(), "Welcome {name}");
    assert!(!localizer.requires_cloud_auth());
}

#[test]
fn json_document_applies_translations_without_changing_shape() {
    let document = JsonLocalizationDocument::new(json!({
        "nav": {
            "title": "Dashboard",
            "count": 3
        },
        "items": ["Open", "{count} files"]
    }));

    let units = document.source_units();
    assert_eq!(
        units.iter().map(|unit| unit.key()).collect::<Vec<_>>(),
        vec!["items/0", "items/1", "nav/title"]
    );

    let translations = BTreeMap::from([
        ("nav/title".to_string(), "Panel".to_string()),
        ("items/0".to_string(), "Abrir".to_string()),
        ("items/1".to_string(), "{count} archivos".to_string()),
    ]);
    let output = document
        .apply_translations(&translations)
        .expect("translations should apply")
        .into_value();

    assert_eq!(output["nav"]["title"], "Panel");
    assert_eq!(output["nav"]["count"], 3);
    assert_eq!(output["items"][0], "Abrir");
    assert_eq!(output["items"][1], "{count} archivos");
}

#[test]
fn json_document_leaves_untranslated_strings_and_shape_unchanged() {
    let document = JsonLocalizationDocument::new(json!({
        "nav": {
            "title": "Dashboard",
            "subtitle": "Welcome"
        },
        "count": 3
    }));
    let translations = BTreeMap::from([
        ("nav/title".to_string(), "Panel".to_string()),
        ("extra/path".to_string(), "Ignored".to_string()),
    ]);

    let output = document
        .apply_translations(&translations)
        .expect("partial translations should apply")
        .into_value();

    assert_eq!(
        output,
        json!({
            "nav": {
                "title": "Panel",
                "subtitle": "Welcome"
            },
            "count": 3
        })
    );
}

#[test]
fn json_document_disambiguates_slash_and_tilde_keys() {
    let document = JsonLocalizationDocument::new(json!({
        "a/b": "Slash key",
        "a~1b": "Tilde one key",
        "nested": {
            "a~b": "Tilde key"
        }
    }));

    let units = document.source_units();

    assert_eq!(
        units.iter().map(|unit| unit.key()).collect::<Vec<_>>(),
        vec!["a~1b", "a~01b", "nested/a~0b"]
    );
}

#[test]
fn lingo_api_provider_serializes_current_localize_shape_without_leaking_key() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        "https://api.lingo.dev",
    ));
    let request = provider
        .localize_request_json("en", "de", &[TranslationUnit::new("cta", "Get started")])
        .expect("request should serialize");

    assert_eq!(request["engineId"], "eng_test");
    assert_eq!(request["sourceLocale"], "en");
    assert_eq!(request["targetLocale"], "de");
    assert_eq!(request["data"]["cta"], "Get started");
    assert!(!request.to_string().contains("secret-key"));
    assert_eq!(
        provider.endpoint(),
        "https://api.lingo.dev/process/localize"
    );
}

#[test]
fn lingo_debug_output_redacts_api_key() {
    let config = test_lingo_config("secret-key", Some("eng_test"), "https://api.lingo.dev");
    let provider = LingoApiProvider::new(config.clone());

    assert!(!format!("{config:?}").contains("secret-key"));
    assert!(!format!("{provider:?}").contains("secret-key"));
}

#[test]
fn lingo_provider_new_uses_bounded_http_timeouts() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        "https://api.lingo.dev",
    ));

    assert_eq!(
        provider.http_timeouts(),
        LingoHttpTimeouts {
            connect_timeout_ms: 10_000,
            request_timeout_ms: 30_000,
        }
    );
}

#[test]
fn lingo_request_and_response_round_trip_json_pointer_keys() {
    let document = JsonLocalizationDocument::new(json!({
        "a/b": "Slash key",
        "a~1b": "Tilde one key"
    }));
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));
    let units = document.source_units();

    let request = provider
        .localize_request_json("en", "es", &units)
        .expect("request should serialize");

    assert_eq!(request["data"]["a/b"], "Slash key");
    assert_eq!(request["data"]["a~1b"], "Tilde one key");

    let response = provider
        .parse_localize_response_json_for_units(
            json!({
                "sourceLocale": "en",
                "targetLocale": "es",
                "data": {
                    "a/b": "Clave con barra",
                    "a~1b": "Clave con tilde uno"
                }
            }),
            "en",
            "es",
            &units,
        )
        .expect("response should parse");
    let output = document
        .apply_translations(&response.translations)
        .expect("translations should apply")
        .into_value();

    assert_eq!(output["a/b"], "Clave con barra");
    assert_eq!(output["a~1b"], "Clave con tilde uno");
}

#[test]
fn lingo_request_preserves_json_array_shape_from_document_units() {
    let document = JsonLocalizationDocument::new(json!({
        "items": ["Open", "Close"],
        "nested": {
            "labels": ["Save", "Cancel"]
        }
    }));
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));

    let request = provider
        .localize_request_json("en", "es", &document.source_units())
        .expect("request should serialize");

    assert_eq!(request["data"]["items"], json!(["Open", "Close"]));
    assert_eq!(
        request["data"]["nested"]["labels"],
        json!(["Save", "Cancel"])
    );
}

#[test]
fn lingo_request_rejects_duplicate_translation_unit_keys() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));

    let error = provider
        .localize_request_json(
            "en",
            "es",
            &[
                TranslationUnit::new("cta", "Get started"),
                TranslationUnit::new("cta", "Start"),
            ],
        )
        .expect_err("duplicate keys should be rejected");

    assert!(error.to_string().contains("duplicate translation unit key"));
}

#[test]
fn lingo_request_can_include_context_hints() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));
    let hints = BTreeMap::from([(
        "cta".to_string(),
        vec!["Landing page".to_string(), "Primary button".to_string()],
    )]);

    let request = provider
        .localize_request_json_with_hints(
            "en",
            "es",
            &[TranslationUnit::new("cta", "Get started")],
            Some(hints),
        )
        .expect("request should serialize");

    assert_eq!(
        request["hints"]["cta"],
        json!(["Landing page", "Primary button"])
    );
}

#[test]
fn lingo_config_accepts_official_api_url_env_alias() {
    let env = HashMap::from([
        ("LINGO_API_KEY".to_string(), "test-key".to_string()),
        (
            LINGO_API_URL_ENV.to_string(),
            "http://127.0.0.1:8787".to_string(),
        ),
    ]);

    let config = LingoApiConfig::from_env_values(|key| env.get(key).cloned())
        .expect("env config should parse")
        .expect("api key should enable config");

    assert_eq!(config.base_url(), "http://127.0.0.1:8787");
}

#[test]
fn lingo_config_uses_lower_priority_api_key_when_dx_alias_is_blank() {
    let env = HashMap::from([
        (DX_LINGO_API_KEY_ENV.to_string(), "   ".to_string()),
        (LINGO_API_KEY_ENV.to_string(), "fallback-key".to_string()),
    ]);

    let config = LingoApiConfig::from_env_values(|key| env.get(key).cloned())
        .expect("env config should parse")
        .expect("fallback key should enable config");

    assert_eq!(config.api_key(), "fallback-key");
}

#[test]
fn lingo_config_uses_lower_priority_engine_and_base_url_when_primary_aliases_are_blank() {
    let env = HashMap::from([
        (LINGO_API_KEY_ENV.to_string(), "test-key".to_string()),
        (DX_LINGO_ENGINE_ID_ENV.to_string(), "  ".to_string()),
        (LINGO_ENGINE_ID_ENV.to_string(), "eng_fallback".to_string()),
        (LINGO_API_URL_ENV.to_string(), "  ".to_string()),
        (
            LINGO_API_BASE_URL_ENV.to_string(),
            "http://127.0.0.1:8788".to_string(),
        ),
    ]);

    let config = LingoApiConfig::from_env_values(|key| env.get(key).cloned())
        .expect("env config should parse")
        .expect("api key should enable config");

    assert_eq!(config.engine_id(), Some("eng_fallback"));
    assert_eq!(config.base_url(), "http://127.0.0.1:8788");
}

#[test]
fn lingo_config_rejects_non_https_non_loopback_api_urls() {
    let env = HashMap::from([
        ("LINGO_API_KEY".to_string(), "test-key".to_string()),
        (
            LINGO_API_URL_ENV.to_string(),
            "http://example.com".to_string(),
        ),
    ]);

    let error = LingoApiConfig::from_env_values(|key| env.get(key).cloned())
        .expect_err("non-loopback HTTP API URLs should be rejected");

    assert!(error.to_string().contains("must use HTTPS"));
}

#[test]
fn lingo_config_rejects_secret_bearing_api_urls() {
    for api_url in [
        "https://user:token@example.com",
        "https://api.example.com?key=secret",
        "https://api.example.com#secret",
    ] {
        let env = HashMap::from([
            ("LINGO_API_KEY".to_string(), "test-key".to_string()),
            (LINGO_API_URL_ENV.to_string(), api_url.to_string()),
        ]);

        let error = LingoApiConfig::from_env_values(|key| env.get(key).cloned())
            .expect_err("secret-bearing API URLs should be rejected");

        assert!(
            error
                .to_string()
                .contains("must not include credentials, query, or fragment"),
            "unexpected error for {api_url}: {error}"
        );
    }
}

#[test]
fn lingo_config_constructor_validates_base_url_and_normalizes_engine() {
    let config = LingoApiConfig::new(
        "  test-key  ",
        Some("  eng_test  ".to_string()),
        "https://api.lingo.dev/process/localize/",
    )
    .expect("constructor should accept official endpoint URL");

    assert_eq!(config.api_key(), "test-key");
    assert_eq!(config.engine_id(), Some("eng_test"));
    assert_eq!(config.base_url(), "https://api.lingo.dev/process/localize");
    assert_eq!(
        LingoApiProvider::new(config).endpoint(),
        "https://api.lingo.dev/process/localize"
    );

    let error = LingoApiConfig::new("test-key", None, "http://example.com")
        .expect_err("constructor should reject non-loopback HTTP");
    assert!(error.to_string().contains("must use HTTPS"));

    let error = LingoApiConfig::new("   ", None, "https://api.lingo.dev")
        .expect_err("constructor should reject empty API keys");
    assert!(error.to_string().contains("API key"));
}

#[test]
fn lingo_config_accepts_official_sdk_and_ci_api_key_alias() {
    let env = HashMap::from([(
        LINGODOTDEV_API_KEY_ENV.to_string(),
        "official-key".to_string(),
    )]);

    let config = LingoApiConfig::from_env_values(|key| env.get(key).cloned())
        .expect("env config should parse")
        .expect("official key alias should enable config");

    assert_eq!(config.api_key(), "official-key");
}

#[test]
fn lingo_response_parser_keeps_model_and_usage_metadata() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));
    let response = provider
        .parse_localize_response_json_for_units(
            json!({
                "sourceLocale": "en",
                "targetLocale": "de",
                "data": { "cta": "Jetzt starten" },
                "model": "anthropic/claude-sonnet-4.5",
                "usage": {
                    "inputTokens": 2789,
                    "outputTokens": 861,
                    "llmCost": 0.02129,
                    "localizationCost": 0.001722,
                    "cost": 0.023012
                }
            }),
            "en",
            "de",
            &[TranslationUnit::new("cta", "Get started")],
        )
        .expect("response should parse");

    assert_eq!(response.translations.get("cta").unwrap(), "Jetzt starten");
    assert_eq!(response.source_locale.as_deref(), Some("en"));
    assert_eq!(response.target_locale.as_deref(), Some("de"));
    assert_eq!(
        response.model.as_deref(),
        Some("anthropic/claude-sonnet-4.5")
    );
    assert_eq!(response.usage.as_ref().unwrap().input_tokens, Some(2789));
    assert_eq!(response.usage.as_ref().unwrap().output_tokens, Some(861));
}

#[test]
fn lingo_safe_response_parser_rejects_locale_echo_mismatch() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));

    let error = provider
        .parse_localize_response_json_for_units(
            json!({
                "sourceLocale": "en",
                "targetLocale": "fr",
                "data": { "cta": "Comenzar" }
            }),
            "en",
            "es",
            &[TranslationUnit::new("cta", "Get started")],
        )
        .expect_err("target locale mismatch should fail");

    assert!(error.to_string().contains("targetLocale mismatch"));
}

#[test]
fn lingo_safe_response_parser_rejects_missing_locale_echoes() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));

    let error = provider
        .parse_localize_response_json_for_units(
            json!({
                "data": { "cta": "Comenzar" }
            }),
            "en",
            "es",
            &[TranslationUnit::new("cta", "Get started")],
        )
        .expect_err("missing locale echoes should fail in the safe parser");

    assert!(error.to_string().contains("sourceLocale missing"));
}

#[test]
fn lingo_safe_response_parser_rejects_array_shape_drift() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));
    let document = JsonLocalizationDocument::new(json!({
        "items": ["Open", "Close"]
    }));

    let error = provider
        .parse_localize_response_json_for_units(
            json!({
                "sourceLocale": "en",
                "targetLocale": "es",
                "data": { "items": { "0": "Abrir", "1": "Cerrar" } }
            }),
            "en",
            "es",
            &document.source_units(),
        )
        .expect_err("object-with-numeric-keys should not replace an array");

    assert!(error.to_string().contains("data shape"));
}

#[test]
fn lingo_safe_response_parser_accepts_array_holes_for_non_string_source_items() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));
    let document = JsonLocalizationDocument::new(json!({
        "items": ["Open", 3, "Close"]
    }));

    let result = provider
        .parse_localize_response_json_for_units(
            json!({
                "sourceLocale": "en",
                "targetLocale": "es",
                "data": { "items": ["Abrir", null, "Cerrar"] }
            }),
            "en",
            "es",
            &document.source_units(),
        )
        .expect("null array holes should preserve source container shape");

    assert_eq!(result.translations.get("items/0").unwrap(), "Abrir");
    assert_eq!(result.translations.get("items/2").unwrap(), "Cerrar");
    assert!(!result.translations.contains_key("items/1"));
}

#[test]
fn provider_response_maps_lingo_metadata_without_losing_translations() {
    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        "https://api.lingo.dev",
    ));
    let lingo_response = provider
        .parse_localize_response_json_for_units(
            json!({
                "sourceLocale": "en",
                "targetLocale": "es",
                "data": { "cta": "Comenzar" },
                "model": "openai/gpt-5.1",
                "usage": { "inputTokens": 3, "outputTokens": 2, "cost": 0.001 }
            }),
            "en",
            "es",
            &[TranslationUnit::new("cta", "Get started")],
        )
        .expect("Lingo response should parse");

    let response = LocalizationResponse::from(lingo_response);

    assert_eq!(response.translations.get("cta").unwrap(), "Comenzar");
    assert_eq!(response.source_locale.as_deref(), Some("en"));
    assert_eq!(response.target_locale.as_deref(), Some("es"));
    assert_eq!(response.provider.as_deref(), Some("lingo.dev"));
    assert_eq!(response.model.as_deref(), Some("openai/gpt-5.1"));
    assert_eq!(response.usage.as_ref().unwrap().input_tokens, Some(3));
}

#[tokio::test]
async fn lingo_api_provider_posts_to_mock_endpoint_without_leaking_key() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move { capture_single_lingo_request(listener).await });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let translated = provider
        .localize_response("en", "es", &[TranslationUnit::new("cta", "Get started")])
        .await
        .expect("mocked HTTP localization should succeed");
    let captured = server.await.expect("mock server task should finish");

    assert!(captured.headers.starts_with("POST /process/localize "));
    assert!(
        captured
            .headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-api-key: secret-key")),
        "API key should be sent as a header"
    );
    assert!(!captured.body.to_string().contains("secret-key"));
    assert_eq!(captured.body["engineId"], "eng_test");
    assert_eq!(captured.body["sourceLocale"], "en");
    assert_eq!(captured.body["targetLocale"], "es");
    assert_eq!(captured.body["data"]["cta"], "Get started");
    assert_eq!(translated.translations.get("cta").unwrap(), "Comenzar");
    assert_eq!(translated.model.as_deref(), Some("mock/model"));
    assert_eq!(translated.usage.as_ref().unwrap().input_tokens, Some(4));
}

#[tokio::test]
async fn lingo_api_provider_does_not_follow_redirects_with_api_key() {
    let redirect_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("redirect server should bind");
    let redirect_address = redirect_listener
        .local_addr()
        .expect("redirect server should expose addr");
    let sink_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("redirect target should bind");
    let sink_address = sink_listener
        .local_addr()
        .expect("redirect target should expose addr");
    let redirect_server = tokio::spawn(async move {
        respond_with_lingo_redirect(
            redirect_listener,
            &format!("http://{sink_address}/process/localize"),
        )
        .await
    });
    let sink_server = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_millis(250),
            unexpected_lingo_redirect_target(sink_listener),
        )
        .await
    });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        None,
        format!("http://{redirect_address}"),
    ));
    let error = provider
        .localize_response("en", "es", &[TranslationUnit::new("cta", "Get started")])
        .await
        .expect_err("redirect response should not be followed");
    let redirected_request = redirect_server
        .await
        .expect("redirect server task should finish");
    let sink_result = sink_server
        .await
        .expect("redirect target task should finish");

    assert!(error.to_string().contains("307"));
    assert!(
        redirected_request
            .headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-api-key: secret-key")),
        "first-hop request should include the API key"
    );
    assert!(
        sink_result.is_err(),
        "redirect target should not receive a forwarded API key request"
    );
}

#[tokio::test]
async fn lingo_api_provider_protects_and_restores_structural_tokens() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move { echo_protected_lingo_message(listener).await });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let translated = provider
        .localize_response(
            "en",
            "es",
            &[TranslationUnit::new(
                "message",
                "Hello {name}, run `dx build`, keep {{count}} and {count, plural, one {# file} other {# files}}.",
            )],
        )
        .await
        .expect("mocked HTTP localization should succeed");
    let captured = server.await.expect("mock server task should finish");
    let protected_message = captured.body["data"]["message"]
        .as_str()
        .expect("request should contain protected message");

    assert!(!protected_message.contains("{name}"));
    assert!(!protected_message.contains("{{count}}"));
    assert!(!protected_message.contains("`dx build`"));
    assert!(!protected_message.contains("{count, plural"));
    assert_eq!(
        translated.translations.get("message").unwrap(),
        "Hola {name}, ejecuta `dx build`, conserva {{count}} and {count, plural, one {# file} other {# files}}."
    );
}

#[tokio::test]
async fn lingo_api_provider_preserves_array_shape_after_protecting_units() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move { echo_lingo_request_data(listener).await });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let document = JsonLocalizationDocument::new(json!({
        "items": ["Hello {name}", "Run `dx build`"]
    }));
    let result = provider
        .localize_response("en", "es", &document.source_units())
        .await
        .expect("mocked HTTP localization should succeed");
    let captured = server.await.expect("mock server task should finish");

    assert!(captured.body["data"]["items"].is_array());
    assert_eq!(result.translations.get("items/0").unwrap(), "Hello {name}");
    assert_eq!(
        result.translations.get("items/1").unwrap(),
        "Run `dx build`"
    );
}

#[tokio::test]
async fn lingo_api_provider_rejects_response_key_drift() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move {
        respond_with_lingo_json(
            listener,
            json!({
                "sourceLocale": "en",
                "targetLocale": "es",
                "data": { "other": "Otro" }
            }),
        )
        .await
    });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let error = provider
        .localize_response("en", "es", &[TranslationUnit::new("cta", "Get started")])
        .await
        .expect_err("response key drift should fail");
    server.await.expect("mock server task should finish");

    assert!(
        error
            .to_string()
            .contains("response key structure mismatch")
    );
    assert!(error.to_string().contains("missing keys: cta"));
    assert!(error.to_string().contains("extra keys: other"));
}

#[tokio::test]
async fn lingo_api_provider_redacts_secret_like_error_bodies() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move {
        respond_with_lingo_error(
            listener,
            "request failed for secret-key and sk-test-1234567890",
        )
        .await
    });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let error = provider
        .localize_response("en", "es", &[TranslationUnit::new("cta", "Get started")])
        .await
        .expect_err("mocked HTTP localization should fail");
    server.await.expect("mock server task should finish");

    let message = error.to_string();
    assert!(!message.contains("secret-key"));
    assert!(!message.contains("sk-test-1234567890"));
    assert!(message.contains("[redacted]"));
}

#[tokio::test]
async fn lingo_api_provider_bounds_error_body_before_connection_closes() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move { respond_with_held_open_lingo_error(listener).await });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        provider.localize_response("en", "es", &[TranslationUnit::new("cta", "Get started")]),
    )
    .await
    .expect("bounded error body should not wait for a held-open response")
    .expect_err("mocked HTTP localization should fail");
    server.abort();

    let message = error.to_string();
    assert!(message.contains("500"));
    assert!(!message.contains("secret-key"));
    assert!(message.contains("[redacted]"));
    assert!(!message.contains("TAIL_MARKER_AFTER_BOUNDARY"));
    assert!(
        message.len() < 2_200,
        "redacted error should stay compact, got {} bytes",
        message.len()
    );
}

#[tokio::test]
async fn lingo_api_provider_bounds_success_body_before_connection_closes() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("mock server should bind");
    let address = listener
        .local_addr()
        .expect("mock server should expose addr");
    let server = tokio::spawn(async move { respond_with_held_open_lingo_success(listener).await });

    let provider = LingoApiProvider::new(test_lingo_config(
        "secret-key",
        Some("eng_test"),
        format!("http://{address}"),
    ));
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        provider.localize_response("en", "es", &[TranslationUnit::new("cta", "Get started")]),
    )
    .await
    .expect("bounded success body should not wait for a held-open response")
    .expect_err("oversized mocked HTTP localization should fail");
    server.abort();

    let message = error.to_string();
    assert!(message.contains("success response body"));
    assert!(message.contains("exceeded"));
    assert!(!message.contains("TAIL_MARKER_AFTER_BOUNDARY"));
}

struct CapturedLingoRequest {
    headers: String,
    body: serde_json::Value,
}

async fn capture_single_lingo_request(listener: TcpListener) -> CapturedLingoRequest {
    let (mut stream, _) = listener
        .accept()
        .await
        .expect("mock server should accept one request");
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .await
            .expect("mock server should read request");
        assert!(read > 0, "request should not close before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
            break index;
        }
    };

    let headers =
        String::from_utf8(bytes[..header_end].to_vec()).expect("request headers should be UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .expect("request should include content length");
    let body_start = header_end + b"\r\n\r\n".len();
    while bytes.len() < body_start + content_length {
        let read = stream
            .read(&mut chunk)
            .await
            .expect("mock server should read request body");
        assert!(read > 0, "request should not close before body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&bytes[body_start..body_start + content_length])
        .expect("request body should be JSON");

    let response = json!({
        "sourceLocale": "en",
        "targetLocale": "es",
        "data": { "cta": "Comenzar" },
        "model": "mock/model",
        "usage": { "inputTokens": 4, "outputTokens": 2, "cost": 0.001 }
    })
    .to_string();
    let response_headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        response.len()
    );
    stream
        .write_all(response_headers.as_bytes())
        .await
        .expect("mock server should write headers");
    stream
        .write_all(response.as_bytes())
        .await
        .expect("mock server should write body");

    CapturedLingoRequest { headers, body }
}

async fn echo_lingo_request_data(listener: TcpListener) -> CapturedLingoRequest {
    let mut captured = capture_http_json_request(listener).await;
    let response = json!({
        "sourceLocale": "en",
        "targetLocale": "es",
        "data": captured.body["data"].clone()
    })
    .to_string();
    write_http_json_response(&mut captured.stream, 200, &response).await;
    captured.into_request()
}

async fn respond_with_lingo_json(listener: TcpListener, response: serde_json::Value) {
    let mut captured = capture_http_json_request(listener).await;
    write_http_json_response(&mut captured.stream, 200, &response.to_string()).await;
}

async fn respond_with_lingo_redirect(
    listener: TcpListener,
    location: &str,
) -> CapturedLingoRequest {
    let mut captured = capture_http_json_request(listener).await;
    let response_headers = format!(
        "HTTP/1.1 307 Temporary Redirect\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
    captured
        .stream
        .write_all(response_headers.as_bytes())
        .await
        .expect("mock server should write redirect");
    captured.into_request()
}

async fn unexpected_lingo_redirect_target(listener: TcpListener) -> CapturedLingoRequest {
    let mut captured = capture_http_json_request(listener).await;
    let response = json!({
        "sourceLocale": "en",
        "targetLocale": "es",
        "data": { "cta": "Comenzar" }
    })
    .to_string();
    write_http_json_response(&mut captured.stream, 200, &response).await;
    captured.into_request()
}

async fn echo_protected_lingo_message(listener: TcpListener) -> CapturedLingoRequest {
    let mut captured = capture_http_json_request(listener).await;
    let protected_message = captured.body["data"]["message"]
        .as_str()
        .expect("request should contain message")
        .replace("Hello", "Hola")
        .replace("run", "ejecuta")
        .replace("keep", "conserva");
    let response = json!({
        "sourceLocale": "en",
        "targetLocale": "es",
        "data": { "message": protected_message },
        "model": "mock/model",
        "usage": { "inputTokens": 8, "outputTokens": 8, "cost": 0.001 }
    })
    .to_string();
    write_http_json_response(&mut captured.stream, 200, &response).await;
    captured.into_request()
}

async fn respond_with_lingo_error(listener: TcpListener, body: &str) {
    let mut captured = capture_http_json_request(listener).await;

    let response_headers = format!(
        "HTTP/1.1 401 Unauthorized\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    captured
        .stream
        .write_all(response_headers.as_bytes())
        .await
        .expect("mock server should write headers");
    captured
        .stream
        .write_all(body.as_bytes())
        .await
        .expect("mock server should write body");
}

async fn respond_with_held_open_lingo_error(listener: TcpListener) {
    let mut captured = capture_http_json_request(listener).await;
    let mut body = format!(
        "request failed for secret-key and sk-test-1234567890 {}",
        "x".repeat(9_000)
    );
    body.push_str("TAIL_MARKER_AFTER_BOUNDARY");
    let response_headers = format!(
        "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: keep-alive\r\n\r\n",
        body.len() + 100_000
    );
    captured
        .stream
        .write_all(response_headers.as_bytes())
        .await
        .expect("mock server should write headers");
    captured
        .stream
        .write_all(body.as_bytes())
        .await
        .expect("mock server should write body prefix");
    tokio::time::sleep(Duration::from_secs(5)).await;
}

async fn respond_with_held_open_lingo_success(listener: TcpListener) {
    let mut captured = capture_http_json_request(listener).await;
    let mut body = format!(
        r#"{{"sourceLocale":"en","targetLocale":"es","data":{{"cta":"{}"#,
        "x".repeat(1_100_000)
    );
    body.push_str("TAIL_MARKER_AFTER_BOUNDARY");
    let response_headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: keep-alive\r\n\r\n",
        body.len() + 100_000
    );
    captured
        .stream
        .write_all(response_headers.as_bytes())
        .await
        .expect("mock server should write headers");
    captured
        .stream
        .write_all(body.as_bytes())
        .await
        .expect("mock server should write body prefix");
    tokio::time::sleep(Duration::from_secs(5)).await;
}

struct CapturedHttpJsonRequest {
    stream: tokio::net::TcpStream,
    headers: String,
    body: serde_json::Value,
}

impl CapturedHttpJsonRequest {
    fn into_request(self) -> CapturedLingoRequest {
        CapturedLingoRequest {
            headers: self.headers,
            body: self.body,
        }
    }
}

async fn capture_http_json_request(listener: TcpListener) -> CapturedHttpJsonRequest {
    let (mut stream, _) = listener
        .accept()
        .await
        .expect("mock server should accept one request");
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 1024];
    let header_end = loop {
        let read = stream
            .read(&mut chunk)
            .await
            .expect("mock server should read request");
        assert!(read > 0, "request should not close before headers");
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
            break index;
        }
    };

    let headers =
        String::from_utf8(bytes[..header_end].to_vec()).expect("request headers should be UTF-8");
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .expect("request should include content length");
    let body_start = header_end + b"\r\n\r\n".len();
    while bytes.len() < body_start + content_length {
        let read = stream
            .read(&mut chunk)
            .await
            .expect("mock server should read request body");
        assert!(read > 0, "request should not close before body");
        bytes.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&bytes[body_start..body_start + content_length])
        .expect("request body should be JSON");

    CapturedHttpJsonRequest {
        stream,
        headers,
        body,
    }
}

async fn write_http_json_response(stream: &mut tokio::net::TcpStream, status: u16, body: &str) {
    let response_headers = format!(
        "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(response_headers.as_bytes())
        .await
        .expect("mock server should write headers");
    stream
        .write_all(body.as_bytes())
        .await
        .expect("mock server should write body");
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
