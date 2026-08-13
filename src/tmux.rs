//! Thin wrappers around tmux, built on the `tmux_interface` crate's typed
//! command builders instead of hand-rolled `tmux` subprocess invocations.

use tmux_interface::{CapturePane, SendKeys, Tmux};

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
    let command = SendKeys::new().target_pane(pane).key(command).key("Enter");
    let status = run(command, socket)?
        .status()
        .map_err(|e| format!("failed to run tmux: {e}"))?;

    if !status.success() {
        return Err(format!("tmux send-keys failed for pane {pane:?}"));
    }
    Ok(())
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
