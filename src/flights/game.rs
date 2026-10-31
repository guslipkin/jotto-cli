use std::fmt;
use std::str::FromStr;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::contexto::game::grouped;
use crate::dict;
use crate::flights::data::{
    letters_apart, network, Network, MAX_CONNECTIONS, STANDARD_CONNECTIONS,
};
use crate::game::sanitize_input;
use crate::save::{self, enum_name, parse_enum, Loaded, Notice, Saveable, SAVE_VERSION};

/// How long a puzzle may be and how much the player is shown. Each standard level
/// deals the easier levels' puzzles too, and shows less than the one before it.
/// Custom plays by the player's own [`Rules`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Difficulty {
    /// Up to 4 hops; a map, with distances and connections
    #[value(alias = "e")]
    Easy,
    /// Up to 6 hops; a map and connections, no distances
    #[default]
    #[value(alias = "m", alias = "med")]
    Medium,
    /// Up to 8 hops; connections, but no map or distances
    #[value(alias = "h")]
    Hard,
    /// Any number of hops; only codes, never connections
    #[value(alias = "p")]
    Pilot,
    /// Your own settings (see the settings page, or `:settings` in plain mode)
    #[value(alias = "c")]
    Custom,
}

impl Difficulty {
    /// The standard levels, easiest first.
    pub const STANDARD: [Self; 4] = [Self::Easy, Self::Medium, Self::Hard, Self::Pilot];

    /// Human-readable label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Easy => "Easy",
            Self::Medium => "Medium",
            Self::Hard => "Hard",
            Self::Pilot => "Pilot",
            Self::Custom => "Custom",
        }
    }

    /// What the level deals and shows.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Easy => "up to 4 hops; map, distances and connections",
            Self::Medium => "up to 6 hops; map and connections",
            Self::Hard => "up to 8 hops; connections only",
            Self::Pilot => "any length; codes only",
            Self::Custom => "your own settings",
        }
    }

    /// Cycles to the next difficulty level.
    pub const fn next(self) -> Self {
        match self {
            Self::Easy => Self::Medium,
            Self::Medium => Self::Hard,
            Self::Hard => Self::Pilot,
            Self::Pilot => Self::Custom,
            Self::Custom => Self::Easy,
        }
    }

    /// The rules a game at this level plays by: a standard level's own, or
    /// `custom` for [`Difficulty::Custom`].
    pub fn rules(self, custom: Rules) -> Rules {
        let (max_hops, view) = match self {
            Self::Easy => (Some(4), self),
            Self::Medium => (Some(6), self),
            Self::Hard => (Some(8), self),
            Self::Pilot => (None, self),
            Self::Custom => return custom,
        };
        Rules {
            view,
            max_hops,
            max_connections: STANDARD_CONNECTIONS,
            show_places: true,
            fixed_route: false,
            start: None,
            end: None,
        }
    }

    /// Number of puzzles a standard level can deal (custom's depend on its rules).
    pub fn puzzle_count(self) -> usize {
        self.rules(Rules::default()).puzzle_count()
    }
}

impl fmt::Display for Difficulty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Difficulty {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "easy" | "e" => Ok(Self::Easy),
            "medium" | "med" | "m" => Ok(Self::Medium),
            "hard" | "h" => Ok(Self::Hard),
            "pilot" | "p" => Ok(Self::Pilot),
            "custom" | "c" => Ok(Self::Custom),
            other => Err(format!(
                "Unknown difficulty '{other}'. Choose: easy, medium, hard, pilot or custom."
            )),
        }
    }
}

/// Smallest and largest hop limit a custom game may set; every puzzle takes at
/// least 3, since start and end differ in every letter. (Custom may also have no limit.)
pub const MIN_HOPS: usize = 3;
/// See [`MIN_HOPS`].
pub const MAX_HOPS: usize = 20;

/// What a game deals and shows: a standard level's fixed rules, or a custom game's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    /// The standard level whose view is used: what's shown (never `Custom`).
    pub view: Difficulty,
    /// Most hops a dealt puzzle's shortest route has (`None`: no limit).
    pub max_hops: Option<usize>,
    /// Most connections a hop may have.
    pub max_connections: usize,
    /// Whether options show their airport's city and country.
    pub show_places: bool,
    /// Whether a custom game plays a route of the player's choosing (`start` and
    /// `end`, at the standard connections and any length) rather than random
    /// puzzles within `max_hops` and `max_connections`. The two are a switch, not
    /// a mix, so a chosen route is never ruled out by limits set for random ones.
    pub fixed_route: bool,
    /// A custom game's own start airport, else a random one.
    pub start: Option<Code>,
    /// A custom game's own destination, else a random one.
    pub end: Option<Code>,
}

