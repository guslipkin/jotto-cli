use std::ffi::OsString;

use clap::{CommandFactory, FromArgMatches, Parser};

use crate::game::{Difficulty, GameMode};

/// A CLI version of the classic Jotto and Wordle 5-letter word games.
#[derive(Debug, Parser)]
#[command(name = "jotto", version, about, long_about = None)]
pub struct CliArgs {
    /// Run in plain line-by-line REPL mode instead of the full-screen TUI
    #[arg(short, long)]
    pub plain: bool,

    /// Word list difficulty level (easy=35, medium=50, hard=70, scrabble=80, insane=95;
    /// default scrabble). Starts a new game instead of resuming a saved one
    #[arg(short, long, value_enum)]
    pub difficulty: Option<Difficulty>,

    /// Play the deterministic UTC daily word challenge (instead of resuming a saved game)
    #[arg(long, conflicts_with = "word")]
    pub daily: bool,

    /// Optional custom 5-letter secret word (for testing or custom games; starts a new
    /// game instead of resuming a saved one)
    #[arg(short, long)]
    pub word: Option<String>,
}

impl CliArgs {
    /// Parses `args` (the program's name first) as the given `GameMode`'s command
    /// (`jotto` or `wordle`).
    pub fn parse_for_mode(mode: GameMode, args: impl IntoIterator<Item = OsString>) -> Self {
        let about = match mode {
            GameMode::Jotto => "A CLI version of the classic Jotto 5-letter word game.",
            GameMode::Wordle => "A CLI version of the classic Wordle 5-letter word game.",
        };
        let cmd = Self::command().name(mode.command_name()).about(about);
        let matches = cmd.get_matches_from(args);
        Self::from_arg_matches(&matches).unwrap_or_else(|e| e.exit())
    }
}
