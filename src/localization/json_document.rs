use crate::error::{I18nError, Result};
use crate::localization::{TranslationPathSegment, TranslationUnit};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct JsonLocalizationDocument {
    value: Value,
}

impl JsonLocalizationDocument {
    pub fn new(value: Value) -> Self {
        Self { value }
    }

    pub fn source_units(&self) -> Vec<TranslationUnit> {
        let mut units = Vec::new();
        collect_units(&self.value, "", &mut Vec::new(), &mut units);
        units
    }

    pub fn apply_translations(
        &self,
        translations: &BTreeMap<String, String>,
    ) -> Result<JsonLocalizationDocument> {
        let mut value = self.value.clone();
        apply_to_value(&mut value, "", translations)?;
        Ok(Self { value })
    }

    pub fn into_value(self) -> Value {
        self.value
    }
}

fn collect_units(
    value: &Value,
    path: &str,
    segments: &mut Vec<TranslationPathSegment>,
    units: &mut Vec<TranslationUnit>,
) {
    match value {
        Value::String(text) => units.push(TranslationUnit::with_path_segments(
            path,
            text,
            segments.clone(),
        )),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                let child_path = join_path(path, &index.to_string());
                segments.push(TranslationPathSegment::ArrayIndex(index));
                collect_units(item, &child_path, segments, units);
                segments.pop();
            }
        }
        Value::Object(map) => collect_object_units(map, path, segments, units),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn collect_object_units(
    map: &Map<String, Value>,
    path: &str,
    segments: &mut Vec<TranslationPathSegment>,
    units: &mut Vec<TranslationUnit>,
) {
    let mut keys = map.keys().collect::<Vec<_>>();
    keys.sort();

    for key in keys {
        let child_path = join_path(path, key);
        segments.push(TranslationPathSegment::ObjectKey(key.clone()));
        collect_units(&map[key], &child_path, segments, units);
        segments.pop();
    }
}

fn apply_to_value(
    value: &mut Value,
    path: &str,
    translations: &BTreeMap<String, String>,
) -> Result<()> {
    match value {
        Value::String(text) => {
            if let Some(translated) = translations.get(path) {
                *text = translated.clone();
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let child_path = join_path(path, &index.to_string());
                apply_to_value(item, &child_path, translations)?;
            }
        }
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                let child_path = join_path(path, key);
                apply_to_value(item, &child_path, translations)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }

    Ok(())
}

fn join_path(parent: &str, child: &str) -> String {
    parent_joined(parent, &escape_json_pointer_segment(child))
}

fn parent_joined(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}/{child}")
    }
}

fn escape_json_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

pub fn ensure_json_object(value: Value) -> Result<Value> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(I18nError::ConfigError(
            "JSON localization document must be an object".to_string(),
        ))
    }
}
