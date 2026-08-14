//! REPL kind presets: named sets of prompt regexes, mirroring the
//! `tmux-repl-mcp` built-in kinds. A kind maps to a *list* of prompt
//! patterns because one REPL can show several prompt styles — e.g. SBCL's
//! top-level `* `, numbered debugger prompts `0]`, and `ldb> `.

use std::collections::HashMap;

/// Built-in kind → prompt regexes.
pub fn builtin() -> HashMap<String, Vec<String>> {
    let mut kinds = HashMap::new();
    kinds.insert("python".into(), vec![r"^>>> ".into()]);
    kinds.insert("ipython".into(), vec![r"^In \[\d+\]: ".into()]);
    kinds.insert("bash".into(), vec![r"^[^$#]+[$#] *".into()]);
    kinds.insert("sh".into(), vec![r"^[^$#]+[$#] *".into()]);
    kinds.insert("zsh".into(), vec![r"^[^$+][$#] *".into()]);
    kinds.insert("node".into(), vec![r"^> ".into()]);
    // Modern irb (Ruby 3.x) prints `irb(main):001> `; older irb had an extra
    // `:0` line-number field (`irb(main):001:0> `). Match both.
    kinds.insert("irb".into(), vec![r"^irb\(.*\):\d+(:\d+)?> *".into()]);
    kinds.insert("iex".into(), vec![r"^iex\(\d+\)> $".into()]);
    // Lisp ready prompts: top-level REPL and debugger prompts. A debugger
    // prompt is a valid command boundary — the user can type at it.
    kinds.insert(
        "lisp".into(),
        vec![
            r"^\? ".into(),              // CCL
            r"^\* ".into(),              // top-level with or without command
            r"^\*$".into(),              // bare idle prompt
            r"^[A-Za-z0-9.-]+> ".into(), // custom package / slynk prompt
            r"^ *[0-9]+(\[[0-9]+\]|\]) ?".into(), // numbered debugger prompt
        ],
    );
    // SBCL: like lisp, plus the `ldb> ` low-level debugger prompt.
    kinds.insert(
        "sbcl".into(),
        vec![
            r"^\* ".into(),
            r"^\*$".into(),
            r"^ *[0-9]+(\[[0-9]+\]|\]) ?".into(),
            r"^ldb> ".into(),
            r"^[A-Za-z0-9.-]+> ".into(),
        ],
    );
    // Goose TUI prompt – the ready-state footer line
    kinds.insert("goose".into(), vec![r"🪿 Enter to send.*".into()]);
    kinds
}

/// Load the effective kind table: built-ins merged with the
/// `TMUX_REPL_KINDS` environment variable, which is a JSON object mapping
/// kind names to a prompt regex string or an array of regex strings.
/// Entries override built-ins, matching the reference's config loading.
pub fn load() -> HashMap<String, Vec<String>> {
    let mut kinds = builtin();

    let Ok(raw) = std::env::var("TMUX_REPL_KINDS") else {
        return kinds;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return kinds;
    }

    let parsed = match serde_json::from_str::<serde_json::Value>(raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("muxxy: WARNING: could not parse TMUX_REPL_KINDS ({e}); ignoring.");
            return kinds;
        }
    };

    let Some(obj) = parsed.as_object() else {
        eprintln!("muxxy: WARNING: TMUX_REPL_KINDS must be a JSON object; ignoring.");
        return kinds;
    };

    for (name, value) in obj {
        let regexes = match value {
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect(),
            _ => {
                eprintln!(
                    "muxxy: WARNING: kind {name:?} in TMUX_REPL_KINDS must be a string or array of strings; ignoring."
                );
                continue;
            }
        };
        if !regexes.is_empty() {
            kinds.insert(name.clone(), regexes);
        }
    }
    kinds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_expected_kinds() {
        let kinds = builtin();
        assert_eq!(kinds["python"], vec!["^>>> "]);
        assert!(kinds.contains_key("sbcl"));
        assert!(kinds["sbcl"].iter().any(|p| p == r"^ *[0-9]+(\[[0-9]+\]|\]) ?"));
        assert!(kinds.contains_key("lisp"));
        assert!(kinds.contains_key("bash"));
    }

    #[test]
    fn irb_kind_matches_old_and_new_prompt_formats() {
        let kinds = builtin();
        let patterns = &kinds["irb"];
        let re = regex::Regex::new(&patterns[0]).unwrap();
        // Modern irb (Ruby 3.x)
        assert!(re.is_match("irb(main):001> "));
        // Older irb with the extra line-number field
        assert!(re.is_match("irb(main):001:0> "));
        // Command typed at the prompt
        assert!(re.is_match("irb(main):001> [1,2].map { |x| x }"));
    }
}
