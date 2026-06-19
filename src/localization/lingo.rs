use crate::error::{I18nError, Result};
use crate::localization::{
    LocaleKeyStructure, ProtectedText, TranslationPathSegment, TranslationUnit,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fmt;
use std::time::Duration;
use url::Url;

pub const DEFAULT_LINGO_API_BASE_URL: &str = "https://api.lingo.dev";
pub const DEFAULT_LINGO_CONNECT_TIMEOUT_MS: u64 = 10_000;
pub const DEFAULT_LINGO_REQUEST_TIMEOUT_MS: u64 = 30_000;
pub const LINGODOTDEV_API_KEY_ENV: &str = "LINGODOTDEV_API_KEY";
pub const LINGO_API_KEY_ENV: &str = "LINGO_API_KEY";
pub const DX_LINGO_API_KEY_ENV: &str = "DX_I18N_LINGO_API_KEY";
pub const LINGO_ENGINE_ID_ENV: &str = "LINGO_ENGINE_ID";
pub const DX_LINGO_ENGINE_ID_ENV: &str = "DX_I18N_LINGO_ENGINE_ID";
pub const LINGO_API_URL_ENV: &str = "LINGO_API_URL";
pub const LINGO_API_BASE_URL_ENV: &str = "LINGO_API_BASE_URL";
const MAX_LINGO_ERROR_BODY_BYTES: usize = 8 * 1024;
const MAX_LINGO_SUCCESS_BODY_BYTES: usize = 1024 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct LingoApiConfig {
    api_key: String,
    engine_id: Option<String>,
    base_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LingoHttpTimeouts {
    pub connect_timeout_ms: u64,
    pub request_timeout_ms: u64,
}

impl Default for LingoHttpTimeouts {
    fn default() -> Self {
        Self {
            connect_timeout_ms: DEFAULT_LINGO_CONNECT_TIMEOUT_MS,
            request_timeout_ms: DEFAULT_LINGO_REQUEST_TIMEOUT_MS,
        }
    }
}

impl LingoHttpTimeouts {
    fn connect_timeout(self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms)
    }

    fn request_timeout(self) -> Duration {
        Duration::from_millis(self.request_timeout_ms)
    }
}

impl fmt::Debug for LingoApiConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LingoApiConfig")
            .field("api_key", &"[redacted]")
            .field("engine_id", &self.engine_id)
            .field("base_url", &debug_safe_lingo_api_base_url(&self.base_url))
            .finish()
    }
}

