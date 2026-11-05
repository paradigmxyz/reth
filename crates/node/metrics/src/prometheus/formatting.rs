//! Helpers for rendering metrics in the Prometheus text exposition format.
//!
//! The sanitization rules are ported from `metrics-exporter-prometheus` 0.18.3
//! (`src/formatting.rs`, MIT licensed, Copyright (c) 2021 Metrics Contributors) so rendered names,
//! labels and descriptions stay byte-identical.

use std::fmt::{Display, Write};

/// Writes a `# HELP` line.
pub(super) fn write_help_line(buffer: &mut String, name: &str, desc: &str) {
    buffer.push_str("# HELP ");
    buffer.push_str(name);
    buffer.push(' ');
    push_sanitized_value(buffer, desc, true);
    buffer.push('\n');
}

/// Writes a `# TYPE` line.
pub(super) fn write_type_line(buffer: &mut String, name: &str, metric_type: &str) {
    buffer.push_str("# TYPE ");
    buffer.push_str(name);
    buffer.push(' ');
    buffer.push_str(metric_type);
    buffer.push('\n');
}

/// Writes a single sample line.
///
/// `labels` must already be rendered via [`render_labels`]. `suffix` is appended to `name` as
/// `_<suffix>` unless `name` already ends with it, and `additional_label` is rendered after
/// `labels`.
pub(super) fn write_metric_line(
    buffer: &mut String,
    name: &str,
    suffix: Option<&str>,
    labels: &str,
    additional_label: Option<(&str, impl Display)>,
    value: impl Display,
) {
    buffer.push_str(name);
    if let Some(suffix) = suffix &&
        !name.ends_with(suffix)
    {
        buffer.push('_');
        buffer.push_str(suffix);
    }

    if !labels.is_empty() || additional_label.is_some() {
        buffer.push('{');
        buffer.push_str(labels);
        if let Some((name, value)) = additional_label {
            if !labels.is_empty() {
                buffer.push(',');
            }
            let _ = write!(buffer, "{name}=\"{value}\"");
        }
        buffer.push('}');
    }

    let _ = writeln!(buffer, " {value}");
}

/// Renders `labels` as the sanitized, comma separated `key="value"` list used inside braces.
pub(super) fn render_labels<'a>(labels: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let mut out = String::new();
    for (key, value) in labels {
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(&sanitize_label_key(key));
        out.push_str("=\"");
        push_sanitized_value(&mut out, value, false);
        out.push('"');
    }
    out
}

