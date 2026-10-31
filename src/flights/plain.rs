use std::io;
use std::str::FromStr;

use crate::contexto::game::grouped;
use crate::flights::data::MAX_CONNECTIONS;
use crate::flights::game::{
    code_str, parse_code, Code, Difficulty, FlightsGame, FlyOutcome, Status, Units, MAX_HOPS,
    MIN_HOPS,
};
use crate::line_input::LineReader;
use crate::save::Autosave;

/// Runs the line-by-line REPL / Plain CLI mode. There's no map here, so Easy and
/// Medium list the options the way Hard does, with Easy adding distances.
pub fn run(mut game: FlightsGame, autosave: &mut Autosave) -> io::Result<()> {
    match game.daily_date() {
        Some(date) => println!(
            "=== FLIGHTS [DAILY {date} UTC] === (Difficulty: {})",
            game.difficulty()
        ),
        None => println!("=== FLIGHTS === (Difficulty: {})", game.difficulty()),
    }
    println!(
        "Fly to the destination by the shortest route, changing one letter of the code per hop."
    );
    println!("Type an airport code, ':back' to go back, or ':help' for commands.");
    if let Some(notice) = autosave.notice.take() {
        println!("{}", notice.text());
    }
    print_position(&game);

    let mut reader = LineReader::new();
    loop {
        // Every command and flight comes back here, so this saves each change.
        autosave.sync(&game.to_saved());
        let Some(raw_line) = reader.read_line(&format!(
            "flights {}> ",
            game.network().code(game.current())
        ))?
        else {
            println!();
            break;
        };
        let lower = raw_line.trim().to_ascii_lowercase();
        if lower.is_empty() {
            continue;
        }
        let (cmd, arg) = match lower.split_once(char::is_whitespace) {
            Some((c, a)) => (c, Some(a.trim())),
            None => (lower.as_str(), None),
        };

        // A bare three-letter word is always a code, so commands need their colon
        // (bar `?`): "new", "end" and the like are airports.
        match cmd {
            ":quit" | ":exit" | ":q" => break,
            ":help" | "?" => print_help(&game),
            ":credits" => print_credits(),
            ":route" | ":options" => print_position(&game),
            ":back" | ":b" => match game.back() {
                Some(_) => print_position(&game),
                None if game.status() != Status::Playing => print_over(),
                None => println!("You're at the start; there's nowhere to go back to."),
            },
            ":new" | ":reset" => match arg.map(Difficulty::from_str).transpose() {
                Ok(d) => new_game(&mut game, d, "New game started!"),
                Err(e) => println!("{e}"),
            },
            ":difficulty" => match arg.map(Difficulty::from_str) {
                Some(Ok(d)) => {
                    new_game(&mut game, Some(d), "Difficulty changed. New game started!")
                }
                Some(Err(e)) => println!("{e}"),
                None => println!(
                    "Current difficulty: {} ({})",
                    game.difficulty(),
                    about(&game)
                ),
            },
            ":settings" => print_settings(&game),
            ":set" => match arg.map(|a| set(&mut game, a)) {
                Some(Ok(())) => print_settings(&game),
                Some(Err(e)) => println!("{e}"),
                None => println!("Usage: :set <setting> <value>. ':settings' lists them."),
            },
            ":units" => match arg.map(Units::from_str) {
                Some(Ok(u)) => {
                    game.set_units(u);
                    println!("Distances now in {u}.");
                }
                Some(Err(e)) => println!("{e}"),
                None => println!("Distances are in {} (choose nm, mi or km).", game.units()),
            },
            ":daily" => {
                game.start_daily();
                println!(
                    "Daily Challenge ({} UTC) started! Difficulty: {}.",
                    game.daily_date().unwrap_or("today"),
                    game.difficulty()
                );
                print_position(&game);
            }
            ":giveup" => {
                if game.status() != Status::Playing {
                    print_over();
                } else {
                    game.give_up();
                    println!(
                        "A shortest route: {} ({}).",
                        game.route_text(game.best_route()),
                        game.units().format(game.best_distance())
                    );
                    println!("{}", game.divergence_text());
                }
            }
            _ if cmd.starts_with(':') => {
                println!("Unknown command '{cmd}'. Type ':help' for commands.")
            }
            _ if game.status() != Status::Playing => print_over(),
            _ => match game.fly(&lower) {
                Ok(FlyOutcome::Flew { .. }) => print_position(&game),
                Ok(FlyOutcome::Longer { over }) => {
                    print_position(&game);
                    let how = if game.rules().shows_amount_over() {
                        format!(
                            "{} longer than the shortest route",
                            game.units().format(over)
                        )
                    } else {
                        "not by the shortest route".to_string()
                    };
                    println!(
                        "You reached {}, but {how}. ':back' to find a shorter way.",
                        game.end_code()
                    );
                }
                Ok(FlyOutcome::Won) => {
                    print_position(&game);
                    println!("CONGRATULATIONS!!! You found the shortest route.");
                    println!("{}", game.share_summary());
                }
                Err(err) => println!("{err}"),
            },
        }
    }

    Ok(())
}

