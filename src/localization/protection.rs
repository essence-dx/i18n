use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

const TOKEN_PREFIX_BASE: &str = "<<<DX_I18N_PROTECTED_";
const TOKEN_SUFFIX: &str = ">>>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtectionError {
    MissingToken(String),
    DuplicateToken(String),
    UnexpectedToken(String),
}

impl fmt::Display for ProtectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingToken(token) => write!(f, "protected token was not returned: {token}"),
            Self::DuplicateToken(token) => {
                write!(f, "protected token was returned more than once: {token}")
            }
            Self::UnexpectedToken(token) => {
                write!(f, "unexpected protected token was returned: {token}")
            }
        }
    }
}

impl Error for ProtectionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedText {
    translatable_text: String,
    segments: Vec<ProtectedSegment>,
}

impl ProtectedText {
    pub fn protect(input: &str) -> Result<Self, ProtectionError> {
        let mut translatable_text = String::new();
        let mut segments = Vec::new();
        let mut cursor = 0;
        let mut literal_start = 0;
        let token_prefix = token_prefix_for(input);

        while cursor < input.len() {
            if let Some(end) = protected_span_end(input, cursor) {
                translatable_text.push_str(&input[literal_start..cursor]);

                let token = format!("{token_prefix}{}{TOKEN_SUFFIX}", segments.len());
                translatable_text.push_str(&token);
                segments.push(ProtectedSegment {
                    token,
                    original: input[cursor..end].to_string(),
                });

                cursor = end;
                literal_start = cursor;
                continue;
            }

            cursor += input[cursor..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        }

        translatable_text.push_str(&input[literal_start..]);

        Ok(Self {
            translatable_text,
            segments,
        })
    }

    pub fn translatable_text(&self) -> &str {
        &self.translatable_text
    }

    pub fn protected_segments(&self) -> impl Iterator<Item = &str> {
        self.segments
            .iter()
            .map(|segment| segment.original.as_str())
    }

    pub fn restore(&self, translated_text: &str) -> Result<String, ProtectionError> {
        let mut restored = translated_text.to_string();
        let expected_tokens = self
            .segments
            .iter()
            .map(|segment| segment.token.as_str())
            .collect::<BTreeSet<_>>();

        for token in protected_token_literals(translated_text) {
            if !expected_tokens.contains(token.as_str()) {
                return Err(ProtectionError::UnexpectedToken(token));
            }
        }

        for segment in &self.segments {
            match restored.matches(&segment.token).count() {
                0 => return Err(ProtectionError::MissingToken(segment.token.clone())),
                1 => {}
                _ => return Err(ProtectionError::DuplicateToken(segment.token.clone())),
            }

            restored = restored.replace(&segment.token, &segment.original);
        }

        Ok(restored)
    }
}

pub fn preserves_protected_tokens(source: &str, candidate: &str) -> bool {
    protected_token_counts(source) == protected_token_counts(candidate)
        && markdown_link_signature(source) == markdown_link_signature(candidate)
        && markdown_emphasis_signature(source) == markdown_emphasis_signature(candidate)
        && markdown_table_signature(source) == markdown_table_signature(candidate)
        && html_tag_signature(source) == html_tag_signature(candidate)
}

fn protected_token_counts(input: &str) -> BTreeMap<String, usize> {
    ProtectedText::protect(input)
        .map(|protected| {
            let mut counts = BTreeMap::new();
            for segment in protected.protected_segments() {
                *counts
                    .entry(canonical_protected_segment(segment))
                    .or_default() += 1;
            }
            counts
        })
        .unwrap_or_default()
}

fn canonical_protected_segment(segment: &str) -> String {
    icu_signature(segment).unwrap_or_else(|| segment.to_string())
}

fn icu_signature(segment: &str) -> Option<String> {
    let inner = segment.strip_prefix('{')?.strip_suffix('}')?;
    let mut parts = inner.splitn(3, ',');
    let variable = parts.next()?.trim();
    let format = parts.next()?.trim();
    let choices = parts.next()?.trim();
    if !is_identifier_like(variable) || !is_identifier_like(format) {
        return None;
    }

    let choices = icu_choice_signatures(choices)?;
    if choices.is_empty() {
        return None;
    }

    Some(format!("icu:{variable}:{format}:{}", choices.join("|")))
}

fn icu_choice_signatures(choices: &str) -> Option<Vec<String>> {
    let mut signatures = Vec::new();
    let mut branch_count = 0usize;
    let mut cursor = 0usize;

    while cursor < choices.len() {
        cursor = skip_ascii_whitespace(choices, cursor);
        if cursor >= choices.len() {
            break;
        }

        let category_start = cursor;
        while cursor < choices.len() {
            let character = choices[cursor..].chars().next()?;
            if character.is_whitespace() || character == '{' {
                break;
            }
            cursor += character.len_utf8();
        }

        let category = choices[category_start..cursor].trim();
        if category.is_empty() {
            return None;
        }

        cursor = skip_ascii_whitespace(choices, cursor);
        if !choices[cursor..].starts_with('{') {
            if is_icu_choice_option(category) {
                signatures.push(format!("option:{category}"));
                continue;
            }
            return None;
        }

        let body_start = cursor + '{'.len_utf8();
        let body_end = matching_brace_end(choices, cursor)?;
        let body = &choices[body_start..body_end - '}'.len_utf8()];
        branch_count += 1;
        signatures.push(format!(
            "{}:#{}:tokens={:?}:links={:?}:emphasis={:?}:tables={:?}:html={:?}",
            category,
            count_unescaped_icu_pound_symbols(body),
            protected_token_counts(body),
            markdown_link_signature(body),
            markdown_emphasis_signature(body),
            markdown_table_signature(body),
            html_tag_signature(body)
        ));
        cursor = body_end;
    }

    (branch_count > 0).then_some(signatures)
}

fn is_icu_choice_option(category: &str) -> bool {
    let Some(offset) = category.strip_prefix("offset:") else {
        return false;
    };

    !offset.is_empty() && offset.chars().all(|character| character.is_ascii_digit())
}

fn skip_ascii_whitespace(input: &str, mut cursor: usize) -> usize {
    while cursor < input.len() {
        let Some(character) = input[cursor..].chars().next() else {
            break;
        };
        if !character.is_ascii_whitespace() {
            break;
        }
        cursor += character.len_utf8();
    }
    cursor
}

fn matching_brace_end(input: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (relative, character) in input[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + relative + character.len_utf8());
                }
            }
            _ => {}
        }
    }

    None
}

