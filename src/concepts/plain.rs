use std::io::{self, BufRead, Write};
use std::str::FromStr;

use crate::concepts::data::lexicon;
use crate::concepts::game::{
    bar_fraction, grouped, Closeness, ConceptsGame, Difficulty, Guess, HintError, Status,
};
use crate::game::supports_color;

/// Closest guesses shown after each guess (`:history` shows them all).
const BOARD_ROWS: usize = 10;
const BAR_WIDTH: usize = 20;
/// Nearest words listed when a game ends.
const NEAREST_SHOWN: usize = 15;

/// Runs the line-by-line REPL / Plain CLI mode.
pub fn run(mut game: ConceptsGame) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let use_color = supports_color();
    let lex = lexicon();

    if let Some(date) = game.daily_date() {
        println!(
            "=== CONCEPTS [DAILY {date} UTC] === (Difficulty: {} | {} words)",
            game.difficulty(),
            grouped(lex.guessable_count())
        );
    } else {
        println!(
            "=== CONCEPTS === (Difficulty: {} | {} words)",
            game.difficulty(),
            grouped(lex.guessable_count())
        );
    }
    println!(
        "Guess the secret word. Each guess is ranked by how close its meaning is (1 = the word)."
    );
    println!("Type a word, or ':help' for commands.");

    let mut lines = stdin.lock().lines();
    loop {
        print!("concepts> ");
        stdout.flush()?;

        let Some(line_res) = lines.next() else {
            println!();
            break;
        };
        let raw_line = line_res?;
        let lower = raw_line.trim().to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }

        // Commands that take a difficulty argument.
        let (cmd, arg) = match lower.split_once(char::is_whitespace) {
            Some((c, a)) => (c, Some(a.trim())),
            None => (lower.as_str(), None),
        };
        match cmd {
            ":new" | "new" | ":reset" | "reset" => {
                match arg.map(Difficulty::from_str).transpose() {
                    Ok(d) => {
                        game.reset(d);
                        println!(
                            "New game started! Difficulty: {} ({} possible answers).",
                            game.difficulty(),
                            grouped(game.difficulty().secret_count())
                        );
                    }
                    Err(e) => println!("{e}"),
                }
                continue;
            }
            ":difficulty" | "difficulty" => {
                match arg.map(Difficulty::from_str) {
                    Some(Ok(d)) => {
                        game.reset(Some(d));
                        println!(
                            "Difficulty set to {} ({} possible answers). New game started!",
                            game.difficulty(),
                            grouped(game.difficulty().secret_count())
                        );
                    }
                    Some(Err(e)) => println!("{e}"),
                    None => println!(
                        "Current difficulty: {} ({})",
                        game.difficulty(),
                        game.difficulty().description()
                    ),
                }
                continue;
            }
            _ => {}
        }

        match lower.as_str() {
            ":quit" | "quit" | ":exit" | "exit" | ":q" => break,
            ":help" | "help" | "?" => {
                print_help();
                continue;
            }
            ":credits" | "credits" => {
                print_credits();
                continue;
            }
            ":history" | "history" => {
                print_board(&game, usize::MAX, None, use_color);
                continue;
            }
            ":hint" | "hint" => {
                match game.hint() {
                    Ok(hint) => {
                        println!(
                            "Hint: '{}' is #{}.",
                            hint.word.to_ascii_uppercase(),
                            hint.rank
                        );
                        print_board(&game, BOARD_ROWS, Some(hint.number), use_color);
                    }
                    Err(HintError::GameOver) => {
                        println!("The current game is over. Type ':new' or ':daily' to play again.")
                    }
                    Err(err) => println!("{err}"),
                }
                continue;
            }
            ":giveup" | "giveup" | "give up" => {
                if game.status() != Status::Playing {
                    println!(
                        "The current game is already over. Type ':new' or ':daily' to play again."
                    );
                } else {
                    game.give_up();
                    println!("The word was: {}", game.secret_word().to_ascii_uppercase());
                    print_nearest(&game);
                }
                continue;
            }
            ":daily" | "daily" => {
                game.start_daily();
                println!(
                    "Daily Challenge ({} UTC) started! Difficulty: {}.",
                    game.daily_date().unwrap_or("today"),
                    game.difficulty()
                );
                continue;
            }
            _ => {}
        }

        if game.status() != Status::Playing {
            println!(
                "The current game is over. Type ':new' to start a new game or ':quit' to exit."
            );
            continue;
        }

        match game.make_guess(&lower) {
            Ok(outcome) if outcome.duplicate => {
                println!(
                    "Already guessed '{}' (#{}).",
                    outcome.guess.word.to_ascii_uppercase(),
                    outcome.guess.rank
                );
            }
            Ok(outcome) => {
                print_board(&game, BOARD_ROWS, Some(outcome.guess.number), use_color);
                if game.status() == Status::Won {
                    println!(
                        "CONGRATULATIONS!!! You found {} in {} {}{}.",
                        game.secret_word().to_ascii_uppercase(),
                        game.guess_count(),
                        if game.guess_count() == 1 {
                            "guess"
                        } else {
                            "guesses"
                        },
                        hints_suffix(&game)
                    );
                    print_nearest(&game);
                }
            }
            Err(err) => println!("{err}"),
        }
    }

    Ok(())
}

