use clap::Parser;

use crate::game::Difficulty;

/// A CLI version of the classic Jotto 5-letter word game.
#[derive(Debug, Parser)]
#[command(name = "jotto", version, about, long_about = None)]
pub struct CliArgs {
    /// Run in plain line-by-line REPL mode instead of the full-screen TUI
    #[arg(short, long)]
    pub plain: bool,

    /// Word list difficulty level (easy=35, medium=50, hard=70, scrabble=80, insane=95)
    #[arg(short, long, value_enum, default_value_t = Difficulty::Scrabble)]
    pub difficulty: Difficulty,

    /// Play the deterministic UTC daily word challenge
    #[arg(long, conflicts_with = "word")]
    pub daily: bool,

    /// Optional custom 5-letter secret word (for testing or custom games)
    #[arg(short, long)]
    pub word: Option<String>,
}
