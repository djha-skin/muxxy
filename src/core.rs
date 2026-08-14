//! Core logic: prompt matching, command/output extraction, and the
//! execute-command wait loop. This mirrors the behavior of the Python
//! `tmux-repl-mcp` server's core module, extended to match *any* of a set
//! of prompt patterns (a REPL may show several prompt styles — e.g. SBCL's
//! `* `, numbered debugger prompts `0]`, and `ldb> `).

use crate::tmux;
use regex::Regex;
use std::time::{Duration, Instant};

/// A set of REPL prompt patterns. Each entry carries a label — a kind name
/// when it came from a `--kind` preset, otherwise the regex source — so the
/// tool can report which prompt matched.
pub struct Prompts {
    entries: Vec<(String, Regex)>,
}

impl Prompts {
    /// Build from raw prompt regexes; each is labelled by its own source.
    #[cfg(test)]
    pub fn from_prompts(prompts: &[String]) -> Result<Self, String> {
        let mut entries = Vec::new();
        for p in prompts {
            entries.push((p.clone(), compile(p)?));
        }
        Ok(Self { entries })
    }

    /// Build from named kinds (expanded through `kinds`) plus extra prompts.
    pub fn from_kinds(
        kind_names: &[String],
        kinds: &std::collections::HashMap<String, Vec<String>>,
        extra_prompts: &[String],
    ) -> Result<Self, String> {
        let mut entries = Vec::new();
        for name in kind_names {
            let regexes = kinds
                .get(name)
                .ok_or_else(|| format!("unknown REPL kind {name:?}"))?;
            for r in regexes {
                entries.push((name.clone(), compile(r)?));
            }
        }
        for p in extra_prompts {
            entries.push((p.clone(), compile(p)?));
        }
        Ok(Self { entries })
    }

    /// Prefix semantics: true when any pattern matches at the start of the
    /// line. This prevents a prompt regex from matching mid-line output (e.g.
    /// `^>>> ` matching `note: see >>> for help`). Used to find prompt lines
    /// in history.
    pub fn is_prompt_line(&self, line: &str) -> bool {
        self.entries.iter().any(|(_, re)| prefix_match(re, line))
    }

    /// Idle semantics: true when any pattern matches a prefix of the line
    /// with only whitespace after it. Used to detect that the REPL is idle
    /// again after a command.
    pub fn is_idle_prompt(&self, line: &str) -> bool {
        self.entries.iter().any(|(_, re)| idle_match(re, line))
    }

    /// Label of the first pattern that idle-matches the line.
    pub fn detect_idle(&self, line: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, re)| idle_match(re, line))
            .map(|(label, _)| label.as_str())
    }

    /// Index of the first pattern that idle-matches the line, or `None`.
    pub fn detect_idle_index(&self, line: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|(_, re)| idle_match(re, line))
    }

    /// True when `line` idle-matches the entry at `index`; when `index` is
    /// `None`, true when it idle-matches any entry.
    pub fn idle_matches_index(&self, index: Option<usize>, line: &str) -> bool {
        match index {
            Some(i) => self.entries.get(i).is_some_and(|(_, re)| idle_match(re, line)),
            None => self.is_idle_prompt(line),
        }
    }

    /// Strip the prompt prefix from a command line: replace the first
    /// position-0 match of the first matching pattern and trim. Unlike
    /// Python `re.sub(..., count=1)`, this only strips when the match is at
    /// the start of the line, so mid-line output is never eaten.
    pub fn strip_prompt(&self, line: &str) -> String {
        for (_, re) in &self.entries {
            if let Some(m) = re.find(line)
                && m.start() == 0
            {
                return line[m.end()..].trim().to_string();
            }
        }
        line.trim().to_string()
    }
}

fn compile(pattern: &str) -> Result<Regex, String> {
    Regex::new(pattern).map_err(|e| format!("invalid prompt regex {pattern:?}: {e}"))
}

/// True when `re` matches at the start of `line` (prefix semantics).
fn prefix_match(re: &Regex, line: &str) -> bool {
    re.find(line).is_some_and(|m| m.start() == 0)
}