fn count_unescaped_icu_pound_symbols(input: &str) -> usize {
    let mut count = 0usize;
    let mut in_quoted_literal = false;
    let mut chars = input.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                } else {
                    in_quoted_literal = !in_quoted_literal;
                }
            }
            '#' if !in_quoted_literal => count += 1,
            _ => {}
        }
    }

    count
}

fn markdown_link_signature(input: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut cursor = 0usize;

    while let Some(open_relative) = input[cursor..].find('[') {
        let open = cursor + open_relative;
        let Some(label_close_relative) = input[open..].find("](") else {
            cursor = open + '['.len_utf8();
            continue;
        };
        let destination_start = open + label_close_relative + "](".len();
        let Some(destination_end) = markdown_link_destination_end(input, destination_start) else {
            cursor = destination_start;
            continue;
        };
        let kind = if open > 0 && input[..open].ends_with('!') {
            "image"
        } else {
            "link"
        };
        links.push(format!(
            "{}:{}",
            kind,
            canonical_markdown_link_destination(&input[destination_start..destination_end],)
        ));
        cursor = destination_end + ')'.len_utf8();
    }

    links.extend(markdown_reference_usage_signature(input));
    links.extend(markdown_reference_link_signature(input));
    links
}

fn markdown_reference_usage_signature(input: &str) -> Vec<String> {
    let mut references = Vec::new();
    let defined_labels = markdown_reference_link_labels(input);
    let mut cursor = 0usize;

    while let Some(open_relative) = input[cursor..].find('[') {
        let open = cursor + open_relative;
        if open > 0 && input[..open].ends_with('!') {
            cursor = open + '['.len_utf8();
            continue;
        }
        let Some(label_end_relative) = input[open + '['.len_utf8()..].find(']') else {
            break;
        };
        let label_end = open + '['.len_utf8() + label_end_relative;
        let label = &input[open + '['.len_utf8()..label_end];
        let after_label = label_end + ']'.len_utf8();
        if input[after_label..].starts_with(':') || input[after_label..].starts_with('(') {
            cursor = after_label;
            continue;
        }
        let Some(reference_start) = input[after_label..].strip_prefix('[') else {
            let label = label.trim().to_ascii_lowercase();
            if defined_labels.contains(&label) {
                references.push(format!("reference-use:{}", label));
            }
            cursor = after_label;
            continue;
        };
        let Some(reference_end_relative) = reference_start.find(']') else {
            cursor = after_label + '['.len_utf8();
            continue;
        };
        let reference = &reference_start[..reference_end_relative];
        let reference = if reference.trim().is_empty() {
            label.trim()
        } else {
            reference.trim()
        };
        if !reference.is_empty() {
            references.push(format!("reference-use:{}", reference.to_ascii_lowercase()));
        }
        cursor = after_label + '['.len_utf8() + reference_end_relative + ']'.len_utf8();
    }

    references
}

