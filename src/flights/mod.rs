//! `flights`: get from one airport to another by hopping between codes one letter
//! apart that have a direct flight, by the shortest total distance.
//!
//! It shares the crate's save, date and terminal helpers but has its own engine and
//! TUI, since its moves are flights on a map rather than words.

use std::process;

use clap::Parser;

pub mod coast;
pub mod data;
pub mod game;
pub mod plain;
pub mod tui;

use crate::save::{self, Autosave};
use game::{Difficulty, FlightsGame, Units};

/// Fly from one airport to another, changing one letter of the code per flight,
/// by the shortest route.
#[derive(Debug, Parser)]
#[command(name = "flights", version, about, long_about = None)]
pub struct FlightsArgs {
    /// Run in plain line-by-line REPL mode instead of the full-screen TUI
    #[arg(short, long)]
    pub plain: bool,

    /// What you're shown (easy = map and flight distances, medium = map, hard =
    /// codes only; default medium). Starts a new game instead of resuming a saved one
    #[arg(short, long, value_enum)]
    pub difficulty: Option<Difficulty>,

    /// Play the deterministic UTC daily puzzle (always medium; instead of resuming a
    /// saved game)
    #[arg(long, conflicts_with = "route")]
    pub daily: bool,

    /// Distance units (default nm, nautical miles; remembered for later games)
    #[arg(short, long, value_enum)]
    pub units: Option<Units>,

    /// Optional custom puzzle as START-END, e.g. LAX-LOS (for testing or custom
    /// games; starts a new game instead of resuming a saved one)
    #[arg(short, long)]
    pub route: Option<String>,
}

/// CLI entrypoint for the `flights` binary.
pub fn run_cli() {
    let args = FlightsArgs::parse();
    let path = save::save_path("flights");
    let loaded = path.as_deref().map_or(save::Loaded::Missing, save::load);

    let (game, notice) = match FlightsGame::start(
        args.difficulty,
        args.daily,
        args.route.as_deref(),
        args.units,
        loaded,
    ) {
        Ok(started) => started,
        Err(err) => {
            eprintln!("Error initializing game with custom route: {err}");
            process::exit(1);
        }
    };

    let mut autosave = Autosave::new(path);
    autosave.notice = notice;
    crate::run_frontend(game, autosave, args.plain, plain::run, tui::run);
}
