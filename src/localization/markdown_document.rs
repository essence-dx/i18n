use crate::localization::TranslationUnit;
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownLocalizationDocument {
    source: String,
    canonical_path: String,
}

impl MarkdownLocalizationDocument {
    pub fn new(source: impl Into<String>, canonical_path: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            canonical_path: canonical_path.into(),
        }
    }

    pub fn source_units(&self) -> Vec<TranslationUnit> {
        let mut units = Vec::new();
        let body = self.collect_frontmatter_units(&mut units);
        self.collect_body_units(body, &mut units);
        units
    }

    pub fn apply_translations(&self, translations: &BTreeMap<String, String>) -> String {
        let mut rendered = String::new();
        let mut body = self.source.as_str();
        let line_ending = self.line_ending();

        if let Some((frontmatter, remaining_body)) = self.frontmatter_parts() {
            rendered.push_str("---");
            rendered.push_str(line_ending);
            for line in frontmatter.lines() {
                rendered.push_str(&self.render_frontmatter_line(line, translations));
                rendered.push_str(line_ending);
            }
            rendered.push_str("---");
            rendered.push_str(line_ending);
            body = remaining_body;
        }

        rendered.push_str(&self.render_body(body, translations, line_ending));
        if self.source.ends_with('\n') && !rendered.ends_with('\n') {
            rendered.push_str(line_ending);
        }

        rendered
    }

    fn collect_frontmatter_units<'a>(&'a self, units: &mut Vec<TranslationUnit>) -> &'a str {
        let Some((frontmatter, body)) = self.frontmatter_parts() else {
            return &self.source;
        };

        for line in frontmatter.lines() {
            let Some((key, value)) = line.split_once(": ") else {
                continue;
            };
            if key.trim() != "title" {
                continue;
            }

            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                units.push(TranslationUnit::new(
                    self.key("frontmatter/title"),
                    value.to_string(),
                ));
            }
        }

        body
    }

    fn frontmatter_parts(&self) -> Option<(&str, &str)> {
        if let Some(rest) = self.source.strip_prefix("---\r\n") {
            return rest.split_once("\r\n---\r\n");
        }

        let rest = self.source.strip_prefix("---\n")?;
        rest.split_once("\n---\n")
    }

    fn render_frontmatter_line(
        &self,
        line: &str,
        translations: &BTreeMap<String, String>,
    ) -> String {
        let Some((key, value)) = line.split_once(": ") else {
            return line.to_string();
        };
        if key.trim() != "title" {
            return line.to_string();
        }
        let Some(translated) = translations.get(&self.key("frontmatter/title")) else {
            return line.to_string();
        };

        if frontmatter_value_needs_quotes(value.trim())
            || frontmatter_value_needs_quotes(translated)
        {
            format!(
                "{key}: \"{}\"",
                escape_frontmatter_double_quoted_value(translated)
            )
        } else {
            format!("{key}: {translated}")
        }
    }

    fn render_body(
        &self,
        body: &str,
        translations: &BTreeMap<String, String>,
        line_ending: &str,
    ) -> String {
        let mut rendered = Vec::new();
        let mut fence_marker: Option<String> = None;
        let mut heading_count = 0usize;
        let mut paragraph_count = 0usize;
        let mut list_count = 0usize;
        let mut quote_count = 0usize;

        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(open_marker) = fence_marker.as_deref() {
                if markdown_closes_fence(line, open_marker) {
                    fence_marker = None;
                }
                rendered.push(line.to_string());
                continue;
            }

            if let Some(marker) = markdown_fence_marker(line) {
                fence_marker = Some(marker);
                rendered.push(line.to_string());
                continue;
            }

            if trimmed.is_empty() || is_markdown_table_separator(trimmed) {
                rendered.push(line.to_string());
                continue;
            }

            if let Some((marker, _)) = markdown_heading_marker(line) {
                heading_count += 1;
                let key = self.key(&format!("heading/{heading_count}"));
                rendered.push(match translations.get(&key) {
                    Some(translation) => format!("{marker}{translation}"),
                    None => line.to_string(),
                });
                continue;
            }

            if let Some((marker, _)) = markdown_list_marker(line) {
                list_count += 1;
                let key = self.key(&format!("list/{list_count}"));
                rendered.push(match translations.get(&key) {
                    Some(translation) => format!("{marker}{translation}"),
                    None => line.to_string(),
                });
                continue;
            }

            if let Some((marker, _)) = markdown_quote_marker(line) {
                quote_count += 1;
                let key = self.key(&format!("quote/{quote_count}"));
                rendered.push(match translations.get(&key) {
                    Some(translation) => format!("{marker}{translation}"),
                    None => line.to_string(),
                });
                continue;
            }

            paragraph_count += 1;
            let key = self.key(&format!("paragraph/{paragraph_count}"));
            rendered.push(match translations.get(&key) {
                Some(translation) => translation.clone(),
                None => line.to_string(),
            });
        }

        rendered.join(line_ending)
    }

    fn collect_body_units(&self, body: &str, units: &mut Vec<TranslationUnit>) {
        let mut fence_marker: Option<String> = None;
        let mut heading_count = 0usize;
        let mut paragraph_count = 0usize;
        let mut list_count = 0usize;
        let mut quote_count = 0usize;

        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(open_marker) = fence_marker.as_deref() {
                if markdown_closes_fence(line, open_marker) {
                    fence_marker = None;
                }
                continue;
            }

            if let Some(marker) = markdown_fence_marker(line) {
                fence_marker = Some(marker);
                continue;
            }

            if trimmed.is_empty() || is_markdown_table_separator(trimmed) {
                continue;
            }

            if let Some((_, heading)) = markdown_heading_marker(line) {
                heading_count += 1;
                units.push(TranslationUnit::new(
                    self.key(&format!("heading/{heading_count}")),
                    heading,
                ));
                continue;
            }

            if let Some((_, item)) = markdown_list_marker(line) {
                list_count += 1;
                units.push(TranslationUnit::new(
                    self.key(&format!("list/{list_count}")),
                    item,
                ));
                continue;
            }

            if let Some((_, quote)) = markdown_quote_marker(line) {
                if !quote.is_empty() {
                    quote_count += 1;
                    units.push(TranslationUnit::new(
                        self.key(&format!("quote/{quote_count}")),
                        quote,
                    ));
                }
                continue;
            }

            paragraph_count += 1;
            units.push(TranslationUnit::new(
                self.key(&format!("paragraph/{paragraph_count}")),
                trimmed,
            ));
        }
    }

    fn key(&self, section: &str) -> String {
        format!("{}#{section}", self.canonical_path)
    }

    fn line_ending(&self) -> &'static str {
        if self.source.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }
}

