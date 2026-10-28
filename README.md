# Jotto CLI

A fast, lightweight Rust CLI version of the classic **Jotto** 5-letter word game (ported from [`guslipkin/jotto`](https://github.com/guslipkin/jotto)).

## Rules of Jotto

Jotto is a word-deduction game where you try to guess the secret five-letter word:
- The secret word has **5 unique letters** (no repeating letters).
- The word is **not a proper noun**.
- After each valid 5-letter guess, you are told **how many letters your guess shares in common** with the secret word (regardless of position).

## Features

- **Hybrid Interface**:
  - **Full-screen Terminal UI (TUI)** by default with a live Guess box, Used Letters tracker, interactive Scratch pad, and scrollable History table.
  - **Plain / REPL Mode (`--plain`)** for line-by-line terminal sessions or non-TTY pipes.
- **Fresh SCOWL Dictionary at Build Time**:
  - Pulls the latest word lists directly from [`en-wl/wordlist`](https://github.com/en-wl/wordlist) during `cargo build` via `build.rs` and embeds the compiled dictionary directly into the standalone binary.
- **Minimal Dependencies**:
  - Only 4 direct dependencies (`ratatui`, `crossterm`, `clap`, `fastrand`).

## Installation

### Pre-built Binaries

Download the latest binary for Linux, macOS, or Windows from the [GitHub Releases](https://github.com/guslipkin/jotto-cli/releases) page.

### Build from Source

Requires Rust (`cargo`), `git`, and Python 3 (for SCOWL database creation at build time; includes a pure-Rust fallback parser):

```bash
cargo install --path .
```

## Usage

```bash
# Launch the full-screen TUI (default difficulty: Scrabble / 80)
jotto

# Launch the deterministic UTC Daily Challenge
jotto --daily

# Launch with a specific difficulty level (easy, medium, hard, scrabble, insane)
jotto --difficulty hard

# Launch in line-by-line Plain / REPL mode
jotto --plain
```

### TUI Controls

All bottom bar buttons (**Guess**, **Help**, **New Game**, **Daily**, **Give Up**, **Difficulty**, **Quit**) as well as the **Guess** input and **Scratch** letters are clickable with the mouse.

| Key / Action | Description |
| --- | --- |
| `Enter` or Click **Guess** | Submit 5-letter guess |
| Click Scratch Letter | Click any letter `A–Z` in the **Scratch** box to cycle `Default → ~X~ Scratched Out → +X+ Positive` without leaving the Guess box |
| `Tab` | Switch keyboard focus between **Guess** box and **Scratch** pad (in Scratch: type `a–z` or use arrows + `Space` to toggle states) |
| `?` or Click **Help** | Open/close Help & Credits modal |
| `Ctrl+N` or Click **New Game** | Start a New Random Game |
| `Ctrl+T` or Click **Daily** | Start today's deterministic UTC Daily Challenge |
| `Ctrl+G` or Click **Give Up** | Give Up and reveal the secret word |
| `Ctrl+D` or Click **Difficulty** | Cycle Difficulty (`Easy` → `Medium` → `Hard` → `Scrabble` → `Insane`) |
| `Up` / `Down` or Mouse Wheel | Scroll guess history |
| `Esc`, `Ctrl+C`, or Click **Quit** | Quit |

### Plain / REPL Commands (`--plain`)

- `<5-letter word>`: Submit a guess
- `:help` or `?`: Display rules and difficulty descriptions
- `:history`: Display guess history, used letters, and scratchpad
- `:scratch [letters]`: Toggle scratchpad states (`~X~` → `+X+` → default) for given letters
- `:new [difficulty]`: Start a new random game
- `:daily [difficulty]`: Start today's deterministic UTC daily challenge
- `:difficulty [level]`: View or change the difficulty level
- `:giveup`: Reveal the secret word
- `:credits`: Show author and dictionary credits
- `:quit` or `:exit`: Exit Jotto

## Difficulty Levels (SCOWL)

Each difficulty setting includes all words from the levels before it:
- **Easy (`35`)**: Recommended small dictionary (~2,400 words).
- **Medium (`50`)**: Recommended medium dictionary (~3,100 words).
- **Hard (`70`)**: Recommended large dictionary (~4,700 words).
- **Scrabble (`80`)**: Strange and unusual words used in word games like Scrabble (~5,700 words, default).
- **🤷 Insane (`95`)**: Every valid 5-letter English word in SCOWL (~7,400 words).

## Credits & License

- Written by [Gus Lipkin](https://guslipkin.me).
- Word list from [SCOWL (Spell Checker Oriented Word Lists)](http://wordlist.aspell.net/) by Kevin Atkinson (see [`LICENSE.SCOWL`](LICENSE.SCOWL)).
