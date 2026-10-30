use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::contexto::data::{lexicon, Ranking};
use crate::dict;
use crate::game::sanitize_input;
use crate::save::{self, enum_name, parse_enum, Loaded, Notice, Saveable, SAVE_VERSION};

/// How abstract the secret word may be. Each level's pool includes the easier ones'.
/// Tiers come from WordNet's category for the word's most common sense
/// (`scripts/build-contexto-data.py` says why not word frequency).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Difficulty {
    /// Things you can point at: objects, people, animals, food, places
    #[value(alias = "e")]
    Easy,
    /// Adds acts, events, groups and times
    #[default]
    #[value(alias = "m", alias = "med")]
    Medium,
    /// Adds ideas, feelings, qualities and states
    #[value(alias = "h")]
    Hard,
}

impl Difficulty {
    /// The highest secret-word tier this level draws from.
    pub const fn max_tier(self) -> u8 {
        match self {
            Self::Easy => 1,
            Self::Medium => 2,
            Self::Hard => 3,
        }
    }

    /// Human-readable label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
        }
    }

    /// What the level's secret words are.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Easy => "objects, people, animals, food, places",
            Self::Medium => "adds acts, events, groups, times",
            Self::Hard => "adds ideas, feelings, qualities, states",
        }
    }

    /// Cycles to the next difficulty level.
    pub const fn next(self) -> Self {
        match self {
            Self::Easy => Self::Medium,
            Self::Medium => Self::Hard,
            Self::Hard => Self::Easy,
        }
    }

    /// Number of secret words at this level.
    pub fn secret_count(self) -> usize {
        lexicon().secret_count(self.max_tier())
    }
}

impl fmt::Display for Difficulty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Difficulty {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "easy" | "e" => Ok(Self::Easy),
            "medium" | "med" | "m" => Ok(Self::Medium),
            "hard" | "h" => Ok(Self::Hard),
            other => Err(format!(
                "Unknown difficulty '{other}'. Choose: easy, medium or hard."
            )),
        }
    }
}

/// Formats a count with thousands separators ("23,191").
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Daily games always use this level, so everyone gets the same word.
pub const DAILY_DIFFICULTY: Difficulty = Difficulty::Medium;

/// Rank a hint reveals before any guess.
pub const FIRST_HINT_RANK: usize = 300;

/// Ranks up to this are "close" (green), up to [`WARM_RANK`] "warm" (orange), the rest "far".
pub const CLOSE_RANK: usize = 300;
/// See [`CLOSE_RANK`].
pub const WARM_RANK: usize = 1500;

/// How close a rank is, for colouring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closeness {
    Close,
    Warm,
    Far,
}

impl Closeness {
    pub fn of(rank: usize) -> Self {
        if rank <= CLOSE_RANK {
            Self::Close
        } else if rank <= WARM_RANK {
            Self::Warm
        } else {
            Self::Far
        }
    }
}

/// Fraction (0.0-1.0) of a closeness bar to fill for `rank`. Logarithmic, since the
/// difference between ranks 5 and 50 matters far more than between 5,000 and 5,050.
pub fn bar_fraction(rank: usize) -> f64 {
    let total = lexicon().ranked_count().max(2) as f64;
    (1.0 - (rank.max(1) as f64).ln() / total.ln()).clamp(0.0, 1.0)
}

/// Number of cells to fill in a closeness bar `width` cells wide for `rank`.
pub fn bar_filled(rank: usize, width: usize) -> usize {
    ((bar_fraction(rank) * width as f64).round() as usize).min(width)
}

/// "1 guess", "3 guesses" and so on.
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Errors returned when a guess can't be scored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuessError {
    GameOver,
    Empty,
    NonAlphabetic,
    UnknownWord(String),
}

impl fmt::Display for GuessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GameOver => {
                f.write_str("The current game is already over. Start a new game to play again.")
            }
            Self::Empty => f.write_str("Type a word to guess."),
            Self::NonAlphabetic => f.write_str("Guesses must contain only letters a-z."),
            Self::UnknownWord(w) => write!(
                f,
                "'{w}' isn't in the word list (names and words like 'the' aren't included)."
            ),
        }
    }
}

impl std::error::Error for GuessError {}

/// Why [`ContextoGame::hint`] gave no hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HintError {
    GameOver,
    /// The best guess is #2, so the only closer word is the answer.
    NoneCloser,
    /// The word this hint would reveal already has been; a new best guess moves it.
    GuessFirst {
        rank: usize,
    },
    /// The first hint (before any guess) is already out; any guess allows another.
    FirstHintTaken,
}

