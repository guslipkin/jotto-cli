//! `concepts`: find a secret word by meaning. Every guess is ranked by how close its
//! meaning is to the secret's, out of every guessable word, using GloVe word vectors.
//!
//! It shares the crate's SCOWL-era helpers (dates, colour detection, input cleanup) but
//! not jotto's and wordle's game engine or TUI, which are built around 5-letter words.

use std::io::{self, IsTerminal};
use std::process;

use clap::Parser;

pub mod data;
pub mod game;
pub mod plain;
pub mod tui;

use game::{ConceptsGame, Difficulty};

/// Find the secret word by its meaning: each guess is ranked by how close it is.
#[derive(Debug, Parser)]
#[command(name = "concepts", version, about, long_about = None)]
pub struct ConceptsArgs {
    /// Run in plain line-by-line REPL mode instead of the full-screen TUI
    #[arg(short, long)]
    pub plain: bool,

    /// How abstract the secret word may be (easy = things, medium adds acts and
    /// events, hard adds ideas and qualities)
    #[arg(short, long, value_enum, default_value_t = Difficulty::Medium)]
    pub difficulty: Difficulty,

    /// Play the deterministic UTC daily word challenge (always medium)
    #[arg(long, conflicts_with = "word")]
    pub daily: bool,

    /// Optional custom secret word (for testing or custom games)
    #[arg(short, long)]
    pub word: Option<String>,
}

/// CLI entrypoint for the `concepts` binary.
pub fn run_cli() {
    let args = ConceptsArgs::parse();

    let game = if args.daily {
        ConceptsGame::new_daily()
    } else {
        match ConceptsGame::new(args.difficulty, args.word.as_deref()) {
            Ok(g) => g,
            Err(err) => {
                eprintln!("Error initializing game with custom word: {err}");
                process::exit(1);
            }
        }
    };

    let use_plain = args.plain || !io::stdin().is_terminal() || !io::stdout().is_terminal();
    let res = if use_plain {
        plain::run(game)
    } else {
        tui::run(game)
    };
    if let Err(err) = res {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}