fn markdown_reference_link_labels(input: &str) -> BTreeSet<String> {
    input
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with('[') {
                return None;
            }
            let label_end = trimmed.find("]:")?;
            let label = trimmed[1..label_end].trim();
            (!label.is_empty()).then(|| label.to_ascii_lowercase())
        })
        .collect()
}

fn markdown_reference_link_signature(input: &str) -> Vec<String> {
    let mut references = Vec::new();

    for line in input.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('[') {
            continue;
        }
        let Some(label_end) = trimmed.find("]:") else {
            continue;
        };
        let label = trimmed[1..label_end].trim();
        if label.is_empty() {
            continue;
        }
        let destination = trimmed[label_end + "]:".len()..].trim_start();
        let Some(destination) = markdown_reference_destination(destination) else {
            continue;
        };
        references.push(format!(
            "reference:{}:{}",
            label.to_ascii_lowercase(),
            canonical_markdown_link_destination(destination)
        ));
    }

    references
}

fn markdown_reference_destination(input: &str) -> Option<&str> {
    if let Some(rest) = input.strip_prefix('<') {
        return rest.find('>').map(|end| &rest[..end]);
    }

    input.split_whitespace().next()
}

fn markdown_link_destination_end(input: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut escaped = false;

    for (relative, character) in input[start..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        match character {
            '\\' => escaped = true,
            '(' => depth += 1,
            ')' if depth == 0 => return Some(start + relative),
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    None
}

fn canonical_markdown_link_destination(destination: &str) -> String {
    let trimmed = destination.trim();
    protect_structural_text(trimmed)
}

fn markdown_emphasis_signature(input: &str) -> BTreeMap<&'static str, usize> {
    BTreeMap::from([
        ("**", non_overlapping_occurrences(input, "**")),
        ("__", non_overlapping_occurrences(input, "__")),
        ("*", non_overlapping_occurrences(input, "*")),
        ("_", non_overlapping_occurrences(input, "_")),
        ("~~", non_overlapping_occurrences(input, "~~")),
    ])
}

fn markdown_table_signature(input: &str) -> Vec<String> {
    input
        .lines()
        .filter_map(|line| {
            let pipe_count = line.chars().filter(|character| *character == '|').count();
            (pipe_count >= 2).then(|| markdown_table_row_signature(line))
        })
        .collect()
}

fn markdown_table_row_signature(line: &str) -> String {
    let cell_signatures = line
        .trim()
        .trim_matches('|')
        .split('|')
        .map(|cell| markdown_table_cell_signature(cell.trim()))
        .collect::<Vec<_>>();

    format!(
        "cells={}:cell_signatures={cell_signatures:?}",
        cell_signatures.len()
    )
}

fn markdown_table_cell_signature(cell: &str) -> String {
    format!(
        "tokens={:?}:links={:?}:emphasis={:?}:html={:?}",
        protected_token_counts(cell),
        markdown_link_signature(cell),
        markdown_emphasis_signature(cell),
        html_tag_signature(cell)
    )
}

fn html_tag_signature(input: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut cursor = 0usize;

    while let Some(open_relative) = input[cursor..].find('<') {
        let open = cursor + open_relative;
        let Some(close) = html_tag_end(input, open) else {
            break;
        };
        if let Some(tag) = canonical_html_tag(&input[open + '<'.len_utf8()..close]) {
            tags.push(tag);
        }
        cursor = close + '>'.len_utf8();
    }

    tags
}

fn html_tag_end(input: &str, open: usize) -> Option<usize> {
    let mut quote: Option<char> = None;

    for (relative, character) in input[open + '<'.len_utf8()..].char_indices() {
        match (quote, character) {
            (Some(active), current) if current == active => quote = None,
            (None, '"' | '\'') => quote = Some(character),
            (None, '>') => return Some(open + '<'.len_utf8() + relative),
            _ => {}
        }
    }

    None
}

fn canonical_html_tag(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with('!') || raw.starts_with('?') {
        return None;
    }

    let (kind, name_start) = if raw.starts_with('/') {
        ("close", 1)
    } else {
        ("open", 0)
    };
    let name = raw[name_start..]
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':')
        })
        .collect::<String>()
        .to_ascii_lowercase();

    if name.is_empty() {
        return None;
    }

    let attributes = canonical_html_attributes(&raw[name_start + name.len()..]);
    if attributes.is_empty() {
        Some(format!("{kind}:{name}"))
    } else {
        Some(format!("{kind}:{name}:{attributes:?}"))
    }
}

