//! Reading terminal events, robust to escape sequences that arrive in pieces.
//!
//! A terminal reports a mouse wheel turn or an arrow key as an escape sequence
//! (`ESC [ < 65 ; 40 ; 12 M`, `ESC [ A`). When one arrives split, as happens
//! with a fast-spun wheel, a browser terminal or a network connection, its
//! `ESC` can be read on its own, and crossterm then reports the Esc key: every
//! game quits on Esc, and the rest of the sequence arrives as typed characters
//! (whose digits are `arcade`'s menu shortcuts). So an `ESC` followed at once by
//! the rest of a sequence is put back together: an arrow key or mouse report
//! becomes the event it always was, and anything else is dropped. An `ESC` with
//! nothing after it is still the Esc key.

use std::cell::RefCell;
use std::io;
use std::time::Duration;

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};

/// How long after an `ESC` the rest of a sequence may still arrive. A real Esc
/// press waits this long before it's seen; anything more would be noticeable.
const SPLIT_WAIT: Duration = Duration::from_millis(50);

thread_local! {
    /// An event read while checking an `ESC`, to hand out next.
    static PENDING: RefCell<Option<Event>> = const { RefCell::new(None) };
}

/// The next terminal event, as [`crossterm::event::read`], except that a split
/// escape sequence is dropped rather than read as Esc and typed characters.
pub fn read() -> io::Result<Event> {
    if let Some(ev) = PENDING.with(|p| p.borrow_mut().take()) {
        return Ok(ev);
    }
    loop {
        let ev = event::read()?;
        if !is_bare_esc(&ev) || !event::poll(SPLIT_WAIT)? {
            return Ok(ev);
        }
        let next = event::read()?;
        let rest = match plain_char(&next) {
            // A control sequence: parameters, then one final byte.
            Some('[') => collect(|c| ('\x40'..='\x7e').contains(&c))?,
            // A single-shift sequence: one more byte.
            Some('O') => collect(|_| true)?,
            // Esc, then some other key straight after: both are real.
            _ => {
                PENDING.with(|p| *p.borrow_mut() = Some(next));
                return Ok(ev);
            }
        };
        if let Some(ev) = decode(&rest) {
            return Ok(ev);
        }
    }
}

/// Reads the rest of a split sequence, up to and including the character for
/// which `is_final` holds. Anything that isn't a plain character ends it early,
/// and is kept to be read next.
fn collect(is_final: impl Fn(char) -> bool) -> io::Result<String> {
    let mut rest = String::new();
    while event::poll(SPLIT_WAIT)? {
        let ev = event::read()?;
        match plain_char(&ev) {
            Some(c) => {
                rest.push(c);
                if is_final(c) {
                    break;
                }
            }
            None => {
                PENDING.with(|p| *p.borrow_mut() = Some(ev));
                break;
            }
        }
    }
    Ok(rest)
}

/// The event a split sequence stood for, from what followed its `ESC [` or
/// `ESC O`: an arrow (or Home or End) key, or an SGR mouse report
/// (`<button;column;row` then `M` for a press, `m` for a release). `None` for
/// anything else, which is dropped.
fn decode(rest: &str) -> Option<Event> {
    let key = |code| Some(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    match rest {
        "A" => return key(KeyCode::Up),
        "B" => return key(KeyCode::Down),
        "C" => return key(KeyCode::Right),
        "D" => return key(KeyCode::Left),
        "H" => return key(KeyCode::Home),
        "F" => return key(KeyCode::End),
        _ => {}
    }
    let report = rest.strip_prefix('<')?;
    let (fields, pressed) = match report.strip_suffix('M') {
        Some(f) => (f, true),
        None => (report.strip_suffix('m')?, false),
    };
    let mut parts = fields.split(';').map(str::parse::<u16>);
    let (code, col, row) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    let button = match code & 0b11 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        _ => MouseButton::Right,
    };
    let kind = match (code & !0b11100, pressed) {
        (64, _) => MouseEventKind::ScrollUp,
        (65, _) => MouseEventKind::ScrollDown,
        (b, _) if b & 32 != 0 && b & 0b11 == 3 => MouseEventKind::Moved,
        (b, _) if b & 32 != 0 => MouseEventKind::Drag(button),
        (_, true) => MouseEventKind::Down(button),
        (_, false) => MouseEventKind::Up(button),
    };
    // Reports count from 1; crossterm, from 0.
    Some(Event::Mouse(MouseEvent {
        kind,
        column: col.checked_sub(1)?,
        row: row.checked_sub(1)?,
        modifiers: KeyModifiers::NONE,
    }))
}

fn is_bare_esc(ev: &Event) -> bool {
    matches!(ev, Event::Key(k) if k.code == KeyCode::Esc
        && k.kind == KeyEventKind::Press
        && k.modifiers.is_empty())
}

/// The character a key event typed, if it's a plain one (shift allowed, as for
/// a sequence's capital final byte).
fn plain_char(ev: &Event) -> Option<char> {
    match ev {
        Event::Key(k)
            if k.kind == KeyEventKind::Press && (k.modifiers - KeyModifiers::SHIFT).is_empty() =>
        {
            match k.code {
                KeyCode::Char(c) => Some(c),
                _ => None,
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse(rest: &str) -> Option<(MouseEventKind, u16, u16)> {
        match decode(rest)? {
            Event::Mouse(m) => Some((m.kind, m.column, m.row)),
            _ => None,
        }
    }

    #[test]
    fn split_sequences_decode_to_what_they_were() {
        assert_eq!(
            mouse("<65;40;12M"),
            Some((MouseEventKind::ScrollDown, 39, 11))
        );
        assert_eq!(mouse("<64;1;1M"), Some((MouseEventKind::ScrollUp, 0, 0)));
        assert_eq!(
            mouse("<0;10;5M"),
            Some((MouseEventKind::Down(MouseButton::Left), 9, 4))
        );
        assert_eq!(
            mouse("<0;10;5m"),
            Some((MouseEventKind::Up(MouseButton::Left), 9, 4))
        );
        assert_eq!(
            mouse("<32;10;5M"),
            Some((MouseEventKind::Drag(MouseButton::Left), 9, 4))
        );
        assert_eq!(mouse("<35;10;5M"), Some((MouseEventKind::Moved, 9, 4)));
        let up = decode("A").unwrap();
        assert!(matches!(up, Event::Key(k) if k.code == KeyCode::Up));
        assert!(matches!(decode("D").unwrap(), Event::Key(k) if k.code == KeyCode::Left));
        // Anything else is dropped rather than typed.
        assert!(decode("2~").is_none());
        assert!(decode("<65;40").is_none());
        assert!(decode("").is_none());
    }
}
