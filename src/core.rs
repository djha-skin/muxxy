//! Core logic: prompt matching, command/output extraction, and the
//! execute-command wait loop. This mirrors the behavior of the Python
//! `tmux-repl-mcp` server's core module.

use crate::tmux;
use regex::Regex;
use std::time::{Duration, Instant};

/// A REPL prompt pattern, with the two matching modes the reference
/// implementation uses: search (find prompt lines in history) and
/// full-match (detect that the REPL is idle again).
pub struct Prompt {
    regex: Regex,
}

impl Prompt {
    /// Compile a prompt pattern (Rust `regex` crate syntax).
    pub fn new(pattern: &str) -> Result<Self, String> {
        let regex =
            Regex::new(pattern).map_err(|e| format!("invalid prompt regex: {e}"))?;
        Ok(Self { regex })
    }

    /// Search semantics (like Python `re.search`): true when the pattern
    /// matches anywhere in the line. Used to find prompt lines in history
    /// and to check readiness.
    pub fn is_prompt_line(&self, line: &str) -> bool {
        self.regex.is_match(line)
    }

    /// Idle semantics: true when the pattern matches a prefix of the line
    /// and everything after it is whitespace. Used to detect that the REPL
    /// is idle again after a command. Trailing whitespace is tolerated so
    /// that pane captures padded to the terminal width (and tmux's own
    /// trimming of trailing spaces) do not defeat prompt patterns that end
    /// in a literal space, e.g. `^>>> `.
    pub fn is_idle_prompt(&self, line: &str) -> bool {
        self.regex.find(line).is_some_and(|m| {
            m.start() == 0 && line[m.end()..].chars().all(|c| c.is_whitespace())
        })
    }

    /// Strip the first match of the prompt from the line and trim the
    /// result (like Python `re.sub(pattern, "", line, count=1).strip()`).
    pub fn strip_prompt(&self, line: &str) -> String {
        self.regex.replace(line, "").trim().to_string()
    }
}

/// Split captured pane text on newlines (like Python `str.split("\n")`).
pub fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// The last non-whitespace line, or `None` if there is none.
pub fn last_meaningful_line<'a>(lines: &[&'a str]) -> Option<&'a str> {
    lines.iter().rev().find(|l| !l.trim().is_empty()).copied()
}

/// True if the last meaningful line of the pane is a bare prompt
/// (i.e. the REPL is idle and ready for input).
///
/// Idle semantics rather than plain search: an echoed command line such as
/// `>>> time.sleep(5)` *starts* with the prompt but the REPL is busy, so it
/// must not count as ready.
pub fn is_repl_ready(prompt: &Prompt, lines: &[&str]) -> bool {
    last_meaningful_line(lines).is_some_and(|l| prompt.is_idle_prompt(l))
}

/// Index of the last prompt line in `lines`, or `None`.
fn last_prompt_index(lines: &[&str], prompt: &Prompt) -> Option<usize> {
    lines.iter().rposition(|l| prompt.is_prompt_line(l))
}

/// Index of the last prompt line strictly before `end_idx`, or `None`.
fn second_to_last_prompt_index(
    lines: &[&str],
    prompt: &Prompt,
    end_idx: usize,
) -> Option<usize> {
    lines[..end_idx].iter().rposition(|l| prompt.is_prompt_line(l))
}

/// Parse `(last_command, output)` out of the pane lines.
///
/// `last_command` is the text of the second-to-last prompt line with the
/// prompt prefix stripped; `output` is everything between that line and
/// the final prompt line. Returns `(None, None)` when no complete
/// prompt → command → output → prompt block can be found.
pub fn extract_last_command_and_output(
    lines: &[&str],
    prompt: &Prompt,
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
    let output = lines[start_idx + 1..end_idx]
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    (Some(last_command), Some(output))
}

/// Poll the pane until its last line full-matches the prompt, meaning the
/// REPL has finished processing and is idle again. Returns the final lines.
pub fn wait_for_idle(
    pane: &str,
    prompt: &Prompt,
    max_lines: usize,
    check: f64,
    timeout: Option<f64>,
    socket: Option<&str>,
) -> Result<Vec<String>, String> {
    let check_dur = Duration::from_secs_f64(check.max(0.001));
    let started = Instant::now();

    loop {
        if let Some(t) = timeout
            && t > 0.0
            && started.elapsed().as_secs_f64() >= t
        {
            return Err(format!(
                "timed out after {t} seconds waiting for the REPL prompt"
            ));
        }

        let captured = tmux::capture_pane(pane, max_lines, socket)?;
        let lines = split_lines(&captured);
        match last_meaningful_line(&lines) {
            Some(last) if prompt.is_idle_prompt(last) => {
                return Ok(lines.into_iter().map(str::to_string).collect());
            }
            _ => std::thread::sleep(check_dur),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn py_prompt() -> Prompt {
        Prompt::new(r"^>>> ").unwrap()
    }

    #[test]
    fn detects_ready_python_prompt() {
        let prompt = py_prompt();
        let lines = split_lines(">>> \n");
        assert!(is_repl_ready(&prompt, &lines));
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
    fn idle_uses_fullmatch_but_detection_uses_search() {
        let prompt = py_prompt();
        assert!(prompt.is_idle_prompt(">>> "));
        assert!(!prompt.is_idle_prompt(">>> 2 + 2"));
        assert!(prompt.is_prompt_line(">>> 2 + 2"));
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
    fn extraction_trims_width_padding_from_output() {
        let prompt = py_prompt();
        let lines = split_lines(">>> print(1)\n1                 \n>>>                 \n");
        let (cmd, out) = extract_last_command_and_output(&lines, &prompt);
        assert_eq!(cmd.as_deref(), Some("print(1)"));
        assert_eq!(out.as_deref(), Some("1"));
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
    fn strip_prompt_removes_prefix() {
        let prompt = py_prompt();
        assert_eq!(prompt.strip_prompt(">>> print(1)"), "print(1)");
    }

    #[test]
    fn bash_prompt_matches_full_line_and_command_lines() {
        let prompt = Prompt::new(r"[^$#]+[$#] *").unwrap();
        assert!(prompt.is_idle_prompt("user@host$ "));
        assert!(prompt.is_prompt_line("user@host$ ls"));
    }

    #[test]
    fn last_meaningful_line_skips_blank_lines() {
        let lines = split_lines("a\n\n   \n");
        assert_eq!(last_meaningful_line(&lines), Some("a"));
        assert_eq!(last_meaningful_line(&[]), None);
    }
}