/// An airport code, kept as bytes so [`Rules`] stays `Copy`.
pub type Code = [u8; 3];

/// Reads a code typed in any case ("lax"), if it's three letters.
pub fn parse_code(text: &str) -> Option<Code> {
    let text = text.trim().to_ascii_uppercase();
    let bytes: Code = text.as_bytes().try_into().ok()?;
    bytes.iter().all(u8::is_ascii_uppercase).then_some(bytes)
}

/// A code as text.
pub fn code_str(code: &Code) -> &str {
    std::str::from_utf8(code).expect("codes are ASCII letters")
}

impl Default for Rules {
    /// Custom's starting settings: Medium's.
    fn default() -> Self {
        Self {
            view: Difficulty::Medium,
            max_hops: Some(6),
            max_connections: STANDARD_CONNECTIONS,
            show_places: true,
            fixed_route: false,
            start: None,
            end: None,
        }
    }
}

impl Rules {
    /// Most hops a dealt puzzle's shortest route has (`None`: no limit); a fixed
    /// route has none.
    pub fn hop_limit(&self) -> Option<usize> {
        if self.fixed_route {
            None
        } else {
            self.max_hops
        }
    }

    /// Most connections a hop may have; a fixed route uses the standard levels'.
    pub fn connections(&self) -> usize {
        if self.fixed_route {
            STANDARD_CONNECTIONS
        } else {
            self.max_connections
        }
    }

    /// The network these rules' hops come from.
    pub fn network(&self) -> &'static Network {
        network(self.connections())
    }

    /// Number of puzzles these rules can deal: from the data file's counts, or for
    /// a custom start or end, by searching from it.
    pub fn puzzle_count(&self) -> usize {
        self.puzzle_count_in(self.network())
    }

    /// [`Rules::puzzle_count`] on `net`.
    pub fn puzzle_count_in(&self, net: &Network) -> usize {
        match self.fixed(net) {
            Err(_) => 0,
            Ok((None, None)) => net.puzzle_count(self.hop_limit()),
            Ok((Some(a), None) | (None, Some(a))) => net.puzzle_ends(a, self.hop_limit()).len(),
            Ok((Some(_), Some(_))) => 1,
        }
    }

    /// The custom start and end as airports in `net`, or why they can't be played:
    /// an airport not in the game at this many connections, or a pair that doesn't
    /// make a puzzle by these rules.
    fn fixed(&self, net: &Network) -> Result<(Option<usize>, Option<usize>), String> {
        let find = |code: &Option<Code>| -> Result<Option<usize>, String> {
            code.as_ref()
                .map(|c| {
                    net.index_of(code_str(c))
                        .filter(|&i| !net.hops(i).is_empty())
                        .ok_or_else(|| {
                            format!(
                                "{} has no hops with {} connection{}",
                                code_str(c),
                                self.connections(),
                                if self.connections() == 1 { "" } else { "s" }
                            )
                        })
                })
                .transpose()
        };
        // Random puzzles ignore a start and end left from fixed-route play.
        if !self.fixed_route {
            return Ok((None, None));
        }
        let (start, end) = (find(&self.start)?, find(&self.end)?);
        if let (Some(a), Some(b)) = (start, end) {
            let (ca, cb) = (net.code(a), net.code(b));
            if letters_apart(ca, cb) != 3 {
                return Err(format!("{ca} and {cb} must differ in every letter"));
            }
            if !net.puzzle_ends(a, self.hop_limit()).contains(&b) {
                return Err(match net.shortest(a, b) {
                    None => format!("there's no way from {ca} to {cb}"),
                    Some((_, path)) if self.hop_limit().is_some_and(|max| path.len() - 1 > max) => {
                        format!(
                            "{ca} to {cb} takes {} hops, more than the max",
                            path.len() - 1
                        )
                    }
                    Some(_) => format!("only one route joins {ca} and {cb}"),
                });
            }
        }
        Ok((start, end))
    }

    /// Why these rules can deal no puzzle, if they can't.
    pub fn problem(&self) -> Option<String> {
        match self.fixed(self.network()) {
            Err(why) => Some(why),
            Ok(_) if self.puzzle_count() == 0 => Some("no puzzles fit these settings".to_string()),
            Ok(_) => None,
        }
    }

    /// A random puzzle from `net` by these rules, keeping a custom start or end.
    /// Puzzles read the same either way round, so a fixed end is searched from.
    pub fn deal(&self, net: &Network, rng: &mut fastrand::Rng) -> Result<(usize, usize), String> {
        let none = || {
            self.problem()
                .unwrap_or_else(|| "no puzzles fit these settings".to_string())
        };
        let pick = |ends: Vec<usize>, rng: &mut fastrand::Rng| {
            (!ends.is_empty()).then(|| ends[rng.usize(0..ends.len())])
        };
        match self.fixed(net)? {
            (None, None) => net.random_puzzle(rng, self.hop_limit()).ok_or_else(none),
            (Some(a), None) => pick(net.puzzle_ends(a, self.hop_limit()), rng)
                .map(|b| (a, b))
                .ok_or_else(none),
            (None, Some(b)) => pick(net.puzzle_ends(b, self.hop_limit()), rng)
                .map(|a| (a, b))
                .ok_or_else(none),
            (Some(a), Some(b)) => Ok((a, b)),
        }
    }

    /// Whether the map is drawn.
    pub const fn shows_map(&self) -> bool {
        matches!(self.view, Difficulty::Easy | Difficulty::Medium)
    }

    /// Whether distances are shown while playing (they all are once a game ends).
    pub const fn shows_distances(&self) -> bool {
        matches!(self.view, Difficulty::Easy)
    }

    /// Whether arriving the long way says by how much.
    pub const fn shows_amount_over(&self) -> bool {
        matches!(self.view, Difficulty::Easy | Difficulty::Medium)
    }

    /// Whether hops show where they connect, before and after they're flown.
    pub const fn shows_connections(&self) -> bool {
        !matches!(self.view, Difficulty::Pilot)
    }

    /// "up to 6 hops" or "any number of hops".
    pub fn hops_text(&self) -> String {
        match self.hop_limit() {
            Some(MIN_HOPS) => format!("{MIN_HOPS} hops"),
            Some(n) => format!("up to {n} hops"),
            None => "any number of hops".to_string(),
        }
    }

    /// One line describing the rules ("Hard view, up to 6 hops, 0 connections, places off").
    pub fn summary(&self) -> String {
        if self.fixed_route {
            let code = |c: &Option<Code>| c.as_ref().map_or("random", code_str).to_string();
            return format!(
                "{} view, route {} → {}{}",
                self.view,
                code(&self.start),
                code(&self.end),
                if self.show_places { "" } else { ", no places" }
            );
        }
        format!(
            "{} view, {}, up to {} connection{}{}",
            self.view,
            self.hops_text(),
            self.connections(),
            if self.connections() == 1 { "" } else { "s" },
            if self.show_places { "" } else { ", no places" }
        )
    }
}