impl fmt::Display for HintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GameOver => {
                f.write_str("The current game is already over. Start a new game to play again.")
            }
            Self::NoneCloser => f.write_str(
                "No hint left: your best guess is #2, so the only closer word is the answer.",
            ),
            Self::GuessFirst { rank } => write!(
                f,
                "You already have the hint for your best guess (#{rank}). Make a closer guess for a new one."
            ),
            Self::FirstHintTaken => f.write_str(
                "You already have the first hint. Make a guess for a new one.",
            ),
        }
    }
}

impl std::error::Error for HintError {}

/// One scored guess (or hint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guess {
    /// 1-based order in which it was made.
    pub number: usize,
    /// The guessable word it was scored as.
    pub word: String,
    /// What the player typed, when it was an inflection of `word` ("mice" for "mouse").
    pub typed: Option<String>,
    /// Closeness rank (1 = the secret).
    pub rank: usize,
    /// Whether this came from a hint rather than a guess.
    pub hint: bool,
}

impl Guess {
    /// The word as listed: upper case, then what was typed for an inflection and `*`
    /// for a hint ("MOUSE (mice)", "OCEAN *").
    pub fn label(&self) -> String {
        let mut label = self.word.to_ascii_uppercase();
        if let Some(typed) = &self.typed {
            label.push_str(&format!(" ({typed})"));
        }
        if self.hint {
            label.push_str(" *");
        }
        label
    }

    /// Reply to guessing this word again.
    pub fn repeat_message(&self) -> String {
        format!(
            "Already guessed '{}' (#{}).",
            self.word.to_ascii_uppercase(),
            self.rank
        )
    }

    /// Reply to this hint being revealed.
    pub fn hint_message(&self) -> String {
        format!(
            "Hint: '{}' is #{}.",
            self.word.to_ascii_uppercase(),
            self.rank
        )
    }
}

/// Result of [`ContextoGame::make_guess`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuessOutcome {
    pub guess: Guess,
    /// The word had already been guessed; `guess` is the earlier entry and nothing was added.
    pub duplicate: bool,
}

/// Current lifecycle status of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Playing,
    Won,
    GivenUp,
}

/// A `contexto` save file (see [`crate::save`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGame {
    pub version: u32,
    /// The unfinished game, if any.
    pub game: Option<SavedPlay>,
}

/// An unfinished `contexto` game, replayed on load. Ranks aren't saved: replaying
/// recomputes them, so a save stays right if the word data changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPlay {
    pub difficulty: String,
    /// Base64 in the file, so opening it doesn't give the answer away.
    #[serde(with = "save::encoded")]
    pub secret: String,
    pub daily_date: Option<String>,
    /// Guesses and hints in the order they were made.
    pub moves: Vec<SavedMove>,
}

/// One guess or hint in a [`SavedPlay`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMove {
    pub word: String,
    /// What was typed, for an inflection ("mice" for "mouse").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hint: bool,
}

/// State machine for one `contexto` session.
#[derive(Debug, Clone)]
pub struct ContextoGame {
    secret: usize,
    ranking: Ranking,
    difficulty: Difficulty,
    daily_date: Option<String>,
    guesses: Vec<Guess>,
    played: HashSet<usize>,
    status: Status,
}

impl ContextoGame {
    /// A game at `difficulty` with a random secret, or `custom_word` if given
    /// (any guessable word, or an inflection of one).
    pub fn new(difficulty: Difficulty, custom_word: Option<&str>) -> Result<Self, GuessError> {
        let lex = lexicon();
        let secret = match custom_word {
            Some(w) => {
                let w = sanitize_input(w);
                lex.resolve(&w).ok_or(GuessError::UnknownWord(w))?
            }
            None => {
                let pool = lex.secret_pool(difficulty.max_tier());
                pool[fastrand::usize(0..pool.len())]
            }
        };
        let mut game = Self::with_secret(secret, difficulty, None);
        game.played.insert(secret);
        Ok(game)
    }

    /// Today's deterministic UTC daily game (always at [`DAILY_DIFFICULTY`]).
    pub fn new_daily() -> Self {
        let day = dict::current_utc_days();
        let secret = lexicon().daily_secret(day, DAILY_DIFFICULTY.max_tier());
        let mut game =
            Self::with_secret(secret, DAILY_DIFFICULTY, Some(dict::format_utc_date(day)));
        game.played.insert(secret);
        game
    }

    fn with_secret(secret: usize, difficulty: Difficulty, daily_date: Option<String>) -> Self {
        Self {
            secret,
            ranking: lexicon().rank_against(secret),
            difficulty,
            daily_date,
            guesses: Vec::new(),
            played: HashSet::new(),
            status: Status::Playing,
        }
    }

