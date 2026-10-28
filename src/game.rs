use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use clap::ValueEnum;

use crate::dict;
pub use crate::dict::GameMode;

/// Maximum number of guesses allowed in a standard Wordle game.
pub const MAX_WORDLE_GUESSES: usize = 6;

/// SCOWL difficulty tiers matching the original R Shiny Jotto app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Difficulty {
    /// SCOWL size 35 (recommended small dictionary)
    #[value(alias = "35")]
    Easy,
    /// SCOWL size 50 (recommended medium dictionary)
    #[value(alias = "50")]
    Medium,
    /// SCOWL size 70 (recommended large dictionary)
    #[value(alias = "70")]
    Hard,
    /// SCOWL size 80 (includes unusual Scrabble words)
    #[default]
    #[value(alias = "80")]
    Scrabble,
    /// SCOWL size 95 (🤷 - every English word in SCOWL)
    #[value(alias = "95", alias = "shrug", alias = "🤷")]
    Insane,
}

impl Difficulty {
    /// Returns the numeric SCOWL size threshold for this difficulty.
    pub const fn scowl_size(self) -> u8 {
        match self {
            Self::Easy => 35,
            Self::Medium => 50,
            Self::Hard => 70,
            Self::Scrabble => 80,
            Self::Insane => 95,
        }
    }

    /// Returns the human-readable label for this difficulty.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy (35)",
            Self::Medium => "Medium (50)",
            Self::Hard => "Hard (70)",
            Self::Scrabble => "Scrabble (80)",
            Self::Insane => "🤷 Insane (95)",
        }
    }

    /// Cycles to the next difficulty level.
    pub const fn next(self) -> Self {
        match self {
            Self::Easy => Self::Medium,
            Self::Medium => Self::Hard,
            Self::Hard => Self::Scrabble,
            Self::Scrabble => Self::Insane,
            Self::Insane => Self::Easy,
        }
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
            "easy" | "35" | "e" => Ok(Self::Easy),
            "medium" | "med" | "50" | "m" => Ok(Self::Medium),
            "hard" | "70" | "h" => Ok(Self::Hard),
            "scrabble" | "80" | "s" => Ok(Self::Scrabble),
            "insane" | "shrug" | "95" | "i" | "🤷" => Ok(Self::Insane),
            other => Err(format!(
                "Unknown difficulty '{other}'. Choose: easy, medium, hard, scrabble, or insane."
            )),
        }
    }
}

/// Errors returned when validating a player's guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuessError {
    GameOver,
    InvalidLength,
    NonAlphabetic,
    DuplicateLetters,
    NotInDictionary,
}

impl fmt::Display for GuessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GameOver => {
                f.write_str("The current game is already over. Start a new game to play again.")
            }
            Self::InvalidLength => {
                f.write_str("Invalid guess: word must be exactly 5 letters long.")
            }
            Self::NonAlphabetic => {
                f.write_str("Invalid guess: word must contain only letters a-z.")
            }
            Self::DuplicateLetters => {
                f.write_str("Invalid guess: word cannot have repeating letters.")
            }
            Self::NotInDictionary => f.write_str(
                "Invalid guess: please choose a valid 5-letter dictionary word (not a proper noun).",
            ),
        }
    }
}

impl std::error::Error for GuessError {}

/// Per-letter Wordle feedback state (`Absent` < `Present` < `Correct`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LetterState {
    /// Letter does not appear in the secret word (or is an excess duplicate).
    #[default]
    Absent,
    /// Letter appears in the secret word, but in a different position.
    Present,
    /// Letter is in the exact position in the secret word.
    Correct,
}

/// A single round entry in the game history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    pub round: usize,
    pub guess: String,
    pub common: usize,
    pub letter_states: [LetterState; 5],
}

/// Current lifecycle status of a Jotto or Wordle game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    Playing,
    Won,
    Lost,
    GivenUp,
}

/// Three-state scratchpad marker for a letter (`Default` -> `ScratchedOut` -> `Positive`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScratchState {
    #[default]
    Default,
    ScratchedOut,
    Positive,
}

