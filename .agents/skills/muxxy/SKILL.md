---
name: muxxy
description: >
    How to drive REPLs (Python, shell, SBCL, ...) running in tmux panes
    using the muxxy CLI, and how to work around its quirks.
---

# muxxy — driving REPLs in tmux panes

`muxxy` is a CLI that talks to a REPL running in a tmux pane: it checks
whether the REPL is idle, sends commands, waits until they finish, and
returns the output as YAML. It is the command-line counterpart of the
`tmux-repl-mcp` MCP server's tools.

```bash
muxxy --prompt '^>>> ' is-repl-ready          # is the REPL idle?
muxxy --prompt '^>>> ' execute-command '2 + 3'  # send + wait + return output
muxxy --prompt '^>>> ' get-last-command       # read the last command + output
```

Target a specific pane with `-t/--pane` (e.g. `0`, `mysess:0.0`, `%1`) and an
alternative tmux server with `--socket`.

## Choosing prompts

Give one or more `--prompt` regexes (Rust `regex` syntax), or use a built-in
`--kind` preset — both can be combined:

| Kind | Prompt styles |
|---|---|
| `python` | `^>>> ` |
| `bash` / `sh` | `^[^$#]+[$#] *` (e.g. `user@host$ `) |
| `zsh` | `^[^$+][$#] *` |
| `node` | `^> ` |
| `janet` | `repl:<line>:> ` and delimiter-aware continuation prompts such as `repl:<line>:(> ` |
| `irb` | `^irb\(.*\):\d+:\d+> $` |
| `sbcl` | `* `, numbered debugger prompts `0]` / nested `0[2]`, `ldb> `, package prompts |
| `lisp` | like `sbcl` without `ldb> ` |
| `ipython` | `^In \[\d+\]: ` |

Custom kinds go in the `TMUX_REPL_KINDS` env var (JSON mapping kind → regex
string or array of regex strings), merged over the built-ins.

## Common languages

### Python

```bash
muxxy --prompt '^>>> ' is-repl-ready
muxxy --prompt '^>>> ' execute-command '[print(i) for i in range(3)]' --check 0.1
# output: 0, 1, 2 (literal block scalar)
```

For multi-line input, include the continuation prompt and send the block with
embedded newlines plus a final blank line:

```bash
muxxy --prompt '^>>> ' --prompt '^\.\.\. ' send-keys $'for i in range(3):\n    print(i)\n'
```

`send-keys` passes key names through to tmux, so `send-keys 'C-c'` interrupts
a stuck block.

### Janet

Janet's default REPL prompt is `repl:<line>:> `. The line number advances as
input is read. For an incomplete form, Janet inserts the parser's nested
open-delimiter state before `> ` — for example, `repl:<line>:(> ` for an
unfinished parenthesized form. The `janet` kind includes both the top-level and
continuation patterns, so multi-line forms can wait for the final top-level
prompt:

```bash
muxxy --kind janet is-repl-ready
muxxy --kind janet execute-command '(+ 40 2)'
muxxy --kind janet execute-command --lines '(+ 10' --lines ' 32)'
```

The Janet documentation describes this state as `parser/state :delimiters`;
it contains nested parentheses, brackets, braces, or string markers.

### Shell

```bash
muxxy --kind bash execute-command 'ls -la'        # any shell prompt
muxxy --kind bash execute-command 'echo hi && sleep 2 && echo done'
```

Shell prompts often carry the hostname/path (`user@host:~/proj$ `); the
`bash`/`zsh` kinds match any leading text before `$`/`#`. Watch out: a
command still running shows its echoed command line as the last line, and
muxxy reports *not ready* until a bare prompt returns.

### SBCL

```bash
muxxy --kind sbcl is-repl-ready
muxxy --kind sbcl execute-command '(ql:quickload :my-system)'
```

The SBCL **debugger prompt is a valid command boundary**: after a command
errors and drops into the debugger (prompt `0]`), muxxy reports ready and you
can keep executing there — run `(describe 42)` or invoke a restart:

```bash
muxxy --kind sbcl execute-command '4' --timeout 10   # numbered restart: exit debugger
```

**Exiting the debugger: use the numbered restart, not `(abort)`.** SBCL
evaluates `(abort)` inside the debugger's own evaluation context, which does
*not* exit to top level — the prompt stays at `0]`. Send the restart number
instead (`0`–`n` as printed in the restart list, e.g. `1` or `4` for *Exit
debugger, returning to top level*).

**Nested debugger prompts are supported.** An error raised *inside* the
debugger (level ≥ 2) changes the prompt to `0[2]` (number `[level]`); the
built-in `sbcl` kind regex `^ *[0-9]+(\[[0-9]+\]|\]) ?` matches both `0]`
and `0[2]`, so `execute-command` keeps working at any debugger level. If you
specify custom prompts instead, remember to include **all** prompt styles
(top-level `^\* ` and the debugger form) or a style transition will time out:

```bash
muxxy --prompt '^\* ' --prompt '^ *[0-9]+(\[[0-9]+\]|\])' is-repl-ready
```

**Multi-line input confuses rlwrap's echo.** `send-keys` (or `execute-command`
with embedded newlines) makes rlwrap echo the input lines *without* the `* `
prompt prefix, so muxxy loses the command boundary: `last_command` comes back
stale and `output` includes earlier history. Prefer single-line forms or
`execute-command --lines ...` when you need clean output capture, and pass
`--max-lines` generously for anything that prints a lot (loads, test runs).

## Setting up a REPL pane (agent-driven)

The AI can set up its own pane, no human needed:

```bash
muxxy split-pane --command 'sbcl' --sleep 3       # split, start REPL, wait
# pane: "%1"                                        → then target it:
muxxy --pane '%1' --kind sbcl execute-command '(+ 1 2)'
muxxy --pane '%1' --kind sbcl get-last-command
muxxy --pane '%1' kill-pane                       # tear it down
```

`split-pane` takes repeatable `--command`/`--sleep` pairs for a setup
sequence, `--vertical`, and `--size` (lines or percent). Use `--directory`
to set the new pane's start directory (`tmux split-window -c`), and prefix
commands with `cd <dir> &&` when the user's shell startup (e.g. a
`pchanged`-style hook) cds elsewhere on launch.

## Working visibly vs headless

**Visible mode — when the user is in tmux and wants to watch.** If the user
says something like *"I'm in tmux, keep things visible"*, split a pane **by
default** in their session and run the REPL there, so they can see exactly
what you're doing:

```bash
muxxy split-pane --directory ~/Code/proj \
  --command 'cd ~/Code/proj && sbcl' --sleep 3
# pane: "%1"
muxxy --pane '%1' --kind sbcl execute-command '(foo)'
# ... and tear it down when done:
muxxy --pane '%1' kill-pane
```

For *several* visible panes (e.g. one per repo), split again — each
`split-pane` prints its own new pane id, and you target one pane per
invocation with `--pane`. Commands in one pane never leak into another.

**Headless mode — run things in your own session.** For isolated work with
nothing visible, use a dedicated tmux server/socket of your own and target it
with `--socket`:

```bash
tmux -L mywork new-session -d -s work 'sbcl'     # your own hidden session
muxxy --socket /tmp/tmux-1000/mywork --pane 0 --kind sbcl execute-command '(foo)'
```

Prefer visible mode when the user is working in tmux; prefer headless when
they asked you to work quietly or in the background.

## Gotchas and workarounds

1. **A "busy" REPL is not ready.** The echoed command line
   (`>>> time.sleep(5)`) *starts* with the prompt but the REPL is working.
   muxxy uses bare-prompt readiness, so `is-repl-ready` returns false and
   `execute-command` refuses with `status: error` — check before sending, or
   just retry after a beat.