fn canonical_html_attributes(raw: &str) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let mut cursor = 0usize;

    while cursor < raw.len() {
        cursor = skip_ascii_whitespace(raw, cursor);
        while raw[cursor..].starts_with('/') {
            cursor += '/'.len_utf8();
            cursor = skip_ascii_whitespace(raw, cursor);
        }
        if cursor >= raw.len() {
            break;
        }

        let name_start = cursor;
        while cursor < raw.len() {
            let Some(character) = raw[cursor..].chars().next() else {
                break;
            };
            if character.is_whitespace() || matches!(character, '=' | '/') {
                break;
            }
            cursor += character.len_utf8();
        }

        let name = raw[name_start..cursor].to_ascii_lowercase();
        cursor = skip_ascii_whitespace(raw, cursor);
        let value = if raw[cursor..].starts_with('=') {
            cursor += '='.len_utf8();
            cursor = skip_ascii_whitespace(raw, cursor);
            let Some(character) = raw[cursor..].chars().next() else {
                attributes.insert(name, String::new());
                break;
            };
            if matches!(character, '"' | '\'') {
                cursor += character.len_utf8();
                let value_start = cursor;
                if let Some(relative_end) = raw[cursor..].find(character) {
                    cursor += relative_end;
                    let value = protect_structural_text(&raw[value_start..cursor]);
                    cursor += character.len_utf8();
                    value
                } else {
                    protect_structural_text(&raw[value_start..])
                }
            } else {
                let value_start = cursor;
                while cursor < raw.len() {
                    let Some(character) = raw[cursor..].chars().next() else {
                        break;
                    };
                    if character.is_whitespace() || character == '/' {
                        break;
                    }
                    cursor += character.len_utf8();
                }
                protect_structural_text(&raw[value_start..cursor])
            }
        } else {
            String::new()
        };

        if !name.is_empty() {
            attributes.insert(name, value);
        }
    }

    attributes
}

fn protect_structural_text(input: &str) -> String {
    ProtectedText::protect(input)
        .map(|protected| {
            protected.segments.iter().fold(
                protected.translatable_text().to_string(),
                |text, segment| {
                    text.replace(
                        &segment.token,
                        &canonical_protected_segment(&segment.original),
                    )
                },
            )
        })
        .unwrap_or_else(|_| input.to_string())
}