fn markdown_heading_marker(line: &str) -> Option<(String, &str)> {
    let rest = line.trim_start();
    let indent = &line[..line.len() - rest.len()];
    let level = rest
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(1..=6).contains(&level) || !rest[level..].starts_with(' ') {
        return None;
    }

    Some((
        format!("{indent}{} ", "#".repeat(level)),
        rest[level + 1..].trim(),
    ))
}

fn markdown_list_marker(line: &str) -> Option<(String, &str)> {
    let rest = line.trim_start();
    let indent = &line[..line.len() - rest.len()];

    for marker in ["- [ ] ", "- [x] ", "- [X] ", "- ", "* ", "+ "] {
        if let Some(text) = rest.strip_prefix(marker) {
            return Some((format!("{indent}{marker}"), text.trim()));
        }
    }

    let digit_count = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .count();
    if digit_count > 0 && rest[digit_count..].starts_with(". ") {
        let marker = &rest[..digit_count + 2];
        return Some((format!("{indent}{marker}"), rest[digit_count + 2..].trim()));
    }

    None
}

fn markdown_quote_marker(line: &str) -> Option<(String, &str)> {
    let rest = line.trim_start();
    let mut cursor = line.len() - rest.len();
    let mut marker_end = cursor;
    let mut saw_marker = false;

    loop {
        if !line[cursor..].starts_with('>') {
            break;
        }

        saw_marker = true;
        cursor += 1;

        while let Some(character) = line[cursor..].chars().next() {
            if !matches!(character, ' ' | '\t') {
                break;
            }
            cursor += character.len_utf8();
        }

        marker_end = cursor;
    }

    saw_marker.then(|| (line[..marker_end].to_string(), line[marker_end..].trim()))
}

