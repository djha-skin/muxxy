//! A minimal YAML emitter for muxxy's fixed output shapes.
//!
//! Multiline strings are emitted as literal block scalars (`|` / `|-`) so
//! captured REPL output reads almost exactly as it appeared in the pane,
//! while remaining machine-consumable YAML.

/// A value in the output mapping. Only the kinds muxxy emits.
pub enum YamlValue<'a> {
    Str(&'a str),
    Bool(bool),
    Null,
}

/// Render an ordered mapping of key → value as YAML.
pub fn render_map(entries: &[(&str, YamlValue)]) -> String {
    let mut out = String::new();
    for (key, value) in entries {
        match value {
            YamlValue::Str(s) if s.contains('\n') => {
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&block_scalar(s, 2));
            }
            YamlValue::Str(s) => {
                out.push_str(key);
                out.push_str(": ");
                out.push_str(&scalar(s));
                out.push('\n');
            }
            YamlValue::Bool(b) => {
                out.push_str(key);
                out.push_str(": ");
                out.push_str(if *b { "true" } else { "false" });
                out.push('\n');
            }
            YamlValue::Null => {
                out.push_str(key);
                out.push_str(": null\n");
            }
        }
    }
    out
}

/// Render a multiline string as a literal block scalar indented `indent`
/// spaces. `|-` strips the trailing newline (content without one), `|`
/// keeps it (content ending in a newline).
fn block_scalar(content: &str, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let indicator = if content.ends_with('\n') { "|" } else { "|-" };
    let mut out = String::from(indicator);
    out.push('\n');
    for line in content.split('\n') {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&pad);
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Render a single-line string: plain when it is a safe plain scalar,
/// double-quoted otherwise.
fn scalar(s: &str) -> String {
    if is_plain_safe(s) {
        s.to_string()
    } else {
        quoted(s)
    }
}

fn is_plain_safe(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) {
        return false;
    }
    let first = s.chars().next().unwrap();
    if "-?:,[]{}#&*!|>'\"%@`".contains(first) {
        return false;
    }
    if s.contains(" #") || s.contains(": ") || s.ends_with(':') {
        return false;
    }
    // Strings that a YAML parser would interpret as another scalar type
    // must be quoted so they round-trip as strings.
    !looks_like_non_string(s)
}

/// True when a YAML parser would resolve `s` as something other than a
/// string (number, bool, null, timestamp, ...).
fn looks_like_non_string(s: &str) -> bool {
    s.parse::<i64>().is_ok()
        || s.parse::<f64>().is_ok()
        || looks_like_hex(s)
        || looks_like_octal(s)
        || looks_like_timestamp(s)
        || matches!(
            s.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~" | ".inf" | ".nan"
        )
}

fn looks_like_hex(s: &str) -> bool {
    let digits = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"));
    digits.is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_hexdigit()))
}

fn looks_like_octal(s: &str) -> bool {
    let digits = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O"));
    if let Some(d) = digits {
        return !d.is_empty() && d.chars().all(|c| ('0'..='7').contains(&c));
    }
    // YAML 1.1 leading-zero octal
    s.len() > 1
        && s.starts_with('0')
        && s.chars().all(|c| ('0'..='7').contains(&c))
}

fn looks_like_timestamp(s: &str) -> bool {
    let mut chars = s.chars();
    for _ in 0..4 {
        if !chars.next().is_some_and(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    matches!(chars.next(), Some('-'))
}

fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(entries: &[(&str, YamlValue)]) -> String {
        render_map(entries)
    }

    #[test]
    fn renders_plain_scalars() {
        let out = render(&[
            ("status", YamlValue::Str("ok")),
            ("is_ready", YamlValue::Bool(true)),
            ("last_command", YamlValue::Str("2 + 3")),
            ("output", YamlValue::Null),
        ]);
        assert_eq!(
            out,
            "status: ok\nis_ready: true\nlast_command: 2 + 3\noutput: null\n"
        );
    }

    #[test]
    fn quotes_strings_that_are_not_plain_safe() {
        let out = render(&[("kind", YamlValue::Str("^>>> "))]);
        assert_eq!(out, "kind: \"^>>> \"\n");
        let out = render(&[("pane", YamlValue::Str("%1"))]);
        assert_eq!(out, "pane: \"%1\"\n");
        let out = render(&[("reason", YamlValue::Str("a: b"))]);
        assert_eq!(out, "reason: \"a: b\"\n");
    }

    #[test]
    fn quotes_strings_that_would_parse_as_other_types() {
        // Numbers, bools, null, hex, timestamps must stay strings on
        // round-trip.
        for s in ["5", "1.5", "1e5", "0x1F", "true", "yes", "off", "null", "~", "2026-08-13"] {
            let out = render(&[("output", YamlValue::Str(s))]);
            assert_eq!(out, format!("output: \"{s}\"\n"), "{s} should be quoted");
            let parsed: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
            assert_eq!(parsed["output"], serde_yaml::Value::String(s.into()));
        }
    }

    #[test]
    fn renders_multiline_output_as_literal_block() {
        let out = render(&[("output", YamlValue::Str("0\n1\n2"))]);
        assert_eq!(out, "output: |-\n  0\n  1\n  2\n");
    }

    #[test]
    fn block_scalar_keeps_trailing_newline_when_present() {
        let out = render(&[("output", YamlValue::Str("a\nb\n"))]);
        assert_eq!(out, "output: |\n  a\n  b\n\n");
    }

    #[test]
    fn block_scalar_handles_blank_lines() {
        let out = render(&[("output", YamlValue::Str("a\n\nb"))]);
        assert_eq!(out, "output: |-\n  a\n\n  b\n");
    }

    #[test]
    fn round_trips_through_serde_yaml() {
        let out = render(&[
            ("status", YamlValue::Str("ok")),
            ("last_command", YamlValue::Str("import time; time.sleep(1)")),
            ("output", YamlValue::Str("0\n1\n2")),
        ]);
        let parsed: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["last_command"], "import time; time.sleep(1)");
        assert_eq!(parsed["output"], "0\n1\n2");
    }
}
