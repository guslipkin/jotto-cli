use std::io::{self, IsTerminal};
use std::panic;
use std::process;

use clap::Parser;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

use jotto::cli::CliArgs;
use jotto::game::GameState;
use jotto::{plain, tui};

fn main() {
    let args = CliArgs::parse();

    let game = if args.daily {
        GameState::new_daily()
    } else {
        match GameState::new(args.difficulty, args.word.as_deref()) {
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
