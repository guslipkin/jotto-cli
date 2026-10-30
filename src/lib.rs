use std::io::{self, IsTerminal};
use std::process;

pub mod cli;
pub mod contexto;
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

    run_frontend(game, args.plain, plain::run, tui::run);
}

/// Runs `game` in plain mode if asked or when either end isn't a terminal (piped
/// input or output), otherwise in the TUI, and exits with an error message on failure.
pub(crate) fn run_frontend<G>(
    game: G,
    plain: bool,
    run_plain: fn(G) -> io::Result<()>,
    run_tui: fn(G) -> io::Result<()>,
) {
    let use_plain = plain || !io::stdin().is_terminal() || !io::stdout().is_terminal();
    let res = if use_plain {
        run_plain(game)
    } else {
        run_tui(game)
    };
    if let Err(err) = res {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}
