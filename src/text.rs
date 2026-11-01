//! Wording shared by the games.

/// What any game says to a move once it has ended.
pub(crate) const GAME_OVER: &str =
    "The current game is already over. Start a new game to play again.";

/// Who wrote the games, for their credits.
pub(crate) const AUTHOR: &str = "Gus Lipkin (https://guslipkin.me)";

/// Formats a count with thousands separators ("23,191").
pub fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "1 guess", "3 guesses" and so on.
pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The "s" ending a count of `n` takes ("1 hop", "2 hops").
pub(crate) fn plural_s(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}
