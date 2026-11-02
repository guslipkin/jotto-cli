use std::cell::Cell as StdCell;
use std::io;
use std::ops::ControlFlow;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Widget, Wrap,
};
use ratatui::Frame;

use crate::flights::coast::{self, clip, turns, wrap_lon, Bounds, Coast};
use crate::flights::data::{Ticket, MAX_CONNECTIONS};
use crate::flights::game::SavedGame;
use crate::flights::game::{
    code_str, parse_code, Code, Difficulty, FlightOption, FlightsGame, FlyOutcome, Rules, Status,
    Units, View, MAX_HOPS, MIN_HOPS,
};
use crate::game::supports_color;
use crate::help::{draw_help_page, HelpLine, HelpPage, HelpSection};
use crate::save::Autosave;
use crate::text::grouped;
use crate::tui::{
    black_on, bold, centered_rect, draw_on_black, draw_too_narrow, header_spans, notice_color,
    rect_contains, run_screen, screen_areas, step_selection, wrapped_line_count, Footer,
    GameScreen, COLOR_ACCENT, COLOR_POSITIVE, COLOR_WARNING,
};

/// The destination's colour: Okabe-Ito reddish purple, apart from the palette's
/// green (where you are), blue (your route) and orange (your options).
const COLOR_DESTINATION: Color = Color::Rgb(204, 121, 167); // #CC79A7

/// Background of the selected option: a dark gray rather than reverse video, so
/// the changed letter keeps its colour, and fixed rather than the terminal's
/// "dark gray", which is often light enough to hide gray text on it.
const HIGHLIGHT_BG: Color = Color::Rgb(52, 52, 52);

/// Longest city and country shown beside an airport; longer ones end in "…". Most
/// fit (the median is 12 characters, 95% are 22 or fewer), and the side column is
/// sized to it, so the cap is what leaves the map its room.
const PLACE_WIDTH: usize = 20;
/// Narrowest the side column gets, so the status line has room to read.
const MIN_SIDE_WIDTH: u16 = 34;

/// An airport's place cut to [`PLACE_WIDTH`], for the options list's column.
fn short_place(text: &str) -> String {
    fit_place(text, PLACE_WIDTH)
}

/// A place cut to `width` characters. Only the city is shortened, ending in
/// "…"; the ", CC" country code always stays whole.
fn fit_place(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let (city, country) = match text.rsplit_once(", ") {
        Some((city, cc)) if cc.chars().count() <= 3 => (city, format!(", {cc}")),
        _ => (text, String::new()),
    };
    let room = width.saturating_sub(country.chars().count() + 1).max(1);
    let cut: String = city.chars().take(room).collect();
    format!("{}…{country}", cut.trim_end())
}

/// Remarks under a stop in the route box after giving up: where the player's
/// route left the shortest, where the shortest went instead, or (when the
/// player hadn't yet strayed) its next hop. Kept short, since the column is
/// sized to hold the longest beside a place.
const YOU_LEFT: &str = "↑ you left here";
const WENT_HERE: &str = "↑ went here";
const NEXT_HOP: &str = "↑ next hop";
/// Indent of a stop's second line, under its code.
const INDENT: &str = "      ";

/// Width of the column beside the map: the longest line it can ever show for
/// these rules, worked out once so the column never changes width (not even
/// when the game ends and every level reveals distances), and no more, so the
/// map gets the rest. Route stops take two lines (see `route_lines`): the code,
/// distance and connections, then a remark and the place.
fn side_width(rules: Rules) -> u16 {
    let count = |s: &str| s.chars().count();
    // "  9,999 nm": in the options only where the level shows it, but in the
    // route box always, as it's revealed once the game is over.
    const DISTANCE: usize = 11;
    // " via XXX" or " via XXX, YYY"
    let via = match (rules.shows_connections(), rules.connections()) {
        (false, _) | (true, 0) => 0,
        (true, 1) => 8,
        (true, _) => 13,
    };
    let place = if rules.show_places { PLACE_WIDTH } else { 0 };
    let option_distance = if rules.shows_distances() { DISTANCE } else { 0 };
    // "> " + "LAS★" + columns.
    let option = 2 + 5 + option_distance + via + if place > 0 { place + 2 } else { 0 };
    // "12. LAS" + distance + via, then the indent, remark and " · " place.
    let first = 7 + DISTANCE + via;
    let remark = [YOU_LEFT, WENT_HERE, NEXT_HOP]
        .map(count)
        .into_iter()
        .max()
        .unwrap_or(0);
    let second = count(INDENT) + remark + if place > 0 { 3 + place } else { 0 };
    (option.max(first).max(second) as u16 + 2).max(MIN_SIDE_WIDTH)
}

/// Map padding around the airports shown, as a fraction of their spread.
const MAP_PADDING: f64 = 0.2;
/// Smallest span of latitude the map shows, so a short hop isn't zoomed to a blur.
const MIN_LAT_SPAN: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FooterAction {
    Help,
    NewGame,
    Daily,
    GiveUp,
    Difficulty,
    Settings,
    Quit,
}

const FOOTER_COUNT: usize = 7;

const FOOTER_ACTIONS: [(FooterAction, &str, &str); FOOTER_COUNT] = [
    (FooterAction::Help, "[?]", "Help"),
    (FooterAction::NewGame, "[^N]", "New Game"),
    (FooterAction::Daily, "[^T]", "Daily"),
    (FooterAction::GiveUp, "[^G]", "Give Up"),
    (FooterAction::Difficulty, "[^D]", "Difficulty"),
    (FooterAction::Settings, "[^S]", "Settings"),
    (FooterAction::Quit, "[Esc]", "Quit"),
];

struct FlightsApp {
    game: FlightsGame,
    use_color: bool,
    footer: Footer<FooterAction>,
    /// Inside of the options list, for mapping clicks to rows.
    options_area: Rect,
    message: (String, Color),
    show_help: bool,
    /// The settings page, while it's open.
    settings: Option<SettingsPage>,
    should_quit: bool,
    options_state: TableState,
    /// The map's view once the player has panned or zoomed it; `None` fits it to
    /// the route and options each frame. Kept until a new game or [0].
    view: Option<Bounds>,
    /// Where the map was last drawn and what it showed, for turning the mouse's
    /// position into a place on it (set while drawing, hence a `Cell`).
    map_frame: StdCell<Option<(Rect, Bounds)>>,
    /// Where a drag on the map began, and the view then.
    drag: Option<(u16, u16, Bounds)>,
    /// How far the route box is scrolled once the player has scrolled it; `None`
    /// shows its usual part (the end while playing, both routes from the top
    /// after giving up). Dropped whenever the route changes.
    route_scroll: Option<u16>,
    /// Where the route box was last drawn, how far it was scrolled and how far
    /// it could be (set while drawing, hence a `Cell`).
    route_frame: StdCell<(Rect, u16, u16)>,
    /// Once the game is over, the hop picked in the route box with Up / Down
    /// and shown on the map: counted through the player's route, then after
    /// giving up on through the shortest. `None` shows the routes whole.
    route_step: Option<usize>,
}

/// How much one zoom key press narrows the view.
const ZOOM_STEP: f64 = 1.5;
/// How much one mouse-wheel event does: far less, since a wheel sends several
/// events a notch, and a trackpad a stream of them.
const WHEEL_STEP: f64 = 1.1;
/// How far one pan key moves the view, as a fraction of its width or height.
const PAN_STEP: f64 = 0.2;
/// Narrowest and widest the view can be zoomed, in degrees of latitude.
const MIN_VIEW_LAT: f64 = 0.5;
const MAX_VIEW_LAT: f64 = 200.0;

/// Inside width of the settings page: a row's marker, name and value (35), its
/// longest note ("airports may repeat as connections", 34) and a space before
/// the border, so no row wraps and the page keeps one width whatever's chosen.
const SETTINGS_WIDTH: u16 = 70;

/// How many of [`SETTINGS`]' rows, from the top, are the player's own
/// preferences rather than the custom game's.
const PREFERENCE_ROWS: usize = 2;

/// A row of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    Units,
    /// The map's hops: straight, or great circle curves.
    Lines,
    /// A preset: sets the four parts of the view below it as a level does.
    View,
    Map,
    Distances,
    Via,
    Over,
    Places,
    Mode,
    Connections,
    Reuse,
    MinHops,
    MaxHops,
    Start,
    End,
    /// Not a setting: puts the custom ones back to their defaults.
    Reset,
}

const SETTINGS: [Setting; 16] = [
    Setting::Units,
    Setting::Lines,
    Setting::View,
    Setting::Map,
    Setting::Distances,
    Setting::Via,
    Setting::Over,
    Setting::Places,
    Setting::Mode,
    Setting::Connections,
    Setting::Reuse,
    Setting::MinHops,
    Setting::MaxHops,
    Setting::Start,
    Setting::End,
    Setting::Reset,
];

/// The settings page: a copy of the preferences and custom settings being
/// edited, kept whenever the page closes, and the row selected.
#[derive(Debug, Clone)]
struct SettingsPage {
    row: usize,
    units: Units,
    curves: bool,
    custom: Rules,
    /// The start and end codes as typed so far (empty: random). Kept apart from
    /// `custom` until they're whole codes.
    start_text: String,
    end_text: String,
    /// Why the last attempt to play couldn't start.
    problem: Option<String>,
    /// The custom settings' puzzle count, or why they have none, worked out
    /// when they change rather than on every redraw, since each look can mean
    /// several searches of the whole network.
    status: Result<usize, String>,
    /// Where the reset row was last drawn, for clicks (set while drawing).
    reset_area: StdCell<Rect>,
}

impl SettingsPage {
    fn new(units: Units, curves: bool, custom: Rules) -> Self {
        let text = |c: &Option<Code>| {
            c.as_ref()
                .map_or(String::new(), |c| code_str(c).to_string())
        };
        Self {
            row: 0,
            units,
            curves,
            start_text: text(&custom.start),
            end_text: text(&custom.end),
            status: custom.check(),
            custom,
            problem: None,
            reset_area: StdCell::new(Rect::default()),
        }
    }

    /// Puts the custom settings back to their defaults (the preferences above
    /// them aren't any).
    fn reset(&mut self) {
        let row = self.row;
        *self = Self::new(self.units, self.curves, Rules::default());
        self.row = row;
    }

    /// Whether a row applies in the custom game's mode: random puzzles use the
    /// limits, a fixed route its start and end.
    fn enabled(&self, setting: Setting) -> bool {
        match setting {
            Setting::Connections | Setting::MinHops | Setting::MaxHops => !self.custom.fixed_route,
            Setting::Start | Setting::End => self.custom.fixed_route,
            // Without connections there's nothing to reuse.
            Setting::Reuse => self.custom.connections() > 0,
            _ => true,
        }
    }

    /// Moves the selection up or down, over rows the mode doesn't use.
    fn step_row(&mut self, down: bool) {
        let mut row = self.row;
        loop {
            row = match (down, row) {
                (true, r) if r + 1 < SETTINGS.len() => r + 1,
                (false, r) if r > 0 => r - 1,
                _ => return,
            };
            if self.enabled(SETTINGS[row]) {
                self.row = row;
                return;
            }
        }
    }

    /// Whether the selected row takes a typed airport code.
    fn is_code_row(&self) -> bool {
        matches!(SETTINGS[self.row], Setting::Start | Setting::End)
    }

    fn code_text(&mut self) -> &mut String {
        if SETTINGS[self.row] == Setting::Start {
            &mut self.start_text
        } else {
            &mut self.end_text
        }
    }

    /// Updates the custom start and end from what's typed: a whole code, else random.
    fn sync_codes(&mut self) {
        self.custom.start = parse_code(&self.start_text);
        self.custom.end = parse_code(&self.end_text);
        self.problem = None;
        self.status = self.custom.check();
    }

    /// Types a letter into the start or end code (a fourth starts it over).
    fn type_char(&mut self, c: char) {
        let text = self.code_text();
        if text.len() >= 3 {
            text.clear();
        }
        text.push(c.to_ascii_uppercase());
        self.sync_codes();
    }

    fn backspace(&mut self) {
        self.code_text().pop();
        self.sync_codes();
    }

    /// Changes the selected setting one step forward (or back).
    fn change(&mut self, forward: bool) {
        let step = |i: usize, n: usize| {
            if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            }
        };
        let c = &mut self.custom;
        match SETTINGS[self.row] {
            Setting::Units => {
                let all = [Units::Nm, Units::Mi, Units::Km];
                let i = all.iter().position(|&u| u == self.units).unwrap_or(0);
                self.units = all[step(i, all.len())];
            }
            Setting::Lines => self.curves = !self.curves,
            Setting::View => {
                let all = Difficulty::STANDARD;
                // A mixed view steps to the first level, or back to the last.
                let next = match c
                    .view
                    .level()
                    .and_then(|l| all.iter().position(|&d| d == l))
                {
                    Some(i) => step(i, all.len()),
                    None if forward => 0,
                    None => all.len() - 1,
                };
                c.view = View::of(all[next]);
            }
            Setting::Map => c.view.map = !c.view.map,
            Setting::Distances => c.view.distances = !c.view.distances,
            Setting::Via => c.view.connections = !c.view.connections,
            Setting::Over => c.view.amount_over = !c.view.amount_over,
            Setting::Connections => {
                c.max_connections = step(c.max_connections, MAX_CONNECTIONS + 1);
            }
            // Each limit steps only as far as the other allows, wrapping round,
            // so the least is never more than the most.
            Setting::MinHops => {
                let top = c.max_hops.unwrap_or(MAX_HOPS);
                let choices: Vec<usize> = (MIN_HOPS..=top).collect();
                let i = choices.iter().position(|&h| h == c.min_hops).unwrap_or(0);
                c.min_hops = choices[step(i, choices.len())];
            }
            Setting::MaxHops => {
                // The least up to MAX_HOPS, then no limit.
                let choices: Vec<Option<usize>> =
                    (c.min_hops..=MAX_HOPS).map(Some).chain([None]).collect();
                let i = choices.iter().position(|&h| h == c.max_hops).unwrap_or(0);
                c.max_hops = choices[step(i, choices.len())];
            }
            Setting::Places => c.show_places = !c.show_places,
            Setting::Mode => c.fixed_route = !c.fixed_route,
            Setting::Reuse => c.reuse_connections = !c.reuse_connections,
            Setting::Reset => return self.reset(),
            // Left and Right go back to a random airport.
            Setting::Start | Setting::End => {
                self.code_text().clear();
                self.sync_codes();
            }
        }
        self.problem = None;
        self.status = self.custom.check();
    }
}

impl FlightsApp {
    fn new(game: FlightsGame) -> Self {
        let message = match game.daily_date() {
            Some(date) => (
                format!("Daily Challenge ({date} UTC) started!"),
                COLOR_ACCENT,
            ),
            None => (welcome(&game), Color::Gray),
        };
        let mut app = Self {
            game,
            use_color: supports_color(),
            footer: Footer::new(&FOOTER_ACTIONS),
            options_area: Rect::default(),
            message,
            show_help: false,
            settings: None,
            view: None,
            map_frame: StdCell::new(None),
            drag: None,
            route_scroll: None,
            route_frame: StdCell::new((Rect::default(), 0, 0)),
            route_step: None,
            should_quit: false,
            options_state: TableState::default(),
        };
        app.reset_selection();
        app
    }

