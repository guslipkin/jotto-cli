use std::fmt;
use std::str::FromStr;

use clap::ValueEnum;

use crate::dict;

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
    InvalidLength,
    NonAlphabetic,
    DuplicateLetters,
    NotInDictionary,
}

impl fmt::Display for GuessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
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

/// A single round entry in the game history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Round {
    pub round: usize,
    pub guess: String,
    pub common: usize,
}

/// Current lifecycle status of a Jotto game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameStatus {
    Playing,
    Won,
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

/// Core state machine for a Jotto game session.
#[derive(Debug, Clone)]
pub struct GameState {
    secret_word: String,
    difficulty: Difficulty,
    daily_date: Option<String>,
    history: Vec<Round>,
    used_letters: [bool; 26],
    scratched_letters: [ScratchState; 26],
    status: GameStatus,
}

impl GameState {
    /// Creates a new game with the given difficulty and optional fixed secret word.
    pub fn new(difficulty: Difficulty, custom_word: Option<&str>) -> Result<Self, GuessError> {
        let secret_word = match custom_word {
            Some(w) => {
                let sanitized = sanitize_input(w);
                validate_word(&sanitized)?;
                sanitized
            }
            None => dict::random_word(difficulty.scowl_size()).to_string(),
        };

        Ok(Self {
            secret_word,
            difficulty,
            daily_date: None,
            history: Vec::new(),
            used_letters: [false; 26],
            scratched_letters: [ScratchState::Default; 26],
            status: GameStatus::Playing,
        })
    }

    /// Creates a new deterministic UTC daily game (using the Scrabble `80` word list).
    pub fn new_daily() -> Self {
        let (word, date_str) = dict::daily_word();
        Self {
            secret_word: word.to_string(),
            difficulty: Difficulty::Scrabble,
            daily_date: Some(date_str),
            history: Vec::new(),
            used_letters: [false; 26],
            scratched_letters: [ScratchState::Default; 26],
            status: GameStatus::Playing,
        }
    }

    /// Resets the game to a freshly chosen random word and optional new difficulty.
    pub fn reset(&mut self, new_difficulty: Option<Difficulty>) {
        if let Some(d) = new_difficulty {
            self.difficulty = d;
        }
        self.secret_word = dict::random_word(self.difficulty.scowl_size()).to_string();
        self.daily_date = None;
        self.history.clear();
        self.used_letters = [false; 26];
        self.scratched_letters = [ScratchState::Default; 26];
        self.status = GameStatus::Playing;
    }

    /// Starts or resets to today's deterministic UTC daily word (always using the Scrabble `80` word list).
    pub fn start_daily(&mut self) {
        self.difficulty = Difficulty::Scrabble;
        let (word, date_str) = dict::daily_word();
        self.secret_word = word.to_string();
        self.daily_date = Some(date_str);
        self.history.clear();
        self.used_letters = [false; 26];
        self.scratched_letters = [ScratchState::Default; 26];
        self.status = GameStatus::Playing;
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

    /// Returns the array of scratchpad states for letters `a..=z`.
    pub fn scratched_letters(&self) -> &[ScratchState; 26] {
        &self.scratched_letters
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

    /// Validates and submits a guess, updating history, used letters, and win status.
    pub fn make_guess(&mut self, raw_guess: &str) -> Result<Round, GuessError> {
        let guess = sanitize_input(raw_guess);
        validate_word(&guess)?;

        let common = compare_guess(&self.secret_word, &guess);

        for b in guess.bytes() {
            if b.is_ascii_lowercase() {
                self.used_letters[(b - b'a') as usize] = true;
            }
        }

        // Deduplicate if already guessed, matching original R Shiny `.add_to_history`
        if let Some(existing) = self.history.iter().find(|r| r.guess == guess).cloned() {
            if common == 5 && guess == self.secret_word {
                self.status = GameStatus::Won;
            }
            return Ok(existing);
        }

        let round_num = self.history.len() + 1;
        let round = Round {
            round: round_num,
            guess: guess.clone(),
            common,
        };
        self.history.push(round.clone());

        if common == 5 && guess == self.secret_word {
            self.status = GameStatus::Won;
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

/// Validates that a word is 5 lowercase ASCII letters, has no duplicate characters, and is in the SCOWL dictionary.
pub fn validate_word(word: &str) -> Result<(), GuessError> {
    if !word.bytes().all(|b| b.is_ascii_lowercase()) {
        if word.len() != 5 {
            return Err(GuessError::InvalidLength);
        }
        return Err(GuessError::NonAlphabetic);
    }
    if word.len() != 5 {
        return Err(GuessError::InvalidLength);
    }
    if !dict::has_no_duplicate_chars(word) {
        return Err(GuessError::DuplicateLetters);
    }
    if !dict::contains(word) {
        return Err(GuessError::NotInDictionary);
    }
    Ok(())
}

/// Counts how many letters in `guess` appear in `secret`.
/// Both words are assumed to be 5 distinct lowercase letters.
pub fn compare_guess(secret: &str, guess: &str) -> usize {
    let secret_bytes = secret.as_bytes();
    guess.bytes().filter(|b| secret_bytes.contains(b)).count()
}
