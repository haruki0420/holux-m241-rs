//! `m241` — the command line interface, as a library.
//!
//! The binary in `main.rs` only parses the arguments, runs one command and turns
//! the result into an exit code. Everything else lives here, in modules that can
//! be tested without a terminal and — through the internal `link::Links` seam —
//! without a serial port.
//!
//! ```text
//!   cli      the argument grammar
//!   exit     the documented exit-code table
//!   link     finding and opening a port (the seam the tests replace)
//!   output   stdout for data, stderr for progress (the seam the tests replace)
//!   fileio   reading and writing the files a command works on
//!   cal      civil dates, so the offset flags need no date library
//!   device   ports, info, doctor
//!   fw       dump, patch and flash
//!   logcmd   download, info, read, edit, erase, geotag, kmz
//!   project  save and load a project file
//!   nmea     repair and proxy
//! ```
//!
//! `fw flash` and `log erase` are the two destructive verbs wired here, and both
//! are thin front ends to `m241-flash`: the erase, the program and the restart
//! live in that crate, and the handler only decides the order of the refusals and
//! what to print. `log erase` is wired because `m241-flash`'s
//! [`BackupProof`](m241_flash::erase::BackupProof) can only be built by handing
//! over the dump that was taken first. That check is deliberately weak — it
//! compares lengths, not contents — but it makes "erase without a copy" a line of
//! code a reviewer sees rather than an argument nobody reads.
//!
//! 🧪 The firmware write has not run against a unit. It is exercised end to end
//! against `m241-flash`'s fake DA; `docs/dev/05-verification.md` records what
//! hardware sessions exist.
//!
//! Machine output goes to stdout; progress and warnings go to stderr. Exit codes
//! are documented in `docs/reference/02-cli.md` and map one-to-one onto
//! [`exit::exit_code_of`].

// A CLI's whole job is to print. The lint that forbids `println!` exists to stop
// library code from doing it; here it is the interface.
#![allow(clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

mod cal;
mod cli;
mod device;
pub mod exit;
mod fileio;
mod fw;
mod link;
mod logcache;
mod logcmd;
mod nmea;
mod output;
mod project;

#[cfg(test)]
// A fixture library: each command's tests take the doubles they need, so a
// double can be unused while the branch does not reach it yet.
#[allow(dead_code)]
mod testing;

use std::time::Duration;

use m241_core::Result;
use m241_core::protocol::pmtk::{DEFAULT_REPLY_TIMEOUT, DEFAULT_STREAM_WINDOW, EntryTiming};

pub use cli::{Cli, Command};
pub use exit::CliError;
pub(crate) use link::Links;
pub(crate) use output::Console;

/// How long the wire commands wait.
///
/// Four numbers, not one: a reply may take seconds because the device has to
/// parse and answer, the stream probe watches for one sentence per second, the
/// `$PMTK180` hand-over has its own quiet/settle/watch sequence, and a full
/// flash log erase takes longer than a normal command reply. All are exposed so
/// a test can run the same code paths without waiting out real device timeouts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    /// How long one command's direct reply may take.
    pub reply: Duration,
    /// How long the "is NMEA streaming?" probe watches the wire.
    pub stream: Duration,
    /// The `$PMTK180` hand-over timings.
    pub entry: EntryTiming,
    /// How long `$PMTK182,6,1` (log erase) may take to acknowledge.
    pub erase: Duration,
}

impl Timings {
    /// The values a real device needs, measured: see `m241_core::protocol::pmtk`.
    #[must_use]
    pub fn real() -> Self {
        Self {
            reply: DEFAULT_REPLY_TIMEOUT,
            stream: DEFAULT_STREAM_WINDOW,
            entry: EntryTiming::real(),
            erase: m241_flash::erase::DEFAULT_REPLY_TIMEOUT,
        }
    }

    /// Short enough for a test that is proving a timeout.
    #[cfg(test)]
    #[must_use]
    pub fn fast() -> Self {
        Self {
            reply: Duration::from_millis(20),
            stream: Duration::from_millis(1),
            entry: EntryTiming::fast(),
            erase: Duration::from_millis(20),
        }
    }