/// True when `re` matches a prefix of `line` and everything after the match
/// is whitespace. Tolerates pane captures padded to the terminal width and
/// tmux's trimming of trailing spaces, so prompt patterns that end in a
/// literal space (e.g. `^>>> `) still match.
fn idle_match(re: &Regex, line: &str) -> bool {
    re.find(line)
        .is_some_and(|m| m.start() == 0 && line[m.end()..].chars().all(|c| c.is_whitespace()))
}

/// Split captured pane text on newlines (like Python `str.split("\n")`).
pub fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// The last non-whitespace line, or `None` if there is none.
pub fn last_meaningful_line<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().rev().find(|l| !l.trim().is_empty()).copied()
}

/// Label of the prompt that the last meaningful line idle-matches, if the
/// REPL is idle and showing a known prompt.
pub fn detect_idle_kind<'a>(prompt: &'a Prompts, lines: &[&str]) -> Option<&'a str> {
    last_meaningful_line(lines).and_then(|l| prompt.detect_idle(l))
}

/// Index of the prompt that the last meaningful line idle-matches, if any.
pub fn detect_idle_index(prompt: &Prompts, lines: &[&str]) -> Option<usize> {
    last_meaningful_line(lines).and_then(|l| prompt.detect_idle_index(l))
}

/// True if the last meaningful line of the pane is a bare prompt (i.e. the
/// REPL is idle and ready for input). Idle semantics rather than plain
/// search: an echoed command line such as `>>> time.sleep(5)` *starts* with
/// the prompt but the REPL is busy, so it must not count as ready.
pub fn is_repl_ready(prompt: &Prompts, lines: &[&str]) -> bool {
    detect_idle_kind(prompt, lines).is_some()
}

/// Index of the last prompt line in `lines`, or `None`.
fn last_prompt_index(lines: &[&str], prompt: &Prompts) -> Option<usize> {
    lines.iter().rposition(|l| prompt.is_prompt_line(l))
}

/// Index of the last prompt line strictly before `end_idx` that actually
/// carries a command, or `None`.
///
/// A bare prompt line (one whose prompt prefix strips to nothing — e.g.
/// python's `...` continuation echo of a blank line, or a plain `>>> `) is
/// an *end* boundary, not the start of a command block, so it is skipped.
fn second_to_last_prompt_index(
    lines: &[&str],
    prompt: &Prompts,
    end_idx: usize,
) -> Option<usize> {
    lines[..end_idx].iter().rposition(|l| {
        prompt.is_prompt_line(l) && !prompt.strip_prompt(l).is_empty()
    })
}

/// Parse `(last_command, output)` out of the pane lines.
///
/// `last_command` is the text of the second-to-last prompt line with the
/// prompt prefix stripped; `output` is everything between that line and the
/// final prompt line. Returns `(None, None)` when no complete
/// prompt → command → output → prompt block can be found.
pub fn extract_last_command_and_output(
    lines: &[&str],
    prompt: &Prompts,
) -> (Option<String>, Option<String>) {
    let end_idx = match last_prompt_index(lines, prompt) {
        Some(i) => i,
        None => return (None, None),
    };
    let start_idx = match second_to_last_prompt_index(lines, prompt, end_idx) {
        Some(i) => i,
        None => return (None, None),
    };

    let last_command = prompt.strip_prompt(lines[start_idx]);
    // Drop bare-prompt-only lines from the output (e.g. the empty `...`
    // continuation line python echoes when a block is closed), and trim
    // width padding from each line.
    let output = join_output(&lines[start_idx + 1..end_idx], prompt);
    (Some(last_command), Some(output))
}

/// Output-like sample lines a custom prompt regex must never match. If a
/// pattern matches any of these — especially as a bare idle prompt — muxxy
/// may treat ordinary output as a prompt and silently drop it.
const OUTPUT_LIKE_SAMPLES: &[&str] = &[
    "1024", "0", "1", "7", "42", "100", "65536", "123456789", "3.14", "-1",
    "ok", "true", "false", "nil", "NIL", "hello", "foo", "(foo)",
];

