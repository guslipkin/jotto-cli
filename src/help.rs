//! The help and credits page every game's TUI shows on `?`: one look for all of
//! them, built from sections of rules, keys, levels and credits.
//!
//! Each page is written to fit whole in an 80x24 terminal in one column, and the
//! box is sized to its content, so it looks the same on a big terminal rather
//! than stretching. A terminal too short for it gets two columns side by side
//! when it's wide enough.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::{black_on, bold, COLOR_ACCENT};

/// One line of a help section.
pub(crate) enum HelpLine {
    /// A bullet point, already styled.
    Bullet(Line<'static>),
    /// Keys and what they do, several to a line: `[Enter] guess  [Tab] hint`.
    Keys(Vec<(&'static str, &'static str)>),
    /// A level or other named row, its name and description in aligned columns
    /// and a note (a count, say) at the right.
    Row {
        name: String,
        desc: String,
        note: String,
    },
    /// A credit: what, then who or where.
    Credit(&'static str, String),
}

impl HelpLine {
    /// A bullet point of `text`.
    pub(crate) fn bullet(text: impl Into<Line<'static>>) -> Self {
        Self::Bullet(text.into())
    }

    /// The games' author's credit.
    pub(crate) fn author() -> Self {
        Self::Credit("By", crate::text::AUTHOR.to_string())
    }
}

/// A titled group of help lines.
pub(crate) struct HelpSection {
    pub title: &'static str,
    pub lines: Vec<HelpLine>,
}

/// A whole help page.
pub(crate) struct HelpPage {
    /// The game's name, for the frame and the heading ("Flights").
    pub game: &'static str,
    /// What the game is, in a line.
    pub tagline: String,
    pub sections: Vec<HelpSection>,
}

/// A key as a chip: dark text on the accent colour.
fn key_chip(key: &str) -> Span<'static> {
    Span::styled(format!(" {key} "), black_on(COLOR_ACCENT))
}

/// Indent of a row's name and of a credit, and of a bullet's text.
const ROW_INDENT: usize = 3;
/// Gap between a row's name column and its description, or a credit's two parts.
const COLUMN_GAP: usize = 2;

/// Widths of a section's shared columns: its rows' names and its credits' first
/// parts, so each lines up down the section. Used both to lay a section out and
/// to measure it, so the two always agree.
fn column_widths(section: &HelpSection) -> (usize, usize) {
    let widest =
        |f: fn(&HelpLine) -> Option<usize>| section.lines.iter().filter_map(f).max().unwrap_or(0);
    (
        widest(|l| match l {
            HelpLine::Row { name, .. } => Some(name.chars().count()),
            _ => None,
        }),
        widest(|l| match l {
            HelpLine::Credit(what, _) => Some(what.chars().count()),
            _ => None,
        }),
    )
}

/// Lays out one section for a column `width` cells wide.
fn section_lines(section: &HelpSection, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    let heading = bold(COLOR_ACCENT);
    let rule = width.saturating_sub(section.title.chars().count() + 2);
    let mut lines = vec![Line::from(vec![
        Span::styled(section.title.to_string(), heading),
        Span::raw(" "),
        Span::styled("─".repeat(rule), Style::default().fg(Color::DarkGray)),
    ])];
    let (name_width, credit_width) = column_widths(section);
    for line in &section.lines {
        lines.push(match line {
            HelpLine::Bullet(text) => {
                let mut spans = vec![Span::styled(" • ", Style::default().fg(COLOR_ACCENT))];
                spans.extend(text.spans.iter().cloned());
                Line::from(spans)
            }
            HelpLine::Keys(keys) => {
                let mut spans = vec![Span::raw(" ")];
                for (i, (key, what)) in keys.iter().enumerate() {
                    if i > 0 {
                        spans.push(Span::raw("  "));
                    }
                    spans.push(key_chip(key));
                    spans.push(Span::raw(format!(" {what}")));
                }
                Line::from(spans)
            }
            HelpLine::Row { name, desc, note } => {
                // The note sits at the right edge; the description takes what's left.
                let used = ROW_INDENT + name_width + COLUMN_GAP + note.chars().count() + 1;
                let room = width.saturating_sub(used);
                let desc: String = if desc.chars().count() > room {
                    let cut: String = desc.chars().take(room.saturating_sub(1)).collect();
                    format!("{cut}…")
                } else {
                    format!("{desc:<room$}")
                };
                Line::from(vec![
                    Span::raw(" ".repeat(ROW_INDENT)),
                    Span::styled(format!("{name:<name_width$}"), bold(Color::White)),
                    Span::raw(" ".repeat(COLUMN_GAP)),
                    Span::raw(desc),
                    Span::raw(" "),
                    Span::styled(note.clone(), Style::default().fg(Color::Gray)),
                ])
            }
            HelpLine::Credit(what, who) => Line::from(vec![
                Span::styled(
                    format!(
                        "{}{what:<credit_width$}{}",
                        " ".repeat(ROW_INDENT),
                        " ".repeat(COLUMN_GAP)
                    ),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw(who.clone()),
            ]),
        });
    }
    lines
}

/// The width a section needs to show every line whole.
fn natural_width(section: &HelpSection) -> usize {
    let (name_width, credit_width) = column_widths(section);
    section
        .lines
        .iter()
        .map(|line| match line {
            HelpLine::Bullet(text) => 3 + text.width(),
            HelpLine::Keys(keys) => {
                1 + keys
                    .iter()
                    .map(|(k, w)| k.chars().count() + 2 + 1 + w.chars().count())
                    .sum::<usize>()
                    + 2 * keys.len().saturating_sub(1)
            }
            HelpLine::Row { desc, note, .. } => {
                ROW_INDENT
                    + name_width
                    + COLUMN_GAP
                    + desc.chars().count()
                    + 1
                    + note.chars().count()
            }
            HelpLine::Credit(_, who) => {
                ROW_INDENT + credit_width + COLUMN_GAP + who.chars().count()
            }
        })
        .chain([section.title.chars().count() + 4])
        .max()
        .unwrap_or(0)
}

/// Splits sections into two columns of about equal height, keeping their order.
fn split_columns(heights: &[usize]) -> usize {
    let total: usize = heights.iter().sum();
    let mut left = 0;
    for (i, h) in heights.iter().enumerate() {
        if left + h / 2 >= total / 2 {
            return i.max(1);
        }
        left += h;
    }
    heights.len()
}

/// Draws `page` as a modal centred over the game, sized to its content: one
/// column when it fits the terminal's height, else two side by side when every
/// line fits whole in half the width.
pub(crate) fn draw_help_page(f: &mut Frame, page: &HelpPage) {
    let screen = f.area();
    let content_width = page
        .sections
        .iter()
        .map(natural_width)
        // "How to Play {game}" and "  {tagline}".
        .chain([12 + page.game.chars().count() + 2 + page.tagline.chars().count()])
        .max()
        .unwrap_or(0) as u16;
    let lines: u16 = page.sections.iter().map(|s| s.lines.len() as u16 + 1).sum();
    let heights: Vec<usize> = page.sections.iter().map(|s| s.lines.len() + 1).collect();
    let split = split_columns(&heights);
    let tallest_half = heights[..split]
        .iter()
        .sum::<usize>()
        .max(heights[split..].iter().sum()) as u16;
    // Borders, a cell of margin each side, and the heading line.
    let frame = |w: u16, h: u16| (w + 4, h + 3);
    let (one_w, one_h) = frame(content_width, lines);
    let (two_w, two_h) = frame(content_width * 2 + 2, tallest_half);
    let two = one_h > screen.height && two_w <= screen.width && page.sections.len() > 1;
    let (w, mut h) = if two { (two_w, two_h) } else { (one_w, one_h) };
    // Narrower than its content (a phone held upright, say), the page is as wide
    // as the screen and its long lines wrap, so it grows taller to fit them.
    let wraps = !two && w > screen.width;
    let header_cells = 14 + page.game.chars().count() + page.tagline.chars().count();
    let mut header_rows = 1;
    if wraps {
        let inner = usize::from(screen.width.saturating_sub(4).max(1));
        let body: usize = page
            .sections
            .iter()
            .flat_map(|s| section_lines(s, inner as u16))
            .map(|l| l.width().max(1).div_ceil(inner))
            .sum();
        header_rows = header_cells.div_ceil(inner).max(1);
        h = (body + header_rows) as u16 + 2;
    }
    let area = Rect {
        x: screen.x + screen.width.saturating_sub(w) / 2,
        y: screen.y + screen.height.saturating_sub(h) / 2,
        width: w.min(screen.width),
        height: h.min(screen.height),
    };
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(COLOR_ACCENT))
        .title(Line::from(format!(" {} · Help & Credits ", page.game)).centered())
        .title_bottom(
            Line::from(Span::styled(
                " Click anywhere or press [Esc] / [Enter] to close ",
                Style::default().fg(Color::Gray),
            ))
            .centered(),
        );
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 2 || inner.height < 2 {
        return;
    }
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width - 2,
        ..inner
    };