impl LingoApiConfig {
    pub fn new(
        api_key: impl Into<String>,
        engine_id: Option<String>,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        let api_key = api_key.into().trim().to_string();
        if api_key.is_empty() {
            return Err(I18nError::ApiKeyRequired(
                "Lingo.dev".to_string(),
                format!(
                    "{DX_LINGO_API_KEY_ENV}, {LINGO_API_KEY_ENV}, or {LINGODOTDEV_API_KEY_ENV}"
                ),
            ));
        }
        let engine_id = normalize_optional_lingo_engine_id(engine_id);
        let base_url = normalize_lingo_api_base_url(base_url.into());
        let base_url = validate_lingo_api_base_url(&base_url)?;

        Ok(Self {
            api_key,
            engine_id,
            base_url,
        })
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn engine_id(&self) -> Option<&str> {
        self.engine_id.as_deref()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn set_engine_id_if_missing(&mut self, engine_id: Option<&str>) {
        if self.engine_id.is_none() {
            self.engine_id = normalize_optional_lingo_engine_id(engine_id.map(str::to_string));
        }
    }

    pub fn from_env() -> Result<Option<Self>> {
        Self::from_env_values(|key| env::var(key).ok())
    }

    pub fn from_env_values(get_env: impl Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        let Some(api_key) = first_non_empty_env(
            &get_env,
            &[
                DX_LINGO_API_KEY_ENV,
                LINGO_API_KEY_ENV,
                LINGODOTDEV_API_KEY_ENV,
            ],
        ) else {
            return Ok(None);
        };

        let engine_id =
            first_non_empty_env(&get_env, &[DX_LINGO_ENGINE_ID_ENV, LINGO_ENGINE_ID_ENV]);
        let base_url = first_non_empty_env(&get_env, &[LINGO_API_URL_ENV, LINGO_API_BASE_URL_ENV])
            .unwrap_or_else(|| DEFAULT_LINGO_API_BASE_URL.to_string());

        Ok(Some(Self::new(api_key, engine_id, base_url)?))
    }
}

fn normalize_optional_lingo_engine_id(engine_id: Option<String>) -> Option<String> {
    engine_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_lingo_api_base_url(base_url: String) -> String {
    base_url.trim().trim_end_matches('/').to_string()
}

fn first_non_empty_env(get_env: &impl Fn(&str) -> Option<String>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        get_env(key)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

#[derive(Clone)]
pub struct LingoApiProvider {
    config: LingoApiConfig,
    client: Client,
    http_timeouts: LingoHttpTimeouts,
}

impl fmt::Debug for LingoApiProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LingoApiProvider")
            .field("config", &self.config)
            .field("client", &"<reqwest::Client>")
            .finish()
    }
}

impl LingoApiProvider {
    pub fn new(config: LingoApiConfig) -> Self {
        let http_timeouts = LingoHttpTimeouts::default();
        Self {
            config,
            client: lingo_http_client(http_timeouts),
            http_timeouts,
        }
    }

    pub fn http_timeouts(&self) -> LingoHttpTimeouts {
        self.http_timeouts
    }

    pub fn endpoint(&self) -> String {
        lingo_localize_endpoint(&self.config.base_url)
    }

    pub fn localize_request_json(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<Value> {
        self.localize_request_json_with_hints(source_locale, target_locale, units, None)
    }

    pub fn localize_request_json_with_hints(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
        hints: Option<BTreeMap<String, Vec<String>>>,
    ) -> Result<Value> {
        let request = LingoLocalizeRequest {
            engine_id: self.config.engine_id.clone(),
            source_locale: source_locale.to_string(),
            target_locale: target_locale.to_string(),
            data: units_to_json_object(units)?,
            hints,
        };

        Ok(serde_json::to_value(request)?)
    }

    pub fn parse_localize_response_json_for_units(
        &self,
        value: Value,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LingoLocalizeResult> {
        let protected = protect_units(units)?;
        let protected_units = protected
            .iter()
            .map(|unit| unit.to_translation_unit())
            .collect::<Vec<_>>();
        let expected_data = units_to_json_object(&protected_units)?;
        let body = parse_localize_response_body(value)?;

        validate_response_locale("sourceLocale", body.source_locale.as_deref(), source_locale)?;
        validate_response_locale("targetLocale", body.target_locale.as_deref(), target_locale)?;

        let mut result = LingoLocalizeResult {
            source_locale: body.source_locale,
            target_locale: body.target_locale,
            translations: json_object_to_units(body.data.clone())?,
            model: body.model,
            usage: body.usage,
        };
        validate_response_keys(&result.translations, &protected)?;
        validate_response_json_shape(&expected_data, &body.data)?;
        restore_units(&mut result.translations, &protected)?;
        Ok(result)
    }

    pub async fn localize_units(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<BTreeMap<String, String>> {
        Ok(self
            .localize_response(source_locale, target_locale, units)
            .await?
            .translations)
    }

    pub async fn localize_response(
        &self,
        source_locale: &str,
        target_locale: &str,
        units: &[TranslationUnit],
    ) -> Result<LingoLocalizeResult> {
        if self.config.api_key.trim().is_empty() {
            return Err(I18nError::ApiKeyRequired(
                "Lingo.dev".to_string(),
                format!(
                    "{DX_LINGO_API_KEY_ENV}, {LINGO_API_KEY_ENV}, or {LINGODOTDEV_API_KEY_ENV}"
                ),
            ));
        }

        let protected = protect_units(units)?;
        let protected_units = protected
            .iter()
            .map(|unit| unit.to_translation_unit())
            .collect::<Vec<_>>();

        let response = self
            .client
            .post(self.endpoint())
            .header("X-API-Key", &self.config.api_key)
            .json(&self.localize_request_json(source_locale, target_locale, &protected_units)?)
            .send()
            .await?;

        if !response.status().is_success() {
            let code = response.status().as_u16();
            let message = bounded_lingo_error_message(response, &self.config.api_key).await;
            return Err(I18nError::ServerError { code, message });
        }

        let body = bounded_lingo_success_json(response).await?;
        self.parse_localize_response_json_for_units(body, source_locale, target_locale, units)
    }
}

fn lingo_http_client(timeouts: LingoHttpTimeouts) -> Client {
    Client::builder()
        .connect_timeout(timeouts.connect_timeout())
        .timeout(timeouts.request_timeout())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static Lingo.dev HTTP timeout configuration should build")
}

async fn bounded_lingo_error_message(mut response: reqwest::Response, api_key: &str) -> String {
    let mut bytes = Vec::new();
    let mut truncated = false;

    while bytes.len() < MAX_LINGO_ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_LINGO_ERROR_BODY_BYTES - bytes.len();
                if chunk.len() > remaining {
                    bytes.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(_) => return String::new(),
        }
    }

    if bytes.len() >= MAX_LINGO_ERROR_BODY_BYTES {
        truncated = true;
    }

    let mut message = String::from_utf8_lossy(&bytes).into_owned();
    if truncated {
        message.push_str("...");
    }

    redact_secret_like_text(&message, api_key)
}

async fn bounded_lingo_success_json(mut response: reqwest::Response) -> Result<Value> {
    let mut bytes = Vec::new();

    while bytes.len() <= MAX_LINGO_SUCCESS_BODY_BYTES {
        match response.chunk().await? {
            Some(chunk) => {
                let next_len = bytes.len().saturating_add(chunk.len());
                if next_len > MAX_LINGO_SUCCESS_BODY_BYTES {
                    return Err(I18nError::UnexpectedResponse(format!(
                        "Lingo.dev success response body exceeded {MAX_LINGO_SUCCESS_BODY_BYTES} bytes"
                    )));
                }
                bytes.extend_from_slice(&chunk);
            }
            None => return Ok(serde_json::from_slice(&bytes)?),
        }
    }

    Err(I18nError::UnexpectedResponse(format!(
        "Lingo.dev success response body exceeded {MAX_LINGO_SUCCESS_BODY_BYTES} bytes"
    )))
}

#[derive(Debug, Clone, PartialEq)]
pub struct LingoLocalizeResult {
    pub source_locale: Option<String>,
    pub target_locale: Option<String>,
    pub translations: BTreeMap<String, String>,
    pub model: Option<String>,
    pub usage: Option<LingoUsage>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LingoUsage {
    #[serde(rename = "inputTokens")]
    pub input_tokens: Option<u64>,
    #[serde(rename = "outputTokens")]
    pub output_tokens: Option<u64>,
    #[serde(rename = "llmCost")]
    pub llm_cost: Option<f64>,
    #[serde(rename = "localizationCost")]
    pub localization_cost: Option<f64>,
    pub cost: Option<f64>,
}

#[derive(Debug, Serialize)]
struct LingoLocalizeRequest {
    #[serde(rename = "engineId", skip_serializing_if = "Option::is_none")]
    engine_id: Option<String>,
    #[serde(rename = "sourceLocale")]
    source_locale: String,
    #[serde(rename = "targetLocale")]
    target_locale: String,
    data: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    hints: Option<BTreeMap<String, Vec<String>>>,
}

#[derive(Debug, Deserialize)]
struct LingoLocalizeResponse {
    #[serde(rename = "sourceLocale")]
    source_locale: Option<String>,
    #[serde(rename = "targetLocale")]
    target_locale: Option<String>,
    data: Value,
    model: Option<String>,
    usage: Option<LingoUsage>,
}

fn parse_localize_response_body(value: Value) -> Result<LingoLocalizeResponse> {
    Ok(serde_json::from_value(value)?)
}

fn validate_lingo_api_base_url(base_url: &str) -> Result<String> {
    let parsed = Url::parse(base_url)
        .map_err(|error| I18nError::ConfigError(format!("invalid Lingo.dev API URL: {error}")))?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(I18nError::ConfigError(
            "LINGO_API_URL must not include credentials, query, or fragment".to_string(),
        ));
    }
    match parsed.scheme() {
        "https" => Ok(base_url.to_string()),
        "http" if parsed.host_str().is_some_and(is_loopback_host) => Ok(base_url.to_string()),
        "http" => Err(I18nError::ConfigError(
            "LINGO_API_URL must use HTTPS unless it targets localhost or loopback".to_string(),
        )),
        scheme => Err(I18nError::ConfigError(format!(
            "LINGO_API_URL must use http or https, got '{scheme}'"
        ))),
    }
}

fn debug_safe_lingo_api_base_url(base_url: &str) -> String {
    match Url::parse(base_url) {
        Ok(parsed)
            if !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some() =>
        {
            "[redacted]".to_string()
        }
        _ => base_url.to_string(),
    }
}

fn lingo_localize_endpoint(base_url: &str) -> String {
    let mut parsed =
        Url::parse(base_url).expect("LingoApiConfig base_url should be validated before use");
    let path = parsed.path().trim_end_matches('/');
    let path = if path.ends_with("/process/localize") {
        path.to_string()
    } else if path.is_empty() {
        "/process/localize".to_string()
    } else {
        format!("{path}/process/localize")
    };
    parsed.set_path(&path);
    parsed.to_string()
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

struct ProtectedUnit {
    key: String,
    path_segments: Option<Vec<TranslationPathSegment>>,
    protected: ProtectedText,
}

impl ProtectedUnit {
    fn to_translation_unit(&self) -> TranslationUnit {
        match &self.path_segments {
            Some(segments) => TranslationUnit::with_path_segments(
                self.key.clone(),
                self.protected.translatable_text(),
                segments.clone(),
            ),
            None => TranslationUnit::new(self.key.clone(), self.protected.translatable_text()),
        }
    }
}

fn protect_units(units: &[TranslationUnit]) -> Result<Vec<ProtectedUnit>> {
    units
        .iter()
        .map(|unit| {
            let protected = ProtectedText::protect(unit.text())
                .map_err(|error| I18nError::ConfigError(error.to_string()))?;
            Ok(ProtectedUnit {
                key: unit.key().to_string(),
                path_segments: unit.path_segments().map(<[_]>::to_vec),
                protected,
            })
        })
        .collect()
}

fn restore_units(
    translations: &mut BTreeMap<String, String>,
    protected_units: &[ProtectedUnit],
) -> Result<()> {
    for unit in protected_units {
        if let Some(translated) = translations.get_mut(&unit.key) {
            *translated = unit
                .protected
                .restore(translated)
                .map_err(|error| I18nError::ConfigError(error.to_string()))?;
        }
    }

    Ok(())
}

fn validate_response_keys(
    translations: &BTreeMap<String, String>,
    protected_units: &[ProtectedUnit],
) -> Result<()> {
    let expected =
        LocaleKeyStructure::from_keys(protected_units.iter().map(|unit| unit.key.as_str()));
    let actual = LocaleKeyStructure::from_keys(translations.keys().map(String::as_str));
    let diff = expected.compare(&actual);
    if diff.is_match() {
        return Ok(());
    }

    let mut parts = Vec::new();
    if !diff.missing_keys().is_empty() {
        parts.push(format!("missing keys: {}", diff.missing_keys().join(", ")));
    }
    if !diff.extra_keys().is_empty() {
        parts.push(format!("extra keys: {}", diff.extra_keys().join(", ")));
    }

    Err(I18nError::UnexpectedResponse(format!(
        "Lingo.dev response key structure mismatch ({})",
        parts.join("; ")
    )))
}

fn validate_response_locale(field: &str, actual: Option<&str>, expected: &str) -> Result<()> {
    let Some(actual) = actual else {
        return Err(I18nError::UnexpectedResponse(format!(
            "Lingo.dev response {field} missing: expected '{expected}'"
        )));
    };
    if actual == expected {
        return Ok(());
    }

    Err(I18nError::UnexpectedResponse(format!(
        "Lingo.dev response {field} mismatch: expected '{expected}', got '{actual}'"
    )))
}

fn validate_response_json_shape(expected: &Value, actual: &Value) -> Result<()> {
    if json_shape_matches(expected, actual) {
        return Ok(());
    }

    Err(I18nError::UnexpectedResponse(
        "Lingo.dev response data shape does not match request data".to_string(),
    ))
}

fn json_shape_matches(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Null, Value::Null) => true,
        (Value::String(_), Value::String(_)) => true,
        (Value::Array(expected), Value::Array(actual)) => {
            expected.len() == actual.len()
                && expected
                    .iter()
                    .zip(actual.iter())
                    .all(|(expected, actual)| json_shape_matches(expected, actual))
        }
        (Value::Object(expected), Value::Object(actual)) => {
            expected.len() == actual.len()
                && expected.iter().all(|(key, expected)| {
                    actual
                        .get(key)
                        .is_some_and(|actual| json_shape_matches(expected, actual))
                })
        }
        _ => false,
    }
}

fn units_to_json_object(units: &[TranslationUnit]) -> Result<Value> {
    let mut root = Value::Object(Map::new());
    let mut seen_keys = BTreeSet::new();
    for unit in units {
        if unit.key().is_empty() {
            return Err(I18nError::ConfigError(
                "translation unit key cannot be empty".to_string(),
            ));
        }
        if !seen_keys.insert(unit.key().to_string()) {
            return Err(I18nError::ConfigError(format!(
                "duplicate translation unit key '{}'",
                unit.key()
            )));
        }

        insert_unit_path(&mut root, unit, Value::String(unit.text().to_string()))?;
    }
    Ok(root)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestPathSegment {
    ObjectKey(String),
    ArrayIndex(usize),
}

fn insert_unit_path(root: &mut Value, unit: &TranslationUnit, value: Value) -> Result<()> {
    let segments = request_path_segments(unit);
    insert_segments(root, &segments, value)
}

fn request_path_segments(unit: &TranslationUnit) -> Vec<RequestPathSegment> {
    if let Some(segments) = unit.path_segments() {
        return segments
            .iter()
            .map(|segment| match segment {
                TranslationPathSegment::ObjectKey(key) => {
                    RequestPathSegment::ObjectKey(key.clone())
                }
                TranslationPathSegment::ArrayIndex(index) => RequestPathSegment::ArrayIndex(*index),
            })
            .collect();
    }

    unit.key()
        .split('/')
        .map(|segment| RequestPathSegment::ObjectKey(unescape_json_pointer_segment(segment)))
        .collect()
}

fn insert_segments(
    current: &mut Value,
    segments: &[RequestPathSegment],
    value: Value,
) -> Result<()> {
    let Some(segment) = segments.first() else {
        return Err(I18nError::ConfigError(
            "translation unit key cannot be empty".to_string(),
        ));
    };

    if segments.len() == 1 {
        insert_leaf(current, segment, value)?;
        return Ok(());
    }

    let child = child_container(current, segment, &segments[1])?;
    insert_segments(child, &segments[1..], value)
}

fn insert_leaf(current: &mut Value, segment: &RequestPathSegment, value: Value) -> Result<()> {
    match segment {
        RequestPathSegment::ObjectKey(key) => {
            let map = ensure_object(current, key)?;
            if map.insert(key.clone(), value).is_some() {
                return Err(I18nError::ConfigError(format!(
                    "duplicate translation unit key segment '{key}'"
                )));
            }
        }
        RequestPathSegment::ArrayIndex(index) => {
            let array = ensure_array(current, &index.to_string())?;
            if array.len() <= *index {
                array.resize(index + 1, Value::Null);
            }
            if !array[*index].is_null() {
                return Err(I18nError::ConfigError(format!(
                    "duplicate translation unit array index '{index}'"
                )));
            }
            array[*index] = value;
        }
    }

    Ok(())
}

fn child_container<'a>(
    current: &'a mut Value,
    segment: &RequestPathSegment,
    next: &RequestPathSegment,
) -> Result<&'a mut Value> {
    match segment {
        RequestPathSegment::ObjectKey(key) => {
            let map = ensure_object(current, key)?;
            let entry = map
                .entry(key.clone())
                .or_insert_with(|| empty_container_for(next));
            ensure_container_kind(entry, key, next)?;
            Ok(entry)
        }
        RequestPathSegment::ArrayIndex(index) => {
            let array = ensure_array(current, &index.to_string())?;
            if array.len() <= *index {
                array.resize_with(index + 1, || Value::Null);
            }
            if array[*index].is_null() {
                array[*index] = empty_container_for(next);
            }
            ensure_container_kind(&mut array[*index], &index.to_string(), next)?;
            Ok(&mut array[*index])
        }
    }
}

fn empty_container_for(next: &RequestPathSegment) -> Value {
    match next {
        RequestPathSegment::ObjectKey(_) => Value::Object(Map::new()),
        RequestPathSegment::ArrayIndex(_) => Value::Array(Vec::new()),
    }
}

fn ensure_container_kind(
    value: &mut Value,
    segment: &str,
    next: &RequestPathSegment,
) -> Result<()> {
    match (value, next) {
        (Value::Object(_), RequestPathSegment::ObjectKey(_))
        | (Value::Array(_), RequestPathSegment::ArrayIndex(_)) => Ok(()),
        _ => Err(I18nError::ConfigError(format!(
            "translation unit key path conflicts at '{segment}'"
        ))),
    }
}

fn ensure_object<'a>(value: &'a mut Value, segment: &str) -> Result<&'a mut Map<String, Value>> {
    let Value::Object(map) = value else {
        return Err(I18nError::ConfigError(format!(
            "translation unit key path conflicts at '{segment}'"
        )));
    };
    Ok(map)
}

