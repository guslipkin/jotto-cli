use std::cell::Cell as StdCell;
use std::io;

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine, Points};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Widget, Wrap,
};
use ratatui::{Frame, Terminal};

use crate::contexto::game::grouped;
use crate::flights::coast::{self, clip, Bounds, Coast};
use crate::flights::data::MAX_CONNECTIONS;
use crate::flights::game::{
    code_str, parse_code, Code, Difficulty, FlightOption, FlightsGame, FlyOutcome, Rules, Status,
    Units, MAX_HOPS, MIN_HOPS,
};
use crate::game::supports_color;
use crate::help::{draw_help_page, HelpLine, HelpPage, HelpSection};
use crate::save::Autosave;
use crate::tui::{
    centered_rect, enter_tui, notice_color, rect_contains, COLOR_ACCENT, COLOR_POSITIVE,
    COLOR_WARNING, MIN_RECOMMENDED_WIDTH,
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
const INDENT: &str = "     ";

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
    // " → LAS" + distance + via, then the indent, remark and " · " place.
    let first = 6 + DISTANCE + via;
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
/// Buttons on the footer's first row when it needs two.
const FOOTER_SPLIT: usize = 4;

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
    footer_buttons: [Rect; FOOTER_COUNT],
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

/// A row of the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
    Units,
    View,
    Places,
    Mode,
    Connections,
    Hops,
    Start,
    End,
}

const SETTINGS: [Setting; 8] = [
    Setting::Units,
    Setting::View,
    Setting::Places,
    Setting::Mode,
    Setting::Connections,
    Setting::Hops,
    Setting::Start,
    Setting::End,
];

/// The settings page: a copy of the units and custom settings being edited, kept
/// whenever the page closes, and the row selected.
#[derive(Debug, Clone)]
struct SettingsPage {
    row: usize,
    units: Units,
    custom: Rules,
    /// The start and end codes as typed so far (empty: random). Kept apart from
    /// `custom` until they're whole codes.
    start_text: String,
    end_text: String,
    /// Why the last attempt to play couldn't start.
    problem: Option<String>,
}

impl SettingsPage {
    fn new(units: Units, custom: Rules) -> Self {
        let text = |c: &Option<Code>| {
            c.as_ref()
                .map_or(String::new(), |c| code_str(c).to_string())
        };
        Self {
            row: 0,
            units,
            start_text: text(&custom.start),
            end_text: text(&custom.end),
            custom,
            problem: None,
        }
    }