2. **Multi-line input parks the REPL.** Sending a `for x:` block leaves
   Python at the `...` continuation prompt. Close blocks with a trailing
   blank line, and put the continuation prompt in your prompt set. If a REPL
   parks anyway, `execute-command` waits forever by default — always pass
   `--timeout` when the command might not complete.
3. **SBCL debugger prompts are normal prompts.** `0]`, `1]`, `0[2]`, `ldb> `
   are all valid ready states — you can type at them. The `sbcl` kind
   matches the nested debugger form `0[2]` too. To leave the debugger, send
   the numbered restart (`1`, `4`, ...) — `(abort)` does not exit (see the
   SBCL section above). If you pass custom prompts, include every prompt
   style the REPL may show or a style transition will time out.
4. **Trailing-space prompts.** tmux trims trailing spaces and pads lines to
   the terminal width; muxxy captures with `-N` and tolerates both, so
   patterns like `^>>> ` (with the space) work.
5. **Control keys go through `send-keys`.** `send-keys 'C-c'` is a real
   interrupt; use it to unstick a REPL.
6. **Empty output is `""`, not `null`.** `x = 5` returns `output: ""`.
7. **Output is YAML, not JSON.** Multiline output is a literal block scalar
   (`|-`), nearly identical to what the REPL printed. Strings that look like
   numbers or booleans are quoted (`output: "5"`) so they round-trip as
   strings.
8. **One pane per invocation.** Target the pane with `--pane`; for "send to
   both panes" just invoke twice. `split-pane` prints the new pane id —
   capture it, then use it.
9. **History depth.** `get-last-command` and `execute-command` look back
   `--max-lines` (default 5000); raise it for panes with very deep history.
   Commands that print heavily (system loads, test suites, long listings)
   scroll their own command line out of the capture window, so
   `execute-command` comes back with `last_command: null, output: null`
   even though it ran fine — pass `--max-lines 20000` (or more) for
   anything that prints extremely heavily.
10. **The last line of a multi-line block is the command.** For a block
    ending in `print(i)`, `last_command` is `print(i)`, not the whole block —
    that is inherent to prompt-line extraction.
11. **Shell startup hooks can move the pane.** If the user's shell config cds
    somewhere on launch (e.g. a `pchanged`-style working-location hook),
    `--directory` is overridden — prefix setup commands with `cd <dir> &&`
    instead.
12. **SBCL starts in `CL-USER`.** Systems are not auto-loaded, so
    `(find-package :foo)` returns NIL until the system is loaded. The SBCL
    prompt is `* ` (kind `sbcl`); give slow startup enough `--sleep` in
    `split-pane`.
13. **Prompt regexes are matched against output lines too.** A `--prompt`
    regex that can match a *prefix* of ordinary output (e.g. `^ *[0-9]+...`
    matching the number `1024`) will make muxxy treat that output line as a
    prompt: it gets stripped from `output` and can even look idle. muxxy now
    restricts history matching/stripping to line prefixes and warns when a
    custom pattern matches common output samples. Keep patterns anchored to
    the real prompt shape (the `sbcl` kind requires the trailing `]`, so
    numeric results like `1024` are safe).
14. **Custom prompt sets must cover every prompt style the REPL can show.**
    If a command changes the prompt (SBCL `* ` ↔ debugger `0]`, Python
    `>>> ` ↔ `...`), and your `--prompt` set only matches the old style,
    `execute-command` will time out waiting for the new style. `--kind`
    presets bundle all styles for this reason; when writing custom regexes
    include them all.
15. **rlwrap echoes multi-line input without the prompt prefix.** After a
    `send-keys` multi-line block, the echoed lines lack the `* ` prefix, so
    `get-last-command` can report stale `last_command` and history-mixed
    `output`. `execute-command` now uses the command it sent to repair this
    when the un-prefixed echo is still visible. For `get-last-command`, this
    remains an rlwrap display limitation: close blocks with a trailing blank
    line, then run a throwaway command (`(+ 1 1)`) to re-establish a clean
    boundary before trusting history.
