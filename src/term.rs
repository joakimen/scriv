//! Terminal capability checks shared by the printing commands.
//!
//! Colour is opt-out per the `NO_COLOR` convention and is never emitted when
//! stdout is redirected, so `scriv … ls` stays pipe-safe by default.

use std::io::IsTerminal;

/// Whether stdout should carry ANSI colour: a terminal, and `NO_COLOR` unset.
pub fn stdout_color() -> bool {
    std::io::stdout().is_terminal() && !no_color()
}

/// Honour the `NO_COLOR` convention: colour is disabled when the variable is
/// present and non-empty.
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

/// Wrap `text` in an ANSI 256-colour sequence when `on`, so the same colour
/// indices the picker uses also drive plain listings.
pub fn paint(text: &str, color: u8, on: bool) -> String {
    if on {
        format!("\x1b[38;5;{color}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Paint `text`, then return to `back` rather than to the terminal default —
/// for a cell that carries its own colour inside a row that already has one.
///
/// [`paint`] ends with a reset, so using it for a cell mid-row would leave
/// everything after that cell uncoloured. Here the row's own colour is
/// re-opened instead, and the row's trailing reset still closes it.
pub fn paint_within(text: &str, color: u8, back: u8, on: bool) -> String {
    if on {
        format!("\x1b[38;5;{color}m{text}\x1b[38;5;{back}m")
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_identity_when_off() {
        assert_eq!(paint("main", 2, false), "main");
    }

    #[test]
    fn paint_wraps_when_on() {
        assert_eq!(paint("main", 2, true), "\x1b[38;5;2mmain\x1b[0m");
    }

    /// A cell painted mid-row has to hand the row's colour back, or everything
    /// after it renders in the terminal default.
    #[test]
    fn paint_within_returns_to_the_row_colour() {
        assert_eq!(
            paint_within("✓", 2, 5, true),
            "\x1b[38;5;2m✓\x1b[38;5;5m",
            "the cell must not end in a reset",
        );
        assert_eq!(paint_within("✓", 2, 5, false), "✓");
    }
}