/// Sanity-check custom `--prompt` regexes against a corpus of ordinary
/// output lines (bare numbers, short words). The built-in kinds are curated
/// to avoid these; hand-written regexes often are not (e.g. a SBCL debugger
/// attempt like `^ *[0-9]+(\\[[0-9]+\\])?` also matches the bare integer
/// `1024`). Returns one warning string per offending pattern, to be printed
/// to stderr — the regex still works, but the user should anchor it to the
/// real prompt shape (e.g. require the trailing `]` or `> `).
pub fn validate_custom_prompts(prompts: &[String]) -> Vec<String> {
    let mut warnings = Vec::new();
    for p in prompts {
        let Ok(re) = Regex::new(p) else {
            continue; // compile errors are reported at prompt-build time
        };
        for sample in OUTPUT_LIKE_SAMPLES {
            if re.is_match(sample) {
                let idle = re
                    .find(sample)
                    .is_some_and(|m| m.start() == 0 && sample[m.end()..].chars().all(char::is_whitespace));
                warnings.push(format!(
                    "prompt regex {p:?} also matches the ordinary output line {sample:?}{}; \
                     muxxy may treat that output as a prompt and drop it — anchor the pattern \
                     to the real prompt shape (e.g. require a trailing ']' or '> ')",
                    if idle { " as a bare idle prompt" } else { "" }
                ));
                break; // one warning per pattern
            }
        }
    }
    warnings
}

/// Extract `(last_command, output)` like [`extract_last_command_and_output`],
/// but repair the result using the command text that was actually sent.
///
/// rlwrap (and similar front ends) echo multi-line pastes *without* the
/// prompt prefix, and can stay confused for the next command too — so the
/// real echo of a freshly sent command may lack the `* ` prefix. Plain
/// prompt-line extraction then walks back to a *stale* prompt-prefixed line
/// and reports an old command plus history-mixed output. When extraction
/// disagrees with what we sent, look for the sent command text among the
/// captured lines and retarget the block there.
///
/// If no complete block or visible echo can be found (e.g. the command line
/// scrolled out of the capture window), returns the sent command with a
/// `None` output: after a successful idle wait, we know it ran even though
/// its output cannot be recovered.
pub fn extract_with_sent_command(
    lines: &[&str],
    prompt: &Prompts,
    sent: &str,
) -> (Option<String>, Option<String>) {
    let sent = sent.trim();
    if sent.is_empty() {
        return extract_last_command_and_output(lines, prompt);
    }

    let (last_command, output) = extract_last_command_and_output(lines, prompt);

    // Normal case: extraction already agrees with what we sent.
    if last_command.as_deref().map(str::trim) == Some(sent) {
        return (last_command, output);
    }

    // No complete block found (e.g. the command echo scrolled out): search
    // the whole visible history for an un-prefixed echo of the sent command
    // so the output can still be recovered.
    let Some(end_idx) = lines.iter().rposition(|l| prompt.is_prompt_line(l)) else {
        return (last_command, output);
    };
    if last_command.is_none() {
        let echo_idx = lines[..end_idx]
            .iter()
            .rposition(|l| prompt.strip_prompt(l).trim() == sent);
        return match echo_idx {
            Some(i) => (
                Some(sent.to_string()),
                Some(join_output(&lines[i + 1..end_idx], prompt)),
            ),
            None => (Some(sent.to_string()), None),
        };
    }

    // Extraction found a *stale* command line (rlwrap echo without prefix):
    // the real echo sits inside the extracted output region. Retarget the
    // block to the first line there that matches the sent command, so output
    // no longer mixes in stale history.
    let Some(ref out) = output else {
        return (last_command, output);
    };
    let out_lines: Vec<&str> = out.split('\n').collect();
    match out_lines
        .iter()
        .position(|l| prompt.strip_prompt(l).trim() == sent)
    {
        Some(pos) => (Some(sent.to_string()), Some(out_lines[pos + 1..].join("\n"))),
        None => (last_command, output),
    }
}