    /// Generous enough that a synchronous fake-device exchange never times out
    /// under parallel CPU load.
    #[cfg(test)]
    #[must_use]
    pub fn generous() -> Self {
        Self {
            reply: Duration::from_millis(500),
            stream: Duration::from_millis(1),
            entry: EntryTiming::fast(),
            erase: Duration::from_millis(500),
        }
    }
}

impl Default for Timings {
    fn default() -> Self {
        Self::real()
    }
}

/// Interactive confirmation seam for commands that need user acknowledgement.
///
/// Making confirmation a trait lets commands be tested without a terminal and
/// keeps "wait for a human" out of the command body.
pub(crate) trait Confirm: std::fmt::Debug {
    /// Show `message` on stderr and wait for the user to acknowledge it.
    ///
    /// # Errors
    ///
    /// [`m241_core::Error::Safety`] when standard input is not an interactive
    /// terminal or reaches EOF before acknowledgement; [`m241_core::Error::Io`]
    /// when the terminal cannot be read.
    fn pause(&self, message: &str) -> Result<()>;
}

/// Everything a command needs from the outside world.
///
/// One struct rather than four parameters: every handler takes it, so adding a
/// seam later does not touch seventeen signatures. `Debug` is written by hand
/// because the traits it holds are object-safe seams: deriving would put a
/// `Debug` bound on every implementation, and a test double has nothing useful
/// to print.
#[derive(Clone, Copy)]
pub(crate) struct Session<'a> {
    /// The machine's serial ports, or a simulator in a test.
    pub links: &'a dyn Links,
    /// Where results and progress go.
    pub console: &'a dyn Console,
    /// Interactive confirmation prompt.
    pub confirm: &'a dyn Confirm,
    /// How long the wire commands wait.
    pub timings: Timings,
}

impl<'a> Session<'a> {
    /// Bundle the seams, with the timings a real device needs.
    #[must_use]
    pub fn new(links: &'a dyn Links, console: &'a dyn Console, confirm: &'a dyn Confirm) -> Self {
        Self {
            links,
            console,
            confirm,
            timings: Timings::real(),
        }
    }

    /// Use different timings.
    #[cfg(test)]
    #[must_use]
    pub fn with_timings(mut self, timings: Timings) -> Self {
        self.timings = timings;
        self
    }
}

impl std::fmt::Debug for Session<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("timings", &self.timings)
            .finish_non_exhaustive()
    }
}

/// A prompt that reads from an interactive terminal, refusing non-TTY input.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SystemConfirm;

impl Confirm for SystemConfirm {
    fn pause(&self, message: &str) -> Result<()> {
        use std::io::IsTerminal as _;

        let stdin = std::io::stdin();
        let is_terminal = stdin.is_terminal();
        pause_reader(message, is_terminal, &mut stdin.lock())
    }
}

/// Print `message` and await one line from an interactive terminal.
///
/// Refuses non-TTY input instead of silently continuing: a command that asks to
/// pause relies on a human acting before it continues.
fn pause_reader(message: &str, is_terminal: bool, reader: &mut dyn std::io::BufRead) -> Result<()> {
    eprintln!("{message}");
    if !is_terminal {
        return Err(m241_core::Error::Safety {
            reason: String::from(
                "interactive confirmation requires a terminal on standard input; \
                 re-run this command from an interactive terminal",
            ),
        });
    }
    let mut line = String::new();
    let bytes = reader
        .read_line(&mut line)
        .map_err(|source| m241_core::Error::io("<stdin>", source))?;
    if bytes == 0 {
        return Err(m241_core::Error::Safety {
            reason: String::from("interactive confirmation cancelled (end of input)"),
        });
    }
    Ok(())
}

/// Carry out one command against the real machine.
///
/// This is the entry point the binary uses. Unit tests call the crate-private
/// `run_with` helper with their own seams instead.
///
/// # Errors
///
/// Whatever the command failed with; [`exit::exit_code_of`] turns it into the
/// documented exit code.
pub fn run(command: Command) -> std::result::Result<(), CliError> {
    run_with(
        Session::new(&link::SystemLinks, &output::SystemConsole, &SystemConfirm),
        command,
    )
}