fn ensure_array<'a>(value: &'a mut Value, segment: &str) -> Result<&'a mut Vec<Value>> {
    let Value::Array(array) = value else {
        return Err(I18nError::ConfigError(format!(
            "translation unit key path conflicts at '{segment}'"
        )));
    };
    Ok(array)
}

fn json_object_to_units(value: Value) -> Result<BTreeMap<String, String>> {
    let mut units = BTreeMap::new();
    collect_strings("", &value, &mut units);
    Ok(units)
}

fn collect_strings(path: &str, value: &Value, units: &mut BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            units.insert(path.to_string(), text.clone());
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let child_path = join_path(path, &index.to_string());
                collect_strings(&child_path, item, units);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let child_path = join_path(path, key);
                collect_strings(&child_path, item, units);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn join_path(parent: &str, child: &str) -> String {
    let child = escape_json_pointer_segment(child);
    if parent.is_empty() {
        child
    } else {
        format!("{parent}/{child}")
    }
}

fn escape_json_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn unescape_json_pointer_segment(segment: &str) -> String {
    let mut unescaped = String::new();
    let mut chars = segment.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '~' {
            unescaped.push(character);
            continue;
        }

        match chars.peek() {
            Some('0') => {
                chars.next();
                unescaped.push('~');
            }
            Some('1') => {
                chars.next();
                unescaped.push('/');
            }
            _ => unescaped.push('~'),
        }
    }
    unescaped
}

fn redact_secret_like_text(message: &str, api_key: &str) -> String {
    let mut redacted = if api_key.trim().is_empty() {
        message.to_string()
    } else {
        message.replace(api_key, "[redacted]")
    };

    for token in message.split_whitespace() {
        let trimmed = token.trim_matches(|character: char| {
            matches!(
                character,
                '"' | '\'' | ',' | ';' | ':' | ')' | '(' | '[' | ']'
            )
        });
        if trimmed.starts_with("sk-") && trimmed.len() > 8 {
            redacted = redacted.replace(trimmed, "[redacted]");
        }
    }

    const MAX_ERROR_MESSAGE_LEN: usize = 2048;
    if redacted.len() > MAX_ERROR_MESSAGE_LEN {
        redacted.truncate(MAX_ERROR_MESSAGE_LEN);
        redacted.push_str("...");
    }

    redacted
}