impl ScratchState {
    /// Cycles through `Default -> ScratchedOut -> Positive -> Default`.
    pub const fn next(self) -> Self {
        match self {
            Self::Default => Self::ScratchedOut,
            Self::ScratchedOut => Self::Positive,
            Self::Positive => Self::Default,
        }
    }
}

/// Core state machine for a Jotto or Wordle game session.
#[derive(Debug, Clone)]
pub struct GameState {
    mode: GameMode,
    secret_word: String,
    difficulty: Difficulty,
    daily_date: Option<String>,
    history: Vec<Round>,
    used_letters: [bool; 26],
    letter_states: [Option<LetterState>; 26],
    scratched_letters: [ScratchState; 26],
    played_words: HashSet<String>,
    status: GameStatus,
}

impl GameState {
    /// Creates a new Jotto game with the given difficulty and optional fixed secret word.
    pub fn new(difficulty: Difficulty, custom_word: Option<&str>) -> Result<Self, GuessError> {
        Self::new_with_mode(GameMode::Jotto, difficulty, custom_word)
    }

    /// Creates a new game in the specified `GameMode` with the given difficulty and optional fixed secret word.
    pub fn new_with_mode(
        mode: GameMode,
        difficulty: Difficulty,
        custom_word: Option<&str>,
    ) -> Result<Self, GuessError> {
        let mut played_words = HashSet::new();
        let secret_word = match custom_word {
            Some(w) => {
                let sanitized = sanitize_input(w);
                validate_word_for_mode(mode, &sanitized)?;
                sanitized
            }
            None => {
                let (w, _) =
                    dict::random_word_excluding(mode, difficulty.scowl_size(), &played_words);
                w.to_string()
            }
        };
        played_words.insert(secret_word.clone());

        Ok(Self {
            mode,
            secret_word,
            difficulty,
            daily_date: None,
            history: Vec::new(),
            used_letters: [false; 26],
            letter_states: [None; 26],
            scratched_letters: [ScratchState::Default; 26],
            played_words,
            status: GameStatus::Playing,
        })
    }

    /// Creates a new deterministic UTC daily Jotto game (using the Scrabble `80` word list).
    pub fn new_daily() -> Self {
        Self::new_daily_with_mode(GameMode::Jotto)
    }

    /// Creates a new deterministic UTC daily game for the specified `GameMode` (using the Scrabble `80` word list).
    pub fn new_daily_with_mode(mode: GameMode) -> Self {
        let (word, date_str) = dict::daily_word(mode);
        let mut played_words = HashSet::new();
        played_words.insert(word.to_string());
        Self {
            mode,
            secret_word: word.to_string(),
            difficulty: Difficulty::Scrabble,
            daily_date: Some(date_str),
            history: Vec::new(),
            used_letters: [false; 26],
            letter_states: [None; 26],
            scratched_letters: [ScratchState::Default; 26],
            played_words,
            status: GameStatus::Playing,
        }
    }

    /// Resets the game to a freshly chosen random word (never repeating until the pool is exhausted)
    /// and optional new difficulty.
    pub fn reset(&mut self, new_difficulty: Option<Difficulty>) {
        if let Some(d) = new_difficulty {
            if d != self.difficulty {
                self.difficulty = d;
                self.played_words.clear();
            }
        }
        let (word, recycled) = dict::random_word_excluding(
            self.mode,
            self.difficulty.scowl_size(),
            &self.played_words,
        );
        if recycled {
            self.played_words.clear();
        }
        self.played_words.insert(word.to_string());
        self.secret_word = word.to_string();
        self.daily_date = None;
        self.history.clear();
        self.used_letters = [false; 26];
        self.letter_states = [None; 26];
        self.scratched_letters = [ScratchState::Default; 26];
        self.status = GameStatus::Playing;
    }

    /// Starts or resets to today's deterministic UTC daily word (always using the Scrabble `80` word list).
    pub fn start_daily(&mut self) {
        self.difficulty = Difficulty::Scrabble;
        let (word, date_str) = dict::daily_word(self.mode);
        self.played_words.insert(word.to_string());
        self.secret_word = word.to_string();
        self.daily_date = Some(date_str);
        self.history.clear();
        self.used_letters = [false; 26];
        self.letter_states = [None; 26];
        self.scratched_letters = [ScratchState::Default; 26];
        self.status = GameStatus::Playing;
    }