/// Carry out one command against a given session.
///
/// # Errors
///
/// As [`run`].
pub(crate) fn run_with(
    session: Session<'_>,
    command: Command,
) -> std::result::Result<(), CliError> {
    use cli::{DeviceCommand, FwCommand, LogCommand, LogProjectCommand, NmeaCommand};
    match command {
        Command::Device { command } => match command {
            DeviceCommand::Ports(options) => device::ports(&options, session)?,
            DeviceCommand::Info(options) => device::info(&options, session)?,
            DeviceCommand::Doctor(options) => device::doctor(&options, session)?,
        },
        Command::Fw { command } => match command {
            FwCommand::Dump(options) => fw::dump(&options, session)?,
            FwCommand::Patch(options) => fw::patch(&options, session)?,
            FwCommand::Flash(options) => fw::flash(&options, session)?,
            FwCommand::ProbeDa(options) => fw::probe_da(&options, session)?,
            FwCommand::DaRestart(options) => fw::da_restart(&options, session)?,
        },
        Command::Log { command } => match command {
            LogCommand::Info(options) => logcmd::info(&options, session)?,
            LogCommand::Download(options) => logcmd::download(&options, session)?,
            LogCommand::Read(options) => logcmd::read(&options, session)?,
            LogCommand::Project { command } => match command {
                LogProjectCommand::Save(options) => project::save(&options, session)?,
                LogProjectCommand::Load(options) => project::load(&options, session)?,
            },
            LogCommand::Edit(options) => logcmd::edit(&options, session)?,
            LogCommand::Erase(options) => logcmd::erase(&options, session)?,
            LogCommand::Geotag(options) => logcmd::geotag(&options, session)?,
            LogCommand::Kmz(options) => logcmd::kmz(&options, session)?,
        },
        Command::Nmea { command } => match command {
            NmeaCommand::Repair(options) => nmea::repair(&options, session)?,
            NmeaCommand::Proxy(options) => nmea::proxy(&options, session)?,
        },
    }
    Ok(())
}

/// Install the tracing subscriber, writing to stderr.
///
/// `tracing` is a dependency for diagnostics only; the command output does not
/// go through it, so a log line can never reach stdout and break a pipe.
pub fn init_tracing(verbose: bool) {
    let level = if verbose { "debug" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;
    use m241_core::protocol::pmtk::{
        DEFAULT_ENTRY_QUIET_BUDGET, DEFAULT_ENTRY_SETTLE, DEFAULT_ENTRY_WATCH,
    };

    #[test]
    fn the_real_timings_are_the_measured_ones() {
        let timings = Timings::real();
        assert_eq!(timings.reply, DEFAULT_REPLY_TIMEOUT);
        assert_eq!(timings.stream, DEFAULT_STREAM_WINDOW);
        assert_eq!(timings.entry.quiet_budget, DEFAULT_ENTRY_QUIET_BUDGET);
        assert_eq!(timings.entry.settle, DEFAULT_ENTRY_SETTLE);
        assert_eq!(timings.entry.watch, DEFAULT_ENTRY_WATCH);
        assert_eq!(timings.erase, m241_flash::erase::DEFAULT_REPLY_TIMEOUT);
        assert_eq!(timings.erase, Duration::from_secs(30));
        assert_eq!(Timings::default().reply, timings.reply);
        assert_eq!(Timings::default().stream, timings.stream);
        assert_eq!(Timings::default().erase, timings.erase);
    }

    #[test]
    fn pause_refuses_non_terminal_stdin_instead_of_silently_continuing() {
        let mut input = std::io::Cursor::new(b"\n".as_slice());
        let err = pause_reader("Press Enter to continue.", false, &mut input)
            .expect_err("non-TTY must be refused");
        assert!(matches!(err, m241_core::Error::Safety { .. }), "got {err}");
        assert_eq!(exit::exit_code_for(&err), 5);
        assert!(
            format!("{err}").contains("interactive terminal"),
            "got {err}"
        );
    }

    #[test]
    fn pause_refuses_eof_and_accepts_a_line_on_a_terminal() {
        let mut empty = std::io::Cursor::new(b"".as_slice());
        let err = pause_reader("Press Enter to continue.", true, &mut empty)
            .expect_err("EOF cancels confirmation");
        assert!(matches!(err, m241_core::Error::Safety { .. }), "got {err}");

        let mut entered = std::io::Cursor::new(b"\n".as_slice());
        pause_reader("Press Enter to continue.", true, &mut entered)
            .expect("newline acknowledges the prompt");
    }
}
