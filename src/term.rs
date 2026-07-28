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
}