    fn reset_selection(&mut self) {
        // Called on every change to the route but going back (which keeps the
        // selection, and drops the scroll itself), so the box shows its new end.
        self.route_scroll = None;
        self.route_step = None;
        let any = !self.game.options().is_empty();
        self.options_state = TableState::default().with_selected(any.then_some(0));
    }

    /// Flies to `code`, typed or picked from the list.
    fn fly(&mut self, code: &str) {
        let code = code.to_ascii_uppercase();
        match self.game.fly(&code) {
            Ok(FlyOutcome::Flew { distance }) => {
                let mut details = Vec::new();
                if self.game.rules().shows_distances() {
                    details.push(self.game.units().format(distance));
                }
                let via = self.arrival_via();
                if !via.is_empty() {
                    details.push(via.trim_start().to_string());
                }
                let details = if details.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", details.join(" "))
                };
                self.message = (format!("Flew to {code}{details}."), Color::Gray);
            }
            Ok(FlyOutcome::Longer { over }) => {
                let how = if self.game.rules().shows_amount_over() {
                    format!(
                        "{} longer than the shortest route",
                        self.game.units().format(over)
                    )
                } else {
                    "not by the shortest route".to_string()
                };
                self.message = (
                    format!("You reached {code}, but {how}.\nGo back [←] to find a shorter way."),
                    COLOR_WARNING,
                );
            }
            Ok(FlyOutcome::Won) => {
                self.message = (
                    format!(
                        "You found the shortest route!\n{}\n[^N] New Game",
                        self.game.share_summary()
                    ),
                    COLOR_POSITIVE,
                );
            }
            Err(err) => self.message = (err.to_string(), COLOR_WARNING),
        }
        self.reset_selection();
    }

    /// " via ZZZ" for the hop that arrived at the end of the route, when shown.
    fn arrival_via(&self) -> String {
        match self.game.legs().last() {
            Some(leg) if self.game.rules().shows_connections() => {
                via_suffix(self.game.network(), &leg.via)
            }
            _ => String::new(),
        }
    }

    fn submit(&mut self) {
        if self.game.status() != Status::Playing {
            return;
        }
        let options = self.game.options();
        if let Some(opt) = self.options_state.selected().and_then(|i| options.get(i)) {
            self.fly(opt.code);
        } else if self.game.at_destination() {
            self.message = (
                "Go back [←] to find a shorter way.".to_string(),
                COLOR_WARNING,
            );
        }
    }

    fn go_back(&mut self) {
        if self.game.status() != Status::Playing {
            return;
        }
        let net = self.game.network();
        match self.game.back() {
            Some(left) => {
                self.route_scroll = None;
                self.message = (
                    format!(
                        "Back at {} (left {}).",
                        net.code(self.game.current()),
                        net.code(left)
                    ),
                    Color::Gray,
                );
                // The airport just left is the one that was chosen from here, so
                // it's chosen again rather than the list starting over at the top.
                let options = self.game.options();
                let i = options.iter().position(|o| o.airport == left);
                self.options_state =
                    TableState::default().with_selected(i.or((!options.is_empty()).then_some(0)));
            }
            None => {
                self.message = (
                    "You're at the start; there's nowhere to go back to.".to_string(),
                    COLOR_WARNING,
                )
            }
        }
    }

    fn start_new_game(&mut self, difficulty: Option<Difficulty>) {
        if let Err(err) = self.game.reset(difficulty) {
            self.message = (err.to_string(), COLOR_WARNING);
            return;
        }
        let d = self.game.difficulty();
        let about = if d == Difficulty::Custom {
            self.game.rules().summary()
        } else {
            d.description().to_string()
        };
        self.after_restart(format!("New game started!\nDifficulty: {d} ({about})."));
    }

    fn open_settings(&mut self) {
        self.settings = Some(SettingsPage::new(
            self.game.units(),
            self.game.curves(),
            self.game.custom(),
        ));
    }

    /// Keeps the settings page's choices and closes it; with `play`, starts a
    /// custom game by them too (leaving the page open to say why if none fits).
    fn close_settings(&mut self, play: bool) {
        let Some(page) = self.settings.take() else {
            return;
        };
        self.game.set_units(page.units);
        self.game.set_curves(page.curves);
        self.game.set_custom(page.custom);
        if !play {
            self.message = ("Settings saved.".to_string(), COLOR_ACCENT);
            return;
        }
        if let Err(why) = &page.status {
            self.settings = Some(SettingsPage {
                problem: Some(format!("Can't play: {why}.")),
                ..page
            });
            return;
        }
        self.start_new_game(Some(Difficulty::Custom));
    }

    /// Moves to the next difficulty, past Custom when its settings can't be
    /// played (so the cycle never sticks there, and says why it skipped it).
    fn cycle_difficulty(&mut self) {
        let next = self.game.difficulty().next();
        self.start_new_game(Some(next));
        if self.game.difficulty() != next {
            let skipped = self.message.0.clone();
            self.start_new_game(Some(next.next()));
            self.message.0 = format!("{}\n(Skipped Custom: {skipped})", self.message.0);
        }
    }

    fn start_daily_game(&mut self) {
        self.game.start_daily();
        let date = self.game.daily_date().unwrap_or("today").to_string();
        self.after_restart(format!(
            "Daily Challenge ({date} UTC) started!\nDifficulty: {}.",
            self.game.difficulty()
        ));
    }

    fn cycle_units(&mut self) {
        let next = self.game.units().next();
        self.game.set_units(next);
        self.message = (format!("Distances now in {next}."), COLOR_ACCENT);
    }

    fn after_restart(&mut self, message: String) {
        self.reset_selection();
        // A new game is somewhere new, so the map fits it again.
        self.view = None;
        self.drag = None;
        self.message = (message, COLOR_ACCENT);
    }

    fn trigger_give_up(&mut self) {
        if self.game.status() != Status::Playing {
            return;
        }
        self.game.give_up();
        self.reset_selection();
        self.message = (
            format!(
                "A shortest route: {} ({})\n{}\n[^N] New Game",
                self.game.route_text(self.game.best_route()),
                self.game.units().format(self.game.best_distance()),
                self.game.divergence_text()
            ),
            COLOR_WARNING,
        );
    }

    /// Moves the selection: through the options while playing, and once the
    /// game is over (when there are none) through the route's hops.
    fn move_selection(&mut self, down: bool) {
        if self.game.status() == Status::Playing {
            step_selection(&mut self.options_state, self.game.options().len(), down);
            return;
        }
        let Some(last) = self.step_count().checked_sub(1) else {
            return;
        };
        self.route_step = Some(match self.route_step {
            None if down => 0,
            None => last,
            Some(i) if down => (i + 1).min(last),
            Some(i) => i.saturating_sub(1),
        });
        // Scrolled to the picked hop rather than where the player left it.
        self.route_scroll = None;
    }

    /// How many hops the finished game's route box steps through.
    fn step_count(&self) -> usize {
        let best = if self.game.status() == Status::GivenUp {
            self.game.best_legs().len()
        } else {
            0
        };
        self.game.legs().len() + best
    }

    /// The hop picked in the finished game's route box: where it's from, its
    /// ticket and where it's to.
    fn selected_hop(&self) -> Option<(usize, &Ticket, usize)> {
        let step = self.route_step?;
        let game = &self.game;
        let yours = game.legs().len();
        let (route, legs, n) = if step < yours {
            (game.route(), game.legs(), step)
        } else {
            (game.best_route(), game.best_legs(), step - yours)
        };
        Some((route[n], legs.get(n)?, *route.get(n + 1)?))
    }

    fn execute_footer_action(&mut self, action: FooterAction) {
        match action {
            FooterAction::Help => self.show_help = !self.show_help,
            FooterAction::NewGame => self.start_new_game(None),
            FooterAction::Daily => self.start_daily_game(),
            FooterAction::GiveUp => self.trigger_give_up(),
            FooterAction::Difficulty => self.cycle_difficulty(),
            FooterAction::Settings => self.open_settings(),
            FooterAction::Quit => self.should_quit = true,
        }
    }

    /// The view the map shows: the player's, or the fitted one last drawn.
    fn current_view(&self) -> Option<(Rect, Bounds)> {
        let (inner, fitted) = self.map_frame.get()?;
        Some((inner, self.view.unwrap_or(fitted)))
    }

    /// Whether (`col`, `row`) is on the map.
    fn on_route(&self, col: u16, row: u16) -> bool {
        rect_contains(self.route_frame.get().0, col, row)
    }

    /// Scrolls the route box `lines` down (or up, negative), within its text.
    fn scroll_route(&mut self, lines: i32) {
        let (_, now, max) = self.route_frame.get();
        let to = (i32::from(now) + lines).clamp(0, i32::from(max));
        self.route_scroll = Some(to as u16);
        self.route_frame
            .set((self.route_frame.get().0, to as u16, max));
    }

    /// A page of the route box: its height less a line, so a line stays in view.
    fn route_page(&self) -> i32 {
        i32::from(self.route_frame.get().0.height.saturating_sub(3).max(1))
    }

    fn on_map(&self, col: u16, row: u16) -> bool {
        self.game.rules().shows_map()
            && self
                .map_frame
                .get()
                .is_some_and(|(inner, _)| rect_contains(inner, col, row))
    }

    /// The map position under terminal cell (`col`, `row`).
    fn map_point(&self, col: u16, row: u16) -> Option<(f64, f64)> {
        let (inner, (x0, x1, y0, y1)) = self.current_view()?;
        let fx = f64::from(col.saturating_sub(inner.x)) / f64::from(inner.width.max(2) - 1);
        let fy = f64::from(row.saturating_sub(inner.y)) / f64::from(inner.height.max(2) - 1);
        Some((x0 + fx * (x1 - x0), y1 - fy * (y1 - y0)))
    }

    /// Zooms the map by `factor` (under 1 zooms in), keeping the point under
    /// (`col`, `row`) in place, or the middle of the view.
    fn zoom(&mut self, factor: f64, at: Option<(u16, u16)>) {
        let Some((_, (x0, x1, y0, y1))) = self.current_view() else {
            return;
        };
        // Out no further than the cap, or the fitted view if that's wider already.
        let widest = self
            .map_frame
            .get()
            .map_or(MAX_VIEW_LAT, |(_, f)| MAX_VIEW_LAT.max(f.3 - f.2));
        let lat_span = (y1 - y0) * factor;
        if lat_span < MIN_VIEW_LAT || lat_span > widest {
            return;
        }
        let (cx, cy) = at
            .and_then(|(c, r)| self.map_point(c, r))
            .unwrap_or(((x0 + x1) / 2.0, (y0 + y1) / 2.0));
        self.view = Some((
            cx - (cx - x0) * factor,
            cx + (x1 - cx) * factor,
            cy - (cy - y0) * factor,
            cy + (y1 - cy) * factor,
        ));
    }

    /// Moves the view by fractions of its width (`dx`, east) and height (`dy`, north).
    fn pan(&mut self, dx: f64, dy: f64) {
        let Some((_, (x0, x1, y0, y1))) = self.current_view() else {
            return;
        };
        let (sx, sy) = ((x1 - x0) * dx, (y1 - y0) * dy);
        self.view = Some((x0 + sx, x1 + sx, y0 + sy, y1 + sy));
    }

    /// Drags the map so the point grabbed at the drag's start follows the mouse.
    fn drag_to(&mut self, col: u16, row: u16) {
        let (Some((start_col, start_row, (x0, x1, y0, y1))), Some((inner, _))) =
            (self.drag, self.map_frame.get())
        else {
            return;
        };
        let per_col = (x1 - x0) / f64::from(inner.width.max(2) - 1);
        let per_row = (y1 - y0) / f64::from(inner.height.max(2) - 1);
        let dx = (f64::from(col) - f64::from(start_col)) * per_col;
        let dy = (f64::from(row) - f64::from(start_row)) * per_row;
        self.view = Some((x0 - dx, x1 - dx, y0 + dy, y1 + dy));
    }

    /// Goes back to fitting the map to the route and options.
    fn fit_map(&mut self) {
        self.view = None;
        self.drag = None;
    }

    fn handle_mouse_click(&mut self, col: u16, row: u16) {
        if let Some(page) = &mut self.settings {
            // The reset row is a button; anywhere else closes the page.
            if rect_contains(page.reset_area.get(), col, row) {
                page.row = SETTINGS.len() - 1;
                page.reset();
            } else {
                self.close_settings(false);
            }
            return;
        }
        if let Some(action) = self.footer.hit(col, row) {
            self.execute_footer_action(action);
            return;
        }
        if rect_contains(self.options_area, col, row) {
            let i = self.options_state.offset() + (row - self.options_area.y) as usize;
            if let Some(opt) = self.game.options().get(i) {
                self.fly(opt.code);
            }
        }
    }
}

fn welcome(game: &FlightsGame) -> String {
    format!(
        "Fly from {} to {} by the shortest route, changing one letter per hop.",
        game.start_code(),
        game.end_code()
    )
}

/// " via ZZZ, YYY", or nothing for a direct flight.
fn via_suffix(net: &crate::flights::data::Network, via: &[usize]) -> String {
    if via.is_empty() {
        String::new()
    } else {
        let codes: Vec<&str> = via.iter().map(|&i| net.code(i)).collect();
        format!(" via {}", codes.join(", "))
    }
}

/// A code with its `changed` letter (if any) highlighted.
fn code_spans(code: &str, changed: Option<usize>, base: Style) -> Vec<Span<'static>> {
    code.chars()
        .enumerate()
        .map(|(i, c)| {
            let style = if Some(i) == changed {
                base.fg(COLOR_WARNING).add_modifier(Modifier::BOLD)
            } else {
                base
            };
            Span::styled(c.to_string(), style)
        })
        .collect()
}

/// Runs the full-screen interactive TUI.
pub fn run(game: FlightsGame, autosave: &mut Autosave) -> io::Result<()> {
    let mut app = FlightsApp::new(game);
    if let Some(notice) = autosave.notice.take() {
        app.message = (notice.text().to_string(), notice_color(&notice));
    }
    run_screen(&mut app, autosave)
}

impl GameScreen for FlightsApp {
    type Saved = SavedGame;

    fn draw(&mut self, f: &mut Frame) {
        draw_ui(f, self);
    }