    fn restart(&mut self, secret: usize, daily_date: Option<String>) {
        self.secret = secret;
        self.ranking = lexicon().rank_against(secret);
        self.daily_date = daily_date;
        self.guesses.clear();
        self.status = Status::Playing;
    }

    /// Starts a new random game, at `new_difficulty` if given, never repeating a
    /// secret this session until the level's pool runs out.
    pub fn reset(&mut self, new_difficulty: Option<Difficulty>) {
        if let Some(d) = new_difficulty {
            if d != self.difficulty {
                self.difficulty = d;
                self.played.clear();
            }
        }
        let pool = lexicon().secret_pool(self.difficulty.max_tier());
        let mut unplayed: Vec<usize> = pool
            .iter()
            .copied()
            .filter(|s| !self.played.contains(s))
            .collect();
        if unplayed.is_empty() {
            self.played.clear();
            unplayed = pool;
        }
        let secret = unplayed[fastrand::usize(0..unplayed.len())];
        self.played.insert(secret);
        self.restart(secret, None);
    }

    /// Starts today's UTC daily game (switching to [`DAILY_DIFFICULTY`]).
    pub fn start_daily(&mut self) {
        let day = dict::current_utc_days();
        if self.difficulty != DAILY_DIFFICULTY {
            self.difficulty = DAILY_DIFFICULTY;
            self.played.clear();
        }
        let secret = lexicon().daily_secret(day, DAILY_DIFFICULTY.max_tier());
        self.played.insert(secret);
        self.restart(secret, Some(dict::format_utc_date(day)));
    }

