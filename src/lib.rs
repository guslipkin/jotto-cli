use std::io::{self, IsTerminal};
use std::process;

pub mod cli;
pub mod contexto;
pub mod dict;
pub mod flights;
pub mod game;
mod help;
pub mod line_input;
pub mod plain;
pub mod save;
pub mod tui;

use cli::CliArgs;
use game::{GameMode, GameState};
use save::Autosave;

/// Plays Jotto or Wordle in its TUI, resuming the save as the command does with no
/// arguments, and returns (rather than exiting on) any error: what `arcade` runs.
pub fn play(mode: GameMode) -> Result<(), String> {
    play_saved(
        mode.command_name(),
        |loaded| GameState::start(mode, None, false, None, loaded).map_err(|e| e.to_string()),
        tui::run,
    )
}

/// Opens a game from its save with `start`, plays it with `run` and keeps the save
/// up to date, returning what went wrong instead of exiting, so a caller such as
/// `arcade` can say so and carry on.
pub(crate) fn play_saved<S: serde::de::DeserializeOwned, G>(
    name: &str,
    start: impl FnOnce(save::Loaded<S>) -> Result<(G, Option<save::Notice>), String>,
    run: fn(G, &mut Autosave) -> io::Result<()>,
) -> Result<(), String> {
    let path = save::save_path(name);
    let loaded = path.as_deref().map_or(save::Loaded::Missing, save::load);
    let (game, notice) = start(loaded)?;
    let mut autosave = Autosave::new(path);
    autosave.notice = notice;
    run(game, &mut autosave).map_err(|e| e.to_string())?;
    match autosave.error() {
        Some(err) => Err(format!("your game wasn't saved: {err}")),
        None => Ok(()),
    }
}

/// Shared CLI entrypoint for both `jotto` and `wordle` binaries.
pub fn run_cli(mode: GameMode) {
    let args = CliArgs::parse_for_mode(mode);
    let path = save::save_path(mode.command_name());
    let loaded = path.as_deref().map_or(save::Loaded::Missing, save::load);

    let (game, notice) = match GameState::start(
        mode,
        args.difficulty,
        args.daily,
        args.word.as_deref(),
        loaded,
    ) {
        Ok(started) => started,
        Err(err) => {
            eprintln!("Error initializing game with custom word: {err}");
            process::exit(1);
        }
    };

    let mut autosave = Autosave::new(path);
    autosave.notice = notice;
    run_frontend(game, autosave, args.plain, plain::run, tui::run);
}

/// Runs `game` in plain mode if asked or when either end isn't a terminal (piped
/// input or output), otherwise in the TUI, and exits with an error message on failure.
/// A save that failed is reported once the game has closed, since the TUI would hide it.
pub(crate) fn run_frontend<G>(
    game: G,
    mut autosave: Autosave,
    plain: bool,
    run_plain: fn(G, &mut Autosave) -> io::Result<()>,
    run_tui: fn(G, &mut Autosave) -> io::Result<()>,
) {
    let use_plain = plain || !io::stdin().is_terminal() || !io::stdout().is_terminal();
    let res = if use_plain {
        run_plain(game, &mut autosave)
    } else {
        run_tui(game, &mut autosave)
    };
    if let Some(err) = autosave.error() {
        eprintln!("Your game wasn't saved: {err}.");
    }
    if let Err(err) = res {
        eprintln!("Error: {err}");
        process::exit(1);
    }
}
