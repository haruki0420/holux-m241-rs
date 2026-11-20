//! The two output channels: machine-readable stdout, progress on stderr.
//!
//! `docs/reference/02-cli.md` states the split as a rule: "a script that pipes
//! stdout must not receive a progress bar". Keeping the helpers behind
//! [`Console`] makes that rule structural — a handler receives a console, and
//! the only methods that write to stdout are [`Console::result`],
//! [`Console::json`] and [`Console::text`].
//!
//! The trait also gives the tests a console that records instead of printing,
//! which is how a command's *text* can be asserted rather than eyeballed.

use m241_core::{Error, Result};
use serde_json::Value;

/// Where a command writes its result and its progress.
pub trait Console: std::fmt::Debug {
    /// One line of machine-readable output, on stdout.
    fn result(&self, line: &str);
    /// Raw machine-readable output, on stdout, exactly as given.
    ///
    /// Used for a converted track: a GPX document is one artefact, not a
    /// sequence of labelled lines, and adding a newline to it would change the
    /// file the user asked for.
    fn text(&self, text: &str);
    /// One object of machine-readable output, on stdout.
    fn json(&self, value: &Value) -> Result<()>;
    /// A progress line, on stderr. Overwrites itself while a transfer runs.
    fn progress(&self, done: usize, total: usize);
    /// Finish a progress line so the next message starts on a fresh line.
    fn progress_done(&self);
    /// A warning, on stderr. Never suppressed.
    fn warn(&self, message: &str);
    /// An informational line for the person at the terminal, on stderr.
    fn note(&self, message: &str);
}

/// Print to the terminal: results on stdout, everything else on stderr.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemConsole;

impl Console for SystemConsole {
    fn result(&self, line: &str) {
        println!("{line}");
    }

    fn text(&self, text: &str) {
        print!("{text}");
    }

    fn json(&self, value: &Value) -> Result<()> {
        let text = serde_json::to_string_pretty(value)
            .map_err(|err| Error::State(format!("could not serialise the JSON document: {err}")))?;
        println!("{text}");
        Ok(())
    }

    fn progress(&self, done: usize, total: usize) {
        // `checked_div` rather than an `if`: a zero-byte transfer is 100% done,
        // and writing that as a branch invites someone to "simplify" it into a
        // division that panics on the empty log the device reports.
        let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
        // `\r` and no newline: the line redraws in place, and stderr is where a
        // human reads it. stdout stays clean for the pipe.
        eprint!("\r{done} / {total} bytes ({percent}%)");
    }

    fn progress_done(&self) {
        eprintln!();
    }

    fn warn(&self, message: &str) {
        eprintln!("m241: warning: {message}");
    }

    fn note(&self, message: &str) {
        eprintln!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json as value;

    #[test]
    fn a_json_document_round_trips_through_the_printer() {
        // The real console prints, so what is asserted here is that it accepts
        // the shapes the handlers build rather than the bytes on stdout.
        assert!(
            SystemConsole
                .json(&value!({"release": "B-core_1.20"}))
                .is_ok()
        );
        assert!(SystemConsole.json(&value!({"ports": []})).is_ok());
    }
}
