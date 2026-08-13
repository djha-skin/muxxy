//! muxxy — interact with a REPL running inside a tmux pane, from the
//! command line. A CLI mirror of the `tmux-repl-mcp` MCP server's three
//! tools, driven by a single `--prompt` regex instead of named REPL kinds.

mod core;
mod tmux;

use clap::{Parser, Subcommand as ClapSubcommand};
use core::{extract_last_command_and_output, is_repl_ready, wait_for_idle, Prompt};
use serde::Serialize;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "muxxy",
    version,
    about = "Interact with a REPL running inside a tmux pane"
)]
struct Cli {
    /// Prompt regular expression for the REPL (Rust regex syntax), e.g. "^>>> "
    #[arg(long, required = true)]
    prompt: String,

    /// tmux pane identifier, e.g. "0" or "%1"
    #[arg(long, global = true, default_value = "0")]
    pane: String,

    /// Maximum number of lines to capture from the pane
    #[arg(long, global = true, default_value_t = 200)]
    max_lines: usize,

    /// tmux server socket path to talk to (defaults to the default server)
    #[arg(long, global = true)]
    socket: Option<String>,

    /// Pretty-print the JSON output
    #[arg(long, global = true)]
    pretty: bool,

    #[command(subcommand)]
    command: Subcommand,
}

#[derive(ClapSubcommand)]
enum Subcommand {
    /// Check whether the pane is currently showing a prompt matching --prompt
    IsReplReady,
    /// Return the last command and its output from the pane history
    GetLastCommand,
    /// Send a command to the REPL and wait for it to finish, then return output
    ExecuteCommand {
        /// The command text to send to the REPL
        command: String,

        /// Seconds to wait between pane-state polls
        #[arg(long, default_value_t = 2.0)]
        check: f64,

        /// Abort after this many seconds (0 = wait forever)
        #[arg(long, default_value_t = 0.0)]
        timeout: f64,
    },
}

#[derive(Serialize)]
struct ReadyOutput {
    /// The prompt regex that matched, or null when the pane is not ready
    kind: Option<String>,
    is_ready: bool,
}

#[derive(Serialize)]
struct LastCommandOutput {
    last_command: Option<String>,
    output: Option<String>,
}

#[derive(Serialize)]
struct ExecOutput {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    last_command: Option<String>,
    output: Option<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let prompt = match Prompt::new(&cli.prompt) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("muxxy: {e}");
            return ExitCode::from(2);
        }
    };

    let result = match &cli.command {
        Subcommand::IsReplReady => run_is_repl_ready(&prompt, &cli),
        Subcommand::GetLastCommand => run_get_last_command(&prompt, &cli),
        Subcommand::ExecuteCommand {
            command,
            check,
            timeout,
        } => run_execute_command(&prompt, &cli, command, *check, *timeout),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("muxxy: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_is_repl_ready(prompt: &Prompt, cli: &Cli) -> Result<(), String> {
    let captured = tmux::capture_pane(&cli.pane, cli.max_lines, cli.socket.as_deref())?;
    let lines = core::split_lines(&captured);
    let ready = is_repl_ready(prompt, &lines);
    print_json(
        &ReadyOutput {
            kind: ready.then(|| cli.prompt.clone()),
            is_ready: ready,
        },
        cli.pretty,
    );
    Ok(())
}

fn run_get_last_command(prompt: &Prompt, cli: &Cli) -> Result<(), String> {
    let captured = tmux::capture_pane(&cli.pane, cli.max_lines, cli.socket.as_deref())?;
    let lines = core::split_lines(&captured);
    let (last_command, output) = extract_last_command_and_output(&lines, prompt);
    print_json(&LastCommandOutput { last_command, output }, cli.pretty);
    Ok(())
}

fn run_execute_command(
    prompt: &Prompt,
    cli: &Cli,
    command: &str,
    check: f64,
    timeout: f64,
) -> Result<(), String> {
    // Pre-flight: is the REPL ready and showing our prompt?
    let captured = tmux::capture_pane(&cli.pane, cli.max_lines, cli.socket.as_deref())?;
    let lines = core::split_lines(&captured);
    if !is_repl_ready(prompt, &lines) {
        print_json(
            &ExecOutput {
                status: "error".into(),
                reason: Some("REPL is not ready (no prompt detected).".into()),
                last_command: None,
                output: None,
            },
            cli.pretty,
        );
        return Ok(());
    }

    // Send the command, then wait until the REPL is idle again.
    tmux::send_keys(&cli.pane, command, cli.socket.as_deref())?;
    let final_lines = wait_for_idle(
        &cli.pane,
        prompt,
        cli.max_lines,
        check,
        Some(timeout),
        cli.socket.as_deref(),
    )?;
    let final_refs: Vec<&str> = final_lines.iter().map(String::as_str).collect();
    let (last_command, output) = extract_last_command_and_output(&final_refs, prompt);

    print_json(
        &ExecOutput {
            status: "ok".into(),
            reason: None,
            last_command,
            output,
        },
        cli.pretty,
    );
    Ok(())
}

fn print_json<T: Serialize>(value: &T, pretty: bool) {
    let rendered = if pretty {
        serde_json::to_string_pretty(value)
    } else {
        serde_json::to_string(value)
    };
    match rendered {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("muxxy: failed to serialize output: {e}"),
    }
}