    fn saved(&self) -> Self::Saved {
        self.game.to_saved()
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    fn show_help(&mut self) -> &mut bool {
        &mut self.show_help
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        {
            let (col, row) = (mouse.column, mouse.row);
            let free = self.settings.is_none();
            // On the map, the wheel zooms and a drag pans; over the route it
            // scrolls that; elsewhere it moves the selection and a click acts.
            let on_map = free && self.on_map(col, row);
            let on_route = free && self.on_route(col, row);
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) if on_map => {
                    self.drag = self.current_view().map(|(_, view)| (col, row, view));
                }
                MouseEventKind::Down(MouseButton::Left) => self.handle_mouse_click(col, row),
                MouseEventKind::Drag(MouseButton::Left) => self.drag_to(col, row),
                MouseEventKind::Up(MouseButton::Left) => self.drag = None,
                MouseEventKind::ScrollUp if on_map => self.zoom(1.0 / WHEEL_STEP, Some((col, row))),
                MouseEventKind::ScrollDown if on_map => self.zoom(WHEEL_STEP, Some((col, row))),
                MouseEventKind::ScrollUp if on_route => self.scroll_route(-1),
                MouseEventKind::ScrollDown if on_route => self.scroll_route(1),
                MouseEventKind::ScrollUp if free => self.move_selection(false),
                MouseEventKind::ScrollDown if free => self.move_selection(true),
                _ => {}
            }
        }
    }

    fn on_key(&mut self, key: KeyEvent, ctrl: bool) -> ControlFlow<()> {
        if let Some(page) = &mut self.settings {
            let typing = page.is_code_row();
            match key.code {
                KeyCode::Esc => self.close_settings(false),
                KeyCode::Char('s') if ctrl => self.close_settings(false),
                KeyCode::Enter if SETTINGS[page.row] == Setting::Reset => page.reset(),
                KeyCode::Enter => self.close_settings(true),
                KeyCode::Up => page.step_row(false),
                KeyCode::Down => page.step_row(true),
                // The start and end rows take a typed code.
                KeyCode::Char(c) if typing && c.is_ascii_alphabetic() => page.type_char(c),
                KeyCode::Backspace if typing => page.backspace(),
                KeyCode::Char('q') if !typing => self.close_settings(false),
                KeyCode::Left => page.change(false),
                KeyCode::Right | KeyCode::Char(' ') => page.change(true),
                _ => {}
            }
            return ControlFlow::Continue(());
        }
        match key.code {
            KeyCode::Esc => return ControlFlow::Break(()),
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('n') if ctrl => self.start_new_game(None),
            KeyCode::Char('d') if ctrl => self.cycle_difficulty(),
            KeyCode::Char('t') if ctrl => self.start_daily_game(),
            KeyCode::Char('g') if ctrl => self.trigger_give_up(),
            KeyCode::Char('u') if ctrl => self.cycle_units(),
            KeyCode::Char('s') if ctrl => self.open_settings(),
            KeyCode::Up => self.move_selection(false),
            KeyCode::Down => self.move_selection(true),
            KeyCode::PageUp => self.scroll_route(-self.route_page()),
            KeyCode::PageDown => self.scroll_route(self.route_page()),
            // Left and Right step back and forward along the route: back a
            // hop, or fly to the selected airport. Backspace and Enter do
            // the same, for anyone used to them.
            KeyCode::Left | KeyCode::Backspace => self.go_back(),
            KeyCode::Right | KeyCode::Enter => self.submit(),
            // The map: + and - zoom, WASD pan, 0 fits it again.
            KeyCode::Char(c) if !ctrl && self.game.rules().shows_map() => {
                match c.to_ascii_lowercase() {
                    '+' | '=' => self.zoom(1.0 / ZOOM_STEP, None),
                    '-' | '_' => self.zoom(ZOOM_STEP, None),
                    'w' => self.pan(0.0, PAN_STEP),
                    's' => self.pan(0.0, -PAN_STEP),
                    'a' => self.pan(-PAN_STEP, 0.0),
                    'd' => self.pan(PAN_STEP, 0.0),
                    '0' => self.fit_map(),
                    _ => {}
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// Draws a frame, on true black when colours are on (see
/// [`crate::tui::paint_background`]).
fn draw_ui(f: &mut Frame, app: &mut FlightsApp) {
    draw_on_black(f, app.use_color, |f| draw_screen(f, app));
}

fn draw_screen(f: &mut Frame, app: &mut FlightsApp) {
    let title = " FLIGHTS ";

    if draw_too_narrow(f, title, "flights") {
        app.footer.clear();
        app.options_area = Rect::default();
        return;
    }

    // The footer's buttons take as many rows as their labels need; a narrow
    // screen stacks everything top to bottom.
    let (chunks, narrow) = screen_areas(f.area(), &app.footer, 12);

    draw_header(f, app, title, chunks[0]);

    let over = app.game.status() != Status::Playing;
    // Once the game is over there are no options, so the routes get the room.
    let options_height = if over {
        Constraint::Length(3)
    } else {
        Constraint::Min(4)
    };
    let status_area = if narrow {
        // One column: the map (if the level has one) on top, then the status,
        // route and options, each the full width.
        let map = if app.game.rules().shows_map() {
            Constraint::Percentage(40)
        } else {
            Constraint::Length(0)
        };
        let col = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                map,
                status_height(app, chunks[1], 5),
                Constraint::Min(4),
                options_height,
            ])
            .split(chunks[1]);
        if app.game.rules().shows_map() {
            draw_map(f, app, col[0]);
        } else {
            app.map_frame.set(None);
        }
        draw_route(f, app, col[2]);
        draw_options(f, app, col[3]);
        col[1]
    } else if app.game.rules().shows_map() {
        // The map takes whatever the side column doesn't need, up to half.
        let side = side_width(app.game.rules()).min(chunks[1].width / 2);
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(side)])
            .split(chunks[1]);
        draw_map(f, app, body[0]);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                status_height(app, body[1], 5),
                Constraint::Min(4),
                options_height,
            ])
            .split(body[1]);
        draw_route(f, app, right[1]);
        draw_options(f, app, right[2]);
        right[0]
    } else {
        // No map: the options get the left half, as the level's only view of the network.
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([status_height(app, body[1], 6), Constraint::Min(4)])
            .split(body[1]);
        // Once it's over, the routes take the options' half.
        if over {
            draw_route(f, app, body[0]);
            draw_options(f, app, right[1]);
        } else {
            draw_options(f, app, body[0]);
            draw_route(f, app, right[1]);
        }
        right[0]
    };
    let (msg, color) = &app.message;
    crate::tui::draw_status(f, status_area, msg, *color);
    app.footer.draw(f, chunks[2]);

    if let Some(page) = &app.settings {
        draw_settings(f, page);
    }
    if app.show_help {
        draw_help(f, &app.game);
    }
}

fn draw_header(f: &mut Frame, app: &FlightsApp, title: &'static str, area: Rect) {
    let game = &app.game;
    let mut spans = header_spans(title, game.daily_date());
    let bold_white = bold(Color::White);
    spans.extend([
        Span::raw("  "),
        Span::styled(game.start_code(), bold_white),
        Span::raw(" → "),
        Span::styled(game.end_code(), bold(COLOR_DESTINATION)),
        Span::raw("  |  Difficulty: "),
        Span::styled(game.difficulty().label(), bold(COLOR_ACCENT)),
        Span::raw("  |  Hops: "),
        Span::styled(game.hops().to_string(), bold_white),
    ]);
    if game.distances_visible() {
        spans.push(Span::raw("  |  "));
        spans.push(Span::styled(
            game.units().format(game.total_distance()),
            bold_white,
        ));
    }
    crate::tui::draw_header(f, spans, area);
}

/// The lines for `route`, two to a stop: the code with its distance and
/// connections (as far as the game shows them), then beneath it, indented, a
/// remark (`note(i)` for the `i`th stop, an arrow pointing up at it) and the
/// place. The same while playing and once the game is over, so the box looks the
/// same and the column beside the map is sized for it (see [`side_width`]).
/// With a line of its own, the place is cut only where it would pass the box's
/// edge (`width`). `legs` are its hops' tickets; `style(airport)` colours a code.
/// Each hop is numbered, so the count matches the header's hops; the start isn't
/// a hop and goes unnumbered. Also returns which line each hop starts on.
fn route_lines(
    game: &FlightsGame,
    route: &[usize],
    legs: &[Ticket],
    width: usize,
    style: &dyn Fn(usize) -> Style,
    note: &dyn Fn(usize) -> Option<Span<'static>>,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let net = game.network();
    let units = game.units();
    let gray = Style::default().fg(Color::Gray);
    let dark = Style::default().fg(Color::DarkGray);
    let mut lines = Vec::new();
    let second = |lines: &mut Vec<Line<'static>>, i: usize, n: usize| {
        let mut spans = vec![Span::raw(INDENT)];
        let remark = note(n);
        let had_remark = remark.is_some();
        spans.extend(remark);
        let text = net.place(i);
        if game.rules().show_places && !text.is_empty() {
            let sep = if had_remark { " · " } else { "· " };
            let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let room = width.saturating_sub(used + sep.chars().count());
            spans.push(Span::styled(
                format!("{sep}{}", fit_place(&text, room)),
                dark,
            ));
        }
        if spans.len() > 1 {
            lines.push(Line::from(spans));
        }
    };
    let mut starts = Vec::new();
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(net.code(route[0]).to_string(), style(route[0])),
        Span::styled("  start", gray),
    ]));
    second(&mut lines, route[0], 0);
    for (n, (w, hop)) in route.windows(2).zip(legs).enumerate() {
        let changed = net
            .code(w[0])
            .bytes()
            .zip(net.code(w[1]).bytes())
            .position(|(a, b)| a != b);
        let mut spans = vec![Span::styled(format!("{:>2}. ", n + 1), gray)];
        spans.extend(code_spans(net.code(w[1]), changed, style(w[1])));
        if game.distances_visible() {
            spans.push(Span::styled(
                format!("  {}", units.format(hop.distance)),
                gray,
            ));
        }
        if game.rules().shows_connections() {
            spans.push(Span::styled(via_suffix(net, &hop.via), dark));
        }
        starts.push(lines.len());
        lines.push(Line::from(spans));
        second(&mut lines, w[1], n + 1);
    }
    (lines, starts)
}

