//! Thin wrappers around tmux, built on the `tmux_interface` crate's typed
//! command builders instead of hand-rolled `tmux` subprocess invocations.

use tmux_interface::{CapturePane, KillPane, PaneSize, SendKeys, SplitWindow, Tmux};

/// Capture the last `max_lines` lines of the given tmux pane as a string.
///
/// Equivalent to `tmux capture-pane -t <pane> -p -S -<max_lines>`. When
/// `socket` is given, the command is sent to that server socket instead of
/// the default one (`tmux -S <socket>`).
pub fn capture_pane(
    pane: &str,
    max_lines: usize,
    socket: Option<&str>,
) -> Result<String, String> {
    let command = CapturePane::new()
        .stdout()
        .trailing_spaces() // -N: keep trailing spaces so prompt regexes like "^>>> " match
        .start_line(format!("-{max_lines}"))
        .target_pane(pane);
    let output = run(command, socket)?
        .output()
        .map_err(|e| format!("failed to run tmux: {e}"))?;

    let inner = output.into_inner();
    if !inner.status.success() {
        let stderr = String::from_utf8_lossy(&inner.stderr);
        return Err(format!(
            "tmux capture-pane failed for pane {pane:?}: {}",
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&inner.stdout).into_owned())
}

/// Send `command` followed by Enter to the given tmux pane.
///
/// Equivalent to `tmux send-keys -t <pane> <command> Enter`.
pub fn send_keys(pane: &str, command: &str, socket: Option<&str>) -> Result<(), String> {
    send_keys_raw(pane, command, socket)?;
    Ok(())
}

fn send_keys_raw(pane: &str, command: &str, socket: Option<&str>) -> Result<(), String> {
    let command = SendKeys::new().target_pane(pane).key(command).key("Enter");
    let status = run(command, socket)?
        .status()
        .map_err(|e| format!("failed to run tmux: {e}"))?;

    if !status.success() {
        return Err(format!("tmux send-keys failed for pane {pane:?}"));
    }
    Ok(())
}

/// Split `target` (a pane or window target), creating a new pane beside it,
/// and return the new pane's id (e.g. `%1`).
///
/// Equivalent to `tmux split-window -h|-v [-l <size>|-p <percent>] -P -F '#{pane_id}' -t <target>`.
pub fn split_pane(
    target: &str,
    vertical: bool,
    size: Option<&str>,
    socket: Option<&str>,
) -> Result<String, String> {
    let mut command = SplitWindow::new().print().format("#{pane_id}").target_pane(target);
    if vertical {
        command = command.vertical();
    } else {
        command = command.horizontal();
    }

    let parsed_size = match size {
        Some(s) => Some(parse_size(s)?),
        None => None,
    };
    if let Some(ps) = &parsed_size {
        command = command.size(ps);
    }

    let output = run(command, socket)?
        .output()
        .map_err(|e| format!("failed to run tmux: {e}"))?;

    let inner = output.into_inner();
    if !inner.status.success() {
        let stderr = String::from_utf8_lossy(&inner.stderr);
        return Err(format!(
            "tmux split-window failed for {target:?}: {}",
            stderr.trim()
        ));
    }
    let pane_id = String::from_utf8_lossy(&inner.stdout).trim().to_string();
    if pane_id.is_empty() {
        return Err("tmux split-window printed no pane id".into());
    }
    Ok(pane_id)
}

/// Destroy the given tmux pane.
///
/// Equivalent to `tmux kill-pane -t <pane>`.
pub fn kill_pane(pane: &str, socket: Option<&str>) -> Result<(), String> {
    let status = run(KillPane::new().target_pane(pane), socket)?
        .status()
        .map_err(|e| format!("failed to run tmux: {e}"))?;

    if !status.success() {
        return Err(format!("tmux kill-pane failed for pane {pane:?}"));
    }
    Ok(())
}

fn parse_size(s: &str) -> Result<PaneSize, String> {
    if let Some(pct) = s.strip_suffix('%') {
        pct.trim()
            .parse()
            .map(PaneSize::Percentage)
            .map_err(|_| format!("invalid pane size {s:?}"))
    } else {
        s.trim()
            .parse()
            .map(PaneSize::Size)
            .map_err(|_| format!("invalid pane size {s:?}"))
    }
}

/// Build a [`Tmux`] for `command`, targeting `socket` when given.
fn run<'a, C>(command: C, socket: Option<&'a str>) -> Result<Tmux<'a>, String>
where
    C: Into<tmux_interface::TmuxCommand<'a>>,
{
    let tmux = Tmux::with_command(command);
    Ok(match socket {
        Some(path) => tmux.socket_path(path),
        None => tmux,
    })
}