/// [`Rules`] as saved: names rather than enums, so the file reads as the
/// command line would.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedRules {
    pub view: String,
    pub max_hops: Option<usize>,
    pub max_connections: usize,
    pub show_places: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fixed_route: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
}

impl From<Rules> for SavedRules {
    fn from(r: Rules) -> Self {
        Self {
            view: enum_name(r.view),
            max_hops: r.max_hops,
            max_connections: r.max_connections,
            show_places: r.show_places,
            fixed_route: r.fixed_route,
            start: r.start.as_ref().map(|c| code_str(c).to_string()),
            end: r.end.as_ref().map(|c| code_str(c).to_string()),
        }
    }
}

impl SavedRules {
    /// The rules, or `None` when the save holds settings this version can't play.
    pub fn rules(&self) -> Option<Rules> {
        let view: Difficulty = parse_enum(&self.view)?;
        let hops_ok = self
            .max_hops
            .is_none_or(|n| (MIN_HOPS..=MAX_HOPS).contains(&n));
        let code = |c: &Option<String>| match c {
            None => Some(None),
            Some(text) => parse_code(text).map(Some),
        };
        let (start, end) = (code(&self.start)?, code(&self.end)?);
        (view != Difficulty::Custom && hops_ok && self.max_connections <= MAX_CONNECTIONS)
            .then_some(Rules {
                view,
                max_hops: self.max_hops,
                max_connections: self.max_connections,
                show_places: self.show_places,
                fixed_route: self.fixed_route,
                start,
                end,
            })
    }
}

/// Daily games always use this level, so everyone gets the same length of puzzle
/// and sees the same thing.
pub const DAILY_DIFFICULTY: Difficulty = Difficulty::Medium;

/// Distance units. Distances are kept in nautical miles, the unit aviation uses,
/// and only converted for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum Units {
    /// Nautical miles
    #[default]
    #[value(alias = "nmi")]
    Nm,
    /// Statute miles
    #[value(alias = "miles")]
    Mi,
    /// Kilometres
    #[value(alias = "kilometers", alias = "kilometres")]
    Km,
}

