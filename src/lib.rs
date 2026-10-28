use std::io::{self, IsTerminal};
use std::panic;
use std::process;

use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

pub mod cli;
pub mod dict;
pub mod game;
pub mod plain;
pub mod tui;

use cli::CliArgs;
use game::{GameMode, GameState};

/// Shared CLI entrypoint for both `jotto` and `wordle` binaries.
pub fn run_cli(mode: GameMode) {
    let args = CliArgs::parse_for_mode(mode);

    let game = if args.daily {
        GameState::new_daily_with_mode(mode)
    } else {
        match GameState::new_with_mode(mode, args.difficulty, args.word.as_deref()) {
            Ok(g) => g,
            Err(err) => {
                eprintln!("Error initializing game with custom word: {err}");
                process::exit(1);
            }
        }
    };

    let use_plain = args.plain || !io::stdin().is_terminal() || !io::stdout().is_terminal();

    if use_plain {
        if let Err(err) = plain::run(game) {
            eprintln!("Error: {err}");
            process::exit(1);
        }
    } else {
        install_panic_hook();
        if let Err(err) = tui::run(game) {
            eprintln!("Error: {err}");
            process::exit(1);
        }
    }
}

fn install_panic_hook() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(panic_info);
    }));
}