    /// Returns the game mode (`Jotto` or `Wordle`).
    pub fn mode(&self) -> GameMode {
        self.mode
    }

    /// Returns the maximum number of guesses allowed (`Some(6)` for Wordle, `None` for Jotto).
    pub fn max_guesses(&self) -> Option<usize> {
        match self.mode {
            GameMode::Jotto => None,
            GameMode::Wordle => Some(MAX_WORDLE_GUESSES),
        }
    }

    /// Returns whether this game is a deterministic UTC daily challenge.
    pub fn is_daily(&self) -> bool {
        self.daily_date.is_some()
    }

    /// Returns the UTC date (`"YYYY-MM-DD"`) if this game is a daily challenge.
    pub fn daily_date(&self) -> Option<&str> {
        self.daily_date.as_deref()
    }

    /// Marks the game as given up and reveals the secret word.
    pub fn give_up(&mut self) {
        if self.status == GameStatus::Playing {
            self.status = GameStatus::GivenUp;
        }
    }

    /// Returns the secret word.
    pub fn secret_word(&self) -> &str {
        &self.secret_word
    }

    /// Returns the current difficulty setting.
    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// Returns the history of rounds (ordered chronologically from round 1).
    pub fn history(&self) -> &[Round] {
        &self.history
    }

    /// Returns the boolean array of used letters `a..=z`.
    pub fn used_letters(&self) -> &[bool; 26] {
        &self.used_letters
    }

    /// Returns the highest-priority Wordle feedback state (`Correct` > `Present` > `Absent`)
    /// seen so far for each letter `a..=z`.
    pub fn letter_states(&self) -> &[Option<LetterState>; 26] {
        &self.letter_states
    }

    /// Returns the array of scratchpad states for letters `a..=z`.
    pub fn scratched_letters(&self) -> &[ScratchState; 26] {
        &self.scratched_letters
    }

    /// Returns how many unique words have been dealt in the current difficulty pool without recycling.
    pub fn played_words_count(&self) -> usize {
        self.played_words.len()
    }

    /// Cycles a letter (`a..=z`, case-insensitive) through `Default -> ScratchedOut -> Positive -> Default`.
    pub fn toggle_scratch(&mut self, c: char) {
        let lower = c.to_ascii_lowercase();
        if lower.is_ascii_lowercase() {
            let idx = (lower as u8 - b'a') as usize;
            self.scratched_letters[idx] = self.scratched_letters[idx].next();
        }
    }

    /// Returns the current game status.
    pub fn status(&self) -> GameStatus {
        self.status
    }

    /// Formats the shareable Wordle emoji summary grid (`Wordle ... X/6` + `🟩🟨⬛` rows).
    pub fn wordle_share_summary(&self) -> String {
        let score = match self.status {
            GameStatus::Won => self.history.len().to_string(),
            _ => "X".to_string(),
        };
        let header = if let Some(date) = self.daily_date() {
            format!("Wordle Daily {date} {score}/{MAX_WORDLE_GUESSES}")
        } else {
            format!("Wordle ({}) {score}/{MAX_WORDLE_GUESSES}", self.difficulty)
        };
        let mut lines = vec![header];
        for r in &self.history {
            let row: String = r
                .letter_states
                .iter()
                .map(|state| match state {
                    LetterState::Correct => '🟩',
                    LetterState::Present => '🟨',
                    LetterState::Absent => '⬛',
                })
                .collect();
            lines.push(row);
        }
        lines.join("\n")
    }