impl Units {
    /// Abbreviation shown after a distance.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Nm => "nm",
            Self::Mi => "mi",
            Self::Km => "km",
        }
    }

    /// Cycles to the next unit.
    pub const fn next(self) -> Self {
        match self {
            Self::Nm => Self::Mi,
            Self::Mi => Self::Km,
            Self::Km => Self::Nm,
        }
    }

    /// Converts nautical miles to this unit.
    pub fn convert(self, nm: f64) -> f64 {
        match self {
            Self::Nm => nm,
            Self::Mi => nm * 1.150_779,
            Self::Km => nm * 1.852,
        }
    }

    /// A distance in nautical miles, rounded and shown in this unit ("1,234 nm").
    pub fn format(self, nm: f64) -> String {
        format!(
            "{} {}",
            grouped(self.convert(nm).round() as usize),
            self.label()
        )
    }
}

impl fmt::Display for Units {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Units {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_enum(s.trim())
            .ok_or_else(|| format!("Unknown units '{}'. Choose: nm, mi or km.", s.trim()))
    }
}

/// Totals within this many nautical miles count as equal, so a route tied with the
/// best one wins despite floating-point rounding.
const TIE_NM: f64 = 1e-6;

/// Why a flight couldn't be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlyError {
    GameOver,
    Empty,
    /// Not three letters.
    NotACode(String),
    /// A code with no route to a code one letter away, so not in the game.
    UnknownAirport(String),
    /// The player is at the destination by a longer route and has to go back.
    AtDestination,
    AlreadyOnRoute(String),
    NotOneLetter {
        from: String,
        to: String,
    },
    NoTicket {
        from: String,
        to: String,
        max_connections: usize,
    },
}

impl fmt::Display for FlyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GameOver => {
                f.write_str("The current game is already over. Start a new game to play again.")
            }
            Self::Empty => f.write_str("Type an airport code to fly to."),
            Self::NotACode(s) => write!(f, "'{s}' isn't an airport code: codes are 3 letters."),
            Self::UnknownAirport(c) => write!(
                f,
                "{c} isn't in the game: it has no flights to a code one letter away."
            ),
            Self::AtDestination => f.write_str(
                "You're at the destination, but not by the shortest route. Go back to try another.",
            ),
            Self::AlreadyOnRoute(c) => write!(
                f,
                "{c} is already on your route. Go back to it instead of flying there."
            ),
            Self::NotOneLetter { from, to } => {
                write!(f, "{to} isn't one letter away from {from}.")
            }
            Self::NoTicket {
                from,
                to,
                max_connections: 0,
            } => write!(f, "There's no direct flight from {from} to {to}."),
            Self::NoTicket {
                from,
                to,
                max_connections: 1,
            } => write!(
                f,
                "There's no ticket from {from} to {to}, direct or with one connection."
            ),
            Self::NoTicket {
                from,
                to,
                max_connections,
            } => write!(
                f,
                "There's no ticket from {from} to {to} with {max_connections} connections or fewer."
            ),
        }
    }
}

impl std::error::Error for FlyError {}

/// Why a custom puzzle (`--route`) couldn't be set up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PuzzleError {
    Format(String),
    UnknownAirport(String),
    Unreachable {
        start: String,
        end: String,
    },
    /// Custom settings no puzzle fits.
    NoPuzzles(String),
}

impl fmt::Display for PuzzleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Format(s) => write!(
                f,
                "'{s}' isn't a route: give two airport codes like LAX-LOS."
            ),
            Self::UnknownAirport(c) => write!(
                f,
                "{c} isn't in the game: it has no flights to a code one letter away."
            ),
            Self::Unreachable { start, end } => {
                write!(f, "There's no way to get from {start} to {end}.")
            }
            Self::NoPuzzles(why) => {
                let mut why = why.clone();
                if let Some(first) = why.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                write!(
                    f,
                    "Can't start a custom game: {why}. Change it in the settings."
                )
            }
        }
    }
}

impl std::error::Error for PuzzleError {}

/// What a flight led to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FlyOutcome {
    /// Landed somewhere on the way; the hop's length in nautical miles.
    Flew { distance: f64 },
    /// Reached the destination by a route this many nautical miles longer than the best.
    Longer { over: f64 },
    /// Reached the destination by a shortest route.
    Won,
}

/// Current lifecycle status of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Playing,
    Won,
    GivenUp,
}

