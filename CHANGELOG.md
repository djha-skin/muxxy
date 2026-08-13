# Changelog

All notable changes to muxxy are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.0] - 2026-08-13

### Added

- **`execute-command --lines`** — repeatable `--lines` sends a multi-line
  command in one call. The block is closed with a trailing blank line
  automatically, and the wait loop requires the *same* prompt pattern that
  was idle up front to reappear — so python's bare `...` continuation echo
  before a slow block is no longer mistaken for completion. Verified live
  against fast and slow multi-line blocks.

### Changed

- **`--timeout` now defaults to 60 seconds** instead of waiting forever;
  pass `--timeout 0` for the old unbounded behavior.

## [0.4.0] - 2026-08-13

### Fixed

- **`irb` kind no longer matches modern Ruby.** Ruby 3.x prints
  `irb(main):001> ` while older irb printed an extra line-number field
  (`irb(main):001:0> `), so `--kind irb` failed to detect the prompt on
  Ruby >= 3.0. The pattern now matches both formats, with a regression
  test covering each. Discovered while live-testing the README examples
  against a real `irb` session.

### Changed

- **README "Usage" rewritten** to show the tool working with *any* REPL.
  The examples now walk through Python multi-line blocks (continuation
  prompts included), Ruby/irb, SBCL, and the SBCL debugger — replacing the
  previous Lisp-heavy blurb with concrete commands that give the right
  first impression: muxxy works with any REPL, Lisp ones included.

## [0.3.0] - 2026-08-13

### Added

- **Agent skill** (`.agents/skills/muxxy/SKILL.md`) — how to drive Python,
  shell, and SBCL REPLs, the visible/headless working modes, and the
  gotchas with workarounds.
- **`split-pane --directory`** — start the new pane in a given directory
  (native `tmux split-window -c`).
- **Logo** (`docs/muxxy.png`) in the README.
- First crates.io release.

## [0.2.0] - 2026-08-13

### Added

- **Multiple prompts** — `--prompt` is repeatable; any pattern is a valid
  block boundary (e.g. SBCL's `* `, `0]`, `ldb> ` debugger prompts).
- **`--kind` presets** — built-in table (`python`, `ipython`, `bash`,
  `sh`, `zsh`, `node`, `irb`, `iex`, `lisp`, `sbcl`, `goose`), overridable
  via `TMUX_REPL_KINDS`.
- **Pane setup tooling** — `split-pane` (with `--command`/`--sleep`
  setup sequences) and `send-keys` (repeatable, with `--sleep`).
- **YAML output** — multiline output as literal block scalars,
  byte-identical to the REPL's own output yet still parseable.
- **tmux-backed integration tests** — isolated per-test servers.

### Fixed

- Readiness now uses bare-prompt semantics (an echoed
  `>>> time.sleep(5)` is reported busy).
- Captures use `tmux capture-pane -N` with a whitespace-tolerant idle
  check so prompt regexes ending in a literal space match.

## [0.1.0] - 2026-08-13

### Added

- Initial CLI mirroring the `tmux-repl-mcp` MCP server's tools:
  `is-repl-ready`, `get-last-command`, `execute-command`.
- Talks to tmux through `tmux_interface`'s typed command builders.
- `--pane`, `--max-lines`, `--socket`, `--check`, `--timeout`, `--pretty`
  options.
- Prompt matching, last-command extraction, and an execute/wait loop that
  polls the pane rather than guessing at `sleep` durations.

[0.5.0]: https://github.com/djha-skin/muxxy/releases/tag/v0.5.0
[0.4.0]: https://github.com/djha-skin/muxxy/releases/tag/v0.4.0
[0.3.0]: https://github.com/djha-skin/muxxy/releases/tag/v0.3.0
[0.2.0]: https://github.com/djha-skin/muxxy/releases/tag/v0.2.0
[0.1.0]: https://github.com/djha-skin/muxxy/releases/tag/v0.1.0
