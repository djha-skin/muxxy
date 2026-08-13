//! muxxy — interact with a REPL running inside a tmux pane, from the
//! command line. A CLI mirror of the `tmux-repl-mcp` MCP server's tools,
//! driven by prompt regexes (repeatable, or via `--kind` presets) instead
//! of a single named kind.

mod core;
mod kinds;
mod tmux;
mod yaml;

use clap::{Parser, Subcommand as ClapSubcommand};
use core::{extract_last_command_and_output, is_repl_ready, wait_for_idle, Prompts};
use std::process::ExitCode;
use std::time::Duration;
use yaml::{render_map, YamlValue};

#[derive(Parser)]
#[command(
    name = "muxxy",
    version,
    about = "Interact with a REPL running inside a tmux pane"
)]
struct Cli {
    /// Prompt regex for the REPL (Rust regex syntax); repeatable for REPLs
    /// with several prompt styles, e.g. '^>>> ' or '^ *[0-9]+\\] ?'
    #[arg(long, value_name = "REGEX")]
    prompt: Vec<String>,

    /// Built-in REPL kind preset (python, bash, sbcl, lisp, irb, ...); repeatable
    #[arg(long, value_name = "KIND")]
    kind: Vec<String>,

    /// tmux pane target, e.g. "0", "mysess:0.0", "%1"
    #[arg(short = 't', long, global = true, default_value = "0")]
    pane: String,

    /// Maximum number of lines to capture from the pane
    #[arg(long, global = true, default_value_t = 200)]
    max_lines: usize,

    /// tmux server socket path (defaults to the default server)
    #[arg(long, global = true)]
    socket: Option<String>,

    #[command(subcommand)]
    command: Subcommand,
}

#[derive(ClapSubcommand)]
enum Subcommand {
    /// Check whether the pane is currently showing a bare prompt (idle)
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
    /// Split the pane, creating a new pane beside it; print the new pane's id
    SplitPane {
        /// Split with the new pane below (stacked) instead of beside
        #[arg(long, conflicts_with = "horizontal")]
        vertical: bool,

        /// Split with the new pane beside (side by side)
        #[arg(long)]
        horizontal: bool,

        /// New pane size: lines ("20") or percent ("50%")
        #[arg(long)]
        size: Option<String>,

        /// Setup command to send to the new pane (repeatable, in order)
        #[arg(long)]
        command: Vec<String>,

        /// Seconds to sleep after the corresponding setup command
        /// (repeatable, defaults to 0)
        #[arg(long, allow_hyphen_values = true)]
        sleep: Vec<f64>,
    },
    /// Send one or more commands (each followed by Enter) to the pane
    SendKeys {
        /// Command(s) to send to the pane
        #[arg(required = true)]
        commands: Vec<String>,

        /// Seconds to sleep between commands
        #[arg(long, default_value_t = 0.0)]
        sleep: f64,
    },
    /// Destroy the pane
    KillPane,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // split-pane, send-keys and kill-pane never need prompt patterns.
    let result = match &cli.command {
        Subcommand::SplitPane {
            vertical,
            horizontal,
            size,
            command,
            sleep,
        } => run_split_pane(&cli, *vertical, *horizontal, size.as_deref(), command, sleep),
        Subcommand::SendKeys { commands, sleep } => run_send_keys(&cli, commands, *sleep),
        Subcommand::KillPane => run_kill_pane(&cli),
        command => {
            let prompts = match build_prompts(&cli) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("muxxy: {e}");
                    return ExitCode::from(2);
                }
            };
            match command {
                Subcommand::IsReplReady => run_is_repl_ready(&prompts, &cli),
                Subcommand::GetLastCommand => run_get_last_command(&prompts, &cli),
                Subcommand::ExecuteCommand {
                    command,
                    check,
                    timeout,
                } => run_execute_command(&prompts, &cli, command, *check, *timeout),
                _ => unreachable!("prompt-less subcommands handled above"),
            }
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("muxxy: {e}");
            ExitCode::FAILURE
        }
    }
}

fn build_prompts(cli: &Cli) -> Result<Prompts, String> {
    if cli.kind.is_empty() && cli.prompt.is_empty() {
        return Err("no prompts configured: pass --prompt REGEX or --kind KIND".into());
    }
    let kinds = kinds::load();
    Prompts::from_kinds(&cli.kind, &kinds, &cli.prompt)
}