    /// Validates and submits a guess, updating history, used letters, letter states, and win/loss status.
    pub fn make_guess(&mut self, raw_guess: &str) -> Result<Round, GuessError> {
        if self.status != GameStatus::Playing {
            return Err(GuessError::GameOver);
        }
        let guess = sanitize_input(raw_guess);
        validate_word_for_mode(self.mode, &guess)?;

        let common = compare_guess(&self.secret_word, &guess);
        let letter_states = score_wordle_guess(&self.secret_word, &guess);

        for (b, &state) in guess.bytes().zip(letter_states.iter()) {
            if b.is_ascii_lowercase() {
                let idx = (b - b'a') as usize;
                self.used_letters[idx] = true;
                self.letter_states[idx] =
                    Some(self.letter_states[idx].map_or(state, |prev| prev.max(state)));
            }
        }

        // Deduplicate if already guessed, matching original R Shiny `.add_to_history`
        if let Some(existing) = self.history.iter().find(|r| r.guess == guess).cloned() {
            if guess == self.secret_word {
                self.status = GameStatus::Won;
            }
            return Ok(existing);
        }

        let round_num = self.history.len() + 1;
        let round = Round {
            round: round_num,
            guess: guess.clone(),
            common,
            letter_states,
        };
        self.history.push(round.clone());

        if guess == self.secret_word {
            self.status = GameStatus::Won;
        } else if self.mode == GameMode::Wordle && self.history.len() >= MAX_WORDLE_GUESSES {
            self.status = GameStatus::Lost;
        }

        Ok(round)
    }
}

/// Sanitizes user input by trimming and removing non-word characters, matching `sub('\\W', '', tolower(guess))`.
pub fn sanitize_input(raw: &str) -> String {
    raw.trim()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Validates that a word is 5 lowercase ASCII letters, has no duplicate characters, and is in the Jotto SCOWL dictionary.
pub fn validate_word(word: &str) -> Result<(), GuessError> {
    validate_word_for_mode(GameMode::Jotto, word)
}

/// Validates that a word is 5 lowercase ASCII letters, satisfies `mode`'s duplicate-letter rule,
/// and is in the SCOWL dictionary for `mode`.
pub fn validate_word_for_mode(mode: GameMode, word: &str) -> Result<(), GuessError> {
    if !word.bytes().all(|b| b.is_ascii_lowercase()) {
        if word.len() != 5 {
            return Err(GuessError::InvalidLength);
        }
        return Err(GuessError::NonAlphabetic);
    }
    if word.len() != 5 {
        return Err(GuessError::InvalidLength);
    }
    if mode.requires_unique_letters() && !dict::has_no_duplicate_chars(word) {
        return Err(GuessError::DuplicateLetters);
    }
    if !dict::contains(mode, word) {
        return Err(GuessError::NotInDictionary);
    }
    Ok(())
}

/// Counts how many letters in `guess` appear in `secret`.
/// Both words are assumed to be 5 distinct lowercase letters in Jotto mode.
pub fn compare_guess(secret: &str, guess: &str) -> usize {
    let secret_bytes = secret.as_bytes();
    guess.bytes().filter(|b| secret_bytes.contains(b)).count()
}

/// Computes standard two-pass Wordle feedback (`Correct`, `Present`, `Absent`) for each of the 5 letters in `guess`.
pub fn score_wordle_guess(secret: &str, guess: &str) -> [LetterState; 5] {
    let mut states = [LetterState::Absent; 5];
    let s_bytes = secret.as_bytes();
    let g_bytes = guess.as_bytes();
    if s_bytes.len() < 5 || g_bytes.len() < 5 {
        return states;
    }

    let mut remaining = [0u8; 26];
    // Pass 1: exact positional matches (Correct)
    for i in 0..5 {
        if g_bytes[i] == s_bytes[i] {
            states[i] = LetterState::Correct;
        } else if s_bytes[i].is_ascii_lowercase() {
            remaining[(s_bytes[i] - b'a') as usize] += 1;
        }
    }

    // Pass 2: non-positional matches (Present), consuming remaining unmatched secret letters
    for i in 0..5 {
        if states[i] != LetterState::Correct && g_bytes[i].is_ascii_lowercase() {
            let idx = (g_bytes[i] - b'a') as usize;
            if remaining[idx] > 0 {
                states[i] = LetterState::Present;
                remaining[idx] -= 1;
            }
        }
    }

    states
}