fn draw_route(f: &mut Frame, app: &FlightsApp, area: Rect) {
    let game = &app.game;
    let units = game.units();
    let total = |nm: f64| {
        if game.distances_visible() {
            format!(": {}", units.format(nm))
        } else {
            String::new()
        }
    };
    // The airport the player is at is green, wherever it is on the route.
    let code_style = |i: usize| {
        if i == game.current() && game.status() == Status::Playing {
            bold(COLOR_POSITIVE)
        } else if i == game.destination() {
            bold(COLOR_DESTINATION)
        } else {
            Style::default().fg(Color::White)
        }
    };
    let no_note = |_: usize| None;
    // Inside the box's borders.
    let width = usize::from(area.width.saturating_sub(2));
    let (mut lines, mut step_lines) = route_lines(
        game,
        game.route(),
        game.legs(),
        width,
        &code_style,
        &no_note,
    );
    let mut scroll_to_end = true;
    if game.status() == Status::GivenUp {
        // Both routes, and where the player's left the shortest one.
        let split = game.divergence();
        let yours_left = split < game.route().len();
        let left_note = |i: usize| {
            (yours_left && i == split).then(|| Span::styled(YOU_LEFT, bold(COLOR_WARNING)))
        };
        lines = vec![Line::from(Span::styled(
            format!("Yours{}", total(game.total_distance())),
            bold(COLOR_ACCENT),
        ))];
        let (yours, starts) = route_lines(
            game,
            game.route(),
            game.legs(),
            width,
            &code_style,
            &left_note,
        );
        step_lines = starts.iter().map(|&i| i + 1).collect();
        lines.extend(yours);
        lines.push(Line::from(Span::styled(
            format!("Shortest{}", total(game.best_distance())),
            bold(COLOR_POSITIVE),
        )));
        // The part shared with the player's route is dimmed; from the split on, green.
        let best = game.best_route();
        let best_style = |i: usize| {
            let pos = best.iter().position(|&b| b == i).unwrap_or(0);
            if pos < split {
                Style::default().fg(Color::Gray)
            } else {
                bold(COLOR_POSITIVE)
            }
        };
        let here_note = |i: usize| {
            (i == split).then(|| {
                let text = if yours_left { WENT_HERE } else { NEXT_HOP };
                Span::styled(text, bold(COLOR_POSITIVE))
            })
        };
        let (shortest, starts) =
            route_lines(game, best, game.best_legs(), width, &best_style, &here_note);
        let offset = lines.len();
        step_lines.extend(starts.iter().map(|&i| i + offset));
        lines.extend(shortest);
        scroll_to_end = false;
    } else if !game.at_destination() {
        let net = game.network();
        let text = net.place(game.destination());
        lines.push(Line::from(vec![
            Span::styled("  … ", Style::default().fg(Color::Gray)),
            Span::styled(game.end_code(), bold(COLOR_DESTINATION)),
            Span::styled("  destination", Style::default().fg(Color::Gray)),
        ]));
        if game.rules().show_places && !text.is_empty() {
            let room = width.saturating_sub(INDENT.len() + 2);
            lines.push(Line::from(vec![
                Span::raw(INDENT),
                Span::styled(
                    format!("· {}", fit_place(&text, room)),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }
    let title = match game.status() {
        Status::Playing => format!(" Route{} ", total(game.total_distance())),
        Status::Won => format!(
            " You won! Route{} · [^N] New Game ",
            total(game.total_distance())
        ),
        Status::GivenUp => " Game over · [^N] New Game ".to_string(),
    };
    let mut block = Block::default().borders(Borders::ALL).title(title);
    if game.status() != Status::Playing && app.step_count() > 0 {
        block = block.title_bottom(Line::from(" [↑↓] each hop ").right_aligned());
    }
    let inner_height = block.inner(area).height;
    // The picked hop's line, highlighted as an option is while playing.
    let picked = app.route_step.and_then(|i| step_lines.get(i).copied());
    if let Some(i) = picked {
        lines[i].style = Style::default().bg(HIGHLIGHT_BG);
    }
    // After giving up, long lines wrap so every stop's place shows.
    let wrap = |paragraph: Paragraph<'static>| {
        if scroll_to_end {
            paragraph
        } else {
            paragraph.wrap(Wrap { trim: false })
        }
    };
    let line_count = |lines: &[Line<'static>]| {
        u16::try_from(wrap(Paragraph::new(lines.to_vec())).line_count(area.width.saturating_sub(2)))
            .unwrap_or(u16::MAX)
    };
    let max = line_count(&lines).saturating_sub(inner_height);
    // While playing, the end of the route stays in view; after giving up, both
    // routes show from the top. Either way, until the player scrolls, and
    // moved only as far as it takes to show a picked hop.
    let usual = if scroll_to_end { max } else { 0 };
    let scroll = app
        .route_scroll
        .unwrap_or_else(|| match picked {
            Some(i) => {
                let top = line_count(&lines[..i]);
                let bottom = line_count(&lines[..=i]);
                usual.min(top).max(bottom.saturating_sub(inner_height))
            }
            None => usual,
        })
        .min(max);
    let paragraph = wrap(Paragraph::new(lines));
    app.route_frame.set((area, scroll, max));
    f.render_widget(paragraph.scroll((scroll, 0)).block(block), area);
}

fn draw_options(f: &mut Frame, app: &mut FlightsApp, area: Rect) {
    let game = &app.game;
    let show_distances = game.rules().shows_distances();
    let show_via = game.rules().shows_connections();
    let options = game.options();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Options ({}) ", options.len()));
    let inner = block.inner(area);
    app.options_area = inner;

    if options.is_empty() {
        let text = match game.status() {
            Status::Playing if game.at_destination() => {
                "You're at the destination by a longer route. Go back [←] to try another way."
            }
            Status::Playing => {
                "No tickets from here to an airport not already on your route. Go back [←]."
            }
            _ => "The game is over. [^N] New Game.",
        };
        f.render_widget(
            Paragraph::new(Span::styled(text, Style::default().fg(Color::Gray)))
                .wrap(Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    }

    let units = game.units();
    let destination = game.destination();
    let net = game.network();
    let show_places = game.rules().show_places;
    let places: Vec<String> = options
        .iter()
        .map(|o| short_place(&net.place(o.airport)))
        .collect();
    let place_width = places.iter().map(|p| p.chars().count()).max().unwrap_or(0) as u16;
    let rows: Vec<Row> = options
        .iter()
        .enumerate()
        .map(|(i, o): (usize, &FlightOption)| {
            let is_destination = o.airport == destination;
            let base = if is_destination {
                bold(COLOR_DESTINATION)
            } else {
                Style::default().fg(Color::White)
            };
            // The destination is marked by a star beside its code (and heads the
            // list in its colour), which keeps the column narrow.
            let mut code = code_spans(o.code, Some(o.changed), base);
            if is_destination {
                code.push(Span::styled("★", bold(COLOR_DESTINATION)));
            }
            let mut cells = vec![Cell::from(Line::from(code))];
            if show_distances {
                cells.push(
                    Cell::from(units.format(o.distance)).style(Style::default().fg(Color::Gray)),
                );
            }
            if show_places {
                cells.push(Cell::from(places[i].clone()).style(Style::default().fg(Color::Gray)));
            }
            let mut note = Vec::new();
            if show_via {
                note.push(Span::styled(
                    if o.via.is_empty() {
                        "direct".to_string()
                    } else {
                        format!("via {}", o.via.join(", "))
                    },
                    // Gray, not dark gray, so it reads on the selected row too.
                    Style::default().fg(Color::Gray),
                ));
            }
            cells.push(Cell::from(Line::from(note)));
            Row::new(cells)
        })
        .collect();
    let mut widths = vec![Constraint::Length(4)];
    if show_distances {
        widths.push(Constraint::Length(9));
    }
    if show_places {
        widths.push(Constraint::Length(place_width));
    }
    widths.push(Constraint::Min(8));
    let table = Table::new(rows, widths)
        .block(block)
        .highlight_symbol("> ")
        .row_highlight_style(if app.use_color {
            Style::default().bg(HIGHLIGHT_BG)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        });
    f.render_stateful_widget(table, area, &mut app.options_state);
}

/// The status box's height in `column`: `least`, or more when the message
/// wraps to more lines (as a give-up's whole shortest route can), up to half
/// the column so the route below still shows.
fn status_height(app: &FlightsApp, column: Rect, least: u16) -> Constraint {
    let lines = wrapped_line_count(&app.message.0, column.width.saturating_sub(2));
    Constraint::Length((lines + 2).clamp(least, (column.height / 2).max(least)))
}

/// Map bounds `(lon_min, lon_max, lat_min, lat_max)` fitting `points` with padding,
/// widened so a degree of longitude and of latitude look the same length in a
/// `cols` by `rows` area of braille (each cell is about twice as tall as wide).
/// The map goes round, so the longitudes covered are the narrowest arc holding
/// every point: a route across the Pacific is centred on it, not stretched over
/// the whole world. Its ends can pass ±180°.
fn map_bounds(points: &[(f64, f64)], cols: u16, rows: u16) -> (f64, f64, f64, f64) {
    let lat_min = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let lat_max = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    // The arc starts just past the widest gap between neighbouring longitudes,
    // counting the gap from the last round to the first.
    let mut lons: Vec<f64> = points.iter().map(|p| p.1).collect();
    lons.sort_by(f64::total_cmp);
    let (lon_min, lon_max) = match (lons.first(), lons.last()) {
        (Some(&first), Some(&last)) => {
            let (gap, start) = lons.windows(2).map(|w| (w[1] - w[0], w[1])).fold(
                (first + 360.0 - last, first),
                |best, next| {
                    if next.0 > best.0 {
                        next
                    } else {
                        best
                    }
                },
            );
            (start, start + 360.0 - gap)
        }
        _ => (-180.0, 180.0),
    };
    let mid_lat = (lat_min + lat_max) / 2.0;
    let mid_lon = (lon_min + lon_max) / 2.0;
    let lat_scale = mid_lat.to_radians().cos().max(0.2);
    let mut lat_span = ((lat_max - lat_min) * (1.0 + 2.0 * MAP_PADDING)).max(MIN_LAT_SPAN);
    let mut lon_span = (lon_max - lon_min) * (1.0 + 2.0 * MAP_PADDING);
    // On screen, width/height = cols / (2 * rows); in degrees, scaled by latitude.
    let want = f64::from(cols.max(1)) / (2.0 * f64::from(rows.max(1)));
    if lon_span * lat_scale / lat_span < want {
        lon_span = want * lat_span / lat_scale;
    } else {
        lat_span = lon_span * lat_scale / want;
    }
    (
        mid_lon - lon_span / 2.0,
        mid_lon + lon_span / 2.0,
        mid_lat - lat_span / 2.0,
        mid_lat + lat_span / 2.0,
    )
}

/// Lays out the map's airport codes so none overlaps another. Each takes the
/// first free spot around its airport, in priority order (destination first), or
/// is left out and its airport shown by its dot alone. A code moved off its
/// airport leaves the airport's cell free, so its dot shows where it is.
struct MapLabels {
    bounds: (f64, f64, f64, f64),
    width: u16,
    height: u16,
    taken: Vec<bool>,
    /// `(canvas x, canvas y, text)` to print, and the airports' dots to draw.
    placed: Vec<(f64, f64, Span<'static>)>,
    dots: Vec<(f64, f64, Color)>,
    /// Keep codes off their airport's own cell, where a line drawn over the
    /// codes would end and cut into them.
    beside: bool,
}

impl MapLabels {
    fn new(bounds: (f64, f64, f64, f64), width: u16, height: u16) -> Self {
        Self {
            bounds,
            width,
            height,
            taken: vec![false; usize::from(width) * usize::from(height)],
            placed: Vec::new(),
            dots: Vec::new(),
            beside: false,
        }
    }

    /// The terminal cell (column, row) a canvas point falls in, the same way the
    /// canvas places its labels.
    fn cell(&self, x: f64, y: f64) -> Option<(i32, i32)> {
        let (x0, x1, y0, y1) = self.bounds;
        if self.width < 2 || self.height < 2 || x < x0 || x > x1 || y < y0 || y > y1 {
            return None;
        }
        let col = (x - x0) * f64::from(self.width - 1) / (x1 - x0);
        let row = (y1 - y) * f64::from(self.height - 1) / (y1 - y0);
        Some((col as i32, row as i32))
    }

    /// The canvas point that lands in cell (`col`, `row`); just inside it, since
    /// the canvas rounds down.
    fn point(&self, col: i32, row: i32) -> (f64, f64) {
        let (x0, x1, y0, y1) = self.bounds;
        let x = x0 + (f64::from(col) + 0.01) * (x1 - x0) / f64::from(self.width - 1);
        let y = y1 - (f64::from(row) + 0.01) * (y1 - y0) / f64::from(self.height - 1);
        (x.min(x1), y.max(y0))
    }

    /// Whether `len` cells from (`col`, `row`) fit on the map with a blank cell
    /// either side, so neighbouring codes don't run together ("SKNSKX").
    fn free(&self, col: i32, row: i32, len: i32) -> bool {
        let width = i32::from(self.width);
        row >= 0
            && row < i32::from(self.height)
            && col >= 0
            && col + len <= width
            && ((col - 1).max(0)..(col + len + 1).min(width))
                .all(|c| !self.taken[(row * width + c) as usize])
    }

    fn take(&mut self, col: i32, row: i32, len: i32) {
        for c in col..col + len {
            self.taken[(row * i32::from(self.width) + c) as usize] = true;
        }
    }

    /// Labels the airport at canvas point (`x`, `y`) with `code` in `style`.
    fn add(&mut self, x: f64, y: f64, code: &str, style: Style, dot: Color) {
        // The copy of the airport nearest the view's middle, past ±180° if need be.
        let x = wrap_lon(x, (self.bounds.0 + self.bounds.1) / 2.0);
        let Some((col, row)) = self.cell(x, y) else {
            return;
        };
        let len = code.len() as i32;
        // On the airport (starting or ending there), then beside it, then on the
        // rows above and below, nearest first.
        let spots = [0, -1, 1, -2, 2].into_iter().flat_map(|dr| {
            let r = row + dr;
            let cols = if dr == 0 {
                vec![col, col - len + 1, col + 1, col - len]
            } else {
                vec![col - 1, col - len + 1, col, col + 1, col - len, col + 2]
            };
            cols.into_iter().map(move |c| (c, r))
        });
        let on_airport = |c: i32| c <= col && col < c + len;
        let beside = self.beside;
        if let Some((c, r)) = spots
            .into_iter()
            .filter(|&(c, r)| !(beside && r == row && on_airport(c)))
            .find(|&(c, r)| {
                self.free(c, r, len) && (r != row || on_airport(c) || self.free(col, row, 1))
            })
        {
            self.take(c, r, len);
            if !(r == row && on_airport(c)) {
                // Keep the airport's own cell clear for its dot.
                self.take(col, row, 1);
                self.dots.push((x, y, dot));
            }
            let (px, py) = self.point(c, r);
            self.placed
                .push((px, py, Span::styled(code.to_string(), style)));
        } else {
            self.dots.push((x, y, dot));
        }
    }
}

/// Degrees of arc between the points a curved hop is drawn through: fine
/// enough to look smooth at any zoom the map allows for a hop that long.
const CURVE_STEP: f64 = 1.0;

/// The points (`(lon, lat)`) a leg from `a` to `b` is drawn through: its ends,
/// for a straight line, or with `curved`, points along the great circle
/// between them, the way the flight goes. Either way it goes the short way
/// round, its longitudes running on from `a`'s past ±180° where it crosses the
/// antimeridian, so the line never jumps across the map.
fn leg_points(a: (f64, f64), b: (f64, f64), curved: bool) -> Vec<(f64, f64)> {
    let straight = vec![a, (wrap_lon(b.0, a.0), b.1)];
    if !curved {
        return straight;
    }
    let unit = |(lon, lat): (f64, f64)| {
        let (lon, lat) = (lon.to_radians(), lat.to_radians());
        [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]
    };
    let (p, q) = (unit(a), unit(b));
    let angle = (0..3)
        .map(|i| p[i] * q[i])
        .sum::<f64>()
        .clamp(-1.0, 1.0)
        .acos();
    // The same place, or opposite sides of the world (where every great circle
    // through one passes the other): no one curve to draw.
    if angle < 1e-9 || std::f64::consts::PI - angle < 1e-9 {
        return straight;
    }
    let steps = (angle.to_degrees() / CURVE_STEP).ceil() as usize;
    let mut points = vec![a];
    for k in 1..=steps {
        // Spherical interpolation: the point a fraction `t` of the way along.
        let t = k as f64 / steps as f64;
        let (wa, wb) = (
            ((1.0 - t) * angle).sin() / angle.sin(),
            (t * angle).sin() / angle.sin(),
        );
        let r: [f64; 3] = std::array::from_fn(|i| wa * p[i] + wb * q[i]);
        let lat = r[2].clamp(-1.0, 1.0).asin().to_degrees();
        let prev = points[points.len() - 1].0;
        points.push((wrap_lon(r[1].atan2(r[0]).to_degrees(), prev), lat));
    }
    points
}

/// A canvas of just `paths` (each hop as the airports it flies through), with
/// nothing else drawn, to lay over the map.
fn path_canvas<'a>(
    net: &'a crate::flights::data::Network,
    (x0, x1, y0, y1): (f64, f64, f64, f64),
    paths: &'a [Vec<usize>],
    color: Color,
    curved: bool,
) -> Canvas<'a, impl Fn(&mut ratatui::widgets::canvas::Context) + 'a> {
    Canvas::default()
        .x_bounds([x0, x1])
        .y_bounds([y0, y1])
        .paint(move |ctx| draw_paths(ctx, net, (x0, x1, y0, y1), paths, color, curved))
}

/// Draws `paths` (each hop as the airports it flies through) clipped to
/// `bounds`: as straight lines, or with `curved`, great circles.
fn draw_paths(
    ctx: &mut ratatui::widgets::canvas::Context,
    net: &crate::flights::data::Network,
    bounds: Bounds,
    paths: &[Vec<usize>],
    color: Color,
    curved: bool,
) {
    let xy = |i: usize| {
        let (lat, lon) = net.coords(i);
        (lon, lat)
    };
    for w in paths.iter().flat_map(|path| path.windows(2)) {
        // Drawn on every copy of the world in view, so a leg off one edge
        // comes back in at the other.
        let points = leg_points(xy(w[0]), xy(w[1]), curved);
        for turn in turns(bounds) {
            let shift = |(x, y): (f64, f64)| (x + turn, y);
            for p in points.windows(2) {
                if let Some(((ax, ay), (bx, by))) = clip(shift(p[0]), shift(p[1]), bounds) {
                    ctx.draw(&CanvasLine::new(ax, ay, bx, by, color));
                }
            }
        }
    }
}

fn draw_map(f: &mut Frame, app: &FlightsApp, area: Rect) {
    let game = &app.game;
    let net = game.network();
    let options = game.options();
    let selected_option = app.options_state.selected().and_then(|i| options.get(i));
    let selected = selected_option.map(|o| o.airport);
    let route = game.route();
    let show_via = game.rules().shows_connections();

    // Each hop as the airports it flies through, connections included when shown.
    let path_of = |a: usize, via: &[usize], b: usize| -> Vec<usize> {
        let mut path = vec![a];
        if show_via {
            path.extend(via);
        }
        path.push(b);
        path
    };
    let paths_of = |route: &[usize], legs: &[Ticket]| -> Vec<Vec<usize>> {
        route
            .windows(2)
            .zip(legs)
            .map(|(w, leg)| path_of(w[0], &leg.via, w[1]))
            .collect()
    };
    let flown = paths_of(route, game.legs());
    let best = if game.status() == Status::GivenUp {
        paths_of(game.best_route(), game.best_legs())
    } else {
        Vec::new()
    };
    // The hop to the airport selected in the list, as it would be flown (straight,
    // when connections aren't shown); once the game is over, the hop picked in
    // the route box.
    let picked = app.selected_hop();
    let preview: Vec<Vec<usize>> = match picked {
        Some((from, ticket, to)) => vec![path_of(from, &ticket.via, to)],
        None => selected_option
            .map(|o| vec![path_of(game.current(), &o.connections, o.airport)])
            .unwrap_or_default(),
    };

    let mut shown: Vec<usize> = vec![game.origin(), game.destination(), game.current()];
    shown.extend(flown.iter().flatten());
    shown.extend(best.iter().flatten());
    shown.extend(preview.iter().flatten());
    shown.extend(options.iter().map(|o| o.airport));
    let mut points: Vec<(f64, f64)> = shown.iter().map(|&i| net.coords(i)).collect();
    // A curve bows toward the pole, past its airports, so the map fits it whole.
    let curved = game.curves();
    if curved {
        let xy = |i: usize| {
            let (lat, lon) = net.coords(i);
            (lon, lat)
        };
        for w in flown
            .iter()
            .chain(&best)
            .chain(&preview)
            .flat_map(|path| path.windows(2))
        {
            points.extend(
                leg_points(xy(w[0]), xy(w[1]), true)
                    .into_iter()
                    .map(|(lon, lat)| (lat, wrap_lon(lon, 0.0))),
            );
        }
    }
    let title = if app.view.is_some() {
        " Map · [0] fit "
    } else {
        " Map "
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let inner = block.inner(area);
    let fitted = map_bounds(&points, inner.width, inner.height);
    app.map_frame.set(Some((inner, fitted)));
    let bounds = app.view.unwrap_or(fitted);
    let xy = |i: usize| {
        let (lat, lon) = net.coords(i);
        (lon, lat)
    };

    // Once the game is over, its routes are drawn again on top of everything, so
    // none of their hops hides under a code: the winning route, or after giving
    // up, the player's own with a shortest route over it. Their cells are kept
    // free of codes too, so they don't cut into them.
    // While playing, the hop to the selected airport is drawn the same way, so
    // even a short one shows.
    let finished: Vec<(&[Vec<usize>], Color)> = match game.status() {
        Status::Playing if preview.is_empty() => Vec::new(),
        Status::Playing => vec![(&preview, COLOR_WARNING)],
        Status::Won => vec![(&flown, COLOR_ACCENT), (&preview, COLOR_WARNING)],
        Status::GivenUp => vec![
            (&flown, COLOR_ACCENT),
            (&best, COLOR_POSITIVE),
            (&preview, COLOR_WARNING),
        ],
    };
    let (x0, x1, y0, y1) = bounds;
    let path_canvas = |path, color| path_canvas(net, bounds, path, color, curved);

    // Labels in priority order, each airport once.
    let mut labels = MapLabels::new(bounds, inner.width, inner.height);
    labels.beside = !finished.is_empty();
    let finished: Vec<_> = finished
        .into_iter()
        .filter(|(path, _)| !path.is_empty())
        .collect();
    for &(path, color) in &finished {
        let local = Rect::new(0, 0, inner.width, inner.height);
        let mut scratch = ratatui::buffer::Buffer::empty(local);
        path_canvas(path, color).render(local, &mut scratch);
        for y in 0..inner.height {
            for x in 0..inner.width {
                if !matches!(scratch[(x, y)].symbol(), " " | "\u{2800}") {
                    labels.take(i32::from(x), i32::from(y), 1);
                }
            }
        }
    }
    let mut labelled = std::collections::HashSet::new();
    let mut label = |i: usize, style: Style, dot: Color| {
        if labelled.insert(i) {
            let (x, y) = xy(i);
            labels.add(x, y, net.code(i), style, dot);
        }
    };
    // The picked hop's ends come first, so they're named whatever's near.
    if let Some((from, _, to)) = picked {
        label(to, black_on(COLOR_WARNING), COLOR_WARNING);
        label(from, bold(COLOR_WARNING), COLOR_WARNING);
    }
    label(
        game.destination(),
        bold(COLOR_DESTINATION),
        COLOR_DESTINATION,
    );
    if game.status() == Status::Playing {
        label(game.current(), bold(COLOR_POSITIVE), COLOR_POSITIVE);
    }
    label(game.origin(), bold(Color::White), Color::White);
    if let Some(i) = selected {
        let style = black_on(COLOR_WARNING);
        label(i, style, COLOR_WARNING);
    }
    for o in &options {
        label(o.airport, Style::default().fg(COLOR_WARNING), COLOR_WARNING);
    }
    for &i in route {
        label(i, Style::default().fg(COLOR_ACCENT), COLOR_ACCENT);
    }
    // After giving up, the shortest route's airports, so it can be followed (its
    // last hop most of all, which may be short enough to hide under labels).
    if game.status() == Status::GivenUp {
        for &i in game.best_route().iter().rev() {
            label(i, bold(COLOR_POSITIVE), COLOR_POSITIVE);
        }
    }
    for path in flown.iter().chain(&best).chain(&preview) {
        for &i in &path[1..path.len() - 1] {
            label(i, Style::default().fg(Color::DarkGray), Color::Gray);
        }
    }

    let canvas = Canvas::default()
        .block(block)
        .x_bounds([x0, x1])
        .y_bounds([y0, y1])
        .paint(|ctx| {
            // Natural Earth's coastlines, as detailed as the view's dots can show.
            ctx.draw(&Coast {
                lines: coast::for_view(bounds.3 - bounds.2, inner.height),
                bounds,
                color: Color::DarkGray,
            });
            ctx.layer();
            draw_paths(ctx, net, bounds, &flown, COLOR_ACCENT, curved);
            draw_paths(ctx, net, bounds, &best, COLOR_POSITIVE, curved);
            ctx.layer();
            for &(x, y, color) in &labels.dots {
                ctx.draw(&Points {
                    coords: &[(x, y)],
                    color,
                });
            }
            for (x, y, text) in &labels.placed {
                ctx.print(*x, *y, text.clone());
            }
        });
    f.render_widget(canvas, area);
    // A canvas only writes the cells it draws in, so the map shows through.
    for (path, color) in finished {
        f.render_widget(path_canvas(path, color), inner);
    }
}

/// The help page: rules, controls, each level with its puzzle count, credits.
fn draw_help(f: &mut Frame, game: &FlightsGame) {
    let custom = game.custom();
    let mut levels: Vec<HelpLine> = Difficulty::STANDARD
        .iter()
        .map(|&d| HelpLine::Row {
            name: d.label().to_string(),
            desc: d.description().to_string(),
            note: grouped(d.puzzle_count()),
        })
        .collect();
    levels.push(HelpLine::Row {
        name: "Custom".to_string(),
        desc: if custom.fixed_route {
            "your route ([^S] settings)".to_string()
        } else {
            format!(
                "{}, {} conn. ([^S] settings)",
                custom.hops_text(),
                custom.max_connections
            )
        },
        // Counted with connections reused, so only a bound when they can't be.
        note: if custom.count_is_exact() {
            grouped(custom.puzzle_count())
        } else {
            format!("≤{}", grouped(custom.puzzle_count()))
        },
    });
    let page = HelpPage {
        game: "Flights",
        tagline: "the shortest way, one letter at a time".to_string(),
        sections: vec![
            HelpSection {
                title: "Rules",
                lines: {
                    let mut rules = vec![
                        HelpLine::bullet(
                            "Each hop changes one letter of the code: LAX → LAS → LOS.",
                        ),
                        HelpLine::bullet(
                            "A hop needs a ticket, direct or 1 connection; every leg counts.",
                        ),
                        HelpLine::bullet(
                            "Reach it the long way and you'll be told; go back to improve.",
                        ),
                    ];
                    if game.rules().once_per_airport() {
                        rules.push(HelpLine::bullet(
                            "This game uses each airport once, connections included.",
                        ));
                    }
                    rules
                },
            },
            HelpSection {
                title: "Controls",
                lines: vec![
                    HelpLine::Keys(vec![
                        ("↑↓", "choose"),
                        ("→ Enter", "fly"),
                        ("← Bksp", "back"),
                        ("PgUp PgDn", "route"),
                    ]),
                    HelpLine::Keys(vec![
                        ("^N", "new"),
                        ("^T", "daily"),
                        ("^G", "give up"),
                        ("^U", "units"),
                    ]),
                    HelpLine::Keys(vec![
                        ("^D", "difficulty"),
                        ("^S", "settings"),
                        ("Esc", "quit"),
                    ]),
                    HelpLine::Keys(vec![
                        ("+ -", "zoom"),
                        ("WASD", "pan"),
                        ("0", "fit"),
                        ("wheel drag", "the map"),
                    ]),
                    HelpLine::Bullet(Line::from(
                        "Once a game is over, ↑↓ steps through its route, showing each hop on the map.",
                    )),
                ],
            },
            HelpSection {
                title: "Difficulty · puzzles",
                lines: levels,
            },
            HelpSection {
                title: "Credits",
                lines: vec![
                    HelpLine::author(),
                    HelpLine::Credit(
                        "Data",
                        "Routes: Wikipedia (CC BY-SA). Airports: OurAirports.".to_string(),
                    ),
                    HelpLine::Credit("Map", "Coastlines: Natural Earth".to_string()),
                ],
            },
        ],
    };
    draw_help_page(f, &page);
}

/// The settings page: units, then the custom game's rules, each with its value
/// between arrows, the selected row highlighted. The puzzle count and keys stay
/// at the bottom; above them, the rows scroll to keep the selected one in view
/// when they don't all fit (as on an 80x24 terminal).
fn draw_settings(f: &mut Frame, page: &SettingsPage) {
    // Just big enough for the page, up to most of the screen; past that, its
    // rows scroll.
    let most = centered_rect(90, 90, f.area());
    let inner_width = most.width.min(SETTINGS_WIDTH + 2).saturating_sub(2);
    let heading = bold(COLOR_ACCENT);
    let c = page.custom;
    let on = |b: bool| if b { "On" } else { "Off" }.to_string();
    let value = |setting: Setting| -> (String, &'static str) {
        match setting {
            // The preferences say their defaults, since Reset leaves them be.
            Setting::Units => (
                page.units.label().to_string(),
                "distance units (Default: nm)",
            ),
            Setting::Lines => (
                if page.curves { "Curved" } else { "Straight" }.to_string(),
                "the hops' shape (Default: Curved)",
            ),
            Setting::View => (
                c.view.label().to_string(),
                "sets the 4 below as a level does",
            ),
            Setting::Map => (on(c.view.map), "else a list of codes"),
            Setting::Distances => (on(c.view.distances), "while playing"),
            Setting::Via => (on(c.view.connections), "where hops connect"),
            Setting::Over => (on(c.view.amount_over), "when you arrive the long way"),
            Setting::Connections => (c.max_connections.to_string(), "per hop's ticket"),
            Setting::MinHops => (c.min_hops.to_string(), "in the best route (3-20)"),
            Setting::MaxHops => (
                c.max_hops.map_or("No limit".to_string(), |n| n.to_string()),
                "in the best route (3-20)",
            ),
            Setting::Places => (on(c.show_places), "beside each option"),
            Setting::Reuse => (
                if c.reuse_connections { "Yes" } else { "No" }.to_string(),
                if c.reuse_connections {
                    "airports may repeat as connections"
                } else {
                    "each airport once, connections too"
                },
            ),
            Setting::Reset => ("Reset".to_string(), "custom settings to defaults"),
            Setting::Mode => (
                if c.fixed_route { "Route" } else { "Random" }.to_string(),
                if c.fixed_route {
                    "your start and end, any length"
                } else {
                    "random, within the limits"
                },
            ),
            Setting::Start | Setting::End => {
                let text = if setting == Setting::Start {
                    &page.start_text
                } else {
                    &page.end_text
                };
                let shown = match text.len() {
                    0 => "Random".to_string(),
                    3 => text.clone(),
                    _ => format!("{text}_"),
                };
                (shown, "type a code; [←→] random")
            }
        }
    };
    let name = |setting: Setting| match setting {
        Setting::Units => "Units",
        Setting::Lines => "Map lines",
        Setting::View => "Shows",
        // The view's parts, indented under the preset that sets them.
        Setting::Map => "  Map",
        Setting::Distances => "  Distances",
        Setting::Via => "  Connections",
        Setting::Over => "  How far over",
        Setting::Connections => "Max connections",
        Setting::MinHops => "Min hops",
        Setting::MaxHops => "Max hops",
        Setting::Places => "City, country",
        Setting::Mode => "Puzzles",
        Setting::Start => "Start",
        Setting::End => "End",
        Setting::Reuse => "Reuse connections",
        Setting::Reset => "Defaults",
    };
    let row_line = |i: usize, setting: Setting| {
        let selected = i == page.row;
        let (v, note) = value(setting);
        // Rows the mode doesn't use are greyed (and skipped by Up / Down).
        let note = if page.enabled(setting) {
            note
        } else if setting == Setting::Reuse {
            "no connections to reuse"
        } else if c.fixed_route {
            "not used for a fixed route"
        } else {
            "only for a fixed route"
        };
        let base = if selected {
            Style::default().bg(HIGHLIGHT_BG).fg(Color::White)
        } else if page.enabled(setting) {
            Style::default().fg(Color::White)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        Line::from(vec![
            Span::styled(if selected { " > " } else { "   " }, base),
            Span::styled(format!("{:<18}", name(setting)), base),
            // A button rather than a value to step through.
            Span::styled(
                if setting == Setting::Reset {
                    format!("[{v:^10}]")
                } else {
                    format!("◀ {v:^8} ▶")
                },
                base.add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("  {note}"), Style::default().fg(Color::DarkGray)),
        ])
    };
    // The rows, in lines, noting where the selected one and the reset button
    // land once wrapped.
    let wrapped = |lines: &[Line<'static>]| {
        u16::try_from(
            Paragraph::new(lines.to_vec())
                .wrap(Wrap { trim: false })
                .line_count(inner_width),
        )
        .unwrap_or(u16::MAX)
    };
    let mut rows = vec![Line::from(Span::styled("Settings", heading))];
    // (top, height) of each row, in wrapped lines.
    let mut spans = Vec::new();
    let reset = SETTINGS.len() - 1;
    for (i, &setting) in SETTINGS.iter().enumerate() {
        if i == PREFERENCE_ROWS {
            rows.push(Line::from(""));
            rows.push(Line::from(Span::styled("Custom game", heading)));
        }
        if i == reset {
            rows.push(Line::from(""));
        }
        let line = row_line(i, setting);
        spans.push((wrapped(&rows), wrapped(std::slice::from_ref(&line))));
        rows.push(line);
    }

    let mut bottom = vec![
        Line::from(""),
        match (&page.problem, &page.status) {
            (Some(problem), _) => Line::from(Span::styled(problem.clone(), bold(COLOR_WARNING))),
            (None, Err(why)) => Line::from(Span::styled(
                format!("Can't play these settings: {why}."),
                bold(COLOR_WARNING),
            )),
            // Counted with connections reused, so only a bound when they can't be.
            (None, Ok(count)) => Line::from(vec![
                Span::raw(if c.count_is_exact() {
                    "Puzzles with these settings: "
                } else {
                    "Puzzles with these settings: up to "
                }),
                Span::styled(grouped(*count), bold(COLOR_POSITIVE)),
            ]),
        },
    ];
    // Each level's own limit, for reference when choosing custom's.
    let levels: Vec<String> = Difficulty::STANDARD
        .iter()
        .map(|&d| {
            let max = d.rules(c).max_hops;
            format!(
                "{} {}",
                d.label(),
                max.map_or("none".to_string(), |n| n.to_string())
            )
        })
        .collect();
    bottom.push(Line::from(Span::styled(
        format!("Levels' max hops: {}", levels.join(", ")),
        Style::default().fg(Color::DarkGray),
    )));
    bottom.push(Line::from(""));
    bottom.push(Line::from(Span::styled(
        "[↑↓] choose  [←→] change  [Enter] play custom  [Esc] close",
        Style::default().fg(Color::Gray),
    )));

    let total = wrapped(&rows);
    let height = (total + wrapped(&bottom) + 2).min(most.height);
    let area = Rect::new(
        most.x + most.width.saturating_sub(inner_width + 2) / 2,
        most.y + (most.height - height) / 2,
        inner_width + 2,
        height,
    );
    f.render_widget(Clear, area);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    // The bottom keeps its lines, unless the page is too short even for them.
    let bottom_height = wrapped(&bottom).min(inner.height / 2);
    let [top_area, bottom_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(bottom_height)]).areas(inner);
    let (sel_top, sel_height) = spans[page.row];
    // Scrolled no further than it takes to show the selected row in full.
    let scroll = (sel_top + sel_height)
        .saturating_sub(top_area.height)
        .min(total.saturating_sub(top_area.height));
    let (reset_top, reset_height) = spans[reset];
    page.reset_area.set(
        Rect::new(
            top_area.x,
            (top_area.y + reset_top).saturating_sub(scroll),
            top_area.width,
            reset_height,
        )
        .intersection(top_area),
    );

    let more = match (scroll > 0, scroll + top_area.height < total) {
        (true, true) => " ↑↓ more ",
        (true, false) => " ↑ more ",
        (false, true) => " ↓ more ",
        (false, false) => "",
    };
    f.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Settings ")
            .title_bottom(Line::from(more).right_aligned())
            .border_style(Style::default().fg(COLOR_ACCENT)),
        area,
    );
    f.render_widget(
        Paragraph::new(rows)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        top_area,
    );
    f.render_widget(
        Paragraph::new(bottom).wrap(Wrap { trim: false }),
        bottom_area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flights::game::Units;
    use crate::tui::{render_buffer, screen_text};

    fn render(app: &mut FlightsApp, width: u16, height: u16) -> String {
        screen_text(&buffer(app, width, height))
    }

    fn app(difficulty: Difficulty) -> FlightsApp {
        FlightsApp::new(FlightsGame::new(difficulty, Units::Nm))
    }

    fn show(screen: &str) {
        if std::env::var_os("FLIGHTS_SHOW_TUI").is_some() {
            println!("{screen}");
        }
    }

    #[test]
    fn easy_shows_map_and_option_details() {
        let mut app = app(Difficulty::Easy);
        let screen = render(&mut app, 120, 34);
        show(&screen);
        assert!(screen.contains("FLIGHTS"));
        assert!(screen.contains(" Map "));
        assert!(screen.contains(app.game.end_code()));
        assert!(screen.contains("nm"));
        assert!(screen.contains("direct") || screen.contains("via "));
    }

    fn buffer(app: &mut FlightsApp, width: u16, height: u16) -> ratatui::buffer::Buffer {
        render_buffer(width, height, |f| draw_ui(f, app))
    }

    #[test]
    fn medium_hides_distances_until_the_end() {
        let mut app = app(Difficulty::Medium);
        let screen = render(&mut app, 120, 34);
        show(&screen);
        assert!(screen.contains(" Map "));
        assert!(screen.contains("direct") || screen.contains("via "));
        assert!(!screen.contains(" nm"), "a distance is showing");
        app.submit();
        let screen = render(&mut app, 120, 34);
        assert!(!screen.contains(" nm"), "a distance is showing");
        app.trigger_give_up();
        let screen = render(&mut app, 120, 34);
        show(&screen);
        assert!(screen.contains(" nm"), "distances aren't revealed");
    }

    #[test]
    fn route_box_scrolls() {
        let mut app = app(Difficulty::Medium);
        // A route longer than the box: keep flying while there's somewhere to go.
        for _ in 0..12 {
            if app.game.status() != Status::Playing || app.game.options().is_empty() {
                break;
            }
            app.submit();
        }
        let start = format!("{}  start", app.game.network().code(app.game.route()[0]));
        let screen = render(&mut app, 120, 24);
        let (area, scroll, max) = app.route_frame.get();
        assert!(max > 0, "the route fits, so this tests nothing:\n{screen}");
        // It opens on the end of the route, with the start scrolled out of view.
        assert_eq!(scroll, max);
        assert!(!screen.contains(&start));
        // The wheel over the box scrolls it, as far as its top and no further.
        for _ in 0..max + 5 {
            app.scroll_route(-1);
        }
        let screen = render(&mut app, 120, 24);
        show(&screen);
        assert_eq!(app.route_frame.get().1, 0);
        assert!(
            screen.contains(&start),
            "the start isn't in view:\n{screen}"
        );
        assert!(app.on_route(area.x + 1, area.y + 1));
        // Page Down goes back down; going back shows the new end.
        app.scroll_route(app.route_page());
        render(&mut app, 120, 24);
        assert!(app.route_frame.get().1 > 0);
        app.go_back();
        render(&mut app, 120, 24);
        let (_, scroll, max) = app.route_frame.get();
        assert_eq!(scroll, max);
    }

    #[test]
    fn give_up_routes_scroll_to_the_shortest_end() {
        let mut app = app(Difficulty::Medium);
        app.submit();
        app.trigger_give_up();
        render(&mut app, 120, 24);
        let (_, scroll, max) = app.route_frame.get();
        // Both routes from the top, and the rest a scroll away.
        assert_eq!(scroll, 0);
        assert!(max > 0);
        app.scroll_route(i32::from(max));
        let screen = render(&mut app, 120, 24);
        show(&screen);
        let end = app.game.end_code();
        assert!(
            screen.matches(end).count() >= 2,
            "the shortest route's end isn't in view:\n{screen}"
        );
    }

    #[test]
    fn give_up_status_shows_whole() {
        // Every layout: map beside, no map, and one narrow column.
        for (difficulty, width, height) in [
            (Difficulty::Medium, 120, 34),
            (Difficulty::Hard, 120, 34),
            (Difficulty::Medium, 44, 50),
            (Difficulty::Hard, 44, 50),
        ] {
            let mut app = app(difficulty);
            app.submit();
            app.trigger_give_up();
            let screen = render(&mut app, width, height);
            show(&screen);
            // The status's last line, which starts at its box's edge (unlike
            // the footer button, the options' note and the route's title).
            assert!(
                screen.contains("│[^N] New Game"),
                "status cut off at {width}x{height} on {difficulty:?}:\n{screen}"
            );
        }
    }

    #[test]
    fn selected_option_is_highlighted_on_the_map() {
        let mut app = app(Difficulty::Easy);
        app.move_selection(true);
        let selected = app.game.options()[app.options_state.selected().unwrap()].code;
        let buf = buffer(&mut app, 120, 34);
        let highlighted: String = buf
            .content()
            .iter()
            .filter(|c| c.bg == COLOR_WARNING)
            .map(|c| c.symbol())
            .collect();
        // Either labelled with its code, or (crowded out) not labelled at all.
        assert!(
            highlighted == selected || highlighted.is_empty(),
            "highlighted '{highlighted}', selected {selected}"
        );
    }

    #[test]
    fn destination_is_marked_in_the_options() {
        let net = Rules::default().network();
        let start = (0..net.len()).find(|&i| !net.hops(i).is_empty()).unwrap();
        let end = net.hops(start)[0].to;
        let game = FlightsGame::with_puzzle(
            net,
            start,
            end,
            Difficulty::Hard,
            Rules::default(),
            Units::Nm,
            None,
        )
        .unwrap();
        let mut app = FlightsApp::new(game);
        let buf = buffer(&mut app, 100, 30);
        let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("★"));
        let purple = buf
            .content()
            .iter()
            .filter(|c| c.fg == COLOR_DESTINATION && c.symbol() == "★")
            .count();
        assert_eq!(purple, 1);
    }

    #[test]
    fn options_show_places_unless_turned_off() {
        let mut app = app(Difficulty::Hard);
        let opt = app.game.options()[0].clone();
        let place = app.game.network().place(opt.airport);
        let screen = render(&mut app, 120, 34);
        show(&screen);
        // Long names are cut to fit the column, keeping the country code.
        let shown = short_place(&place);
        assert!(screen.contains(&shown), "no '{shown}' for '{place}'");

        let custom = Rules {
            view: View::of(Difficulty::Hard),
            show_places: false,
            ..Rules::default()
        };
        let game = FlightsGame::random(Difficulty::Custom, custom, Units::Nm, None).unwrap();
        let mut app = FlightsApp::new(game);
        let screen = render(&mut app, 120, 34);
        for o in app.game.options() {
            let place = app.game.network().place(o.airport);
            assert!(!screen.contains(&place), "'{place}' is showing");
        }
    }

    #[test]
    fn settings_page_edits_and_plays_custom() {
        let mut app = app(Difficulty::Medium);
        app.open_settings();
        let screen = render(&mut app, 80, 24);
        show(&screen);
        for text in [
            "Settings",
            "Units",
            "Map lines",
            "Curved",
            "Custom game",
            "Max connections",
            "Puzzles with these",
        ] {
            assert!(screen.contains(text), "no '{text}'");
        }
        let page = app.settings.as_mut().unwrap();
        let row = |s: Setting| SETTINGS.iter().position(|&r| r == s).unwrap();
        page.change(true); // units: nm -> mi
        page.row = row(Setting::Lines);
        page.change(true); // map lines: curved -> straight
        page.row = row(Setting::View);
        page.change(true); // view: medium -> hard
        page.row = row(Setting::Connections);
        page.change(false); // connections: 1 -> 0
        page.row = row(Setting::MaxHops);
        page.change(false); // max hops: 6 -> 5
        page.row = row(Setting::MinHops);
        page.change(true); // min hops: 3 -> 4
        page.row = row(Setting::Places);
        page.change(true); // places: off
        page.row = row(Setting::Distances);
        page.change(true); // Hard's view, but with distances: no level's
        let screen = render(&mut app, 80, 24);
        show(&screen);
        assert!(screen.contains("Mixed"), "{screen}");
        app.close_settings(true);
        assert!(app.settings.is_none());
        assert_eq!(app.game.units(), Units::Mi);
        assert!(!app.game.curves());
        assert_eq!(app.game.difficulty(), Difficulty::Custom);
        let want = Rules {
            view: View {
                distances: true,
                ..View::of(Difficulty::Hard)
            },
            min_hops: 4,
            max_hops: Some(5),
            max_connections: 0,
            show_places: false,
            fixed_route: false,
            start: None,
            end: None,
            reuse_connections: true,
        };
        assert_eq!(app.game.rules(), want);
        assert_eq!(app.game.custom(), want);
        // Escape keeps edits without starting a game.
        app.open_settings();
        let page = app.settings.as_mut().unwrap();
        page.row = row(Setting::MaxHops);
        page.change(false); // max hops: 5 -> 4, the min...
        page.change(false); // ...below which it wraps round to no limit
        assert_eq!(page.custom.max_hops, None);
        page.change(true); // and back round to the least
        assert_eq!(page.custom.max_hops, Some(4));
        page.row = row(Setting::MinHops);
        page.change(true); // nor min hops above max: wraps to 3
        assert_eq!(page.custom.min_hops, MIN_HOPS);
        page.row = row(Setting::MaxHops);
        for _ in 0..(MAX_HOPS - 4 + 1) {
            page.change(true); // 4 .. 20, then no limit
        }
        page.row = row(Setting::View);
        page.change(false); // a mixed view steps back to the last level
        assert_eq!(page.custom.view, View::of(Difficulty::Pilot));
        app.close_settings(false);
        assert_eq!(app.game.custom().max_hops, None);
        assert_eq!(app.game.rules().max_hops, Some(5));
    }

    #[test]
    fn settings_rows_scroll_to_keep_the_selection_in_view() {
        let mut app = app(Difficulty::Medium);
        app.open_settings();
        let screen = render(&mut app, 80, 24);
        show(&screen);
        // On 80x24 the last rows don't fit at first, and the bottom says so.
        assert!(
            screen.contains("Units") && !screen.contains("Defaults"),
            "{screen}"
        );
        assert!(screen.contains("↓ more"), "{screen}");
        // Every row stays visible as the selection moves down to the last.
        for _ in 1..SETTINGS.len() {
            app.settings.as_mut().unwrap().step_row(true);
            let screen = render(&mut app, 80, 24);
            assert!(
                screen.contains(" > "),
                "the selected row is hidden:\n{screen}"
            );
            assert!(screen.contains("Puzzles with these"), "{screen}");
        }
        let screen = render(&mut app, 80, 24);
        show(&screen);
        assert!(
            screen.contains("Defaults") && screen.contains("↑ more"),
            "{screen}"
        );
        // The reset button is clicked where it's drawn, scrolled or not.
        let reset = app.settings.as_ref().unwrap().reset_area.get();
        assert!(reset.height > 0);
        let row_text: String = screen.lines().nth(reset.y as usize).unwrap().to_string();
        assert!(row_text.contains("Defaults"), "{row_text}");
        // On a narrow screen, where rows wrap, the selection still stays in view.
        let page = app.settings.as_mut().unwrap();
        page.row = 0;
        for _ in 1..SETTINGS.len() {
            app.settings.as_mut().unwrap().step_row(true);
            let screen = render(&mut app, 40, 30);
            assert!(
                screen.contains(" > "),
                "the selected row is hidden:\n{screen}"
            );
        }
        show(&render(&mut app, 40, 30));
    }

    #[test]
    fn settings_page_is_just_big_enough() {
        let mut app = app(Difficulty::Medium);
        app.open_settings();
        for reuse in [false, true] {
            app.settings.as_mut().unwrap().custom.reuse_connections = reuse;
            let screen = render(&mut app, 120, 50);
            show(&screen);
            let lines: Vec<&str> = screen.lines().collect();
            let top = lines.iter().position(|l| l.contains("Settings ─")).unwrap();
            let keys = lines
                .iter()
                .position(|l| l.contains("[Esc] close"))
                .unwrap();
            let bottom = keys + 1;
            assert!(lines[bottom].contains("└────"), "{screen}");
            // Shorter than the screen, with nothing scrolled away.
            assert!(top > 2 && bottom < 47, "{screen}");
            assert!(!screen.contains("more"), "{screen}");
            // The count and keys follow the rows, not the bottom of the page.
            let defaults = lines.iter().position(|l| l.contains("Defaults")).unwrap();
            let count = lines
                .iter()
                .position(|l| l.contains("Puzzles with these"))
                .unwrap();
            assert_eq!(count, defaults + 2, "{screen}");
            // No row wraps: each note ends on its row's line.
            let reuse_row = lines
                .iter()
                .find(|l| l.contains("Reuse connections"))
                .unwrap();
            assert!(reuse_row.contains("connections too") || reuse_row.contains("as connections"));
            // And the page is no wider than its rows.
            let left = lines[top].find('┌').unwrap();
            let right = lines[top].rfind('┐').unwrap();
            assert_eq!(lines[top][left..right].chars().count(), 71, "{screen}");
        }
    }

    #[test]
    fn route_hops_are_numbered_and_stepped_through_once_over() {
        let mut app = app(Difficulty::Easy);
        app.submit();
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains(" 1. "), "{screen}");
        app.trigger_give_up();
        let orange = |app: &mut FlightsApp| {
            let buf = buffer(app, 120, 34);
            buf.content()
                .iter()
                .filter(|c| c.fg == COLOR_WARNING && braille(c))
                .count()
        };
        let picked = |app: &mut FlightsApp| {
            let buf = buffer(app, 120, 34);
            let rows: Vec<u16> = (0..34u16)
                .filter(|&y| (0..120u16).any(|x| buf[(x, y)].bg == HIGHLIGHT_BG))
                .collect();
            rows
        };
        // Both routes whole, and nothing picked, until a key is pressed.
        assert_eq!(orange(&mut app), 0);
        assert!(picked(&mut app).is_empty());
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains("[↑↓] each hop"), "{screen}");
        let steps = app.step_count();
        assert_eq!(steps, app.game.legs().len() + app.game.best_legs().len());
        for i in 0..steps {
            app.move_selection(true);
            assert_eq!(app.route_step, Some(i));
            // The hop is drawn on the map and its line highlighted, in view.
            assert!(orange(&mut app) > 0, "hop {i} isn't on the map");
            assert_eq!(picked(&mut app).len(), 1, "hop {i} isn't highlighted");
        }
        show(&render(&mut app, 120, 34));
        // The last hop is the shortest route's, arriving at the destination.
        let (_, _, to) = app.selected_hop().unwrap();
        assert_eq!(to, app.game.destination());
        app.move_selection(true);
        assert_eq!(app.route_step, Some(steps - 1));
        // A small screen still scrolls the picked hop into view.
        for _ in 0..steps {
            app.move_selection(false);
            assert_eq!(
                (0..18u16)
                    .filter(|&y| {
                        let buf = buffer(&mut app, 80, 18);
                        (0..80u16).any(|x| buf[(x, y)].bg == HIGHLIGHT_BG)
                    })
                    .count(),
                1
            );
        }
        assert_eq!(app.route_step, Some(0));
        app.start_new_game(None);
        assert_eq!(app.route_step, None);
    }

    #[test]
    fn finished_routes_are_drawn_over_the_codes() {
        let braille = |c: &ratatui::buffer::Cell| {
            c.symbol()
                .chars()
                .all(|ch| ('\u{2801}'..='\u{28FF}').contains(&ch))
        };
        for _ in 0..5 {
            let mut app = app(Difficulty::Medium);
            let buf = buffer(&mut app, 120, 34);
            let white_before = buf
                .content()
                .iter()
                .filter(|c| c.fg == COLOR_POSITIVE && braille(c))
                .count();
            assert_eq!(white_before, 0, "a shortest route shows before giving up");
            app.trigger_give_up();
            let buf = buffer(&mut app, 120, 34);
            // The shortest route's line, drawn over everything else.
            let white = buf
                .content()
                .iter()
                .filter(|c| c.fg == COLOR_POSITIVE && braille(c))
                .count();
            assert!(white > 0, "no shortest route drawn");
            // Its airports are labelled too, where the line leaves room.
            let map: String = (3..31u16)
                .map(|y| (1..73u16).map(|x| buf[(x, y)].symbol()).collect::<String>())
                .collect();
            let shown = app
                .game
                .best_route()
                .iter()
                .filter(|&&i| map.contains(app.game.network().code(i)))
                .count();
            assert!(shown >= 1, "none of the route's airports is labelled");
        }
    }

    #[test]
    fn game_over_keeps_the_side_column_and_stacks_each_stop() {
        for d in [Difficulty::Easy, Difficulty::Medium] {
            let mut app = app(d);
            let width_of_side = |app: &mut FlightsApp| {
                let buf = buffer(app, 120, 34);
                // The side column's left border: the last "┌" on the row under the header.
                (0..120u16)
                    .rev()
                    .find(|&x| buf[(x, 3)].symbol() == "┌")
                    .unwrap()
            };
            let playing = width_of_side(&mut app);
            app.submit();
            app.trigger_give_up();
            assert_eq!(
                width_of_side(&mut app),
                playing,
                "{d}: the side column moved"
            );
            let screen = render(&mut app, 120, 34);
            show(&screen);
            // Each stop's place sits on the line under its code, indented.
            let net = app.game.network();
            let stop = app.game.best_route()[1];
            // The start of it: a long one is cut at the box's edge.
            let place: String = net.place(stop).chars().take(16).collect();
            let lines: Vec<&str> = screen.lines().collect();
            // Below the route box's title: the status above says the route in words.
            let title = lines.iter().position(|l| l.contains("Game over")).unwrap();
            let row = title
                + lines[title..]
                    .iter()
                    .position(|l| l.contains(&format!(" 1. {}", net.code(stop))))
                    .expect("the shortest route's first stop is listed");
            assert!(
                lines[row + 1..=row + 2]
                    .iter()
                    .any(|l| l.contains(&format!("· {place}"))),
                "{d}: '{place}' isn't under its stop"
            );
        }
    }

    #[test]
    fn route_places_use_their_whole_line() {
        // A start whose place is longer than the options' 20-character column,
        // but short enough for a line of its own.
        let net = Rules::default().network();
        let start = (0..net.len())
            .find(|&i| !net.hops(i).is_empty() && (24..=30).contains(&net.place(i).chars().count()))
            .unwrap();
        let end = net.hops(start)[0].to;
        let game = FlightsGame::with_puzzle(
            net,
            start,
            end,
            Difficulty::Medium,
            Rules::default(),
            Units::Nm,
            None,
        )
        .unwrap();
        let mut app = FlightsApp::new(game);
        let screen = render(&mut app, 120, 34);
        show(&screen);
        let place = net.place(start);
        assert!(screen.contains(&format!("· {place}")), "'{place}' was cut");
        // In the options it's still cut to its column.
        assert!(fit_place(&place, PLACE_WIDTH).contains('…'));
        // And where the box is too narrow, it's cut at the edge.
        let long = "Saint-Jacques-de-la-Lande, Ille-et-Vilaine, FR";
        // Only the city is cut; the country code stays.
        assert_eq!(fit_place(long, 14), "Saint-Jac…, FR");
        assert_eq!(fit_place(long, PLACE_WIDTH).chars().count(), PLACE_WIDTH);
        assert!(fit_place(long, PLACE_WIDTH).ends_with("…, FR"));
        assert_eq!(fit_place("Las Vegas, US", 14), "Las Vegas, US");
    }

    #[test]
    fn the_map_zooms_pans_and_fits_again() {
        let mut app = app(Difficulty::Medium);
        let screen = render(&mut app, 120, 34);
        assert!(screen.contains(" Map ") && !screen.contains("[0] fit"));
        let (inner, fitted) = app.map_frame.get().unwrap();
        let span = |b: Bounds| (b.1 - b.0, b.3 - b.2);

        // Zooming in narrows the view about its middle.
        app.zoom(1.0 / ZOOM_STEP, None);
        let zoomed = app.view.unwrap();
        let ((fw, fh), (zw, zh)) = (span(fitted), span(zoomed));
        assert!((zw - fw / ZOOM_STEP).abs() < 1e-9 && (zh - fh / ZOOM_STEP).abs() < 1e-9);
        let mid = |b: Bounds| ((b.0 + b.1) / 2.0, (b.2 + b.3) / 2.0);
        assert!((mid(zoomed).0 - mid(fitted).0).abs() < 1e-9);
        let screen = render(&mut app, 120, 34);
        show(&screen);
        assert!(screen.contains("[0] fit"));

        // The wheel keeps the point under the pointer where it is.
        let (col, row) = (inner.x + 5, inner.y + 3);
        let before = app.map_point(col, row).unwrap();
        app.zoom(1.0 / ZOOM_STEP, Some((col, row)));
        let after = app.map_point(col, row).unwrap();
        assert!((before.0 - after.0).abs() < 1e-6 && (before.1 - after.1).abs() < 1e-6);

        // Panning east and north moves the view that way.
        let v = app.view.unwrap();
        app.pan(PAN_STEP, PAN_STEP);
        let p = app.view.unwrap();
        assert!(p.0 > v.0 && p.2 > v.2);

        // Dragging right moves the map right: the view moves west.
        app.drag = Some((inner.x + 10, inner.y + 5, p));
        app.drag_to(inner.x + 20, inner.y + 5);
        assert!(app.view.unwrap().0 < p.0);
        app.drag = None;

        // The view is kept while playing...
        let kept = app.view;
        app.submit();
        render(&mut app, 120, 34);
        assert_eq!(app.view, kept);
        // ...until [0] or a new game.
        app.fit_map();
        assert_eq!(app.view, None);
        app.zoom(1.0 / ZOOM_STEP, None);
        app.start_new_game(None);
        assert_eq!(app.view, None);
    }

    #[test]
    fn close_zoom_draws_detailed_coast_quickly() {
        // Zoomed far in on a coast (around Oslo), the finest coastlines are drawn,
        // fast enough to redraw on every key.
        let mut app = app(Difficulty::Medium);
        render(&mut app, 120, 34);
        app.view = Some((9.5, 12.5, 58.8, 60.2));
        let started = std::time::Instant::now();
        let buf = buffer(&mut app, 120, 34);
        let elapsed = started.elapsed();
        let coast = buf
            .content()
            .iter()
            .filter(|c| c.fg == Color::DarkGray && braille(c))
            .count();
        assert!(coast > 50, "only {coast} cells of coastline");
        assert!(elapsed.as_millis() < 500, "a frame took {elapsed:?}");
        show(&render(&mut app, 120, 34));
    }

    #[test]
    fn zoom_has_limits() {
        let mut app = app(Difficulty::Medium);
        render(&mut app, 120, 34);
        for _ in 0..40 {
            app.zoom(1.0 / ZOOM_STEP, None);
        }
        let (_, (_, _, y0, y1)) = app.current_view().unwrap();
        assert!(y1 - y0 >= MIN_VIEW_LAT);
        for _ in 0..40 {
            app.zoom(ZOOM_STEP, None);
        }
        let (_, (_, _, y0, y1)) = app.current_view().unwrap();
        let (_, fitted) = app.map_frame.get().unwrap();
        assert!(y1 - y0 <= MAX_VIEW_LAT.max(fitted.3 - fitted.2) + 1e-9);
    }

    #[test]
    fn going_back_keeps_the_choice() {
        // Find a start with a few options, choose one further down, fly, come back.
        let mut app = (0..50)
            .map(|_| app(Difficulty::Medium))
            .find(|a| a.game.options().len() >= 3)
            .expect("a start with three options");
        app.move_selection(true);
        app.move_selection(true);
        let chosen = app.game.options()[2].airport;
        app.submit();
        assert_eq!(app.game.current(), chosen);
        app.go_back();
        let options = app.game.options();
        let selected = app.options_state.selected().unwrap();
        assert_eq!(options[selected].airport, chosen, "the choice was lost");
        assert_eq!(selected, 2);
    }

    #[test]
    fn difficulty_cycle_skips_custom_settings_that_cant_play() {
        let mut app = app(Difficulty::Pilot);
        app.game.set_custom(Rules {
            fixed_route: true,
            start: Some(*b"QQQ"),
            ..Rules::default()
        });
        app.cycle_difficulty();
        assert_eq!(
            app.game.difficulty(),
            Difficulty::Easy,
            "stuck before Custom"
        );
        assert!(
            app.message.0.contains("Skipped Custom"),
            "{}",
            app.message.0
        );
    }

    #[test]
    fn map_labels_never_overlap() {
        let mut labels = MapLabels::new((0.0, 10.0, 0.0, 10.0), 20, 10);
        let style = Style::default();
        // Six airports at nearly the same point.
        for (i, code) in ["AAA", "BBB", "CCC", "DDD", "EEE", "FFF"]
            .iter()
            .enumerate()
        {
            labels.add(5.0 + i as f64 * 0.01, 5.0, code, style, Color::Gray);
        }
        let mut cells = std::collections::HashSet::new();
        for (x, y, text) in &labels.placed {
            let (col, row) = labels.cell(*x, *y).unwrap();
            // Each label and the blank cell after it.
            for c in col..=col + text.content.len() as i32 {
                assert!(cells.insert((c, row)), "labels touch at ({c}, {row})");
            }
        }
        assert!(
            labels.placed.len() >= 4,
            "only {} placed",
            labels.placed.len()
        );
        // The first keeps its airport's own cell.
        let (col, row) = labels.cell(labels.placed[0].0, labels.placed[0].1).unwrap();
        assert_eq!((col, row), labels.cell(5.0, 5.0).unwrap());
    }

    #[test]
    fn hard_and_pilot_have_no_map() {
        for d in [Difficulty::Hard, Difficulty::Pilot] {
            let mut app = app(d);
            let screen = render(&mut app, 100, 30);
            show(&screen);
            assert!(!screen.contains(" Map "));
            assert!(screen.contains("Options"));
        }
    }

    #[test]
    fn flying_and_going_back() {
        let mut app = app(Difficulty::Medium);
        let first = app.game.options()[0].code;
        app.submit();
        assert_eq!(app.game.network().code(app.game.current()), first);
        let screen = render(&mut app, 100, 30);
        show(&screen);
        assert!(screen.contains(&format!("Flew to {first}")));
        app.go_back();
        assert_eq!(app.game.current(), app.game.origin());
        app.go_back();
        assert!(app.message.0.contains("nowhere to go back"));
    }

    fn braille(c: &ratatui::buffer::Cell) -> bool {
        c.symbol()
            .chars()
            .all(|ch| ('\u{2801}'..='\u{28FF}').contains(&ch))
    }

    #[test]
    fn selected_hop_is_previewed_on_the_map() {
        let mut app = app(Difficulty::Easy);
        let buf = buffer(&mut app, 120, 34);
        let orange = buf
            .content()
            .iter()
            .filter(|c| c.fg == COLOR_WARNING && braille(c))
            .count();
        assert!(orange > 0, "no line to the selected airport");
    }

    #[test]
    fn curved_legs_follow_great_circles() {
        // Straight: just the ends.
        assert_eq!(
            leg_points((-73.8, 40.6), (-0.5, 51.5), false),
            vec![(-73.8, 40.6), (-0.5, 51.5)]
        );
        // New York to London bows north, past both, and ends where it should.
        let curve = leg_points((-73.8, 40.6), (-0.5, 51.5), true);
        let (first, last) = (curve[0], curve[curve.len() - 1]);
        assert_eq!(first, (-73.8, 40.6));
        assert!((last.0 - -0.5).abs() < 1e-9 && (last.1 - 51.5).abs() < 1e-9);
        let north = curve.iter().map(|p| p.1).fold(f64::MIN, f64::max);
        assert!(north > 53.0, "{north}");
        // Tokyo to Los Angeles crosses the Pacific, longitudes running on past
        // 180° without a jump, and bows north too.
        let curve = leg_points((140.4, 35.8), (-118.4, 33.9), true);
        assert!(curve.windows(2).all(|w| (w[1].0 - w[0].0).abs() < 5.0));
        assert!((curve[curve.len() - 1].0 - 241.6).abs() < 1e-9);
        assert!(curve.iter().any(|p| p.1 > 45.0));
        // Opposite sides of the world have no one great circle: straight.
        assert_eq!(leg_points((0.0, 0.0), (180.0, 0.0), true).len(), 2);
    }

    #[test]
    fn curves_are_drawn_and_kept() {
        // Far north, where a great circle bows well away from a straight line.
        let text = "A\tAAA\t60\t0\nA\tABA\t60\t40\nA\tABB\t60\t60\nA\tBAA\t70\t0\n\
                    A\tBBA\t70\t45\nA\tBBB\t60\t90\n\
                    H\tAAA\tABA\t-\t-\t-\nH\tABA\tABB\t-\t-\t-\nH\tABB\tBBB\t-\t-\t-\n\
                    H\tAAA\tBAA\t-\t-\t-\nH\tBAA\tBBA\t-\t-\t-\nH\tBBA\tBBB\t-\t-\t-\n";
        let net: &'static crate::flights::data::Network = Box::leak(Box::new(
            crate::flights::data::Network::parse(text, 1).unwrap(),
        ));
        let at = |c: &str| net.index_of(c).unwrap();
        let game = FlightsGame::with_puzzle(
            net,
            at("AAA"),
            at("BBB"),
            Difficulty::Medium,
            Rules::default(),
            Units::Nm,
            None,
        )
        .unwrap();
        let mut app = FlightsApp::new(game);
        app.fly("ABA");
        // The highest row the route's blue line reaches, and its cells.
        let route = |app: &mut FlightsApp| {
            let buf = buffer(app, 120, 34);
            let cells: Vec<(u16, u16)> = (0..34u16)
                .flat_map(|y| (0..120u16).map(move |x| (x, y)))
                .filter(|&(x, y)| buf[(x, y)].fg == COLOR_ACCENT && braille(&buf[(x, y)]))
                .collect();
            cells
        };
        // One view for both, since a curve refits the map to hold it.
        app.view = Some((-20.0, 110.0, 40.0, 85.0));
        let curved = route(&mut app);
        assert!(app.game.curves(), "curves are the default");
        app.game.set_curves(false);
        let straight = route(&mut app);
        show(&render(&mut app, 120, 34));
        assert!(!straight.is_empty() && !curved.is_empty());
        assert_ne!(straight, curved);
        let top = |cells: &[(u16, u16)]| cells.iter().map(|c| c.1).min().unwrap();
        assert!(top(&curved) < top(&straight), "the curve doesn't bow north");
        // Straight lines are kept through new games and the save, which notes
        // only that, so a save without it (an older one, too) has curves.
        app.start_new_game(None);
        assert!(!app.game.curves());
        let saved = app.game.to_saved();
        assert!(!saved.curves);
        let json = serde_json::to_string(&saved).unwrap();
        assert!(json.contains("\"curves\":false"), "{json}");
        app.game.set_curves(true);
        let json = serde_json::to_string(&app.game.to_saved()).unwrap();
        assert!(!json.contains("curves"), "{json}");
        let old: SavedGame = serde_json::from_str(&json).unwrap();
        assert!(old.curves);
    }

    #[test]
    fn hops_cross_the_date_line_the_short_way() {
        // Around the antimeridian: from 170°E east to 160°W.
        let text = "A\tAAA\t0\t170\nA\tABA\t0\t-175\nA\tABB\t0\t-165\nA\tBAA\t10\t175\n\
                    A\tBBA\t10\t-170\nA\tBBB\t0\t-160\n\
                    H\tAAA\tABA\t-\t-\t-\nH\tABA\tABB\t-\t-\t-\nH\tABB\tBBB\t-\t-\t-\n\
                    H\tAAA\tBAA\t-\t-\t-\nH\tBAA\tBBA\t-\t-\t-\nH\tBBA\tBBB\t-\t-\t-\n";
        let net: &'static crate::flights::data::Network = Box::leak(Box::new(
            crate::flights::data::Network::parse(text, 1).unwrap(),
        ));
        let at = |c: &str| net.index_of(c).unwrap();
        // On the whole world, the hop leaves the right edge and comes in at the
        // left, with nothing across the middle.
        let (w, h) = (90u16, 20u16);
        let mut buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, w, h));
        let paths = vec![vec![at("AAA"), at("ABA")]];
        path_canvas(
            net,
            (-180.0, 180.0, -45.0, 45.0),
            &paths,
            COLOR_ACCENT,
            false,
        )
        .render(buf.area, &mut buf);
        let drawn = |cols: std::ops::Range<u16>| {
            cols.flat_map(|x| (0..h).map(move |y| (x, y)))
                .any(|(x, y)| braille(&buf[(x, y)]))
        };
        assert!(drawn(w - 3..w), "nothing at the right edge");
        assert!(drawn(0..3), "nothing at the left edge");
        assert!(!drawn(10..w - 10), "the hop went the long way round");

        // Playing there, the map centres on the Pacific rather than the world.
        let game = FlightsGame::with_puzzle(
            net,
            at("AAA"),
            at("BBB"),
            Difficulty::Medium,
            Rules::default(),
            Units::Nm,
            None,
        )
        .unwrap();
        let mut app = FlightsApp::new(game);
        app.fly("ABA");
        let screen = render(&mut app, 120, 34);
        show(&screen);
        let (_, (x0, x1, ..)) = app.current_view().unwrap();
        assert!(x0 < 170.0 && x1 > 200.0 && x1 - x0 < 120.0, "{x0} {x1}");
        for code in ["AAA", "ABA", "BBB"] {
            let map: String = screen.lines().flat_map(|l| l.chars().take(60)).collect();
            assert!(map.contains(code), "{code} isn't on the map:\n{screen}");
        }
    }

    #[test]
    fn giving_up_shows_both_routes() {
        // A small network where the routes part at once: the shortest heads east
        // along the equator, the player north.
        let text = "A\tAAA\t0\t0\nA\tABA\t0\t10\nA\tABB\t0\t20\nA\tBAA\t10\t0\n\
                    A\tBBA\t10\t15\nA\tBBB\t0\t30\n\
                    H\tAAA\tABA\t-\t-\t-\nH\tABA\tABB\t-\t-\t-\nH\tABB\tBBB\t-\t-\t-\n\
                    H\tAAA\tBAA\t-\t-\t-\nH\tBAA\tBBA\t-\t-\t-\nH\tBBA\tBBB\t-\t-\t-\n";
        let net: &'static crate::flights::data::Network = Box::leak(Box::new(
            crate::flights::data::Network::parse(text, 1).unwrap(),
        ));
        let at = |c: &str| net.index_of(c).unwrap();
        let game = FlightsGame::with_puzzle(
            net,
            at("AAA"),
            at("BBB"),
            Difficulty::Medium,
            Rules::default(),
            Units::Nm,
            None,
        )
        .unwrap();
        let mut app = FlightsApp::new(game);
        app.fly("BAA");
        app.trigger_give_up();
        let buf = buffer(&mut app, 120, 34);
        let count = |color: Color| {
            buf.content()
                .iter()
                .filter(|c| c.fg == color && braille(c))
                .count()
        };
        assert!(count(COLOR_POSITIVE) > 0, "the shortest route isn't drawn");
        assert!(count(COLOR_ACCENT) > 0, "the player's route isn't drawn");
        show(&render(&mut app, 120, 34));
        // The route box lists both, and where they part.
        let screen: String = buf.content().iter().map(|c| c.symbol()).collect();
        for text in ["Yours", "Shortest", "↑ you left here", "↑ went here"] {
            assert!(screen.contains(text), "no '{text}'");
        }
        assert!(app
            .message
            .0
            .contains("You left it after AAA: you flew to BAA"));
    }

    #[test]
    fn help_explains_the_controls() {
        let mut app = app(Difficulty::Medium);
        let screen = render(&mut app, 100, 30);
        for gone in ["Controls", "[Enter]", "[Bksp]"] {
            assert!(!screen.contains(gone), "the footer still has '{gone}'");
        }
        app.show_help = true;
        let screen = render(&mut app, 80, 24);
        show(&screen);
        for text in ["Controls", "→ Enter  fly", "← Bksp  back", "^S  settings"] {
            assert!(screen.contains(text), "no '{text}'");
        }
    }

    #[test]
    fn settings_switch_between_limits_and_a_route() {
        let mut app = app(Difficulty::Medium);
        app.open_settings();
        let page = app.settings.as_mut().unwrap();
        let row = |s: Setting| SETTINGS.iter().position(|&r| r == s).unwrap();
        // Random mode: Up / Down skip the start and end, on to the reset button.
        page.row = row(Setting::MaxHops);
        page.step_row(true);
        assert_eq!(SETTINGS[page.row], Setting::Reset);
        // Route mode: they skip the limits instead (a route still has
        // connections, so whether to reuse them still applies).
        page.row = row(Setting::Mode);
        page.change(true);
        assert!(page.custom.fixed_route);
        page.step_row(true);
        assert_eq!(SETTINGS[page.row], Setting::Reuse);
        page.step_row(true);
        assert_eq!(SETTINGS[page.row], Setting::Start);
        page.step_row(false);
        assert_eq!(SETTINGS[page.row], Setting::Reuse);
        let screen = render(&mut app, 100, 30);
        show(&screen);
        assert!(screen.contains("not used for a fixed route"));
    }

    #[test]
    fn settings_reuse_and_reset() {
        let mut app = app(Difficulty::Medium);
        app.open_settings();
        let row = |s: Setting| SETTINGS.iter().position(|&r| r == s).unwrap();
        let page = app.settings.as_mut().unwrap();
        page.row = row(Setting::Reuse);
        page.change(true);
        assert!(!page.custom.reuse_connections);
        assert!(!page.custom.count_is_exact());
        // Without connections, there's nothing to reuse: the row is skipped.
        page.row = row(Setting::Connections);
        page.change(false);
        assert_eq!(page.custom.max_connections, 0);
        page.step_row(true);
        assert_eq!(SETTINGS[page.row], Setting::MinHops);
        page.row = row(Setting::Units);
        page.change(true);
        let units = page.units;
        let screen = render(&mut app, 100, 30);
        show(&screen);
        for text in [
            "Reuse connections",
            "no connections to reuse",
            "[  Reset   ]",
        ] {
            assert!(screen.contains(text), "no '{text}':\n{screen}");
        }
        // A click on the reset button puts the custom settings back, but not
        // the units, and leaves the page open.
        let reset = app.settings.as_ref().unwrap().reset_area.get();
        assert!(screen
            .lines()
            .nth(reset.y as usize)
            .unwrap()
            .contains("Reset"));
        app.handle_mouse_click(reset.x + 2, reset.y);
        let page = app.settings.as_mut().unwrap();
        assert_eq!(page.custom, Rules::default());
        assert_eq!(page.units, units);
        assert_eq!(SETTINGS[page.row], Setting::Reset);
        // And so does the keyboard on it ([Enter], [←→] or Space).
        page.row = row(Setting::Places);
        page.change(true);
        page.row = row(Setting::Reset);
        page.change(true);
        assert_eq!(page.custom, Rules::default());
        // A click anywhere else still closes the page.
        app.handle_mouse_click(0, 0);
        assert!(app.settings.is_none());
    }

    #[test]
    fn settings_take_a_start_and_end() {
        let mut app = app(Difficulty::Medium);
        app.open_settings();
        let page = app.settings.as_mut().unwrap();
        page.row = SETTINGS.iter().position(|&s| s == Setting::Mode).unwrap();
        page.change(true); // a fixed route
        page.row = SETTINGS.iter().position(|&s| s == Setting::Start).unwrap();
        for c in "lax".chars() {
            page.type_char(c);
        }
        assert_eq!(page.custom.start, Some(*b"LAX"));
        page.row += 1;
        page.type_char('J');
        assert_eq!(page.custom.end, None, "a partial code counts as random");
        page.backspace();
        // LAS is one letter from LAX: not a puzzle.
        for c in "LAS".chars() {
            page.type_char(c);
        }
        let screen = render(&mut app, 100, 30);
        show(&screen);
        assert!(screen.contains("must differ in every letter"));
        app.close_settings(true);
        assert!(app.settings.is_some(), "an impossible route started a game");
        let page = app.settings.as_mut().unwrap();
        page.change(true); // back to a random end
        app.close_settings(true);
        assert!(app.settings.is_none());
        assert_eq!(app.game.start_code(), "LAX");
    }

    #[test]
    fn pilot_hides_connections() {
        let mut app = app(Difficulty::Pilot);
        // Fly a hop with connections, if there is one, and check they aren't named.
        let Some(opt) = app.game.options().into_iter().find(|o| !o.via.is_empty()) else {
            return;
        };
        app.fly(opt.code);
        let screen = render(&mut app, 100, 30);
        show(&screen);
        assert!(!screen.contains("via "));
    }

    #[test]
    fn help_fits_a_small_terminal() {
        let mut app = app(Difficulty::Medium);
        app.show_help = true;
        let screen = render(&mut app, 80, 24);
        show(&screen);
        for text in [
            "How to Play",
            "Pilot",
            "Custom",
            "puzzles",
            "OurAirports",
            "to close",
        ] {
            assert!(screen.contains(text), "help is cut off before '{text}'");
        }
        // A big terminal shows the same box, centred, with everything on it.
        let wide = render(&mut app, 130, 34);
        show(&wide);
        for text in ["Rules", "Controls", "Difficulty", "Credits", "OurAirports"] {
            assert!(wide.contains(text), "the wide help has no '{text}'");
        }
    }

    #[test]
    fn narrow_terminal_shows_warning() {
        let mut app = app(Difficulty::Medium);
        let screen = render(&mut app, 30, 20);
        assert!(screen.contains("Terminal Width Too Narrow"));
    }

    #[test]
    fn narrow_screens_stack_top_to_bottom() {
        for d in [Difficulty::Medium, Difficulty::Hard] {
            let mut app = app(d);
            let screen = render(&mut app, 44, 50);
            show(&screen);
            assert!(!screen.contains("Too Narrow"));
            let row = |text: &str| screen.lines().position(|l| l.contains(text));
            let status = row("Status").expect("no status");
            let route = row("Route").expect("no route");
            let options = row("Options").expect("no options");
            assert!(
                status < route && route < options,
                "{d}: not stacked in order"
            );
            if d == Difficulty::Medium {
                assert!(row(" Map ").unwrap() < status, "the map isn't on top");
            } else {
                assert!(row(" Map ").is_none(), "Hard has no map");
            }
            for (_, key, label) in FOOTER_ACTIONS {
                assert!(
                    screen.contains(&format!("{key} {label}")),
                    "{d}: '{label}' is cut"
                );
            }
            // The options can still be clicked: the first row flies there.
            let first = app.game.options()[0].airport;
            let area = app.options_area;
            app.handle_mouse_click(area.x + 2, area.y);
            assert_eq!(
                app.game.current(),
                first,
                "{d}: clicking an option didn't fly"
            );
        }
    }

    #[test]
    fn give_up_and_footer_click() {
        let mut app = app(Difficulty::Easy);
        app.trigger_give_up();
        let screen = render(&mut app, 120, 34);
        show(&screen);
        assert!(screen.contains("A shortest route"));
        let btn = app.footer.buttons[2];
        app.handle_mouse_click(btn.x + 1, btn.y + 1);
        assert_eq!(app.game.status(), Status::Playing);
    }

    #[test]
    fn map_bounds_keep_aspect() {
        let (x0, x1, y0, y1) = map_bounds(&[(0.0, 0.0), (0.0, 10.0)], 80, 20);
        // 80 columns by 20 rows is twice as wide as tall on screen.
        assert!(((x1 - x0) / (y1 - y0) - 2.0).abs() < 1e-9);
        // Across the antimeridian, the view is the 20° between, not the 340°.
        let (x0, x1, ..) = map_bounds(&[(0.0, -170.0), (0.0, 170.0)], 80, 20);
        assert!(x0 < 170.0 && x1 > 190.0 && x1 - x0 < 60.0, "{x0} {x1}");
        assert_eq!(wrap_lon(-170.0, 180.0), 190.0);
        assert_eq!(wrap_lon(170.0, 0.0), 170.0);
    }
}