/// Join `lines` into output text: drop bare-prompt-only lines and trim
/// width padding from each line (same filtering as
/// [`extract_last_command_and_output`]).
fn join_output(lines: &[&str], prompt: &Prompts) -> String {
    lines
        .iter()
        .filter(|l| !prompt.strip_prompt(l).is_empty())
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wait for a command to finish after it has been sent to the pane.
///
/// Two phases, mirroring the reference design:
///
/// 1. Wait until the pane content changes from `pre_send` (the command has
///    been echoed and the REPL has started processing it). Without this,
///    the very first capture after `send-keys` can still show the old idle
///    prompt and the function would return before the command ran.
/// 2. Wait until the last line idle-matches any prompt (the REPL is idle
///    again), then return the final lines.
///
/// A fast command that echoes and finishes between polls passes phase 1
/// immediately (the pane already differs from `pre_send`) and is caught by
/// phase 2's idle check, so it cannot hang.
#[allow(clippy::too_many_arguments)] // one knob per call site; grouping would hide intent
pub fn wait_for_idle(
    pane: &str,
    prompt: &Prompts,
    max_lines: usize,
    check: f64,
    timeout: Option<f64>,
    socket: Option<&str>,
    pre_send: &str,
    require_index: Option<usize>,
) -> Result<Vec<String>, String> {
    let check_dur = Duration::from_secs_f64(check.max(0.001));
    let started = Instant::now();

    // Phase 1: wait for the pane to change (the command echo).
    loop {
        if timeout_is_up(timeout, started) {
            return Err(format!(
                "timed out after {} seconds waiting for the REPL prompt",
                timeout.unwrap_or(0.0)
            ));
        }
        let captured = tmux::capture_pane(pane, max_lines, socket)?;
        if captured != pre_send {
            break;
        }
        std::thread::sleep(check_dur);
    }

    // Phase 2: wait until the REPL is idle again. Normally *any* prompt
    // counts (a command that errors into the SBCL debugger is still
    // "idle"). With `require_index`, only that specific prompt pattern
    // counts — used for multi-line blocks, where the bare `...`
    // continuation echo that python prints *before* executing a slow block
    // must not be mistaken for completion.
    loop {
        if timeout_is_up(timeout, started) {
            return Err(format!(
                "timed out after {} seconds waiting for the REPL prompt",
                timeout.unwrap_or(0.0)
            ));
        }
        let captured = tmux::capture_pane(pane, max_lines, socket)?;
        let lines = split_lines(&captured);
        match last_meaningful_line(&lines) {
            Some(last) if prompt.idle_matches_index(require_index, last) => {
                return Ok(lines.into_iter().map(str::to_string).collect());
            }
            _ => std::thread::sleep(check_dur),
        }
    }
}

/// Join `--lines` into a single sendable command: one line per element,
/// with a trailing newline so the block is closed by a blank line (python
/// compound statements execute on the blank line). The `send_keys` helper
/// appends its own final Enter, so the string's trailing `\n` plus that
/// Enter produce exactly one blank line.
pub fn build_multi_line_text(lines: &[String]) -> String {
    lines.join("\n") + "\n"
}

fn timeout_is_up(timeout: Option<f64>, started: Instant) -> bool {
    timeout.is_some_and(|t| t > 0.0 && started.elapsed().as_secs_f64() >= t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn py_prompt() -> Prompts {
        Prompts::from_prompts(&["^>>> ".to_string()]).unwrap()
    }

    fn sbcl_prompt() -> Prompts {
        let mut kinds = HashMap::new();
        kinds.insert(
            "sbcl".to_string(),
            vec![
                r"^\* ".to_string(),
                r"^\*$".to_string(),
                r"^ *\d+(\[\d+\]|\]) ?".to_string(),
                r"^ldb> ".to_string(),
            ],
        );
        Prompts::from_kinds(&["sbcl".to_string()], &kinds, &[]).unwrap()
    }

    #[test]
    fn detects_ready_python_prompt() {
        let prompt = py_prompt();
        let lines = split_lines(">>> \n");
        assert!(is_repl_ready(&prompt, &lines));
        assert_eq!(detect_idle_kind(&prompt, &lines), Some("^>>> "));
    }

    #[test]
    fn not_ready_when_busy() {
        let prompt = py_prompt();
        let lines = split_lines(">>> \nCalculating...\nstill working\n");
        assert!(!is_repl_ready(&prompt, &lines));
    }

    #[test]
    fn not_ready_when_echoed_command_starts_with_prompt() {
        let prompt = py_prompt();
        // Mid-execution: the echoed command line starts with ">>> "
        let lines = split_lines(">>> import time; time.sleep(5)\n");
        assert!(!is_repl_ready(&prompt, &lines));
    }

    #[test]
    fn idle_uses_bare_prompt_but_detection_uses_prefix() {
        let prompt = py_prompt();
        assert!(prompt.is_idle_prompt(">>> "));
        assert!(!prompt.is_idle_prompt(">>> 2 + 2"));
        assert!(prompt.is_prompt_line(">>> 2 + 2"));
        // Prompt-looking text in ordinary output is not a command boundary.
        assert!(!prompt.is_prompt_line("note: see >>> for help"));
    }

    #[test]
    fn idle_tolerates_width_padding_and_trimmed_prompts() {
        let prompt = py_prompt();
        // Padded to terminal width by capture-pane -N
        assert!(prompt.is_idle_prompt(">>>                 "));
        // Prompt line with content after the prompt is not idle
        assert!(!prompt.is_idle_prompt(">>> 2 + 2                 "));
    }

    #[test]
    fn extracts_last_command_and_output() {
        let prompt = py_prompt();
        let lines = split_lines(">>> print(1)\n1\n>>> \n");
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd.as_deref(), Some("print(1)"));
        assert_eq!(out.as_deref(), Some("1"));
    }

    #[test]
    fn extraction_returns_none_when_no_complete_block() {
        let prompt = py_prompt();
        let lines = split_lines(">>> print(1)\n1\n");
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd, None);
        assert_eq!(out, None);
    }

    #[test]
    fn strip_prompt_removes_only_a_prefix() {
        let prompt = py_prompt();
        assert_eq!(prompt.strip_prompt(">>> print(1)"), "print(1)");
        assert_eq!(
            prompt.strip_prompt("note: see >>> for help"),
            "note: see >>> for help"
        );
    }

    #[test]
    fn warns_when_custom_prompt_matches_bare_output() {
        let bad = r"^ *[0-9]+(\[[0-9]+\])?".to_string();
        let safe = r"^ *[0-9]+(\[[0-9]+\]|\]) ?".to_string();
        let warnings = validate_custom_prompts(&[bad, safe, "^>>> ".to_string()]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("1024"));
        assert!(warnings[0].contains("bare idle prompt"));
    }

    #[test]
    fn sent_command_repairs_rlwrap_unprefixed_echo() {
        let prompt = sbcl_prompt();
        // rlwrap prefixed the old multi-line command's first line, but not
        // its continuation or the next command's echo. Plain extraction
        // therefore finds the stale `(defun ...)` boundary.
        let lines = split_lines(
            "* (defun foo (n)\n  (* n n))\n(foo 9)\n81\n* \n",
        );
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd.as_deref(), Some("(defun foo (n)"));
        assert_eq!(out.as_deref(), Some("  (* n n))\n(foo 9)\n81"));

        let (cmd, out) = extract_with_sent_command(&lines, &prompt, "(foo 9)");
        assert_eq!(cmd.as_deref(), Some("(foo 9)"));
        assert_eq!(out.as_deref(), Some("81"));
    }

    #[test]
    fn sent_command_is_reported_when_echo_scrolled_out() {
        let prompt = py_prompt();
        let lines = split_lines("lots of output\n>>> \n");
        let (cmd, out) = extract_with_sent_command(&lines, &prompt, "print('lost')");
        assert_eq!(cmd.as_deref(), Some("print('lost')"));
        assert_eq!(out, None);
    }

    #[test]
    fn bash_prompt_matches_full_line_and_command_lines() {
        let prompt = Prompts::from_prompts(&[r"[^$#]+[$#] *".to_string()]).unwrap();
        assert!(prompt.is_idle_prompt("user@host$ "));
        assert!(prompt.is_prompt_line("user@host$ ls"));
    }

    #[test]
    fn last_meaningful_line_skips_blank_lines() {
        let lines = split_lines("a\n\n   \n");
        assert_eq!(last_meaningful_line(&lines), Some("a"));
        assert_eq!(last_meaningful_line(&[]), None);
    }

    #[test]
    fn sbcl_kind_matches_multiple_prompt_styles() {
        let prompt = sbcl_prompt();
        // Top-level prompt, idle
        assert!(prompt.is_idle_prompt("* "));
        assert_eq!(prompt.detect_idle("* "), Some("sbcl"));
        // Debugger prompt with a number, idle
        assert!(prompt.is_idle_prompt("0] "));
        assert_eq!(prompt.detect_idle("0] "), Some("sbcl"));
        // Nested debugger prompt (error inside the debugger), idle
        assert!(prompt.is_idle_prompt("0[2] "));
        assert_eq!(prompt.detect_idle("0[2] "), Some("sbcl"));
        assert!(prompt.is_idle_prompt("0[3]"));
        // Bare numeric output must NOT look like a prompt
        assert!(!prompt.is_idle_prompt("1024"));
        assert!(!prompt.is_prompt_line("1024"));
        // Debugger prompt while a command is on the line
        assert!(prompt.is_prompt_line("0] (restart 1)"));
        assert!(!prompt.is_idle_prompt("0] (restart 1)"));
        assert!(prompt.is_prompt_line("0[2] (error \"nested\")"));
        // ldb prompt
        assert!(prompt.is_idle_prompt("ldb> "));
    }

    #[test]
    fn sbcl_extraction_across_debugger_boundaries() {
        let prompt = sbcl_prompt();
        // A command errored, dropped into the debugger, and was restarted:
        // * (foo)  ->  0] (restart 1)  ->  * 
        let lines =
            split_lines("* (foo)\n\nDebugger invoked...\n0] (restart 1)\nBack to top level.\n* \n");
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd.as_deref(), Some("(restart 1)"));
        assert_eq!(out.as_deref(), Some("Back to top level."));
    }

    #[test]
    fn sbcl_nested_debugger_extraction() {
        let prompt = sbcl_prompt();
        // Nested debugger: command errors inside the debugger, then restart:
        // 0] (error "nested")  ->  0[2] (restart 4)  ->  0] (restart 1)  ->  *
        let lines = split_lines(
            "0] (error \"nested\")\n\nError in debugger...\n0[2] (restart 4)\n0] (restart 1)\nBack to top level.\n* \n",
        );
        assert!(is_repl_ready(&prompt, &lines));
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd.as_deref(), Some("(restart 1)"));
        assert_eq!(out.as_deref(), Some("Back to top level."));
    }

    #[test]
    fn strip_prompt_uses_first_matching_pattern() {
        let prompt = sbcl_prompt();
        assert_eq!(prompt.strip_prompt("0] (restart 1)"), "(restart 1)");
        assert_eq!(prompt.strip_prompt("* (foo)"), "(foo)");
    }

    #[test]
    fn multi_line_text_joins_lines_with_closing_blank_line() {
        let text = build_multi_line_text(&[
            "for i in range(3):".to_string(),
            "    print(i)".to_string(),
        ]);
        assert_eq!(text, "for i in range(3):\n    print(i)\n");
    }

    #[test]
    fn multi_line_text_single_line_gets_closing_enter() {
        let text = build_multi_line_text(&["2 + 3".to_string()]);
        assert_eq!(text, "2 + 3\n");
    }

    #[test]
    fn detect_idle_index_returns_pattern_position() {
        let prompt = py_prompt();
        assert_eq!(prompt.detect_idle_index(">>> "), Some(0));
        assert_eq!(prompt.detect_idle_index(">>> 2 + 2"), None);
        assert_eq!(prompt.detect_idle_index("garbage"), None);
    }

    #[test]
    fn idle_matches_index_respects_require_index() {
        let prompt =
            Prompts::from_prompts(&["^>>> ".to_string(), r"^\.\.\. ".to_string()]).unwrap();
        // A bare continuation prompt idle-matches its own index but not the
        // top-level index — that is how the multi-line wait loop avoids
        // returning while a slow block is still running.
        assert!(prompt.idle_matches_index(Some(0), ">>> "));
        assert!(!prompt.idle_matches_index(Some(0), "... "));
        assert!(prompt.idle_matches_index(Some(1), "... "));
        assert!(prompt.idle_matches_index(None, "... "));
    }

    #[test]
    fn multiline_block_extraction_skips_bare_continuation_prompts() {
        let prompt =
            Prompts::from_prompts(&["^>>> ".to_string(), r"^\.\.\. ".to_string()]).unwrap();
        // A multi-line python block: the closing blank line echoes a bare
        // "..." prompt, which must not be taken as the command start, and
        // must not leak into the output.
        // capture-pane -N pads lines to the terminal width, so bare
        // continuation prompts carry trailing spaces.
        let lines = split_lines(
            ">>> for i in range(3):\n...     print(\"row\", i)          \n...                    \nrow 0                  \nrow 1                  \n>>>                    \n",
        );
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd.as_deref(), Some("print(\"row\", i)"));
        assert_eq!(out.as_deref(), Some("row 0\nrow 1"));
    }
}