fn markdown_fence_marker(line: &str) -> Option<String> {
    let trimmed = markdown_fence_line_content(line);
    let marker_char = if trimmed.starts_with("```") {
        '`'
    } else if trimmed.starts_with("~~~") {
        '~'
    } else {
        return None;
    };

    let count = trimmed
        .chars()
        .take_while(|character| *character == marker_char)
        .count();
    (count >= 3).then(|| marker_char.to_string().repeat(count))
}

fn markdown_closes_fence(line: &str, open_marker: &str) -> bool {
    let Some(marker) = markdown_fence_marker(line) else {
        return false;
    };

    if !marker.starts_with(open_marker) {
        return false;
    }

    let trimmed = markdown_fence_line_content(line);
    trimmed[marker.len()..].trim().is_empty()
}

fn markdown_fence_line_content(line: &str) -> &str {
    let mut rest = line.trim_start();

    while let Some(after_marker) = rest.strip_prefix('>') {
        rest = after_marker.trim_start();
    }

    rest
}

fn is_markdown_table_separator(trimmed: &str) -> bool {
    if !trimmed.contains('|') {
        return false;
    }

    let cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .collect::<Vec<_>>();

    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim_matches(':');
            cell.len() >= 3 && cell.chars().all(|character| character == '-')
        })
}

fn frontmatter_value_needs_quotes(value: &str) -> bool {
    value.is_empty()
        || value.starts_with('"')
        || value.ends_with('"')
        || value.chars().any(|character| {
            matches!(
                character,
                '\r' | '\n' | '\t' | '"' | '\'' | '\\' | ':' | '#' | '[' | ']' | '{' | '}'
            )
        })
}

fn escape_frontmatter_double_quoted_value(value: &str) -> String {
    let mut escaped = String::new();
    let mut chars = value.chars().peekable();

    while let Some(character) = chars.next() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                escaped.push_str("\\n");
            }
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(character),
        }
    }

    escaped
}

#[cfg(test)]
mod tests {
    use super::MarkdownLocalizationDocument;
    use std::collections::BTreeMap;

    #[test]
    fn extracts_markdown_units_without_code_fences_or_routing_frontmatter() {
        let document = MarkdownLocalizationDocument::new(
            "---\ntitle: \"DX launch notes\"\nstatus: \"stable\"\n---\n\n# Hello, {name}\n\nVisit [dashboard]({url}).\n\n```tsx\n<Button />\n```\n\n> Preserve {{productName}}.",
            "docs/product.md",
        );

        let units = document.source_units();

        assert_eq!(
            units.iter().map(|unit| unit.key()).collect::<Vec<_>>(),
            vec![
                "docs/product.md#frontmatter/title",
                "docs/product.md#heading/1",
                "docs/product.md#paragraph/1",
                "docs/product.md#quote/1",
            ]
        );
        assert!(!units.iter().any(|unit| unit.text().contains("<Button")));
    }