/// An airport the player can fly to next.
#[derive(Debug, Clone, PartialEq)]
pub struct FlightOption {
    pub airport: usize,
    pub code: &'static str,
    /// Hop length in nautical miles, every flight counted.
    pub distance: f64,
    /// Codes of the airports the hop connects through.
    pub via: Vec<&'static str>,
    /// Which letter (0-2) differs from the current airport's code.
    pub changed: usize,
}

/// A `flights` save file (see [`crate::save`]). Units and custom settings are kept
/// even when no game is in progress, since they're the player's preferences
/// rather than the game's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedGame {
    pub version: u32,
    pub units: String,
    /// Settings for custom games.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<SavedRules>,
    /// The unfinished game, if any.
    pub game: Option<SavedPlay>,
}

/// An unfinished `flights` game, replayed on load so a save stays right if the
/// route data changes (or says it no longer can).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedPlay {
    pub difficulty: String,
    /// The rules it's played by, kept so a custom game resumes as it began even
    /// if the custom settings have changed since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<SavedRules>,
    pub start: String,
    pub end: String,
    pub daily_date: Option<String>,
    /// Airports flown to so far, in order, after the start.
    pub route: Vec<String>,
    /// Times the destination was reached by a longer route.
    #[serde(default)]
    pub longer_arrivals: usize,
}

/// State machine for one `flights` session.
#[derive(Debug, Clone)]
pub struct FlightsGame {
    /// The embedded network for the rules' connections, or a small one in tests.
    net: &'static Network,
    start: usize,
    end: usize,
    /// Airports on the current route, `start` first. Going back pops the last one,
    /// so legs that were flown and backtracked never count.
    route: Vec<usize>,
    best_distance: f64,
    best_route: Vec<usize>,
    difficulty: Difficulty,
    /// What this game deals and shows.
    rules: Rules,
    /// The player's settings for custom games, kept for the next one.
    custom: Rules,
    units: Units,
    daily_date: Option<String>,
    longer_arrivals: usize,
    status: Status,
}

impl FlightsGame {
    /// A game from `start` to `end` (airport indices in `net`), played by
    /// `difficulty`'s rules (`custom` for a custom game).
    pub fn with_puzzle(
        net: &'static Network,
        start: usize,
        end: usize,
        difficulty: Difficulty,
        custom: Rules,
        units: Units,
        daily_date: Option<String>,
    ) -> Option<Self> {
        let (best_distance, best_route) = net.shortest(start, end)?;
        Some(Self {
            net,
            start,
            end,
            route: vec![start],
            best_distance,
            best_route,
            difficulty,
            rules: difficulty.rules(custom),
            custom,
            units,
            daily_date,
            longer_arrivals: 0,
            status: Status::Playing,
        })
    }

    /// A random puzzle at a standard `difficulty`, with default custom settings.
    pub fn new(difficulty: Difficulty, units: Units) -> Self {
        Self::random(difficulty, Rules::default(), units, None)
            .expect("the standard levels have puzzles")
    }

    /// A random puzzle at `difficulty` (by `custom` for a custom game), from `net`
    /// if given (for tests), else the embedded network for the rules' connections.
    pub fn random(
        difficulty: Difficulty,
        custom: Rules,
        units: Units,
        net: Option<&'static Network>,
    ) -> Result<Self, PuzzleError> {
        let rules = difficulty.rules(custom);
        let net = net.unwrap_or_else(|| rules.network());
        let (start, end) = rules
            .deal(net, &mut fastrand::Rng::new())
            .map_err(PuzzleError::NoPuzzles)?;
        Ok(
            Self::with_puzzle(net, start, end, difficulty, custom, units, None)
                .expect("puzzles are connected"),
        )
    }

    /// A custom puzzle from a route like "LAX-LOS" (any separator, or none), played
    /// by `difficulty`'s rules. Any connected pair is allowed, even one that
    /// wouldn't be dealt.
    pub fn new_route(
        route: &str,
        difficulty: Difficulty,
        custom: Rules,
        units: Units,
    ) -> Result<Self, PuzzleError> {
        let letters: String = route
            .chars()
            .filter(char::is_ascii_alphabetic)
            .collect::<String>()
            .to_ascii_uppercase();
        if letters.len() != 6 || route.chars().any(|c| c.is_ascii_digit()) {
            return Err(PuzzleError::Format(route.to_string()));
        }
        let (a, b) = letters.split_at(3);
        let net = difficulty.rules(custom).network();
        let find = |c: &str| {
            net.index_of(c)
                .ok_or_else(|| PuzzleError::UnknownAirport(c.to_string()))
        };
        let (start, end) = (find(a)?, find(b)?);
        if start == end {
            return Err(PuzzleError::Format(route.to_string()));
        }
        Self::with_puzzle(net, start, end, difficulty, custom, units, None).ok_or(
            PuzzleError::Unreachable {
                start: a.to_string(),
                end: b.to_string(),
            },
        )
    }