    let header = Line::from(vec![
        Span::styled(format!("How to Play {}", page.game), bold(Color::White)),
        Span::styled(
            format!("  {}", page.tagline),
            Style::default().fg(Color::Gray),
        ),
    ]);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_rows as u16), Constraint::Min(1)])
        .split(inner);
    f.render_widget(Paragraph::new(header).wrap(Wrap { trim: true }), rows[0]);

    let column = |sections: &[HelpSection], width: u16| -> Vec<Line<'static>> {
        sections
            .iter()
            .flat_map(|s| section_lines(s, width))
            .collect()
    };
    if two {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio(1, 2),
                Constraint::Length(2),
                Constraint::Ratio(1, 2),
            ])
            .split(rows[1]);
        f.render_widget(
            Paragraph::new(column(&page.sections[..split], cols[0].width)),
            cols[0],
        );
        f.render_widget(
            Paragraph::new(column(&page.sections[split..], cols[2].width)),
            cols[2],
        );
    } else {
        let body = Paragraph::new(column(&page.sections, rows[1].width));
        let body = if wraps {
            body.wrap(Wrap { trim: false })
        } else {
            body
        };
        f.render_widget(body, rows[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{render_buffer, screen_text};

    #[test]
    fn a_long_tagline_fits_its_box() {
        let tagline = "a tagline longer than any line in the sections below it";
        let page = HelpPage {
            game: "Test",
            tagline: tagline.to_string(),
            sections: vec![HelpSection {
                title: "Rules",
                lines: vec![HelpLine::Bullet(Line::from("short"))],
            }],
        };
        let screen = screen_text(&render_buffer(120, 30, |f| draw_help_page(f, &page)));
        assert!(screen.contains(tagline), "the tagline was cut");
    }

    #[test]
    fn a_narrow_screen_wraps_the_page_instead_of_cutting_it() {
        let long = "a rule long enough that it can't fit on one line of a phone's screen";
        let page = HelpPage {
            game: "Test",
            tagline: "a tagline".to_string(),
            sections: vec![
                HelpSection {
                    title: "Rules",
                    lines: vec![HelpLine::Bullet(Line::from(long))],
                },
                HelpSection {
                    title: "Credits",
                    lines: vec![HelpLine::Credit("By", "someone".to_string())],
                },
            ],
        };
        let screen = screen_text(&render_buffer(40, 30, |f| draw_help_page(f, &page)));
        // Every word of the long rule is there, and the section after it.
        for word in long.split(' ') {
            assert!(screen.contains(word), "'{word}' was cut");
        }
        assert!(screen.contains("someone"), "the page was cut short");
    }

    #[test]
    fn columns_split_near_the_middle() {
        assert_eq!(split_columns(&[5, 5, 5, 5]), 2);
        assert_eq!(split_columns(&[10, 2, 2, 2]), 1);
        assert_eq!(split_columns(&[2, 2, 2, 10]), 3);
        assert_eq!(split_columns(&[4]), 1);
    }

    #[test]
    fn rows_align_and_fit() {
        let section = HelpSection {
            title: "Levels",
            lines: vec![
                HelpLine::Row {
                    name: "Easy".into(),
                    desc: "a long description that will not fit in the space".into(),
                    note: "12 words".into(),
                },
                HelpLine::Row {
                    name: "Scrabble".into(),
                    desc: "short".into(),
                    note: "9 words".into(),
                },
            ],
        };
        let lines = section_lines(&section, 40);
        for line in &lines {
            assert!(line.width() <= 40, "{line:?} is {} wide", line.width());
        }
        let text = |l: &Line| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        // Names share a column, so both descriptions start at the same place.
        assert_eq!(
            text(&lines[1]).find("a long"),
            text(&lines[2]).find("short")
        );
        assert!(text(&lines[1]).contains('…'));
        assert!(text(&lines[1]).ends_with("12 words"));
    }
}