    #[test]
    fn skips_blockquoted_markdown_code_fences() {
        let document = MarkdownLocalizationDocument::new(
            "> ```tsx\n> <Button label=\"{cta_label}\" />\n> ```\n\nAfter {name}\n",
            "docs/product.md",
        );

        let units = document.source_units();

        assert_eq!(
            units.iter().map(|unit| unit.key()).collect::<Vec<_>>(),
            vec!["docs/product.md#paragraph/1"]
        );
        assert_eq!(units[0].text(), "After {name}");
    }

    #[test]
    fn applies_translations_without_changing_code_fences_or_locked_frontmatter() {
        let document = MarkdownLocalizationDocument::new(
            "---\ntitle: \"DX launch notes\"\nstatus: \"stable\"\nslug: \"dx-launch\"\n---\n\n# Hello, {name}\n\nVisit `dx run`.\n\n```tsx\n<Button label=\"{cta_label}\" />\n```\n\n> Preserve {{productName}}.\n",
            "docs/product.md",
        );
        let translations = BTreeMap::from([
            (
                "docs/product.md#frontmatter/title".to_string(),
                "Notas de lanzamiento de DX".to_string(),
            ),
            (
                "docs/product.md#heading/1".to_string(),
                "Hola, {name}".to_string(),
            ),
            (
                "docs/product.md#paragraph/1".to_string(),
                "Visita `dx run`.".to_string(),
            ),
            (
                "docs/product.md#quote/1".to_string(),
                "Conserva {{productName}}.".to_string(),
            ),
        ]);

        assert_eq!(
            document.apply_translations(&translations),
            "---\ntitle: \"Notas de lanzamiento de DX\"\nstatus: \"stable\"\nslug: \"dx-launch\"\n---\n\n# Hola, {name}\n\nVisita `dx run`.\n\n```tsx\n<Button label=\"{cta_label}\" />\n```\n\n> Conserva {{productName}}.\n"
        );
    }

    #[test]
    fn escapes_frontmatter_title_scalars_when_applying_translations() {
        let document = MarkdownLocalizationDocument::new(
            "---\ntitle: \"DX launch notes\"\nslug: \"dx-launch\"\n---\n\nBody\n",
            "docs/product.md",
        );
        let translations = BTreeMap::from([(
            "docs/product.md#frontmatter/title".to_string(),
            "Notas\nslug: hacked\r\n\"quoted\" \\ path".to_string(),
        )]);

        assert_eq!(
            document.apply_translations(&translations),
            "---\ntitle: \"Notas\\nslug: hacked\\n\\\"quoted\\\" \\\\ path\"\nslug: \"dx-launch\"\n---\n\nBody\n"
        );
    }

    #[test]
    fn preserves_crlf_frontmatter_and_body_layout() {
        let document = MarkdownLocalizationDocument::new(
            "---\r\ntitle: \"DX launch notes\"\r\nstatus: \"stable\"\r\n---\r\n\r\n# Hello, {name}\r\n\r\n```rust\r\nfn main() {}\r\n```\r\n\r\n> Preserve {{productName}}.\r\n",
            "docs/product.md",
        );
        let translations = BTreeMap::from([
            (
                "docs/product.md#frontmatter/title".to_string(),
                "Notas de lanzamiento de DX".to_string(),
            ),
            (
                "docs/product.md#heading/1".to_string(),
                "Hola, {name}".to_string(),
            ),
            (
                "docs/product.md#quote/1".to_string(),
                "Conserva {{productName}}.".to_string(),
            ),
        ]);

        assert_eq!(
            document
                .source_units()
                .iter()
                .map(|unit| unit.key())
                .collect::<Vec<_>>(),
            vec![
                "docs/product.md#frontmatter/title",
                "docs/product.md#heading/1",
                "docs/product.md#quote/1",
            ]
        );
        assert_eq!(
            document.apply_translations(&translations),
            "---\r\ntitle: \"Notas de lanzamiento de DX\"\r\nstatus: \"stable\"\r\n---\r\n\r\n# Hola, {name}\r\n\r\n```rust\r\nfn main() {}\r\n```\r\n\r\n> Conserva {{productName}}.\r\n"
        );
    }

