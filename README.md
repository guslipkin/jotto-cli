# Jotto & Wordle CLI

Fast, lightweight Rust CLI versions of the classic **Jotto** and **Wordle** 5-letter word games (ported from [`guslipkin/jotto`](https://github.com/guslipkin/jotto)). Installing this crate provides both the `jotto` and `wordle` commands.

## Rules

### Jotto (`jotto`)
Jotto is a word-deduction game where you try to guess the secret five-letter word:
- The secret word has **5 unique letters** (no repeating letters).
- The word is **not a proper noun**.
- After each valid 5-letter guess, you are told **how many letters your guess shares in common** with the secret word (regardless of position).

### Wordle (`wordle`)
Wordle gives you **6 guesses** to find the secret five-letter word:
- **Repeating letters are allowed** (uses the full 5-letter dictionary for the selected difficulty).
- The word is **not a proper noun**.
- After each guess, each letter is marked with colorblind-safe feedback:
  - `+X+` (Green): Letter is in the word and in the **exact position**.
  - `?X?` (Orange): Letter is in the word, but in a **different position**.
  - `~X~` (Gray / Crossed Out): Letter is **not in the word** (or is an excess duplicate).

## Features

- **Two Games, One Shared Codebase (`jotto` & `wordle`)**:
  - **Full-screen Terminal UI (TUI)** by default with a live Guess box, Used Letters tracker (manual Scratch pad in Jotto; auto-colored letter states in Wordle), and History / 6-Guess Board.
  - **Plain / REPL Mode (`--plain`)** for line-by-line terminal sessions or non-TTY pipes.
- **Minimal Word Recycling**:
  - **Daily Mode (`--daily` or `Ctrl+T`)**: Uses a deterministic 90%-pool cooldown (~14.2 years for Jotto, ~22.0 years for Wordle) on the Scrabble (`80`) word list before any daily word can repeat.
  - **Random Mode**: Tracks dealt words across resets so a word never repeats in a session until the entire difficulty pool has been played.
- **Fresh SCOWL Dictionary at Build Time**:
  - Pulls the latest word lists directly from [`en-wl/wordlist`](https://github.com/en-wl/wordlist) during `cargo build` via `build.rs` and embeds a single compact dictionary shared by both games.
- **Minimal Dependencies**:
  - Only 4 direct dependencies (`ratatui`, `crossterm`, `clap`, `fastrand`).

## Installation

### Pre-built Binaries

Download the latest `jotto` and `wordle` binaries for Linux, macOS, or Windows from the [GitHub Releases](https://github.com/guslipkin/jotto-cli/releases) page.

### Build from Source

Requires Rust (`cargo`), `git`, and Python 3 (for SCOWL database creation at build time; includes a pure-Rust fallback parser):

```bash
cargo install --path .
```

## Usage

```bash
# Launch Jotto or Wordle in full-screen TUI mode (default difficulty: Scrabble / 80)
jotto
wordle

# Launch the deterministic UTC Daily Challenge
jotto --daily
wordle --daily

# Launch with a specific difficulty level (easy, medium, hard, scrabble, insane)
jotto --difficulty hard
wordle --difficulty easy

# Launch in line-by-line Plain / REPL mode
jotto --plain
wordle --plain
```

### TUI Controls

All bottom bar buttons (**Guess**, **Help**, **New Game**, **Daily**, **Give Up**, **Difficulty**, **Quit**) as well as the **Guess** input and **Scratch** letters (in Jotto) are clickable with the mouse.

| Key / Action | Description |
| --- | --- |
| `Enter` or Click **Guess** | Submit 5-letter guess |
| Click Scratch Letter *(Jotto)* | Click any letter `A–Z` in the **Scratch** box to cycle `Default → ~X~ Scratched Out → +X+ Positive` without leaving the Guess box |
| `Tab` *(Jotto)* | Switch keyboard focus between **Guess** box and **Scratch** pad (in Scratch: type `a–z` or use arrows + `Space` to toggle states) |
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
- `:history`: Display guess history and used letters
- `:scratch [letters]` *(Jotto)*: Toggle scratchpad states (`~X~` → `+X+` → default) for given letters
- `:new [difficulty]`: Start a new random game
- `:daily`: Start today's deterministic UTC daily challenge
- `:difficulty [level]`: View or change the difficulty level
- `:giveup`: Reveal the secret word
- `:credits`: Show author and dictionary credits
- `:quit` or `:exit`: Exit

## Difficulty Levels (SCOWL)

Each difficulty setting includes all words from the levels before it:
- **Easy (`35`)**: Recommended small dictionary (`2,443` Jotto words / `3,615` Wordle words).
- **Medium (`50`)**: Recommended medium dictionary (`3,157` Jotto words / `4,722` Wordle words).
- **Hard (`70`)**: Recommended large dictionary (`4,768` Jotto words / `7,309` Wordle words).
- **Scrabble (`80`)**: Strange and unusual words used in word games like Scrabble (`5,768` Jotto words / `8,920` Wordle words, default).
- **🤷 Insane (`95`)**: Every valid 5-letter English word in SCOWL (`7,463` Jotto words / `11,542` Wordle words).

## Credits & License

- Written by [Gus Lipkin](https://guslipkin.me).
- Word list from [SCOWL (Spell Checker Oriented Word Lists)](http://wordlist.aspell.net/) by Kevin Atkinson (see [`LICENSE.SCOWL`](LICENSE.SCOWL)).