/// Starts a new game (at `difficulty` if given) and says so, or says why not.
fn new_game(game: &mut FlightsGame, difficulty: Option<Difficulty>, done: &str) {
    match game.reset(difficulty) {
        Ok(()) => {
            println!(
                "{done} Difficulty: {} ({}).",
                game.difficulty(),
                about(game)
            );
            print_position(game);
        }
        Err(e) => println!("{e}"),
    }
}

/// What the game's difficulty deals and shows; its rules, for a custom game.
fn about(game: &FlightsGame) -> String {
    match game.difficulty() {
        Difficulty::Custom => game.rules().summary(),
        d => d.description().to_string(),
    }
}

fn print_settings(game: &FlightsGame) {
    let c = game.custom();
    println!();
    println!("Settings (':set <setting> <value>' changes one):");
    println!("  units        {:<10} nm, mi or km", game.units().label());
    println!("Custom game (':new custom' plays one):");
    println!(
        "  view         {:<10} easy, medium, hard or pilot: what's shown",
        c.view.label().to_ascii_lowercase()
    );
    println!(
        "  places       {:<10} on or off: city and country beside options",
        if c.show_places { "on" } else { "off" }
    );
    println!(
        "  mode         {:<10} random (within the limits below) or route (your start and end)",
        if c.fixed_route { "route" } else { "random" }
    );
    // The mode decides which of these apply; the others are kept for later.
    let unused = |used: bool| if used { "" } else { " (not used in this mode)" };
    println!(
        "  connections  {:<10} 0 to {MAX_CONNECTIONS}: connections a hop's ticket may have{}",
        c.max_connections,
        unused(!c.fixed_route)
    );
    println!(
        "  hops         {:<10} {MIN_HOPS} to {MAX_HOPS}, or none: hops in the shortest route{}",
        c.max_hops.map_or("none".to_string(), |n| n.to_string()),
        unused(!c.fixed_route)
    );
    let code = |c: &Option<Code>| c.as_ref().map_or("random", code_str).to_string();
    println!(
        "  start        {:<10} an airport code, or random{}",
        code(&c.start),
        unused(c.fixed_route)
    );
    println!(
        "  end          {:<10} an airport code, or random{}",
        code(&c.end),
        unused(c.fixed_route)
    );
    match (c.problem(), c.puzzle_count()) {
        (Some(why), _) => println!("Can't play these settings: {why}."),
        (None, n) => println!("Puzzles with these settings: {}", grouped(n)),
    }
    println!();
}

