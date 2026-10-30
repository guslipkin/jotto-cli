//! `contexto`: find a secret word by meaning. Every guess is ranked by how close its
//! meaning is to the secret's, out of every guessable word, using GloVe word vectors.
//!
//! It shares the crate's SCOWL-era helpers (dates, colour detection, input cleanup) but
//! not jotto's and wordle's game engine or TUI, which are built around 5-letter words.

use std::process;

use clap::Parser;

pub mod data;
pub mod game;
pub mod plain;
pub mod tui;

use crate::save::{self, Autosave};
use game::{ContextoGame, Difficulty};

/// Find the secret word by its meaning: each guess is ranked by how close it is.
#[derive(Debug, Parser)]
#[command(name = "contexto", version, about, long_about = None)]
pub struct ContextoArgs {
    /// Run in plain line-by-line REPL mode instead of the full-screen TUI
    #[arg(short, long)]
    pub plain: bool,

    /// How abstract the secret word may be (easy = things, medium adds acts and
    /// events, hard adds ideas and qualities; default medium). Starts a new game
    /// instead of resuming a saved one
    #[arg(short, long, value_enum)]
    pub difficulty: Option<Difficulty>,

    /// Play the deterministic UTC daily word challenge (always medium; instead of
    /// resuming a saved game)
    #[arg(long, conflicts_with = "word")]
    pub daily: bool,

    /// Optional custom secret word (for testing or custom games; starts a new game
    /// instead of resuming a saved one)
    #[arg(short, long)]
    pub word: Option<String>,
}

/// CLI entrypoint for the `contexto` binary.
pub fn run_cli() {
    let args = ContextoArgs::parse();
    let path = save::save_path("contexto");
    let loaded = path.as_deref().map_or(save::Loaded::Missing, save::load);

    let (game, notice) =
        match ContextoGame::start(args.difficulty, args.daily, args.word.as_deref(), loaded) {
            Ok(started) => started,
            Err(err) => {
                eprintln!("Error initializing game with custom word: {err}");
                process::exit(1);
            }
        };

    let mut autosave = Autosave::new(path);
    autosave.notice = notice;
    crate::run_frontend(game, autosave, args.plain, plain::run, tui::run);
}