    #[test]
    fn preserves_extended_markdown_markers_when_applying_translations() {
        let document = MarkdownLocalizationDocument::new(
            "## Setup\n\n- Run `dx build`\n- [ ] Keep {count} tasks\n1. Ship safely\n\n> Quote {{name}}\n",
            "docs/product.md",
        );
        let translations = BTreeMap::from([
            (
                "docs/product.md#heading/1".to_string(),
                "Configuracion".to_string(),
            ),
            (
                "docs/product.md#list/1".to_string(),
                "Ejecuta `dx build`".to_string(),
            ),
            (
                "docs/product.md#list/2".to_string(),
                "Conserva {count} tareas".to_string(),
            ),
            (
                "docs/product.md#list/3".to_string(),
                "Publica con seguridad".to_string(),
            ),
            (
                "docs/product.md#quote/1".to_string(),
                "Cita {{name}}".to_string(),
            ),
        ]);

        assert_eq!(
            document
                .source_units()
                .iter()
                .map(|unit| unit.key())
                .collect::<Vec<_>>(),
            vec![
                "docs/product.md#heading/1",
                "docs/product.md#list/1",
                "docs/product.md#list/2",
                "docs/product.md#list/3",
                "docs/product.md#quote/1",
            ]
        );
        assert_eq!(
            document.apply_translations(&translations),
            "## Configuracion\n\n- Ejecuta `dx build`\n- [ ] Conserva {count} tareas\n1. Publica con seguridad\n\n> Cita {{name}}\n"
        );
    }

    #[test]
    fn preserves_nested_blockquote_markers_when_applying_translations() {
        let document = MarkdownLocalizationDocument::new(
            "> > Keep {name}\n> Plain {{product}}\n",
            "docs/product.md",
        );
        let translations = BTreeMap::from([
            (
                "docs/product.md#quote/1".to_string(),
                "Conserva {name}".to_string(),
            ),
            (
                "docs/product.md#quote/2".to_string(),
                "Simple {{product}}".to_string(),
            ),
        ]);

        assert_eq!(
            document
                .source_units()
                .iter()
                .map(|unit| unit.text())
                .collect::<Vec<_>>(),
            vec!["Keep {name}", "Plain {{product}}"]
        );
        assert_eq!(
            document.apply_translations(&translations),
            "> > Conserva {name}\n> Simple {{product}}\n"
        );
    }

    #[test]
    fn code_fence_state_tracks_marker_family() {
        let document = MarkdownLocalizationDocument::new(
            "~~~md\n```literal\nStill code\n~~~\n\nVisible text\n",
            "docs/product.md",
        );

        let units = document.source_units();

        assert_eq!(
            units.iter().map(|unit| unit.text()).collect::<Vec<_>>(),
            vec!["Visible text"]
        );
    }

    #[test]
    fn code_fence_state_does_not_close_on_same_marker_with_trailing_text() {
        let document = MarkdownLocalizationDocument::new(
            "```md\n```literal\nStill code {name}\n```\n\nVisible text\n",
            "docs/product.md",
        );

        let units = document.source_units();

        assert_eq!(
            units.iter().map(|unit| unit.text()).collect::<Vec<_>>(),
            vec!["Visible text"]
        );
    }

    #[test]
    fn does_not_translate_markdown_table_separator_rows() {
        let document = MarkdownLocalizationDocument::new(
            "| Name | Status |\n| --- | --- |\n| {name} | Ready |\n",
            "docs/product.md",
        );

        let units = document.source_units();

        assert!(!units.iter().any(|unit| unit.text() == "| --- | --- |"));
        assert_eq!(
            units.iter().map(|unit| unit.text()).collect::<Vec<_>>(),
            vec!["| Name | Status |", "| {name} | Ready |"]
        );
    }
}