/// Sanitizes a metric name to be valid under the Prometheus [data model].
///
/// [data model]: https://prometheus.io/docs/concepts/data_model/#metric-names-and-labels
pub(super) fn sanitize_metric_name(name: &str) -> String {
    // The first character must be [a-zA-Z_:], and all subsequent characters must be [a-zA-Z0-9_:].
    name.chars()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 && valid_metric_name_start_character(c) ||
                i != 0 && valid_metric_name_character(c)
            {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Sanitizes a label key to be valid under the Prometheus [data model].
///
/// [data model]: https://prometheus.io/docs/concepts/data_model/#metric-names-and-labels
fn sanitize_label_key(key: &str) -> String {
    // The first character must be [a-zA-Z_], and all subsequent characters must be [a-zA-Z0-9_].
    key.chars()
        .enumerate()
        .map(|(i, c)| {
            if i == 0 && valid_label_key_start_character(c) ||
                i != 0 && valid_label_key_character(c)
            {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn push_sanitized_value(sanitized: &mut String, value: &str, is_desc: bool) {
    // All Unicode characters are valid, but backslashes, double quotes, and line feeds must be
    // escaped.
    let mut previous_backslash = false;
    for c in value.chars() {
        match c {
            // Any raw newlines get escaped, period.
            '\n' => sanitized.push_str("\\n"),
            // Any double quote we see gets escaped, but only for label values, not descriptions.
            '"' if !is_desc => {
                previous_backslash = false;
                sanitized.push_str("\\\"");
            }
            // If we see a backslash, we might be either seeing one that is being used to escape
            // something, or seeing one that has being escaped. If our last character was a
            // backslash, then we know this one has already been escaped, and we just emit the
            // escaped backslash.
            '\\' => {
                if previous_backslash {
                    sanitized.push_str("\\\\");
                }
                previous_backslash = !previous_backslash;
            }
            c => {
                // A backslash in holding that wasn't escaping something we care about is emitted
                // as an escaped backslash before the current character.
                if previous_backslash {
                    previous_backslash = false;
                    sanitized.push_str("\\\\");
                }
                sanitized.push(c);
            }
        }
    }

    // Handle any dangling backslash by writing it out in an escaped fashion.
    if previous_backslash {
        sanitized.push_str("\\\\");
    }
}

#[inline]
const fn valid_metric_name_start_character(c: char) -> bool {
    // Essentially, needs to match the regex pattern of [a-zA-Z_:].
    c.is_ascii_alphabetic() || c == '_' || c == ':'
}

#[inline]
const fn valid_metric_name_character(c: char) -> bool {
    // Essentially, needs to match the regex pattern of [a-zA-Z0-9_:].
    c.is_ascii_alphanumeric() || c == '_' || c == ':'
}

#[inline]
const fn valid_label_key_start_character(c: char) -> bool {
    // Essentially, needs to match the regex pattern of [a-zA-Z_].
    c.is_ascii_alphabetic() || c == '_'
}

#[inline]
const fn valid_label_key_character(c: char) -> bool {
    // Essentially, needs to match the regex pattern of [a-zA-Z0-9_].
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sanitize_label_value(value: &str) -> String {
        let mut out = String::new();
        push_sanitized_value(&mut out, value, false);
        out
    }

    fn sanitize_description(value: &str) -> String {
        let mut out = String::new();
        push_sanitized_value(&mut out, value, true);
        out
    }

    #[test]
    fn sanitize_metric_name_known_cases() {
        let cases = &[
            ("*", "_"),
            ("\"", "_"),
            ("foo_bar", "foo_bar"),
            ("foo1_bar", "foo1_bar"),
            ("1foobar", "_foobar"),
            ("foo1:bar2", "foo1:bar2"),
            ("123", "_23"),
        ];
        for (input, expected) in cases {
            assert_eq!(expected, &sanitize_metric_name(input));
        }
    }

    #[test]
    fn sanitize_label_key_known_cases() {
        let cases = &[
            ("*", "_"),
            ("\"", "_"),
            (":", "_"),
            ("foo_bar", "foo_bar"),
            ("1foobar", "_foobar"),
            ("__foobar", "__foobar"),
            ("foo1bar2", "foo1bar2"),
            ("123", "_23"),
        ];
        for (input, expected) in cases {
            assert_eq!(expected, &sanitize_label_key(input));
        }
    }

    #[test]
    fn sanitize_label_value_known_cases() {
        let cases = &[
            ("*", "*"),
            ("\"", "\\\""),
            ("\\", "\\\\"),
            ("\\\\", "\\\\"),
            ("\n", "\\n"),
            ("foo_bar", "foo_bar"),
            ("1foobar", "1foobar"),
        ];
        for (input, expected) in cases {
            assert_eq!(expected, &sanitize_label_value(input));
        }
    }

    #[test]
    fn sanitize_description_known_cases() {
        let cases = &[
            ("*", "*"),
            ("\"", "\""),
            ("\\", "\\\\"),
            ("\\\\", "\\\\"),
            ("\n", "\\n"),
            ("foo_bar", "foo_bar"),
            ("1foobar", "1foobar"),
        ];
        for (input, expected) in cases {
            assert_eq!(expected, &sanitize_description(input));
        }
    }

    #[test]
    fn metric_line_suffix_and_labels() {
        let mut out = String::new();
        write_metric_line(&mut out, "a", None, "", None::<(&str, u8)>, 1u64);
        write_metric_line(&mut out, "a", Some("sum"), "k=\"v\"", None::<(&str, u8)>, 1.5f64);
        write_metric_line(&mut out, "a_sum", Some("sum"), "", Some(("le", "+Inf")), 2u64);
        write_metric_line(&mut out, "a", Some("bucket"), "k=\"v\"", Some(("le", 0.5)), 3u64);
        assert_eq!(
            out,
            "a 1\na_sum{k=\"v\"} 1.5\na_sum{le=\"+Inf\"} 2\na_bucket{k=\"v\",le=\"0.5\"} 3\n"
        );
    }

    proptest! {
        #[test]
        fn proptest_sanitize_metric_name(input in "[\n\"\\\\]?.*[\n\"\\\\]?") {
            let result = sanitize_metric_name(&input);
            let as_chars = result.chars().collect::<Vec<_>>();
            if let Some(c) = as_chars.first() {
                prop_assert!(valid_metric_name_start_character(*c));
            }
            prop_assert!(as_chars.iter().all(|c| valid_metric_name_character(*c)));
        }

        #[test]
        fn proptest_sanitize_label_key(input in "[\n\"\\\\:]?.*[\n\"\\\\:]?") {
            let result = sanitize_label_key(&input);
            let as_chars = result.chars().collect::<Vec<_>>();
            if let Some(c) = as_chars.first() {
                prop_assert!(valid_label_key_start_character(*c));
            }
            prop_assert!(as_chars.iter().all(|c| valid_label_key_character(*c)));
        }

        #[test]
        fn proptest_sanitize_label_value(input in "[\n\"\\\\]?.*[\n\"\\\\]?") {
            let result = sanitize_label_value(&input);
            prop_assert!(!result.contains('\n'), "raw/unescaped newlines present");

            // Only dangling backslashes matter, so drop escaped ones first.
            let as_chars = result.replace("\\\\", "").chars().collect::<Vec<_>>();
            prop_assert!(as_chars.first().is_none_or(|c| *c != '"'));
            let unescaped = as_chars.windows(2).any(|s| match (s[0], s[1]) {
                (c, '"') => c != '\\',
                ('\\', c) => c != 'n',
                _ => false,
            });
            prop_assert!(!unescaped, "invalid or missing escape detected");
        }

        #[test]
        fn proptest_sanitize_description(input in "[\n\"\\\\]?.*[\n\"\\\\]?") {
            let result = sanitize_description(&input);
            prop_assert!(!result.contains('\n'), "raw/unescaped newlines present");

            let as_chars = result.replace("\\\\", "").chars().collect::<Vec<_>>();
            let unescaped = as_chars.windows(2).any(|s| matches!(s, ['\\', c] if *c != 'n'));
            prop_assert!(!unescaped, "invalid or missing escape detected");
        }
    }
}