    /// Whether a row applies in the custom game's mode: random puzzles use the
    /// limits, a fixed route its start and end.
    fn enabled(&self, setting: Setting) -> bool {
        match setting {
            Setting::Connections | Setting::Hops => !self.custom.fixed_route,
            Setting::Start | Setting::End => self.custom.fixed_route,
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
            Setting::View => {
                let all = Difficulty::STANDARD;
                let i = all.iter().position(|&d| d == c.view).unwrap_or(0);
                c.view = all[step(i, all.len())];
            }
            Setting::Connections => {
                c.max_connections = step(c.max_connections, MAX_CONNECTIONS + 1);
            }
            Setting::Hops => {
                // MIN_HOPS..=MAX_HOPS, then no limit, wrapping around.
                let choices: Vec<Option<usize>> =
                    (MIN_HOPS..=MAX_HOPS).map(Some).chain([None]).collect();
                let i = choices.iter().position(|&h| h == c.max_hops).unwrap_or(0);
                c.max_hops = choices[step(i, choices.len())];
            }
            Setting::Places => c.show_places = !c.show_places,
            Setting::Mode => c.fixed_route = !c.fixed_route,
            // Left and Right go back to a random airport.
            Setting::Start | Setting::End => {
                self.code_text().clear();
                self.sync_codes();
            }
        }
        self.problem = None;
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
            footer_buttons: [Rect::default(); FOOTER_COUNT],
            options_area: Rect::default(),
            message,
            show_help: false,
            settings: None,
            view: None,
            map_frame: StdCell::new(None),
            drag: None,
            should_quit: false,
            options_state: TableState::default(),
        };
        app.reset_selection();
        app
    }

    fn reset_selection(&mut self) {
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
        let route = self.game.route();
        if !self.game.rules().shows_connections() || route.len() < 2 {
            return String::new();
        }
        let net = self.game.network();
        let hop = net
            .hop(route[route.len() - 2], route[route.len() - 1])
            .expect("route legs are hops");
        via_suffix(net, &hop.via)
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
                self.message = (
                    format!(
                        "Back at {} (left {}).",
                        net.code(self.game.current()),
                        net.code(left)
                    ),
                    Color::Gray,
                );
                self.reset_selection();
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
        self.settings = Some(SettingsPage::new(self.game.units(), self.game.custom()));
    }

    /// Keeps the settings page's choices and closes it; with `play`, starts a
    /// custom game by them too (leaving the page open to say why if none fits).
    fn close_settings(&mut self, play: bool) {
        let Some(page) = self.settings.take() else {
            return;
        };
        self.game.set_units(page.units);
        self.game.set_custom(page.custom);
        if !play {
            self.message = ("Settings saved.".to_string(), COLOR_ACCENT);
            return;
        }
        if let Some(why) = page.custom.problem() {
            self.settings = Some(SettingsPage {
                problem: Some(format!("Can't play: {why}.")),
                ..page
            });
            return;
        }
        self.start_new_game(Some(Difficulty::Custom));
    }

    fn cycle_difficulty(&mut self) {
        let next = self.game.difficulty().next();
        self.start_new_game(Some(next));
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

    fn move_selection(&mut self, down: bool) {
        let len = self.game.options().len();
        if len == 0 {
            return;
        }
        let i = self.options_state.selected().unwrap_or(0);
        let next = if down {
            (i + 1).min(len - 1)
        } else {
            i.saturating_sub(1)
        };
        self.options_state.select(Some(next));
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
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.settings.is_some() {
            self.close_settings(false);
            return;
        }
        if let Some(i) = self
            .footer_buttons
            .iter()
            .position(|&r| rect_contains(r, col, row))
        {
            self.execute_footer_action(FOOTER_ACTIONS[i].0);
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
    let (_guard, mut terminal) = enter_tui()?;
    run_event_loop(&mut terminal, &mut app, autosave)
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut FlightsApp,
    autosave: &mut Autosave,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| draw_ui(f, app))?;
        // Every key and click comes back here, so this saves each change.
        autosave.sync(&app.game.to_saved());
        if app.should_quit {
            return Ok(());
        }

        match event::read()? {
            Event::Mouse(mouse) => {
                let (col, row) = (mouse.column, mouse.row);
                let free = !app.show_help && app.settings.is_none();
                // On the map, the wheel zooms and a drag pans; elsewhere the wheel
                // moves the selection and a click acts.
                let on_map = free && app.on_map(col, row);
                match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) if on_map => {
                        app.drag = app.current_view().map(|(_, view)| (col, row, view));
                    }
                    MouseEventKind::Down(MouseButton::Left) => app.handle_mouse_click(col, row),
                    MouseEventKind::Drag(MouseButton::Left) => app.drag_to(col, row),
                    MouseEventKind::Up(MouseButton::Left) => app.drag = None,
                    MouseEventKind::ScrollUp if on_map => {
                        app.zoom(1.0 / WHEEL_STEP, Some((col, row)))
                    }
                    MouseEventKind::ScrollDown if on_map => app.zoom(WHEEL_STEP, Some((col, row))),
                    MouseEventKind::ScrollUp if free => app.move_selection(false),
                    MouseEventKind::ScrollDown if free => app.move_selection(true),
                    _ => {}
                }
            }
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('c') {
                    return Ok(());
                }
                if app.show_help {
                    if matches!(
                        key.code,
                        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q')
                    ) {
                        app.show_help = false;
                    }
                    continue;
                }
                if let Some(page) = &mut app.settings {
                    let typing = page.is_code_row();
                    match key.code {
                        KeyCode::Esc => app.close_settings(false),
                        KeyCode::Char('s') if ctrl => app.close_settings(false),
                        KeyCode::Enter => app.close_settings(true),
                        KeyCode::Up => page.step_row(false),
                        KeyCode::Down => page.step_row(true),
                        // The start and end rows take a typed code.
                        KeyCode::Char(c) if typing && c.is_ascii_alphabetic() => page.type_char(c),
                        KeyCode::Backspace if typing => page.backspace(),
                        KeyCode::Char('q') if !typing => app.close_settings(false),
                        KeyCode::Left => page.change(false),
                        KeyCode::Right | KeyCode::Char(' ') => page.change(true),
                        _ => {}
                    }
                    continue;
                }
                match key.code {
                    KeyCode::Esc => return Ok(()),
                    KeyCode::Char('?') => app.show_help = true,
                    KeyCode::Char('n') if ctrl => app.start_new_game(None),
                    KeyCode::Char('d') if ctrl => app.cycle_difficulty(),
                    KeyCode::Char('t') if ctrl => app.start_daily_game(),
                    KeyCode::Char('g') if ctrl => app.trigger_give_up(),
                    KeyCode::Char('u') if ctrl => app.cycle_units(),
                    KeyCode::Char('s') if ctrl => app.open_settings(),
                    KeyCode::Up => app.move_selection(false),
                    KeyCode::Down => app.move_selection(true),
                    // Left and Right step back and forward along the route: back a
                    // hop, or fly to the selected airport. Backspace and Enter do
                    // the same, for anyone used to them.
                    KeyCode::Left | KeyCode::Backspace => app.go_back(),
                    KeyCode::Right | KeyCode::Enter => app.submit(),
                    // The map: + and - zoom, WASD pan, 0 fits it again.
                    KeyCode::Char(c) if !ctrl && app.game.rules().shows_map() => {
                        match c.to_ascii_lowercase() {
                            '+' | '=' => app.zoom(1.0 / ZOOM_STEP, None),
                            '-' | '_' => app.zoom(ZOOM_STEP, None),
                            'w' => app.pan(0.0, PAN_STEP),
                            's' => app.pan(0.0, -PAN_STEP),
                            'a' => app.pan(-PAN_STEP, 0.0),
                            'd' => app.pan(PAN_STEP, 0.0),
                            '0' => app.fit_map(),
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn draw_ui(f: &mut Frame, app: &mut FlightsApp) {
    let title = " FLIGHTS ";

    if f.area().width < MIN_RECOMMENDED_WIDTH {
        app.footer_buttons = [Rect::default(); FOOTER_COUNT];
        app.options_area = Rect::default();
        let warning = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Terminal Width Too Narrow",
                Style::default()
                    .fg(COLOR_WARNING)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(format!(
                "Current width: {} columns (minimum recommended: {MIN_RECOMMENDED_WIDTH} columns).",
                f.area().width
            )),
            Line::from("Please widen your terminal window, or run `flights --plain`."),
            Line::from(""),
            Line::from(Span::styled(
                "Press [Esc] or [Ctrl+C] to quit.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(COLOR_WARNING)),
        );
        f.render_widget(warning, f.area());
        return;
    }

    // Footer on one row when every button's "[key] label" fits, else on two.
    let one_row = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, FOOTER_COUNT as u32); FOOTER_COUNT])
        .split(f.area());
    let fits_one_row = FOOTER_ACTIONS.iter().enumerate().all(|(i, &(_, k, l))| {
        (k.len() + 1 + l.len()) as u16 <= one_row[i].width.saturating_sub(2)
    });
    let footer_height = if fits_one_row { 3 } else { 6 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(12),
            Constraint::Length(footer_height),
        ])
        .split(f.area());

    draw_header(f, app, title, chunks[0]);

    let over = app.game.status() != Status::Playing;
    let status_area = if app.game.rules().shows_map() {
        // The map takes whatever the side column doesn't need, up to half.
        let side = side_width(app.game.rules()).min(chunks[1].width / 2);
        let body = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(0), Constraint::Length(side)])
            .split(chunks[1]);
        draw_map(f, app, body[0]);
        // Once the game is over there are no options, so the routes get the room.
        let options_height = if over {
            Constraint::Length(3)
        } else {
            Constraint::Min(4)
        };
        let right = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Min(4), options_height])
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
            .constraints([Constraint::Length(6), Constraint::Min(4)])
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
    draw_status(f, app, status_area);
    draw_footer(f, app, chunks[2], fits_one_row);

    if let Some(page) = &app.settings {
        draw_settings(f, page);
    }
    if app.show_help {
        draw_help(f, &app.game);
    }
}