fn hints_suffix(game: &ConceptsGame) -> String {
    match game.hint_count() {
        0 => String::new(),
        1 => " and 1 hint".to_string(),
        n => format!(" and {n} hints"),
    }
}

/// Formats one board row: rank, word (with what was typed, for an inflection) and a closeness bar.
fn format_row(g: &Guess, use_color: bool) -> String {
    let filled = (bar_fraction(g.rank) * BAR_WIDTH as f64).round() as usize;
    let mut label = g.word.to_ascii_uppercase();
    if let Some(typed) = &g.typed {
        label.push_str(&format!(" ({typed})"));
    }
    if g.hint {
        label.push_str(" *");
    }
    let bar = if use_color {
        let rgb = match Closeness::of(g.rank) {
            Closeness::Close => "0;158;115",
            Closeness::Warm => "230;159;0",
            Closeness::Far => "213;94;0",
        };
        format!(
            "\x1b[38;2;{rgb}m{}\x1b[0m\x1b[90m{}\x1b[0m",
            "█".repeat(filled),
            "░".repeat(BAR_WIDTH - filled)
        )
    } else {
        format!("[{}{}]", "#".repeat(filled), ".".repeat(BAR_WIDTH - filled))
    };
    format!("{:>6}  {:<24} {bar}", format!("#{}", g.rank), label)
}

/// Prints the `limit` closest guesses, plus the latest guess if it isn't among them.
fn print_board(game: &ConceptsGame, limit: usize, latest: Option<usize>, use_color: bool) {
    println!();
    if game.guesses().is_empty() {
        println!("No guesses yet.");
        println!();
        return;
    }
    if let Some(g) = latest.and_then(|n| game.guesses().iter().find(|g| g.number == n)) {
        println!("Latest:");
        println!("{}", format_row(g, use_color));
        println!();
    }
    let sorted = game.guesses_by_rank();
    println!(
        "Closest ({} {}{}):",
        game.guess_count(),
        if game.guess_count() == 1 {
            "guess"
        } else {
            "guesses"
        },
        hints_suffix(game)
    );
    for g in sorted.iter().take(limit) {
        println!("{}", format_row(g, use_color));
    }
    if sorted.len() > limit {
        println!("  ... {} more (':history' shows all)", sorted.len() - limit);
    }
    println!();
}

fn print_nearest(game: &ConceptsGame) {
    println!(
        "Closest words to {}: {}",
        game.secret_word().to_ascii_uppercase(),
        game.nearest(NEAREST_SHOWN).join(", ")
    );
}

fn print_help() {
    let lex = lexicon();
    println!();
    println!("Concepts is a game where you find a secret word by its meaning:");
    println!("  - The secret word is a common noun");
    println!(
        "  - {} words are accepted; each guess is ranked by closeness out of {} (#1 is the word)",
        grouped(lex.guessable_count()),
        grouped(lex.ranked_count())
    );
    println!("  - Plurals and other forms count as their base word ('mice' is 'mouse')");
    println!("  - Ranks come from how words are used, so 'closeness' can be about topic, not just synonyms");
    println!("  - Guesses are unlimited; a hint (marked *) reveals the word at half your best guess's rank");
    println!("    (#300 before any guess). Hints don't count as guesses, so a new hint needs a closer guess");
    println!();
    println!("Difficulty Levels (each includes the easier levels' words):");
    for d in [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard] {
        println!(
            "  - {:<7} {} ({} possible answers{})",
            d.label().to_ascii_lowercase(),
            d.description(),
            grouped(d.secret_count()),
            if d == Difficulty::Medium {
                ", default and daily"
            } else {
                ""
            }
        );
    }
    println!();
    println!("Commands:");
    println!("  :help                Show this help message");
    println!("  :history             Show every guess, closest first");
    println!("  :hint                Reveal a closer word");
    println!("  :new [difficulty]    Start a new random game");
    println!("  :daily               Start today's deterministic UTC daily challenge (medium)");
    println!("  :difficulty [level]  Show or change difficulty");
    println!("  :giveup              Reveal the secret word");
    println!("  :credits             Show credits");
    println!("  :quit                Exit CONCEPTS");
    println!();
}

fn print_credits() {
    println!();
    println!("CONCEPTS CLI was written by Gus Lipkin (https://guslipkin.me).");
    println!("It's inspired by Contexto: play the original at https://contexto.me/");
    println!("The word list comes from SCOWL (http://wordlist.aspell.net/).");
    println!("Difficulty levels come from WordNet 3.0's noun categories (https://wordnet.princeton.edu/), Princeton University.");
    println!("Word meanings come from GloVe (https://nlp.stanford.edu/projects/glove/), Public Domain Dedication and License.");
    println!();
}