/// Applies ":set <setting> <value>" to the units or the custom settings.
fn set(game: &mut FlightsGame, arg: &str) -> Result<(), String> {
    let (name, value) = arg
        .split_once(char::is_whitespace)
        .map(|(n, v)| (n, v.trim()))
        .ok_or("Usage: :set <setting> <value>. ':settings' lists them.")?;
    let mut c = game.custom();
    match name {
        "units" | "unit" => game.set_units(Units::from_str(value)?),
        "view" | "level" | "shows" => {
            let view = Difficulty::from_str(value)?;
            if view == Difficulty::Custom {
                return Err("Choose a view: easy, medium, hard or pilot.".to_string());
            }
            c.view = view;
        }
        "mode" => {
            c.fixed_route = match value {
                "route" | "fixed" => true,
                "random" => false,
                _ => return Err("Mode must be random or route.".to_string()),
            };
        }
        "connections" | "conn" => {
            // Limits are for random puzzles, so setting one switches to them.
            c.fixed_route = false;
            c.max_connections = value
                .parse()
                .ok()
                .filter(|&n| n <= MAX_CONNECTIONS)
                .ok_or(format!("Connections must be 0 to {MAX_CONNECTIONS}."))?;
        }
        "hops" | "maxhops" => {
            c.fixed_route = false;
            c.max_hops = match value {
                "none" | "any" | "no" | "unlimited" => None,
                _ => Some(
                    value
                        .parse()
                        .ok()
                        .filter(|n| (MIN_HOPS..=MAX_HOPS).contains(n))
                        .ok_or(format!("Hops must be {MIN_HOPS} to {MAX_HOPS}, or none."))?,
                ),
            };
        }
        "places" | "cities" | "city" => {
            c.show_places = match value {
                "on" | "yes" | "true" | "show" => true,
                "off" | "no" | "false" | "hide" => false,
                _ => return Err("Places must be on or off.".to_string()),
            };
        }
        "start" | "from" | "end" | "to" => {
            let code = match value {
                "random" | "none" | "any" => None,
                _ => Some(parse_code(value).ok_or(format!(
                    "'{value}' isn't an airport code: give 3 letters, or random."
                ))?),
            };
            // A start or end is for a fixed route, so setting one switches to it.
            c.fixed_route = true;
            if matches!(name, "start" | "from") {
                c.start = code;
            } else {
                c.end = code;
            }
        }
        other => {
            return Err(format!(
                "Unknown setting '{other}'. ':settings' lists them."
            ))
        }
    }
    game.set_custom(c);
    Ok(())
}

fn print_over() {
    println!("The current game is over. Type ':new' or ':daily' to play again.");
}

/// " via ZZZ, YYY", or " direct".
fn via_text(via: &[&str]) -> String {
    if via.is_empty() {
        " direct".to_string()
    } else {
        format!(" via {}", via.join(", "))
    }
}