fn draw_header(f: &mut Frame, app: &FlightsApp, title: &'static str, area: Rect) {
    let game = &app.game;
    let mut spans = vec![Span::styled(
        title,
        Style::default()
            .fg(Color::Black)
            .bg(COLOR_ACCENT)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(date) = game.daily_date() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!(" DAILY {date} UTC "),
            Style::default()
                .fg(Color::Black)
                .bg(COLOR_POSITIVE)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let bold_white = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
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
    f.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn bold(color: Color) -> Style {
    Style::default().fg(color).add_modifier(Modifier::BOLD)
}

/// The lines for `route`, two to a stop: the code with its distance and
/// connections (as far as the game shows them), then beneath it, indented, a
/// remark (`note(i)` for the `i`th stop, an arrow pointing up at it) and the
/// place. The same while playing and once the game is over, so the box looks the
/// same and the column beside the map is sized for it (see [`side_width`]).
/// With a line of its own, the place is cut only where it would pass the box's
/// edge (`width`). `style(airport)` colours a code.
fn route_lines(
    game: &FlightsGame,
    route: &[usize],
    width: usize,
    style: &dyn Fn(usize) -> Style,
    note: &dyn Fn(usize) -> Option<Span<'static>>,
) -> Vec<Line<'static>> {
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
    lines.push(Line::from(vec![
        Span::raw("   "),
        Span::styled(net.code(route[0]).to_string(), style(route[0])),
        Span::styled("  start", gray),
    ]));
    second(&mut lines, route[0], 0);
    for (n, w) in route.windows(2).enumerate() {
        let hop = net.hop(w[0], w[1]).expect("route legs are hops");
        let changed = net
            .code(w[0])
            .bytes()
            .zip(net.code(w[1]).bytes())
            .position(|(a, b)| a != b);
        let mut spans = vec![Span::styled(" → ", gray)];
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
        lines.push(Line::from(spans));
        second(&mut lines, w[1], n + 1);
    }
    lines
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
    let mut lines = route_lines(game, game.route(), width, &code_style, &no_note);
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
        lines.extend(route_lines(
            game,
            game.route(),
            width,
            &code_style,
            &left_note,
        ));
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
        lines.extend(route_lines(game, best, width, &best_style, &here_note));
        scroll_to_end = false;
    } else if !game.at_destination() {
        let net = game.network();
        let text = net.place(game.destination());
        lines.push(Line::from(vec![
            Span::styled(" … ", Style::default().fg(Color::Gray)),
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
    let block = Block::default().borders(Borders::ALL).title(title);
    // Keep the end of the route in view when it's longer than the box.
    let inner_height = block.inner(area).height as usize;
    if scroll_to_end {
        let scroll = lines.len().saturating_sub(inner_height) as u16;
        f.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), area);
    } else {
        // After giving up, both routes from the top, long lines wrapped so every
        // stop's place shows.
        f.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(block),
            area,
        );
    }
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

fn draw_status(f: &mut Frame, app: &FlightsApp, area: Rect) {
    let (msg, color) = &app.message;
    f.render_widget(
        Paragraph::new(
            msg.lines()
                .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(*color))))
                .collect::<Vec<_>>(),
        )
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Status ")),
        area,
    );
}

