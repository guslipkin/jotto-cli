# Jotto, Wordle, Contexto & Flights CLI

Fast, lightweight Rust CLI versions of the classic **Jotto** and **Wordle** 5-letter word games (ported from [`guslipkin/jotto`](https://github.com/guslipkin/jotto)), plus **Contexto**, an unofficial version of [Contexto](https://contexto.me/), a game of finding a word by its meaning, and **Flights**, a game of hopping between airport codes. Installing this crate provides the `jotto`, `wordle`, `contexto` and `flights` commands, and `arcade`, all four in one program, chosen from a menu.

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

### Contexto (`contexto`)
Contexto asks you to find a secret word by its meaning:
- The secret word is a **common noun**. Guesses can be any common noun, verb, adjective or adverb (not names, places, brands, or function words like "the"). Words that are also names count when they have a common meaning of their own ("robin", "creek", "apple").
- Each guess gets a **rank**: how close its meaning is to the secret word's, out of every ranked word. **#1 is the secret word.** Rarer words ("lemongrass", "ptarmigan") are accepted too: each shares the rank of the ranked word it falls next to, but never shows up in hints or the closest-words list.
- Plurals and other forms count as their base word: "mice" is scored as "mouse", "went" as "go".
- Closeness comes from how words are used together, so a close word can share a topic ("ocean" and "coast") as well as a meaning.
- Guesses are unlimited. A **hint** reveals the word at half your best guess's rank (best #84 reveals #42), or the #300 word before you've guessed. Hints don't count as guesses, so after one you need a closer guess of your own for another.

### Flights (`flights`)
Flights asks you to get from one airport to another by the shortest distance:
- You're given a start and a destination airport by their three-letter IATA codes, which differ in every letter.
- Each **hop** changes **one letter** of your current airport's code: LAX → LAS → LOS.
- A hop needs a real ticket: a direct flight, or one with **1 connection** (a custom game can allow 0, 1 or 2). Its distance is the shortest such itinerary's great-circle distance, **every connection counted**.
- Each option, and every stop on your route, shows its airport's city and country ("Las Vegas, US"); a custom game can turn that off.
- You can't fly to an airport already on your route. **Go back** a hop at any time, as far as the start; hops you back out of don't count.
- You win by reaching the destination by a **shortest route** (any route tied for shortest counts). Arrive the long way and you're told so (on Easy and Medium, by how much), and can go back and improve.
- Every puzzle has at least two routes from start to destination.

## Features

- **Four Games, One Shared Codebase (`jotto`, `wordle`, `contexto` & `flights`)**:
  - **Full-screen Terminal UI (TUI)** by default with a live Guess box, Used Letters tracker (manual Scratch pad in Jotto; auto-colored letter states in Wordle), and History / 6-Guess Board; in Contexto, a closest-first list of guesses with colorblind-safe closeness bars; in Flights, a braille world map that fits itself to your route and options, and that you can pan and zoom.
  - **Plain / REPL Mode (`--plain`)** for line-by-line terminal sessions or non-TTY pipes. In a terminal, `Up` / `Down` bring back earlier guesses and commands, and `Left` / `Right`, `Home` / `End`, `Ctrl+A` / `Ctrl+E`, `Ctrl+U` / `Ctrl+K` edit the line; `Ctrl+D` on an empty line or `Ctrl+C` quits.
- **Minimal Word Recycling**:
  - **Daily Mode (`--daily` or `Ctrl+T`)**: Uses a deterministic cooldown of 90% of the Scrabble (`80`) word list's size in days (well over a decade for both games) before any daily word can repeat.
  - **Random Mode**: Tracks dealt words across resets so a word never repeats in a session until the entire difficulty pool has been played.
  - **Contexto** daily words are always Medium: a fixed shuffle of its Medium secret nouns, one a day, so none repeats until every one has been used (several years).
- **Fresh SCOWL Dictionary at Build Time**:
  - Pulls the latest word lists directly from [`en-wl/wordlist`](https://github.com/en-wl/wordlist) during `cargo build` via `build.rs` and embeds a single compact dictionary shared by Jotto and Wordle.
- **Offline Word Meanings (Contexto)**:
  - `data/contexto.bin` (~18 MB, embedded in the `contexto` binary) holds its word list and a 300-dimension [GloVe](https://nlp.stanford.edu/projects/glove/) vector per word, so ranking needs no network. It's committed rather than built, since GloVe is an 822 MB download; see [Updating the Contexto data](#updating-the-contexto-data).
- **Offline Airports & Routes (Flights)**:
  - `data/flights.tsv` (~550 KB, embedded in the `flights` binary) holds every airport and hop the game uses, scraped from Wikipedia's airport articles, and `data/coastlines.bin` (~1.9 MB) holds [Natural Earth](https://www.naturalearthdata.com/)'s coastlines at three scales, so the map stays sharp as you zoom in; see [Updating the Flights data](#updating-the-flights-data).
- **Saved Games**: Quit mid-game and the next launch picks up where you left off (see [Saved games](#saved-games)).
- **Minimal Dependencies**:
  - Only 6 direct dependencies (`ratatui`, `crossterm`, `clap`, `fastrand`, and `serde` / `serde_json` for saved games).

## Installation

### Quick Install Script

**Linux & macOS:**
```bash
curl -fsSL https://raw.githubusercontent.com/guslipkin/jotto-cli/main/install.sh | sh
```
*(Installs `jotto`, `wordle`, `contexto`, `flights`, and `arcade` to `~/.local/bin` by default; pass `-s -- --bin-dir /usr/local/bin` or set `INSTALL_DIR` to customize.)*

**Windows (PowerShell):**
```powershell
irm https://raw.githubusercontent.com/guslipkin/jotto-cli/main/install.ps1 | iex
```

### Pre-built Binaries

Download the latest `jotto`, `wordle`, `contexto`, `flights` and `arcade` binaries for Linux, macOS, or Windows from the [GitHub Releases](https://github.com/guslipkin/jotto-cli/releases) page.

### Build from Source

Requires Rust (`cargo`), `git`, and Python 3 (for SCOWL database creation at build time; includes a pure-Rust fallback parser):

```bash
cargo install --path .
```

### Play in a Browser (Docker)

The `Dockerfile` and `docker-compose.yml` serve all four games in a web browser, for any number of players at once. They need only Docker (on Linux, macOS or Windows, x86 or ARM):

```bash
docker compose up            # the published image (latest main), from GHCR
docker compose up --build    # or build this checkout
```

Or without this repository, the released image on its own (with the same locked-down settings `docker-compose.yml` uses):

```bash
docker run --rm -p 7681:7681 --read-only --tmpfs /tmp:mode=1777 \
  --cap-drop ALL --security-opt no-new-privileges:true \
  ghcr.io/guslipkin/jotto-arcade:latest
```

Then open <http://localhost:7681>. Each browser tab gets its own session: a menu of the four games, each played in its full-screen TUI. Leaving a game (`Esc`) always comes back to the menu, and the menu's **Quit** (or `q` / `Esc`) ends the session; reload the page for a new one. Players never get a command line: the image is built `FROM scratch`, with nothing in it but `arcade` (which has the four games built in, so it never starts another program) and [ttyd](https://github.com/tsl0922/ttyd), the web terminal, which runs only the menu. Saves are kept for the session alone, in memory, and are gone when it ends.

The container is read-only, with every capability dropped and no privilege escalation; ttyd allows up to 32 sessions at once (change `--max-clients` in the `Dockerfile`'s `CMD`). To build for both architectures, use `docker buildx build --platform linux/amd64,linux/arm64 .`. GitHub Actions also builds both (on native x86_64 and arm64 runners) and pushes them to `ghcr.io/guslipkin/jotto-arcade` as one image: every commit to `main` as `latest` and `sha-<commit>`, and each published GitHub Release as its version (`1.2.3` and `1.2`), on the same rules as the binaries' release workflow. `latest` follows `main`, so a release of an older commit never moves it back. Running the Docker workflow by hand only builds it. Each session runs `arcade --session-saves` (see [Usage](#usage)).

## Usage

```bash
# Launch Jotto, Wordle, Contexto or Flights in full-screen TUI mode (default difficulty: Scrabble / 80)
jotto
wordle
contexto
flights

# Or pick any of them from a menu: leaving a game comes back to it, and quitting
# it returns to your shell. Games resume and save just as their own commands do.
arcade
arcade --session-saves   # saves kept for this run only, then removed (as the Docker image does)

# Launch the deterministic UTC Daily Challenge
jotto --daily
wordle --daily
contexto --daily
flights --daily

# Launch with a specific difficulty level (easy, medium, hard, scrabble, insane)
jotto --difficulty hard
wordle --difficulty easy

# Launch in line-by-line Plain / REPL mode
jotto --plain
wordle --plain
contexto --plain
flights --plain
```

Contexto's difficulty (`--difficulty easy|medium|hard`, default medium) sets how abstract the secret word may be; see [Contexto difficulty levels](#contexto-difficulty-levels). `--word <word>` sets its secret for a custom game: any word it accepts as a guess, including a plural or other form (`--word mice` makes the secret "mouse").

Flights' difficulty (`--difficulty easy|medium|hard|pilot|custom`, default medium) sets how long a puzzle may be and what you're shown; see [Flights difficulty levels](#flights-difficulty-levels). `--units nm|mi|km` sets the distance units (default `nm`, nautical miles, the unit aviation uses); the choice is remembered for later games, and `Ctrl+U`, the settings page (`Ctrl+S`) or `:units` changes it mid-game. `--route LAX-LOS` plays a custom puzzle between any two connected airports in the game.

### Saved Games

Each game saves itself after every guess, hint, scratch-pad mark and hop, so you can quit (or close the terminal) and come back. Launching it again resumes the unfinished game and says so; a daily game from an earlier day resumes too, and `Ctrl+T` / `:daily` switches to today's. Passing `--difficulty`, `--daily`, `--word` or (in Flights) `--route` starts that new game instead, replacing the saved one.

Saves live in a hidden `.ndt` folder in your home directory (`~/.ndt/jotto.json`, `wordle.json`, `contexto.json` and `flights.json`; set `NDT_SAVE_DIR` to use another folder), along with a `README.txt` saying what made the folder and what it's for. A finished game isn't kept, so the next launch starts a new one. So a glance at the file doesn't spoil the game, the secret word is base64-encoded; the folder's `README.txt` shows how to decode or change it (`echo 'Y3JhbmU=' | base64 --decode` prints `crane`). Flights' save isn't encoded, since its puzzle is shown from the start, and it keeps your units and custom-game settings even between games. Deleting the file starts you fresh. If a save can't be read (it's damaged, or from a different version), the game says so and starts a new one.

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

Contexto's TUI has the same **Guess**, **Help**, **New Game**, **Daily**, **Give Up** and **Quit** controls and a **Difficulty** button (`Ctrl+D` cycles Easy → Medium → Hard), no Scratch pad, and one more: `Tab` or Click **Hint** reveals a closer word. Its **Latest** box shows your most recent guess, and the list shows every guess closest first (hints marked `*`).

Flights' TUI has no typed input: you move with the arrow keys (the **Help** page lists them). It has the same **Help**, **New Game**, **Daily**, **Give Up**, **Difficulty** (`Ctrl+D` cycles Easy → Medium → Hard → Pilot → Custom) and **Quit** controls, and:

| Key / Action | Description |
| --- | --- |
| `Up` / `Down` or Mouse Wheel | Choose an airport from the **Options** list |
| `Right` (or `Enter`), or Click an option | Fly to the chosen airport |
| `Left` (or `Backspace`) | Go back a hop |
| `+` / `-`, or Mouse Wheel over the map | Zoom the map (the wheel zooms around the pointer) |
| `W` `A` `S` `D`, or drag the map | Pan the map |
| `0` | Fit the map to your route and options again (a new game does too) |
| `Ctrl+S` or Click **Settings** | Open the settings page: units, and the custom game's settings (`Up` / `Down` choose, `Left` / `Right` change, typing a code sets the start or end on a fixed route, `Enter` plays a custom game, `Esc` closes; changes are kept either way) |
| `Ctrl+U` | Cycle units (`nm` → `mi` → `km`) |

Each option's changed letter is highlighted, and the destination heads the list, in its colour with a `★` beside its code, when you can fly straight to it. The column beside the map is only as wide as its longest line needs (in the options list, place names over 20 characters end in `…`; on the route, where each has a line of its own, they're cut only at the box's edge), so the map gets the rest, with the status line at its top. The **Route** box lists every hop (with its distance on Easy, and connections except on Pilot). On Easy and Medium, the map draws your route in blue through its connections, the airport you're at in green, the destination in reddish purple, your options in orange, and the option chosen in the list as black on orange with an orange line showing the hop to it (through its connections, where the level shows them) (colors from the colorblind-safe Okabe-Ito palette). Codes that would overlap on the map are moved beside their airport, or left as a dot when there's no room. When a game ends, its route is drawn over everything else on the map, so no hop hides under a code, and the codes are placed beside the line rather than on it: your winning route in blue, or after giving up, your route in blue with a shortest route in green on top. After giving up, the **Route** box lists both routes and marks where yours left the shortest one (`↑ you left here`, `↑ went here`, under the stop they mean), and the status line says the same in words. Each stop on a route takes two lines, during play and after: the code, distance and connections, then the remark and place. The column beside the map is sized once for the longest line it can show (game-over distances and remarks included), so it never changes width.

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

Contexto's plain mode takes any word as a guess, so its commands need their colon (typing `exit` or `help` guesses that word). It has `:help`, `:history` (every guess, closest first), `:hint`, `:new [difficulty]`, `:daily`, `:difficulty [level]`, `:giveup`, `:credits` and `:quit`.

Flights' plain mode takes any three letters as an airport code, so its commands need their colon too. It has `<code>`, `:back`, `:route` (your route and options), `:new [difficulty]`, `:daily`, `:difficulty [level]`, `:units [nm|mi|km]`, `:settings`, `:set <setting> <value>` (`:set hops 6`, `:set connections 0`, `:set view hard`, `:set places off`, `:set mode route`, `:set start LAX`, `:set end random`, `:set units km`), `:giveup`, `:credits` and `:quit`. With no map, it lists your options at every level: with distances and connections on Easy, with connections on Medium and Hard (which look the same here), and as codes alone on Pilot.

## Contexto Difficulty Levels

Contexto's levels set how abstract the secret noun may be, since a thing you can point at is far easier to find by meaning than an idea. Each level includes the easier levels' words:
- **Easy**: objects, people, animals, food and places ("elephant", "bread", "doctor").
- **Medium**: adds acts, events, groups and times ("baseball", "ceremony", "parliament"; the default, and always used for daily games).
- **Hard**: adds ideas, feelings, qualities and states ("credibility", "democracy", "anxiety").

Each word's level comes from WordNet's category for its most common sense. Ranks and guesses are the same at every level; only the secret changes.

## Flights Difficulty Levels

Flights' levels set how many hops a puzzle's shortest route may take, and what you're shown. Each level deals the easier levels' puzzles too, and shows less than the one before it. Every puzzle takes at least 3 hops, since start and destination differ in every letter. The in-game help (`?` or `:help`) shows how many puzzles each level has.
- **Easy**: up to 4 hops. A map of the airports you can fly to, with each option's distance and connections.
- **Medium**: up to 6 hops. The map and connections, but no distances (the default, and always used for daily games).
- **Hard**: up to 8 hops. Medium without the map: a list of the codes you can fly to, with their connections.
- **Pilot**: any number of hops. Hard without connections: codes only.
- **Custom**: your own settings, from the settings page (`Ctrl+S`) or `:set` in plain mode: what's shown (as Easy, Medium, Hard or Pilot), whether options show their city and country, and whether it deals **random puzzles** (within a max connections, 0 to 2, and max hops, 3 to 20 or no limit) or plays a **fixed route** (a start and end of your own, either of which may be left random, at the standard one connection and any length; a pair must still make a valid puzzle: codes differ in every letter, with two or more routes). The two modes are a switch: the page greys out the settings the other mode uses, and in plain mode, setting a limit switches to random puzzles and setting a start or end to a fixed route. The page shows how many puzzles fit and each standard level's max hops for comparison, and won't start a game when none do. Settings are saved with your game; a custom game keeps the settings it began with.

The standard levels allow one connection per hop. Two would make about three times as many puzzles, but mostly with tickets few would book; none leaves only 44 puzzles in all, though a custom game can still choose it.

Distances are shown only on Easy until the game ends, when every level shows them. Reach the destination the long way and Easy and Medium say by how much; Hard and Pilot only say it wasn't the shortest route.

## Difficulty Levels (SCOWL, Jotto & Wordle)

Each difficulty setting includes all words from the levels before it. The word lists come from the latest SCOWL at build time, so each level's exact count can change between builds; the in-game help (`?` or `:help`) shows the current ones, as it does for Contexto.
- **Easy (`35`)**: Recommended small dictionary.
- **Medium (`50`)**: Recommended medium dictionary.
- **Hard (`70`)**: Recommended large dictionary.
- **Scrabble (`80`)**: Strange and unusual words used in word games like Scrabble (default).
- **🤷 Insane (`95`)**: Every valid 5-letter English word in SCOWL. Wordle accepts every word in the list as a guess, plurals included, at any level.

## Updating the Contexto data

`scripts/build-contexto-data.py` writes `data/contexto.bin`. It needs only Python 3, the SCOWL database `build.rs` leaves in `OUT_DIR` after any `cargo build`, and GloVe 6B's 300-dimension vectors (`glove.6B.300d.txt` from `glove.6B.zip`, at [Stanford](https://nlp.stanford.edu/projects/glove/) or [Hugging Face](https://huggingface.co/stanfordnlp/glove)):

```bash
python3 scripts/build-contexto-data.py \
  --scowl-db target/release/build/jotto-*/out/wordlist/scowl.db \
  --glove glove.6B.300d.txt \
  --wordnet wordnet \
  --show ocean guitar   # optional: print each word's nearest neighbours to check the result
```

Add `--cased-vocab glove.840B.300d.txt` to refresh `data/contexto-proper-nouns.txt` as well.

Ranked words are SCOWL's medium (`50`) list's nouns, verbs, adjectives and adverbs among GloVe's 100,000 most frequent words. Guess-only words are every other SCOWL word in GloVe's whole 400,000-word vocabulary, less inflections of ranked words ("stained" scores as "stain"); they're kept out of the ranks because past size 50 SCOWL adds names and rarer words, whose vectors are noisier. Secret words are the medium (`50`) list's nouns among GloVe's 50,000 most frequent, minus the script's hand-checked exclusions (words mostly used as something other than a noun, such as verbs, numbers and shortened forms, and unpleasant or crude words). Names, places and brands are left out entirely, from the committed `data/contexto-proper-nouns.txt`: `--cased-vocab` (GloVe 840B, a 2 GB download from the same pages, needed only to refresh that list) marks a word when it appears far more often capitalized than not, SCOWL's lists of names, surnames or towns have it capitalized (so "Americanization", merely spelled capitalized, stays), and WordNet has no commonly used lowercase sense of it; `PROPER_NOUN_KEEP` and `PROPER_NOUN_DROP` in the script fix the cases the rule gets wrong. Secret words' difficulty tiers come from WordNet 3.0's dict files (the `wordnet` directory, from [Princeton](https://wordnet.princeton.edu/) or NLTK's data repository's `packages/corpora/wordnet.zip`).

## Updating the Flights data

`scripts/build-flights-data.py` writes `data/flights.tsv`. It needs only Python 3 and a network connection: it downloads [OurAirports](https://ourairports.com/data/)' `airports.csv` (or pass `--airports`) and reads each airport's English Wikipedia article through the MediaWiki API, caching what it fetches so a rerun is quick:

```bash
python3 scripts/build-flights-data.py --cache-dir ~/.cache/flights-wiki --log flights-build.log
```

Routes are the scheduled passenger flights, year-round or seasonal, in each article's "Airlines and destinations" section (not cargo; charter only with `--include-charter`), counted when either airport's article lists them. Airport codes and coordinates come from OurAirports, whose `wikipedia_link` column ties each article to its code. Direct flights between codes one letter apart are rare (about 340 of some 34,000 routes, in clusters too small for puzzles), so a hop is the shortest itinerary with connections. The file holds each pair's best itinerary with at most 0, 1 and 2 connections (the best one-stop ticket isn't always the two-stop one less a stop), along with each airport's city and country. For each limit, the script logs the hops and the network's largest components, and counts the puzzles by the number of hops in their shortest route (writing the counts into the file, for the in-game help and the settings page).

The map's coastlines come from `scripts/build-coastline-data.py`, which downloads Natural Earth's 1:110m, 1:50m and 1:10m coastlines (or pass `--geojson-dir` for ones already downloaded) and packs them into `data/coastlines.bin`:

```bash
python3 scripts/build-coastline-data.py
```

The map draws them as lines rather than dots, at the scale its zoom can show: 1:110m (5,128 points) for the world, 1:50m (60,416) for a region, 1:10m (410,582) close in.

## Credits & License

- Written by [Gus Lipkin](https://guslipkin.me).
- [Wordle](https://www.nytimes.com/games/wordle) was created by Josh Wardle and is now published by The New York Times; `wordle` is an unofficial version. Play the original at <https://www.nytimes.com/games/wordle>.
- `contexto` is an unofficial version of [Contexto](https://contexto.me/); play the original at <https://contexto.me/>.
- Word list from [SCOWL (Spell Checker Oriented Word Lists)](http://wordlist.aspell.net/) by Kevin Atkinson (see [`LICENSE.SCOWL`](LICENSE.SCOWL)).
- Contexto's word meanings (and, from GloVe 840B's cased vocabulary, which words are names) from [GloVe](https://nlp.stanford.edu/projects/glove/) by Jeffrey Pennington, Richard Socher and Christopher D. Manning, made available under the [Open Data Commons Public Domain Dedication and License](https://opendatacommons.org/licenses/pddl/1-0/).
- Flights' routes from the "Airlines and destinations" sections of [English Wikipedia](https://en.wikipedia.org/)'s airport articles, by Wikipedia's contributors, under [CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/); `data/flights.tsv` is a derivative of them under the same license. Airport codes and coordinates from [OurAirports](https://ourairports.com/data/) (public domain). Map coastlines from [Natural Earth](https://www.naturalearthdata.com/) (public domain).
- Contexto's difficulty levels from [WordNet 3.0](https://wordnet.princeton.edu/) (Princeton University, "WordNet 3.0", 2006) noun categories; only the derived level, not WordNet itself, is in the binary.