    /// Today's deterministic UTC daily puzzle (always at [`DAILY_DIFFICULTY`]).
    pub fn new_daily(custom: Rules, units: Units) -> Self {
        Self::daily_in(None, dict::current_utc_days(), custom, units)
    }

    /// The daily puzzle for a UTC day (days since 1970-01-01), from `net` if given
    /// (for tests), else the embedded network.
    pub fn daily_in(
        net: Option<&'static Network>,
        utc_day: u64,
        custom: Rules,
        units: Units,
    ) -> Self {
        let rules = DAILY_DIFFICULTY.rules(custom);
        let net = net.unwrap_or_else(|| rules.network());
        let (start, end) = net
            .daily_puzzle(utc_day, rules.hop_limit())
            .expect("the daily level has puzzles");
        Self::with_puzzle(
            net,
            start,
            end,
            DAILY_DIFFICULTY,
            custom,
            units,
            Some(dict::format_utc_date(utc_day)),
        )
        .expect("puzzles are connected")
    }

    /// The network to deal from for `rules`: this game's, when it has the same
    /// connections (which keeps a test's small network), else the embedded one.
    fn network_for(&self, rules: Rules) -> &'static Network {
        if self.net.max_connections() == rules.connections() {
            self.net
        } else {
            rules.network()
        }
    }

    /// Starts a new random game, at `new_difficulty` if given, keeping the units
    /// and custom settings. Fails (leaving this game as it was) when custom
    /// settings fit no puzzle.
    pub fn reset(&mut self, new_difficulty: Option<Difficulty>) -> Result<(), PuzzleError> {
        let difficulty = new_difficulty.unwrap_or(self.difficulty);
        let net = self.network_for(difficulty.rules(self.custom));
        *self = Self::random(difficulty, self.custom, self.units, Some(net))?;
        Ok(())
    }

    /// Starts today's UTC daily game (switching to [`DAILY_DIFFICULTY`]).
    pub fn start_daily(&mut self) {
        let net = self.network_for(DAILY_DIFFICULTY.rules(self.custom));
        *self = Self::daily_in(Some(net), dict::current_utc_days(), self.custom, self.units);
    }

    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }

    /// What this game deals and shows.
    pub fn rules(&self) -> Rules {
        self.rules
    }

    /// The player's settings for custom games.
    pub fn custom(&self) -> Rules {
        self.custom
    }

    /// Changes the settings for custom games. The game being played keeps its own
    /// rules; the next custom game uses these.
    pub fn set_custom(&mut self, custom: Rules) {
        self.custom = custom;
    }

    pub fn units(&self) -> Units {
        self.units
    }

    /// Changes the units distances are shown in. Display only, so mid-game is fine.
    pub fn set_units(&mut self, units: Units) {
        self.units = units;
    }

    pub fn daily_date(&self) -> Option<&str> {
        self.daily_date.as_deref()
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn origin(&self) -> usize {
        self.start
    }

    pub fn destination(&self) -> usize {
        self.end
    }

    pub fn start_code(&self) -> &'static str {
        self.net.code(self.start)
    }

    pub fn end_code(&self) -> &'static str {
        self.net.code(self.end)
    }

    /// Airports on the current route, start first.
    pub fn route(&self) -> &[usize] {
        &self.route
    }

    /// Where the player is now.
    pub fn current(&self) -> usize {
        *self.route.last().expect("the route always holds the start")
    }

    /// Number of hops on the current route.
    pub fn hops(&self) -> usize {
        self.route.len() - 1
    }

    /// The current route's length in nautical miles.
    pub fn total_distance(&self) -> f64 {
        let net = self.net;
        self.route
            .windows(2)
            .map(|w| net.hop(w[0], w[1]).expect("route legs are hops").distance)
            .sum()
    }

    /// The shortest route's length in nautical miles.
    pub fn best_distance(&self) -> f64 {
        self.best_distance
    }

    /// How many airports, from the start, the player's route shares with the
    /// shortest one: where it left the shortest, if it did.
    pub fn divergence(&self) -> usize {
        self.route
            .iter()
            .zip(&self.best_route)
            .take_while(|(a, b)| a == b)
            .count()
    }

    /// Where the player's route left the shortest one, in words, for giving up.
    pub fn divergence_text(&self) -> String {
        let split = self.divergence();
        let code = |i: usize| self.net.code(i);
        if self.route.len() == 1 {
            "You hadn't left the start.".to_string()
        } else if split == self.route.len() {
            "Your route so far follows it.".to_string()
        } else {
            format!(
                "You left it after {}: you flew to {}, the shortest goes to {}.",
                code(self.route[split - 1]),
                code(self.route[split]),
                code(self.best_route[split])
            )
        }
    }

    /// A shortest route (shown on giving up).
    pub fn best_route(&self) -> &[usize] {
        &self.best_route
    }

    /// Whether distances are shown now: when the rules show them, and on every
    /// level once the game is over.
    pub fn distances_visible(&self) -> bool {
        self.rules.shows_distances() || self.status != Status::Playing
    }

    /// Times the destination was reached by a longer route.
    pub fn longer_arrivals(&self) -> usize {
        self.longer_arrivals
    }

    /// Whether the player is at the destination (by a longer route, while playing).
    pub fn at_destination(&self) -> bool {
        self.current() == self.end
    }

    /// Where the player can fly next: airports one letter away with a ticket, not
    /// already on the route, the destination first and the rest by code. None at
    /// the destination, where the only move is back.
    pub fn options(&self) -> Vec<FlightOption> {
        if self.status != Status::Playing || self.at_destination() {
            return Vec::new();
        }
        let net = self.net;
        let here = net.code(self.current());
        let mut options: Vec<FlightOption> = net
            .hops(self.current())
            .iter()
            .filter(|h| !self.route.contains(&h.to))
            .map(|h| {
                let code = net.code(h.to);
                FlightOption {
                    airport: h.to,
                    code,
                    distance: h.distance,
                    via: h.via.iter().map(|&i| net.code(i)).collect(),
                    changed: here
                        .bytes()
                        .zip(code.bytes())
                        .position(|(a, b)| a != b)
                        .expect("neighbours are one letter apart"),
                }
            })
            .collect();
        // The destination first, when it's one hop away.
        options.sort_by_key(|o| o.airport != self.end);
        options
    }

    /// Flies to the airport typed. The destination ends the game when the route is
    /// as short as the best one; otherwise the player is told how much longer it was
    /// and plays on, going back to find a shorter way.
    pub fn fly(&mut self, raw: &str) -> Result<FlyOutcome, FlyError> {
        if self.status != Status::Playing {
            return Err(FlyError::GameOver);
        }
        let code = sanitize_input(raw).to_ascii_uppercase();
        if code.is_empty() {
            return Err(FlyError::Empty);
        }
        if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(FlyError::NotACode(code));
        }
        if self.at_destination() {
            return Err(FlyError::AtDestination);
        }
        let net = self.net;
        let from = net.code(self.current()).to_string();
        let to = net
            .index_of(&code)
            .ok_or_else(|| FlyError::UnknownAirport(code.clone()))?;
        if self.route.contains(&to) {
            return Err(FlyError::AlreadyOnRoute(code));
        }
        if letters_apart(&from, &code) != 1 {
            return Err(FlyError::NotOneLetter { from, to: code });
        }
        let distance = net
            .hop(self.current(), to)
            .ok_or(FlyError::NoTicket {
                from,
                to: code,
                max_connections: net.max_connections(),
            })?
            .distance;
        self.route.push(to);
        if to != self.end {
            return Ok(FlyOutcome::Flew { distance });
        }
        let over = self.total_distance() - self.best_distance;
        if over <= TIE_NM {
            self.status = Status::Won;
            Ok(FlyOutcome::Won)
        } else {
            self.longer_arrivals += 1;
            Ok(FlyOutcome::Longer { over })
        }
    }

    /// Goes back to the previous airport, returning the one left, or `None` at the start.
    pub fn back(&mut self) -> Option<usize> {
        if self.status != Status::Playing || self.route.len() < 2 {
            return None;
        }
        self.route.pop()
    }

    /// Ends the game; [`FlightsGame::best_route`] is then the answer to show.
    pub fn give_up(&mut self) {
        if self.status == Status::Playing {
            self.status = Status::GivenUp;
        }
    }

    /// The network being played.
    pub fn network(&self) -> &'static Network {
        self.net
    }

    /// Codes of a route joined by arrows ("LAX → LAS → LOS").
    pub fn route_text(&self, route: &[usize]) -> String {
        route
            .iter()
            .map(|&i| self.net.code(i))
            .collect::<Vec<_>>()
            .join(" → ")
    }

    /// One line about a win, for sharing: puzzle, hops, distance and longer arrivals.
    pub fn share_summary(&self) -> String {
        let hops = self.hops();
        let tries = match self.longer_arrivals {
            0 => "first try".to_string(),
            1 => "after 1 longer route".to_string(),
            n => format!("after {n} longer routes"),
        };
        let daily = self
            .daily_date
            .as_deref()
            .map(|d| format!(" (daily {d})"))
            .unwrap_or_default();
        format!(
            "Flights {} → {}{daily}: shortest route in {hops} hop{}, {}, {tries}",
            self.start_code(),
            self.end_code(),
            if hops == 1 { "" } else { "s" },
            self.units.format(self.total_distance())
        )
    }

    /// Starts a game from the command line's choices and the player's save (see
    /// [`save::start`]). Units and custom settings come from the save (units from
    /// the command line first), so they stick between games.
    pub fn start(
        difficulty: Option<Difficulty>,
        daily: bool,
        route: Option<&str>,
        units: Option<Units>,
        loaded: Loaded<SavedGame>,
    ) -> Result<(Self, Option<Notice>), PuzzleError> {
        let (saved_units, saved_custom) = match &loaded {
            Loaded::Found(saved) => (
                parse_enum(&saved.units),
                saved.custom.as_ref().and_then(SavedRules::rules),
            ),
            _ => (None, None),
        };
        let units = units.or(saved_units).unwrap_or_default();
        let custom = saved_custom.unwrap_or_default();
        let asked_for_game = difficulty.is_some() || daily || route.is_some();
        let (mut game, notice) = save::start(loaded, asked_for_game, Self::from_saved, || {
            let difficulty = difficulty.unwrap_or_default();
            if daily {
                Ok(Self::new_daily(custom, units))
            } else if let Some(route) = route {
                Self::new_route(route, difficulty, custom, units)
            } else {
                Self::random(difficulty, custom, units, None)
            }
        })?;
        game.units = units;
        game.custom = custom;
        Ok((game, notice))
    }

    /// What to save: the preferences, and the game while it's being played.
    pub fn to_saved(&self) -> SavedGame {
        let net = self.net;
        let game = (self.status == Status::Playing).then(|| SavedPlay {
            difficulty: enum_name(self.difficulty),
            rules: (self.difficulty == Difficulty::Custom).then(|| self.rules.into()),
            start: self.start_code().to_string(),
            end: self.end_code().to_string(),
            daily_date: self.daily_date.clone(),
            route: self.route[1..]
                .iter()
                .map(|&i| net.code(i).to_string())
                .collect(),
            longer_arrivals: self.longer_arrivals,
        });
        SavedGame {
            version: SAVE_VERSION,
            units: enum_name(self.units),
            custom: Some(self.custom.into()),
            game,
        }
    }

    /// Rebuilds a saved unfinished game by replaying its flights (`None` when the
    /// save has no game in progress).
    pub fn from_saved(saved: &SavedGame) -> Result<Option<Self>, String> {
        let Some(play) = &saved.game else {
            return Ok(None);
        };
        let difficulty: Difficulty = parse_enum(&play.difficulty)
            .ok_or_else(|| format!("unknown difficulty '{}'", play.difficulty))?;
        let units = parse_enum(&saved.units).unwrap_or_default();
        let custom = saved
            .custom
            .as_ref()
            .and_then(SavedRules::rules)
            .unwrap_or_default();
        // A custom game plays by the rules it began with.
        let game_rules = match &play.rules {
            Some(r) if difficulty == Difficulty::Custom => {
                r.rules().ok_or("its custom settings can't be read")?
            }
            _ => custom,
        };
        let net = difficulty.rules(game_rules).network();
        let find = |c: &str| {
            net.index_of(c)
                .ok_or_else(|| format!("{c} is no longer in the flight data"))
        };
        let mut game = Self::with_puzzle(
            net,
            find(&play.start)?,
            find(&play.end)?,
            difficulty,
            game_rules,
            units,
            play.daily_date.clone(),
        )
        .ok_or_else(|| format!("{} and {} are no longer connected", play.start, play.end))?;
        for code in &play.route {
            game.fly(code)
                .map_err(|e| e.to_string().trim_end_matches('.').to_string())?;
        }
        if game.status != Status::Playing {
            return Err("its flights already finish it".to_string());
        }
        game.longer_arrivals = play.longer_arrivals;
        game.custom = custom;
        Ok(Some(game))
    }
}

impl Saveable for FlightsGame {
    fn daily_date(&self) -> Option<&str> {
        self.daily_date.as_deref()
    }
}