/// Map bounds `(lon_min, lon_max, lat_min, lat_max)` fitting `points` with padding,
/// widened so a degree of longitude and of latitude look the same length in a
/// `cols` by `rows` area of braille (each cell is about twice as tall as wide).
/// A spread crossing the antimeridian falls back to the whole width of the world,
/// since the coastline data stops at ±180°.
fn map_bounds(points: &[(f64, f64)], cols: u16, rows: u16) -> (f64, f64, f64, f64) {
    let lat_min = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let lat_max = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let mut lon_min = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let mut lon_max = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if lon_max - lon_min > 180.0 {
        lon_min = -180.0;
        lon_max = 180.0;
    }
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

/// A canvas of just `paths` (each hop as the airports it flies through), with
/// nothing else drawn, to lay over the map.
fn path_canvas<'a>(
    net: &'a crate::flights::data::Network,
    (x0, x1, y0, y1): (f64, f64, f64, f64),
    paths: &'a [Vec<usize>],
    color: Color,
) -> Canvas<'a, impl Fn(&mut ratatui::widgets::canvas::Context) + 'a> {
    Canvas::default()
        .x_bounds([x0, x1])
        .y_bounds([y0, y1])
        .paint(move |ctx| {
            for hop in paths {
                for w in hop.windows(2) {
                    let ((alat, alon), (blat, blon)) = (net.coords(w[0]), net.coords(w[1]));
                    if let Some(((ax, ay), (bx, by))) =
                        clip((alon, alat), (blon, blat), (x0, x1, y0, y1))
                    {
                        ctx.draw(&CanvasLine::new(ax, ay, bx, by, color));
                    }
                }
            }
        })
}