/// Prints the route so far and, while playing, where the player can fly next.
/// With no map, Medium and Hard look the same here: codes and connections.
fn print_position(game: &FlightsGame) {
    let net = game.network();
    let units = game.units();
    let show_via = game.rules().shows_connections();
    let show_distances = game.distances_visible();
    println!();
    let total = if show_distances {
        format!(", {}", units.format(game.total_distance()))
    } else {
        String::new()
    };
    // Every stop says where it is, as options do.
    let place = |i: usize| {
        let text = net.place(i);
        if game.rules().show_places && !text.is_empty() {
            format!(" ({text})")
        } else {
            String::new()
        }
    };
    println!(
        "{}{} → {}{}  |  {} hop{}{total}",
        game.start_code(),
        place(game.origin()),
        game.end_code(),
        place(game.destination()),
        game.hops(),
        if game.hops() == 1 { "" } else { "s" },
    );
    println!(
        "  {}  start{}",
        net.code(game.origin()),
        place(game.origin())
    );
    for w in game.route().windows(2) {
        let hop = net.hop(w[0], w[1]).expect("route legs are hops");
        let mut line = format!("→ {}", net.code(w[1]));
        if show_distances {
            line.push_str(&format!("  {}", units.format(hop.distance)));
        }
        if show_via {
            let codes: Vec<&str> = hop.via.iter().map(|&i| net.code(i)).collect();
            line.push_str(&via_text(&codes));
        }
        line.push_str(&place(w[1]));
        println!("{line}");
    }
    if game.status() == Status::Playing && !game.at_destination() {
        let options = game.options();
        if options.is_empty() {
            println!(
                "No tickets from here to an airport not already on your route. ':back' to go back."
            );
            println!();
            return;
        }
        let star = |airport: usize| {
            if airport == game.destination() {
                "  ★ destination"
            } else {
                ""
            }
        };
        let rules = game.rules();
        if !rules.shows_distances() && !show_via && !rules.show_places {
            let codes: Vec<String> = options
                .iter()
                .map(|o| {
                    if o.airport == game.destination() {
                        format!("{}★", o.code)
                    } else {
                        o.code.to_string()
                    }
                })
                .collect();
            println!("Options: {}", codes.join(" "));
        } else {
            let places: Vec<String> = options.iter().map(|o| net.place(o.airport)).collect();
            let place_width = places.iter().map(|p| p.chars().count()).max().unwrap_or(0);
            println!("Options:");
            for (o, place) in options.iter().zip(&places) {
                let mut line = format!("  {}", o.code);
                if rules.shows_distances() {
                    line.push_str(&format!("  {:>10}", units.format(o.distance)));
                }
                if rules.show_places {
                    line.push_str(&format!("  {place:<place_width$}"));
                }
                if show_via {
                    line.push_str(&via_text(&o.via));
                }
                line.push_str(star(o.airport));
                println!("{}", line.trim_end());
            }
        }
    }
    println!();
}

fn print_help(game: &FlightsGame) {
    println!();
    println!("Flights is a game about finding the shortest way between two airports:");
    println!("  - Each hop changes one letter of the airport code (LAX -> LAS -> LOS)");
    println!("  - A hop needs a ticket: a direct flight, or one connection (custom games choose)");
    println!("  - Distance is great-circle, and every connection counts toward it");
    println!("  - You can't fly to an airport already on your route; ':back' instead");
    println!(
        "  - Reach the destination the long way and you'll be told (and on Easy and Medium, by how"
    );
    println!("    much): go back and improve");
    println!();
    println!("Difficulty Levels (shortest route's hops; what you're shown):");
    for d in Difficulty::STANDARD {
        println!(
            "  - {:<7} {} ({} puzzles){}",
            d.label().to_ascii_lowercase(),
            d.description(),
            grouped(d.puzzle_count()),
            if d == Difficulty::Medium {
                " (default and daily)"
            } else {
                ""
            }
        );
    }
    let custom = game.custom();
    println!(
        "  - custom  {} ({} puzzles): ':settings' to change",
        custom.summary(),
        grouped(custom.puzzle_count())
    );
    println!("  Distances are shown on Easy only, until the game ends. Plain mode has no map,");
    println!("  so Medium and Hard look the same here.");
    println!();
    println!("Commands:");
    println!("  <code>               Fly to that airport");
    println!("  :back                Go back one hop");
    println!("  :route               Show your route and options");
    println!("  :new [difficulty]    Start a new random game");
    println!("  :daily               Start today's deterministic UTC daily challenge (medium)");
    println!("  :difficulty [level]  Show or change difficulty");
    println!("  :units [nm|mi|km]    Show or change distance units");
    println!("  :settings            Show the units and custom game settings");
    println!("  :set <name> <value>  Change a setting (':set hops 6', ':set start LAX')");
    println!("  :giveup              Reveal a shortest route");
    println!("  :credits             Show credits");
    println!("  :quit                Exit FLIGHTS");
    println!();
}

fn print_credits() {
    println!();
    println!("FLIGHTS CLI was written by Gus Lipkin (https://guslipkin.me).");
    println!("Routes come from English Wikipedia's airport articles (CC BY-SA 4.0).");
    println!("Airports and coordinates come from OurAirports (https://ourairports.com/data/), public domain.");
    println!();
}