fn run_is_repl_ready(prompt: &Prompts, cli: &Cli) -> Result<(), String> {
    let captured = tmux::capture_pane(&cli.pane, cli.max_lines, cli.socket.as_deref())?;
    let lines = core::split_lines(&captured);
    let kind = core::detect_idle_kind(prompt, &lines);
    let out = render_map(&[
        (
            "kind",
            match kind {
                Some(k) => YamlValue::Str(k),
                None => YamlValue::Null,
            },
        ),
        ("is_ready", YamlValue::Bool(kind.is_some())),
    ]);
    print!("{out}");
    Ok(())
}

fn run_get_last_command(prompt: &Prompts, cli: &Cli) -> Result<(), String> {
    let captured = tmux::capture_pane(&cli.pane, cli.max_lines, cli.socket.as_deref())?;
    let lines = core::split_lines(&captured);
    let (last_command, output) = extract_last_command_and_output(&lines, prompt);
    let out = render_map(&[
        ("last_command", opt_str(last_command.as_deref())),
        ("output", opt_str(output.as_deref())),
    ]);
    print!("{out}");
    Ok(())
}

fn run_execute_command(
    prompt: &Prompts,
    cli: &Cli,
    command: &str,
    check: f64,
    timeout: f64,
) -> Result<(), String> {
    // Pre-flight: is the REPL idle and showing one of our prompts?
    let captured = tmux::capture_pane(&cli.pane, cli.max_lines, cli.socket.as_deref())?;
    let lines = core::split_lines(&captured);
    if !is_repl_ready(prompt, &lines) {
        let out = render_map(&[
            ("status", YamlValue::Str("error")),
            (
                "reason",
                YamlValue::Str("REPL is not ready (no prompt detected)."),
            ),
            ("last_command", YamlValue::Null),
            ("output", YamlValue::Null),
        ]);
        print!("{out}");
        return Ok(());
    }

    // Send the command, then wait until the REPL is idle again. The
    // pre-flight capture doubles as the pre-send state for change detection.
    tmux::send_keys(&cli.pane, command, cli.socket.as_deref())?;
    let final_lines = wait_for_idle(
        &cli.pane,
        prompt,
        cli.max_lines,
        check,
        Some(timeout),
        cli.socket.as_deref(),
        &captured,
    )?;
    let final_refs: Vec<&str> = final_lines.iter().map(String::as_str).collect();
    let (last_command, output) = extract_last_command_and_output(&final_refs, prompt);

    let out = render_map(&[
        ("status", YamlValue::Str("ok")),
        ("last_command", opt_str(last_command.as_deref())),
        ("output", opt_str(output.as_deref())),
    ]);
    print!("{out}");
    Ok(())
}

fn run_split_pane(
    cli: &Cli,
    vertical: bool,
    horizontal: bool,
    size: Option<&str>,
    commands: &[String],
    sleeps: &[f64],
) -> Result<(), String> {
    let use_vertical = vertical || !horizontal;
    let new_pane = tmux::split_pane(&cli.pane, use_vertical, size, cli.socket.as_deref())?;

    // Feed setup commands to the new pane, sleeping in between.
    for (i, command) in commands.iter().enumerate() {
        tmux::send_keys(&new_pane, command, cli.socket.as_deref())?;
        if let Some(s) = sleeps.get(i)
            && *s > 0.0
        {
            std::thread::sleep(Duration::from_secs_f64(*s));
        }
    }

    let out = render_map(&[("pane", YamlValue::Str(&new_pane))]);
    print!("{out}");
    Ok(())
}

fn run_send_keys(cli: &Cli, commands: &[String], sleep: f64) -> Result<(), String> {
    for (i, command) in commands.iter().enumerate() {
        if i > 0 && sleep > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(sleep));
        }
        tmux::send_keys(&cli.pane, command, cli.socket.as_deref())?;
    }
    Ok(())
}

fn run_kill_pane(cli: &Cli) -> Result<(), String> {
    tmux::kill_pane(&cli.pane, cli.socket.as_deref())
}

fn opt_str(s: Option<&str>) -> YamlValue<'_> {
    match s {
        Some(v) => YamlValue::Str(v),
        None => YamlValue::Null,
    }
}