fn draw_map(f: &mut Frame, app: &FlightsApp, area: Rect) {
    let game = &app.game;
    let net = game.network();
    let options = game.options();
    let selected = app
        .options_state
        .selected()
        .and_then(|i| options.get(i))
        .map(|o| o.airport);
    let route = game.route();
    let show_via = game.rules().shows_connections();

    // Each hop as the airports it flies through, connections included when shown.
    let path_of = |a: usize, b: usize| -> Vec<usize> {
        let mut path = vec![a];
        if show_via {
            path.extend(&net.hop(a, b).expect("route legs are hops").via);
        }
        path.push(b);
        path
    };
    let flown: Vec<Vec<usize>> = route.windows(2).map(|w| path_of(w[0], w[1])).collect();
    let best: Vec<Vec<usize>> = if game.status() == Status::GivenUp {
        game.best_route()
            .windows(2)
            .map(|w| path_of(w[0], w[1]))
            .collect()
    } else {
        Vec::new()
    };
    // The hop to the airport selected in the list, as it would be flown (straight,
    // when connections aren't shown).
    let preview: Vec<Vec<usize>> = selected
        .map(|to| vec![path_of(game.current(), to)])
        .unwrap_or_default();

    let mut shown: Vec<usize> = vec![game.origin(), game.destination(), game.current()];
    shown.extend(flown.iter().flatten());
    shown.extend(best.iter().flatten());
    shown.extend(preview.iter().flatten());
    shown.extend(options.iter().map(|o| o.airport));
    let points: Vec<(f64, f64)> = shown.iter().map(|&i| net.coords(i)).collect();
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
        Status::Won => vec![(&flown, COLOR_ACCENT)],
        Status::GivenUp => vec![(&flown, COLOR_ACCENT), (&best, COLOR_POSITIVE)],
    };
    let (x0, x1, y0, y1) = bounds;
    let path_canvas = |path, color| path_canvas(net, bounds, path, color);

    // Labels in priority order, each airport once.
    let mut labels = MapLabels::new(bounds, inner.width, inner.height);
    labels.beside = !finished.is_empty();
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
        let style = Style::default()
            .fg(Color::Black)
            .bg(COLOR_WARNING)
            .add_modifier(Modifier::BOLD);
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
            let mut segments = |paths: &[Vec<usize>], color: Color| {
                for path in paths {
                    for w in path.windows(2) {
                        if let Some(((ax, ay), (bx, by))) = clip(xy(w[0]), xy(w[1]), bounds) {
                            ctx.draw(&CanvasLine::new(ax, ay, bx, by, color));
                        }
                    }
                }
            };
            segments(&flown, COLOR_ACCENT);
            segments(&best, COLOR_POSITIVE);
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