fn non_overlapping_occurrences(input: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }

    let mut count = 0usize;
    let mut cursor = 0usize;
    while let Some(relative) = input[cursor..].find(needle) {
        count += 1;
        cursor += relative + needle.len();
    }
    count
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtectedSegment {
    token: String,
    original: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleKeyStructure {
    keys: BTreeSet<String>,
}

impl LocaleKeyStructure {
    pub fn from_keys<I, K>(keys: I) -> Self
    where
        I: IntoIterator<Item = K>,
        K: Into<String>,
    {
        Self {
            keys: keys.into_iter().map(Into::into).collect(),
        }
    }

    pub fn compare(&self, candidate: &Self) -> LocaleKeyDiff {
        LocaleKeyDiff {
            missing_keys: self.keys.difference(&candidate.keys).cloned().collect(),
            extra_keys: candidate.keys.difference(&self.keys).cloned().collect(),
        }
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleKeyDiff {
    missing_keys: Vec<String>,
    extra_keys: Vec<String>,
}

impl LocaleKeyDiff {
    pub fn is_match(&self) -> bool {
        self.missing_keys.is_empty() && self.extra_keys.is_empty()
    }

    pub fn missing_keys(&self) -> &[String] {
        &self.missing_keys
    }

    pub fn extra_keys(&self) -> &[String] {
        &self.extra_keys
    }
}

fn protected_span_end(input: &str, start: usize) -> Option<usize> {
    if let Some(marker) = markdown_fence_marker_at(input, start) {
        return Some(fenced_code_end(input, start, &marker));
    }

    let backtick_count = repeated_char_count(input, start, '`');
    if backtick_count > 0 {
        return inline_code_end(input, start, backtick_count);
    }

    if input[start..].starts_with("{{") {
        return double_brace_placeholder_end(input, start);
    }

    if input[start..].starts_with('{') {
        return placeholder_end(input, start);
    }

    None
}

fn token_prefix_for(input: &str) -> String {
    for namespace in 0.. {
        let prefix = format!("{TOKEN_PREFIX_BASE}{namespace}_");
        if !input.contains(&prefix) {
            return prefix;
        }
    }

    unreachable!("usize namespace is effectively unbounded for a single text")
}

fn protected_token_literals(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cursor = 0usize;

    while let Some(relative_start) = input[cursor..].find(TOKEN_PREFIX_BASE) {
        let start = cursor + relative_start;
        let Some(relative_end) = input[start..].find(TOKEN_SUFFIX) else {
            break;
        };
        let end = start + relative_end + TOKEN_SUFFIX.len();
        let token = &input[start..end];
        if is_protected_token_literal(token) {
            tokens.push(token.to_string());
        }
        cursor = end;
    }

    tokens
}

fn is_protected_token_literal(token: &str) -> bool {
    let Some(body) = token
        .strip_prefix(TOKEN_PREFIX_BASE)
        .and_then(|value| value.strip_suffix(TOKEN_SUFFIX))
    else {
        return false;
    };
    let Some((namespace, index)) = body.split_once('_') else {
        return false;
    };

    !namespace.is_empty()
        && !index.is_empty()
        && namespace
            .chars()
            .all(|character| character.is_ascii_digit())
        && index.chars().all(|character| character.is_ascii_digit())
}

fn markdown_fence_marker_at(input: &str, start: usize) -> Option<String> {
    let line_start = input[..start]
        .rfind('\n')
        .map(|position| position + 1)
        .unwrap_or(0);
    if !input[line_start..start].trim().is_empty() {
        return None;
    }

    let marker_char = match input[start..].chars().next()? {
        '`' => '`',
        '~' => '~',
        _ => return None,
    };
    let marker_len = repeated_char_count(input, start, marker_char);
    (marker_len >= 3).then(|| marker_char.to_string().repeat(marker_len))
}

fn fenced_code_end(input: &str, start: usize, marker: &str) -> usize {
    let marker_char = marker
        .chars()
        .next()
        .expect("fenced code marker should not be empty");
    let after_opening = start + marker.len();
    let search_start = input[after_opening..]
        .find('\n')
        .map(|offset| after_opening + offset + 1)
        .unwrap_or(after_opening);

    let mut search = search_start;
    while let Some(relative) = input[search..].find(marker) {
        let marker_start = search + relative;
        let marker_len = repeated_char_count(input, marker_start, marker_char);
        let marker_end = marker_start + marker_len;
        let line_start = input[..marker_start]
            .rfind('\n')
            .map(|position| position + 1)
            .unwrap_or(0);

        let line_after_marker = input[marker_end..]
            .split_once('\n')
            .map(|(line, _)| line)
            .unwrap_or(&input[marker_end..]);

        if marker_len >= marker.len()
            && input[line_start..marker_start].trim().is_empty()
            && line_after_marker.trim().is_empty()
        {
            return marker_end;
        }

        search = marker_end;
    }

    input.len()
}

fn inline_code_end(input: &str, start: usize, backtick_count: usize) -> Option<usize> {
    let marker = "`".repeat(backtick_count);
    let search_start = start + backtick_count;
    input[search_start..]
        .find(&marker)
        .map(|relative| search_start + relative + backtick_count)
}

fn double_brace_placeholder_end(input: &str, start: usize) -> Option<usize> {
    let inner_start = start + "{{".len();
    let relative_end = input[inner_start..].find("}}")?;
    let inner_end = inner_start + relative_end;
    let inner = input[inner_start..inner_end].trim();

    is_double_brace_template_like(inner).then_some(inner_end + "}}".len())
}

fn is_double_brace_template_like(inner: &str) -> bool {
    !inner.is_empty()
        && !inner.contains('\n')
        && !inner.contains('\r')
        && !inner.contains("{{")
        && !inner.contains("}}")
        && inner
            .chars()
            .any(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn placeholder_end(input: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;

    for (relative, character) in input[start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let end = start + relative + character.len_utf8();
                    return is_placeholder_like(&input[start..end]).then_some(end);
                }
            }
            _ => {}
        }
    }

    None
}

fn is_placeholder_like(candidate: &str) -> bool {
    let inner = candidate
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .map(str::trim)
        .unwrap_or_default();

    let identifier = inner
        .split(|character: char| character == ',' || character.is_whitespace())
        .next()
        .unwrap_or_default();

    is_identifier_like(identifier)
}

fn is_identifier_like(identifier: &str) -> bool {
    !identifier.is_empty()
        && identifier.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn repeated_char_count(input: &str, start: usize, expected: char) -> usize {
    input[start..]
        .chars()
        .take_while(|character| *character == expected)
        .count()
}

#[cfg(test)]
mod tests {
    use super::{LocaleKeyStructure, ProtectedText, TOKEN_PREFIX_BASE, preserves_protected_tokens};

    #[test]
    fn restores_placeholders_markdown_code_and_nested_icu_blocks() {
        let original = "Hello {name}, run `dx build`, then:\n```rust\nfn main() {}\n```\n{count, plural, one {# file} other {# files}}";
        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert!(!protected.translatable_text().contains("{name}"));
        assert!(!protected.translatable_text().contains("`dx build`"));
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

    #[test]
    fn reports_missing_and_extra_locale_keys_without_accepting_shape_drift() {
        let source = LocaleKeyStructure::from_keys(["nav/title", "nav/subtitle", "items/0"]);
        let candidate = LocaleKeyStructure::from_keys(["nav/title", "items/0", "items/1"]);

        let diff = source.compare(&candidate);

        assert!(!diff.is_match());
        assert_eq!(diff.missing_keys(), ["nav/subtitle"]);
        assert_eq!(diff.extra_keys(), ["items/1"]);
    }

    #[test]
    fn protects_double_brace_interpolation_tokens() {
        let original = "You have {{count}} tasks for {{productName}}.";
        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert!(!protected.translatable_text().contains("{{count}}"));
        assert!(!protected.translatable_text().contains("{{productName}}"));

        let translated = protected
            .translatable_text()
            .replace("You have", "Tienes")
            .replace("tasks for", "tareas para");

        assert_eq!(
            protected
                .restore(&translated)
                .expect("tokens should restore"),
            "Tienes {{count}} tareas para {{productName}}."
        );
    }

    #[test]
    fn restore_preserves_literal_sentinel_text() {
        let original = "Literal <<<DX_I18N_PROTECTED_0>>> plus {name}.";
        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert_eq!(
            protected
                .restore(protected.translatable_text())
                .expect("tokens should restore"),
            original
        );
    }

    #[test]
    fn restore_rejects_duplicate_protected_tokens() {
        let protected = ProtectedText::protect("Hello {name}").expect("text should be protected");
        let token = protected
            .translatable_text()
            .split_whitespace()
            .find(|part| part.starts_with(TOKEN_PREFIX_BASE))
            .expect("protected text should contain a sentinel token");

        let error = protected
            .restore(&format!("Hola {token} {token}"))
            .expect_err("duplicated protected tokens should be rejected");

        assert!(error.to_string().contains("more than once"));
    }

    #[test]
    fn restore_rejects_unexpected_same_namespace_sentinel_tokens() {
        let protected = ProtectedText::protect("Hello {name}").expect("text should be protected");
        let translated = format!(
            "{} <<<DX_I18N_PROTECTED_0_999>>>",
            protected.translatable_text().replace("Hello", "Hola")
        );

        protected
            .restore(&translated)
            .expect_err("unexpected protected sentinels should be rejected");
    }

    #[test]
    fn inline_triple_backticks_do_not_consume_following_text_as_fence() {
        let original = "Use ```code``` then translate this {name}.";
        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert!(
            protected
                .translatable_text()
                .contains("then translate this")
        );
        assert!(!protected.translatable_text().contains("{name}"));
    }

    #[test]
    fn protects_entire_fence_when_code_line_starts_with_same_marker_text() {
        let original = "Before\n```md\n```literal\nStill code {name}\n```\nAfter {name}\n";
        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert!(!protected.translatable_text().contains("Still code"));
        assert!(protected.translatable_text().contains("After"));
    }

    #[test]
    fn protects_entire_longer_markdown_fence() {
        let original = "Before\n````md\n```literal\nStill code {name}\n````\nAfter {name}\n";
        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert!(!protected.translatable_text().contains("```literal"));
        assert!(!protected.translatable_text().contains("Still code"));
        assert!(protected.translatable_text().contains("After"));
    }

    #[test]
    fn protects_complex_double_brace_template_expressions() {
        let original = r#"Hello {{ user.name | default: "Guest" }} from {{ product_name }}."#;
        let protected = ProtectedText::protect(original).expect("text should be protected");
        let segments = protected.protected_segments().collect::<Vec<_>>();

        assert!(segments.contains(&r#"{{ user.name | default: "Guest" }}"#));
        assert!(segments.contains(&"{{ product_name }}"));
        assert!(!protected.translatable_text().contains("user.name"));
        assert!(!protected.translatable_text().contains("product_name"));
    }

    #[test]
    fn compares_placeholder_code_and_icu_token_sets() {
        assert!(preserves_protected_tokens(
            "Hello {name}, run `dx build`.",
            "Hola {name}, ejecuta `dx build`."
        ));
        assert!(!preserves_protected_tokens(
            "Hello {name}, run `dx build`.",
            "Hola, ejecuta `dx build`."
        ));
        assert!(!preserves_protected_tokens(
            "Hello {name}.",
            "Hola {name} {unexpected}."
        ));
        assert!(!preserves_protected_tokens(
            "{count, plural, one {# file} other {# files}}",
            "{count, plural, one {# archivo}}"
        ));
        assert!(preserves_protected_tokens(
            "{count, plural, one {# file} other {# files}}",
            "{count, plural, one {# archivo} other {# archivos}}"
        ));
        assert!(preserves_protected_tokens(
            "{count, plural, offset:1 one {# file} other {# files}}",
            "{count, plural, offset:1 one {# archivo} other {# archivos}}"
        ));
        assert!(!preserves_protected_tokens(
            "{count, plural, offset:1 one {# file} other {# files}}",
            "{count, plural, one {# archivo} other {# archivos}}"
        ));
    }

    #[test]
    fn rejects_icu_branch_marker_drift() {
        assert!(!preserves_protected_tokens(
            "{count, plural, one {# file} other {# files}}",
            "{count, plural, one {archivo} other {archivos}}"
        ));
        assert!(!preserves_protected_tokens(
            "{count, plural, one {# file} other {# files}}",
            "{count, plural, one {'#' archivo} other {'#' archivos}}"
        ));
    }

    #[test]
    fn preserves_icu_plural_offset_exact_selector_structure() {
        let source = "{count, plural, offset:1 =0 {No guests} one {# guest} other {# guests}}";

        assert!(preserves_protected_tokens(
            source,
            "{count, plural, offset:1 =0 {Sin invitados} one {# invitado} other {# invitados}}"
        ));
        assert!(!preserves_protected_tokens(
            source,
            "{count, plural, offset:0 =0 {Sin invitados} one {# invitado} other {# invitados}}"
        ));
        assert!(!preserves_protected_tokens(
            source,
            "{count, plural, offset:1 one {# invitado} other {# invitados}}"
        ));
        assert!(!preserves_protected_tokens(
            source,
            "{count, plural, offset:1 =0 {Sin invitados} one {invitado} other {invitados}}"
        ));
    }

    #[test]
    fn rejects_markdown_and_html_structure_drift() {
        assert!(preserves_protected_tokens(
            "Visit [dashboard]({url}).",
            "Visita [panel]({url})."
        ));
        assert!(!preserves_protected_tokens(
            "Visit [dashboard]({url}).",
            "Visita panel ({url})."
        ));
        assert!(!preserves_protected_tokens(
            "Open [profile](/users/{user_id}/settings).",
            "Abre [perfil](/usuarios/{user_id}/settings)."
        ));
        assert!(!preserves_protected_tokens(
            "Open [profile](https://dx.local/users/(active)?tab=overview).",
            "Abre [perfil](https://dx.local/users/(active)?tab=settings)."
        ));
        assert!(!preserves_protected_tokens(
            "Open [team](/users/{user_id}/teams/{team_id}).",
            "Abre [equipo](/users/{team_id}/teams/{user_id})."
        ));
        assert!(preserves_protected_tokens(
            "Visit [dashboard][main].\n\n[main]: /docs/{locale}/dashboard",
            "Visita [panel][main].\n\n[main]: /docs/{locale}/dashboard"
        ));
        assert!(!preserves_protected_tokens(
            "Visit [dashboard][main].\n\n[main]: /docs/{locale}/dashboard",
            "Visita el panel.\n\n[main]: /docs/{locale}/dashboard"
        ));
        assert!(!preserves_protected_tokens(
            "Visit [dashboard][main].\n\n[main]: /docs/{locale}/dashboard",
            "Visita [panel][main].\n\n[main]: /help/{locale}/dashboard"
        ));
        assert!(!preserves_protected_tokens(
            "See [guide] before release.\n\n[guide]: /docs/{locale}/guide",
            "Consulta la guia antes del lanzamiento.\n\n[guide]: /docs/{locale}/guide"
        ));
        assert!(!preserves_protected_tokens(
            "Keep **{label}** visible.",
            "Keep {label} visible."
        ));
        assert!(!preserves_protected_tokens(
            "Keep *{label}* visible.",
            "Keep {label} visible."
        ));
        assert!(!preserves_protected_tokens(
            "Logo ![alt]({image_url}).",
            "Logo [alt]({image_url})."
        ));
        assert!(!preserves_protected_tokens(
            "<strong>{label}</strong>",
            "<b>{label}</b>"
        ));
        assert!(!preserves_protected_tokens(
            r#"<a href="/users/{user_id}" data-route="profile">Profile</a>"#,
            r#"<a href="/usuarios/{user_id}" data-route="perfil">Perfil</a>"#
        ));
        assert!(!preserves_protected_tokens(
            r#"<a href="/users/{user_id}/teams/{team_id}">Team</a>"#,
            r#"<a href="/users/{team_id}/teams/{user_id}">Equipo</a>"#
        ));
        assert!(!preserves_protected_tokens(
            r#"<a title="2 > 1" href="/users/{user_id}">Profile</a>"#,
            r#"<a title="2 > 1" href="/usuarios/{user_id}">Perfil</a>"#
        ));
        assert!(!preserves_protected_tokens(
            "| Action | Notes |\n| --- | --- |\n| [Open](/admin) | Plain |",
            "| Accion | Notas |\n| --- | --- |\n| Plain | [Open](/admin) |"
        ));
        assert!(!preserves_protected_tokens(
            "| Action | Notes |\n| --- | --- |\n| <kbd>Enter</kbd> | Plain |",
            "| Accion | Notas |\n| --- | --- |\n| Plain | <kbd>Enter</kbd> |"
        ));
        assert!(!preserves_protected_tokens(
            "| Action | Notes |\n| --- | --- |\n| **Open** | Plain |",
            "| Accion | Notas |\n| --- | --- |\n| Plain | **Open** |"
        ));
        assert!(!preserves_protected_tokens(
            "| User | Team |\n| --- | --- |\n| {user_id} | {team_id} |",
            "| Usuario | Equipo |\n| --- | --- |\n| {team_id} | {user_id} |"
        ));
    }

    #[test]
    fn protects_inline_code_spans_with_longer_backtick_runs() {
        let original = "Keep ````literal ` code```` stable for {name}.";

        let protected = ProtectedText::protect(original).expect("text should be protected");

        assert!(
            protected
                .protected_segments()
                .any(|segment| segment == "````literal ` code````")
        );
        assert!(!protected.translatable_text().contains("````"));
        assert!(!protected.translatable_text().contains("literal ` code"));
        assert!(!protected.translatable_text().contains("{name}"));
        assert_eq!(
            protected
                .restore(protected.translatable_text())
                .expect("protected text should restore"),
            original
        );
    }
}