    /// The current difficulty level.
    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    pub fn secret_word(&self) -> &'static str {
        lexicon().word(self.secret)
    }

    pub fn daily_date(&self) -> Option<&str> {
        self.daily_date.as_deref()
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// Guesses and hints in the order they were made.
    pub fn guesses(&self) -> &[Guess] {
        &self.guesses
    }

    /// Guesses and hints, closest first.
    pub fn guesses_by_rank(&self) -> Vec<&Guess> {
        let mut sorted: Vec<&Guess> = self.guesses.iter().collect();
        sorted.sort_by_key(|g| g.rank);
        sorted
    }

    /// Number of guesses made, not counting hints.
    pub fn guess_count(&self) -> usize {
        self.guesses.iter().filter(|g| !g.hint).count()
    }

    /// Number of hints taken.
    pub fn hint_count(&self) -> usize {
        self.guesses.iter().filter(|g| g.hint).count()
    }

    /// "3 guesses", or "3 guesses and 1 hint" once a hint has been taken.
    pub fn count_summary(&self) -> String {
        let guesses = plural(self.guess_count(), "guess", "guesses");
        match self.hint_count() {
            0 => guesses,
            n => format!("{guesses} and {}", plural(n, "hint", "hints")),
        }
    }

    /// Best (lowest) rank reached so far, hints included.
    pub fn best_rank(&self) -> Option<usize> {
        self.guesses.iter().map(|g| g.rank).min()
    }

    /// Best (lowest) rank reached by a guess of the player's own, not counting hints.
    pub fn best_guess_rank(&self) -> Option<usize> {
        self.guesses
            .iter()
            .filter(|g| !g.hint)
            .map(|g| g.rank)
            .min()
    }

    /// The `n` words closest to the secret, closest first (the secret itself excluded).
    pub fn nearest(&self, n: usize) -> Vec<&'static str> {
        let lex = lexicon();
        (2..=(n + 1).min(self.ranking.rank_count()))
            .map(|r| lex.word(self.ranking.word_at(r)))
            .collect()
    }

    /// Scores a guess. Inflections count as their base form, and a repeat guess
    /// returns the earlier entry instead of adding another.
    pub fn make_guess(&mut self, raw: &str) -> Result<GuessOutcome, GuessError> {
        if self.status != Status::Playing {
            return Err(GuessError::GameOver);
        }
        let typed = sanitize_input(raw);
        if typed.is_empty() {
            return Err(GuessError::Empty);
        }
        if !typed.bytes().all(|b| b.is_ascii_lowercase()) {
            return Err(GuessError::NonAlphabetic);
        }
        let lex = lexicon();
        let idx = lex
            .resolve(&typed)
            .ok_or_else(|| GuessError::UnknownWord(typed.clone()))?;
        let word = lex.word(idx);

        if let Some(existing) = self.guesses.iter().find(|g| g.word == word) {
            return Ok(GuessOutcome {
                guess: existing.clone(),
                duplicate: true,
            });
        }

        let guess = Guess {
            number: self.guesses.len() + 1,
            word: word.to_string(),
            typed: (typed != word).then_some(typed),
            rank: self.ranking.rank_of(idx),
            hint: false,
        };
        if idx == self.secret {
            self.status = Status::Won;
        }
        self.guesses.push(guess.clone());
        Ok(GuessOutcome {
            guess,
            duplicate: false,
        })
    }

    /// Reveals the word at half the rank of the player's best guess, or at
    /// [`FIRST_HINT_RANK`] before any guess. Earlier hints don't count as the best
    /// guess, so hints can't be chained down to the answer: once a guess's hint is
    /// out, the player has to guess closer for another.
    pub fn hint(&mut self) -> Result<Guess, HintError> {
        if self.status != Status::Playing {
            return Err(HintError::GameOver);
        }
        let target = match self.best_guess_rank() {
            None => FIRST_HINT_RANK.min(self.ranking.rank_count()),
            Some(best) if best <= 2 => return Err(HintError::NoneCloser),
            Some(best) => (best / 2).max(2),
        };
        let word = lexicon().word(self.ranking.word_at(target));
        if self.guesses.iter().any(|g| g.word == word) {
            return Err(match self.best_guess_rank() {
                Some(rank) => HintError::GuessFirst { rank },
                None => HintError::FirstHintTaken,
            });
        }
        let guess = Guess {
            number: self.guesses.len() + 1,
            word: word.to_string(),
            typed: None,
            rank: target,
            hint: true,
        };
        self.guesses.push(guess.clone());
        Ok(guess)
    }

    /// Starts a game from the command line's choices and the player's save (see
    /// [`save::start`]).
    pub fn start(
        difficulty: Option<Difficulty>,
        daily: bool,
        word: Option<&str>,
        loaded: Loaded<SavedGame>,
    ) -> Result<(Self, Option<Notice>), GuessError> {
        let asked_for_game = difficulty.is_some() || daily || word.is_some();
        save::start(loaded, asked_for_game, Self::from_saved, || {
            if daily {
                Ok(Self::new_daily())
            } else {
                Self::new(difficulty.unwrap_or_default(), word)
            }
        })
    }

    /// What to save: the game while it's being played, otherwise nothing.
    pub fn to_saved(&self) -> SavedGame {
        let game = (self.status == Status::Playing).then(|| SavedPlay {
            difficulty: enum_name(self.difficulty),
            secret: self.secret_word().to_string(),
            daily_date: self.daily_date.clone(),
            moves: self
                .guesses
                .iter()
                .map(|g| SavedMove {
                    word: g.word.clone(),
                    typed: g.typed.clone(),
                    hint: g.hint,
                })
                .collect(),
        });
        SavedGame {
            version: SAVE_VERSION,
            game,
        }
    }

    /// Rebuilds a saved unfinished game by replaying its guesses and hints (`None`
    /// when the save has no game in progress).
    pub fn from_saved(saved: &SavedGame) -> Result<Option<Self>, String> {
        let Some(play) = &saved.game else {
            return Ok(None);
        };
        let difficulty = parse_enum(&play.difficulty)
            .ok_or_else(|| format!("unknown difficulty '{}'", play.difficulty))?;
        let mut game = Self::new(difficulty, Some(&play.secret))
            .map_err(|_| format!("its word '{}' is no longer in the word list", play.secret))?;
        game.daily_date = play.daily_date.clone();
        let lex = lexicon();
        for m in &play.moves {
            let gone = || format!("'{}' is no longer in the word list", m.word);
            if m.hint {
                // The hint itself, not a fresh one: hints depend on the guesses
                // around them, and the player saw this word.
                let idx = lex.index_of(&m.word).ok_or_else(gone)?;
                game.guesses.push(Guess {
                    number: game.guesses.len() + 1,
                    word: m.word.clone(),
                    typed: None,
                    rank: game.ranking.rank_of(idx),
                    hint: true,
                });
            } else {
                game.make_guess(m.typed.as_deref().unwrap_or(&m.word))
                    .map_err(|_| gone())?;
            }
        }
        if game.status != Status::Playing {
            return Err("its guesses already finish it".to_string());
        }
        Ok(Some(game))
    }

    /// Ends the game and reveals the secret.
    pub fn give_up(&mut self) {
        if self.status == Status::Playing {
            self.status = Status::GivenUp;
        }
    }
}

impl Saveable for ContextoGame {
    fn daily_date(&self) -> Option<&str> {
        self.daily_date.as_deref()
    }
}
