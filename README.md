# Jotto, Wordle & Concepts CLI

Fast, lightweight Rust CLI versions of the classic **Jotto** and **Wordle** 5-letter word games (ported from [`guslipkin/jotto`](https://github.com/guslipkin/jotto)), plus **Concepts**, a game of finding a word by its meaning (inspired by [Contexto](https://contexto.me/)). Installing this crate provides the `jotto`, `wordle` and `concepts` commands.

## Rules

### Jotto (`jotto`)
Jotto is a word-deduction game where you try to guess the secret five-letter word:
- The secret word has **5 unique letters** (no repeating letters).
- The word is **not a proper noun**.
- After each valid 5-letter guess, you are told **how many letters your guess shares in common** with the secret word (regardless of position).

### Wordle (`wordle`)
Wordle gives you **6 guesses** to find the secret five-letter word:
- **Repeating letters are allowed** (uses the full 5-letter dictionary for the selected difficulty).
- Like the original, the answer is **never a plural** (or an -s verb form like "tells"), though plurals are fine as guesses.
- The word is **not a proper noun**.
- After each guess, each letter is marked with colorblind-safe feedback:
  - `+X+` (Green): Letter is in the word and in the **exact position**.
  - `?X?` (Orange): Letter is in the word, but in a **different position**.
  - `~X~` (Gray / Crossed Out): Letter is **not in the word** (or is an excess duplicate).

### Concepts (`concepts`)
Concepts asks you to find a secret word by its meaning:
- The secret word is a **common noun**. Guesses can be any common noun, verb, adjective or adverb (not names, places, brands, or function words like "the"). Words that are also names count when they have a common meaning of their own ("robin", "creek", "apple").
- Each guess gets a **rank**: how close its meaning is to the secret word's, out of every ranked word. **#1 is the secret word.** Rarer words ("lemongrass", "ptarmigan") are accepted too: each shares the rank of the ranked word it falls next to, but never shows up in hints or the closest-words list.
- Plurals and other forms count as their base word: "mice" is scored as "mouse", "went" as "go".
- Closeness comes from how words are used together, so a close word can share a topic ("ocean" and "coast") as well as a meaning.
- Guesses are unlimited. A **hint** reveals the word at half your best guess's rank (best #84 reveals #42), or the #300 word before you've guessed. Hints don't count as guesses, so after one you need a closer guess of your own for another.

## Features

- **Three Games, One Shared Codebase (`jotto`, `wordle` & `concepts`)**:
  - **Full-screen Terminal UI (TUI)** by default with a live Guess box, Used Letters tracker (manual Scratch pad in Jotto; auto-colored letter states in Wordle), and History / 6-Guess Board; in Concepts, a closest-first list of guesses with colorblind-safe closeness bars.
  - **Plain / REPL Mode (`--plain`)** for line-by-line terminal sessions or non-TTY pipes.
- **Minimal Word Recycling**:
  - **Daily Mode (`--daily` or `Ctrl+T`)**: Uses a deterministic cooldown of 90% of the Scrabble (`80`) word list's size in days (well over a decade for both games) before any daily word can repeat.
  - **Random Mode**: Tracks dealt words across resets so a word never repeats in a session until the entire difficulty pool has been played.
  - **Concepts** daily words are always Medium: a fixed shuffle of its Medium secret nouns, one a day, so none repeats until every one has been used (several years).
- **Fresh SCOWL Dictionary at Build Time**:
  - Pulls the latest word lists directly from [`en-wl/wordlist`](https://github.com/en-wl/wordlist) during `cargo build` via `build.rs` and embeds a single compact dictionary shared by Jotto and Wordle.
- **Offline Word Meanings (Concepts)**:
  - `data/concepts.bin` (~18 MB, embedded in the `concepts` binary) holds its word list and a 300-dimension [GloVe](https://nlp.stanford.edu/projects/glove/) vector per word, so ranking needs no network. It's committed rather than built, since GloVe is an 822 MB download; see [Updating the Concepts data](#updating-the-concepts-data).
- **Minimal Dependencies**:
  - Only 4 direct dependencies (`ratatui`, `crossterm`, `clap`, `fastrand`).

## Installation

### Pre-built Binaries

Download the latest `jotto`, `wordle` and `concepts` binaries for Linux, macOS, or Windows from the [GitHub Releases](https://github.com/guslipkin/jotto-cli/releases) page.

### Build from Source

Requires Rust (`cargo`), `git`, and Python 3 (for SCOWL database creation at build time; includes a pure-Rust fallback parser):

```bash
cargo install --path .
```

## Usage

```bash
# Launch Jotto, Wordle or Concepts in full-screen TUI mode (default difficulty: Scrabble / 80)
jotto
wordle
concepts

# Launch the deterministic UTC Daily Challenge
jotto --daily
wordle --daily
concepts --daily

# Launch with a specific difficulty level (easy, medium, hard, scrabble, insane)
jotto --difficulty hard
wordle --difficulty easy

# Launch in line-by-line Plain / REPL mode
jotto --plain
wordle --plain
concepts --plain
```

Concepts' difficulty (`--difficulty easy|medium|hard`, default medium) sets how abstract the secret word may be; see [Concepts difficulty levels](#concepts-difficulty-levels). `--word <word>` sets its secret for a custom game.

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

Concepts' TUI has the same **Guess**, **Help**, **New Game**, **Daily**, **Give Up** and **Quit** controls and a **Difficulty** button (`Ctrl+D` cycles Easy → Medium → Hard), no Scratch pad, and one more: `Tab` or Click **Hint** reveals a closer word. Its **Latest** box shows your most recent guess, and the list shows every guess closest first (hints marked `*`).

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

Concepts' plain mode takes any word as a guess and has `:help`, `:history` (every guess, closest first), `:hint`, `:new [difficulty]`, `:daily`, `:difficulty [level]`, `:giveup`, `:credits` and `:quit`.

## Concepts Difficulty Levels

Concepts' levels set how abstract the secret noun may be, since a thing you can point at is far easier to find by meaning than an idea. Each level includes the easier levels' words:
- **Easy**: objects, people, animals, food and places ("elephant", "bread", "doctor").
- **Medium**: adds acts, events, groups and times ("baseball", "ceremony", "parliament"; the default, and always used for daily games).
- **Hard**: adds ideas, feelings, qualities and states ("credibility", "democracy", "anxiety").

Each word's level comes from WordNet's category for its most common sense. Ranks and guesses are the same at every level; only the secret changes.

## Difficulty Levels (SCOWL, Jotto & Wordle)

Each difficulty setting includes all words from the levels before it. The word lists come from the latest SCOWL at build time, so each level's exact count can change between builds; the in-game help (`?` or `:help`) shows the current ones, as it does for Concepts.
- **Easy (`35`)**: Recommended small dictionary.
- **Medium (`50`)**: Recommended medium dictionary.
- **Hard (`70`)**: Recommended large dictionary.
- **Scrabble (`80`)**: Strange and unusual words used in word games like Scrabble (default).
- **🤷 Insane (`95`)**: Every valid 5-letter English word in SCOWL. Wordle accepts every word in the list as a guess, plurals included, at any level.

## Updating the Concepts data

`scripts/build-concepts-data.py` writes `data/concepts.bin`. It needs only Python 3, the SCOWL database `build.rs` leaves in `OUT_DIR` after any `cargo build`, and GloVe 6B's 300-dimension vectors (`glove.6B.300d.txt` from `glove.6B.zip`, at [Stanford](https://nlp.stanford.edu/projects/glove/) or [Hugging Face](https://huggingface.co/stanfordnlp/glove)):

```bash
python3 scripts/build-concepts-data.py \
  --scowl-db target/release/build/jotto-*/out/wordlist/scowl.db \
  --glove glove.6B.300d.txt \
  --wordnet wordnet \
  --show ocean guitar   # optional: print each word's nearest neighbours to check the result
```

Add `--cased-vocab glove.840B.300d.txt` to refresh `data/concepts-proper-nouns.txt` as well.

Ranked words are SCOWL's medium (`50`) list's nouns, verbs, adjectives and adverbs among GloVe's 100,000 most frequent words. Guess-only words are every other SCOWL word in GloVe's whole 400,000-word vocabulary, less inflections of ranked words ("stained" scores as "stain"); they're kept out of the ranks because past size 50 SCOWL adds names and rarer words, whose vectors are noisier. Secret words are the medium (`50`) list's nouns among GloVe's 50,000 most frequent, minus the script's hand-checked exclusions (words mostly used as something other than a noun, such as verbs, numbers and shortened forms, and unpleasant or crude words). Names, places and brands are left out entirely, from the committed `data/concepts-proper-nouns.txt`: `--cased-vocab` (GloVe 840B, a 2 GB download from the same pages, needed only to refresh that list) marks a word when it appears far more often capitalized than not, SCOWL's lists of names, surnames or towns have it capitalized (so "Americanization", merely spelled capitalized, stays), and WordNet has no commonly used lowercase sense of it; `PROPER_NOUN_KEEP` and `PROPER_NOUN_DROP` in the script fix the cases the rule gets wrong. Secret words' difficulty tiers come from WordNet 3.0's dict files (the `wordnet` directory, from [Princeton](https://wordnet.princeton.edu/) or NLTK's data repository's `packages/corpora/wordnet.zip`).

## Credits & License

- Written by [Gus Lipkin](https://guslipkin.me).
- [Wordle](https://www.nytimes.com/games/wordle) was created by Josh Wardle and is now published by The New York Times; `wordle` is an unofficial version. Play the original at <https://www.nytimes.com/games/wordle>.
- Concepts is inspired by [Contexto](https://contexto.me/); play the original at <https://contexto.me/>.
- Word list from [SCOWL (Spell Checker Oriented Word Lists)](http://wordlist.aspell.net/) by Kevin Atkinson (see [`LICENSE.SCOWL`](LICENSE.SCOWL)).
- Concepts' word meanings (and, from GloVe 840B's cased vocabulary, which words are names) from [GloVe](https://nlp.stanford.edu/projects/glove/) by Jeffrey Pennington, Richard Socher and Christopher D. Manning, made available under the [Open Data Commons Public Domain Dedication and License](https://opendatacommons.org/licenses/pddl/1-0/).
- Concepts' difficulty levels from [WordNet 3.0](https://wordnet.princeton.edu/) (Princeton University, "WordNet 3.0", 2006) noun categories; only the derived level, not WordNet itself, is in the binary.