fn draw_footer(f: &mut Frame, app: &mut FlightsApp, area: Rect, one_row: bool) {
    let button_areas: [Rect; FOOTER_COUNT] = if one_row {
        let row = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, FOOTER_COUNT as u32); FOOTER_COUNT])
            .split(area);
        std::array::from_fn(|i| row[i])
    } else {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Length(3)])
            .split(area);
        let top = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(1, FOOTER_SPLIT as u32); FOOTER_SPLIT])
            .split(rows[0]);
        let rest = (FOOTER_COUNT - FOOTER_SPLIT) as u32;
        let bot = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, rest); rest as usize])
            .split(rows[1]);
        std::array::from_fn(|i| {
            if i < FOOTER_SPLIT {
                top[i]
            } else {
                bot[i - FOOTER_SPLIT]
            }
        })
    };
    for (i, &(_, key_hint, label)) in FOOTER_ACTIONS.iter().enumerate() {
        app.footer_buttons[i] = button_areas[i];
        let btn = Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{key_hint} "),
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(Color::White)),
        ]))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(btn, button_areas[i]);
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
        note: grouped(custom.puzzle_count()),
    });
    let bullet = |text: &'static str| HelpLine::Bullet(Line::from(text));
    let page = HelpPage {
        game: "Flights",
        tagline: "the shortest way, one letter at a time".to_string(),
        sections: vec![
            HelpSection {
                title: "Rules",
                lines: vec![
                    bullet("Each hop changes one letter of the code: LAX → LAS → LOS."),
                    bullet("A hop needs a ticket, direct or 1 connection; every leg counts."),
                    bullet("Reach it the long way and you'll be told; go back to improve."),
                ],
            },
            HelpSection {
                title: "Controls",
                lines: vec![
                    HelpLine::Keys(vec![
                        ("↑↓", "choose"),
                        ("→ Enter", "fly"),
                        ("← Bksp", "back"),
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
                ],
            },
            HelpSection {
                title: "Difficulty · puzzles",
                lines: levels,
            },
            HelpSection {
                title: "Credits",
                lines: vec![
                    HelpLine::Credit("By", "Gus Lipkin (https://guslipkin.me)".to_string()),
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
/// between arrows, the selected row highlighted. Kept to fit an 80x24 terminal.
fn draw_settings(f: &mut Frame, page: &SettingsPage) {
    let area = centered_rect(90, 90, f.area());
    f.render_widget(Clear, area);
    let heading = bold(COLOR_ACCENT);
    let c = page.custom;
    let value = |setting: Setting| -> (String, &'static str) {
        match setting {
            Setting::Units => (page.units.label().to_string(), "distance units"),
            Setting::View => (c.view.label().to_string(), view_note(c.view)),
            Setting::Connections => (c.max_connections.to_string(), "per hop's ticket"),
            Setting::Hops => (
                c.max_hops.map_or("No limit".to_string(), |n| n.to_string()),
                "in the best route (3-20)",
            ),
            Setting::Places => (
                if c.show_places { "On" } else { "Off" }.to_string(),
                "beside each option",
            ),
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
        Setting::View => "Shows",
        Setting::Connections => "Max connections",
        Setting::Hops => "Max hops",
        Setting::Places => "City, country",
        Setting::Mode => "Puzzles",
        Setting::Start => "Start",
        Setting::End => "End",
    };
    let row_line = |i: usize, setting: Setting| {
        let selected = i == page.row;
        let (v, note) = value(setting);
        // Rows the mode doesn't use are greyed (and skipped by Up / Down).
        let note = if page.enabled(setting) {
            note
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
            Span::styled(format!("{:<16}", name(setting)), base),
            Span::styled(format!("◀ {v:^8} ▶"), base.add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {note}"), Style::default().fg(Color::DarkGray)),
        ])
    };
    let count = c.puzzle_count();
    let mut lines = vec![
        Line::from(Span::styled("Settings", heading)),
        row_line(0, SETTINGS[0]),
        Line::from(""),
        Line::from(Span::styled("Custom game", heading)),
    ];
    lines.extend(
        SETTINGS
            .iter()
            .enumerate()
            .skip(1)
            .map(|(i, &s)| row_line(i, s)),
    );
    lines.push(Line::from(""));
    lines.push(match (&page.problem, c.problem()) {
        (Some(problem), _) => Line::from(Span::styled(problem.clone(), bold(COLOR_WARNING))),
        (None, Some(why)) => Line::from(Span::styled(
            format!("Can't play these settings: {why}."),
            bold(COLOR_WARNING),
        )),
        (None, None) => Line::from(vec![
            Span::raw("Puzzles with these settings: "),
            Span::styled(grouped(count), bold(COLOR_POSITIVE)),
        ]),
    });
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
    lines.push(Line::from(Span::styled(
        format!("Levels' max hops: {}", levels.join(", ")),
        Style::default().fg(Color::DarkGray),
    )));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "[↑↓] choose  [←→] change  [Enter] play custom  [Esc] close",
        Style::default().fg(Color::Gray),
    )));
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Settings ")
                .border_style(Style::default().fg(COLOR_ACCENT)),
        ),
        area,
    );
}

/// What a view shows, for the settings page.
fn view_note(view: Difficulty) -> &'static str {
    match view {
        Difficulty::Easy => "map, distances, connections",
        Difficulty::Medium => "map and connections",
        Difficulty::Hard => "connections only",
        _ => "codes only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flights::game::Units;
    use ratatui::backend::TestBackend;

    fn render(app: &mut FlightsApp, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw_ui(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| (0..width).map(|x| buf[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
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
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw_ui(f, app)).unwrap();
        terminal.backend().buffer().clone()
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
            view: Difficulty::Hard,
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
            "Custom game",
            "Max hops",
            "Puzzles with these",
        ] {
            assert!(screen.contains(text), "no '{text}'");
        }
        let page = app.settings.as_mut().unwrap();
        let row = |s: Setting| SETTINGS.iter().position(|&r| r == s).unwrap();
        page.change(true); // units: nm -> mi
        page.row = row(Setting::View);
        page.change(true); // view: medium -> hard
        page.row = row(Setting::Connections);
        page.change(false); // connections: 1 -> 0
        page.row = row(Setting::Hops);
        page.change(false); // hops: 6 -> 5
        page.row = row(Setting::Places);
        page.change(true); // places: off
        app.close_settings(true);
        assert!(app.settings.is_none());
        assert_eq!(app.game.units(), Units::Mi);
        assert_eq!(app.game.difficulty(), Difficulty::Custom);
        let want = Rules {
            view: Difficulty::Hard,
            max_hops: Some(5),
            max_connections: 0,
            show_places: false,
            fixed_route: false,
            start: None,
            end: None,
        };
        assert_eq!(app.game.rules(), want);
        assert_eq!(app.game.custom(), want);
        // Escape keeps edits without starting a game.
        app.open_settings();
        let page = app.settings.as_mut().unwrap();
        page.row = row(Setting::Hops);
        for _ in 0..(MAX_HOPS - 5 + 1) {
            page.change(true); // 5 .. 20, then no limit
        }
        app.close_settings(false);
        assert_eq!(app.game.custom().max_hops, None);
        assert_eq!(app.game.rules().max_hops, Some(5));
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
                    .position(|l| l.contains(&format!("→ {}", net.code(stop))))
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
        // Random mode: Up / Down skip the start and end.
        page.row = row(Setting::Hops);
        page.step_row(true);
        assert_eq!(page.row, row(Setting::Hops));
        // Route mode: they skip the limits instead.
        page.row = row(Setting::Mode);
        page.change(true);
        assert!(page.custom.fixed_route);
        page.step_row(true);
        assert_eq!(SETTINGS[page.row], Setting::Start);
        page.step_row(false);
        assert_eq!(SETTINGS[page.row], Setting::Mode);
        let screen = render(&mut app, 100, 30);
        show(&screen);
        assert!(screen.contains("not used for a fixed route"));
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
        let screen = render(&mut app, 60, 20);
        assert!(screen.contains("Terminal Width Too Narrow"));
    }

    #[test]
    fn give_up_and_footer_click() {
        let mut app = app(Difficulty::Easy);
        app.trigger_give_up();
        let screen = render(&mut app, 120, 34);
        show(&screen);
        assert!(screen.contains("A shortest route"));
        let btn = app.footer_buttons[2];
        app.handle_mouse_click(btn.x + 1, btn.y + 1);
        assert_eq!(app.game.status(), Status::Playing);
    }

    #[test]
    fn map_bounds_keep_aspect() {
        let (x0, x1, y0, y1) = map_bounds(&[(0.0, 0.0), (0.0, 10.0)], 80, 20);
        // 80 columns by 20 rows is twice as wide as tall on screen.
        assert!(((x1 - x0) / (y1 - y0) - 2.0).abs() < 1e-9);
        let (x0, x1, ..) = map_bounds(&[(0.0, -170.0), (0.0, 170.0)], 80, 20);
        assert!(x0 <= -180.0 && x1 >= 180.0);
    }
}
