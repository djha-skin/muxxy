# muxxy

A command-line tool for interacting with a REPL running inside a
[tmux](https://github.com/tmux/tmux) pane — a Rust CLI mirror of the
[`tmux-repl-mcp`](https://github.com/djha-skin/tmux-repl-mcp) MCP server's
three tools. Where the MCP server keys off named REPL "kinds", muxxy takes a
single `--prompt` regex, so any REPL — Python, `irb`, Lisp, a shell, whatever —
works as long as you can describe its prompt.

It talks to tmux through the [`tmux_interface`](https://crates.io/crates/tmux_interface)
Rust crate's typed command builders.

## Usage

```text
muxxy [OPTIONS] --prompt <PROMPT> <COMMAND>
```

`--prompt` is a Rust `regex` syntax pattern matched against the last line of
the pane, e.g. `'^>>> '` for a Python REPL or `'[^$#]+[$#] *'` for a shell.

### `is-repl-ready`

Check whether the pane is currently showing a bare prompt (i.e. the REPL is
idle and ready for input). Returns `{"kind": <prompt>, "is_ready": true}` when
idle, or `{"kind": null, "is_ready": false}` when busy or unrecognised:

```bash
muxxy --prompt '^>>> ' is-repl-ready
# {"kind":"^>>> ","is_ready":true}
```

### `get-last-command`

Look back through the pane history for a complete prompt → command → output →
prompt block and return the last command and its output:

```bash
muxxy --prompt '^>>> ' get-last-command
# {"last_command":"2 + 3","output":"5"}
```

Both fields are `null` when no complete block is found or the REPL is busy.

### `execute-command`

Send a command to the REPL and wait — polling the pane, never guessing at
`sleep` durations — until the REPL is idle again, then return the output:

```bash
muxxy --prompt '^>>> ' execute-command '2 + 3' --check 0.2
# {"status":"ok","last_command":"2 + 3","output":"5"}
```

Steps:

1. Verifies the REPL is idle and showing the `--prompt` pattern.
2. Sends the command via `tmux send-keys`.
3. Waits (polling every `--check` seconds) until the prompt reappears.
4. Returns `{"status": "ok", "last_command": ..., "output": ...}`.

If the REPL is busy or no prompt is detected up front, it returns
`{"status": "error", "reason": ...}` instead.

## Options

| Flag | Description | Default |
|---|---|---|
| `--prompt <REGEX>` | Prompt pattern for the REPL (required) | — |
| `--pane <PANE>` | tmux pane target, e.g. `0` or `mysess:0.0` | `0` |
| `--max-lines <N>` | Lines to capture from the pane | `200` |
| `--socket <PATH>` | tmux server socket path (`tmux -S`) | default server |
| `--check <SECS>` | Poll interval for `execute-command` | `2.0` |
| `--timeout <SECS>` | Abort `execute-command` after this long (`0` = forever) | `0` |
| `--pretty` | Pretty-print JSON output | off |

## Build

```bash
cargo build --release     # binary at target/release/muxxy
cargo install --path .    # install into ~/.cargo/bin
```

## Development

```bash
cargo test      # unit tests for prompt matching and extraction
cargo clippy    # lints
```

## Notes

- Captures use `tmux capture-pane -p -N` so prompt patterns ending in a
  literal space (e.g. `^>>> `) match despite tmux trimming trailing
  whitespace; output lines have trailing whitespace trimmed.
- Readiness uses "bare prompt" semantics: a line like `>>> time.sleep(5)`
  *starts* with the prompt but the REPL is busy, so it is not reported ready.
- Like the MCP server, `execute-command` waits indefinitely by default; pass
  `--timeout` to bound the wait.
