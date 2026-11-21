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
//!   fw       dump and patch (flash is a stub, see below)
//!   logcmd   download, info, read, geotag (erase is a stub, see below)
//!   nmea     repair and proxy
//! ```
//!
//! Two verbs are deliberately not wired: `fw flash` and `log erase`, the two
//! commands that change the device. They parse, so the help is complete, and they
//! fail with a message naming themselves and pointing at `m241 --help`. Adding a
//! handler for either would put a destructive command behind this binary; the
//! reference says why that is the write crate's job, not this one's.
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
mod logcmd;
mod nmea;
mod output;

#[cfg(test)]
// A fixture library: each command's tests take the doubles they need, so a
// double can be unused while the branch does not reach it yet.
#[allow(dead_code)]
mod testing;

use std::time::Duration;

use m241_core::Result;
use m241_core::protocol::pmtk::{DEFAULT_REPLY_TIMEOUT, DEFAULT_STREAM_WINDOW};

pub use cli::{Cli, Command};
pub use exit::CliError;
pub(crate) use link::Links;
pub(crate) use output::Console;

/// How long the wire commands wait.
///
/// Two numbers, not one: a reply may take seconds because the device has to
/// parse and answer, while the stream probe watches for one sentence per second.
/// Both are exposed so a test can run the same code paths without spending five
/// seconds proving a timeout.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timings {
    /// How long one command's direct reply may take.
    pub reply: Duration,
    /// How long the "is NMEA streaming?" probe watches the wire.
    pub stream: Duration,
}

impl Timings {
    /// The values a real device needs, measured: see `m241_core::protocol::pmtk`.
    #[must_use]
    pub fn real() -> Self {
        Self {
            reply: DEFAULT_REPLY_TIMEOUT,
            stream: DEFAULT_STREAM_WINDOW,
        }
    }

    /// Short enough for a test that is proving a timeout.
    #[cfg(test)]
    #[must_use]
    pub fn fast() -> Self {
        Self {
            reply: Duration::from_millis(20),
            stream: Duration::from_millis(1),
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
/// seam later does not touch thirteen signatures. `Debug` is written by hand
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
    use cli::{DeviceCommand, FwCommand, LogCommand, NmeaCommand};
    match command {
        Command::Device { command } => match command {
            DeviceCommand::Ports(options) => device::ports(&options, session)?,
            DeviceCommand::Info(options) => device::info(&options, session)?,
            DeviceCommand::Doctor(options) => device::doctor(&options, session)?,
        },
        Command::Fw { command } => match command {
            FwCommand::Dump(options) => fw::dump(&options, session)?,
            FwCommand::Patch(options) => fw::patch(&options, session)?,
            FwCommand::Flash => return Err(not_implemented("fw flash").into()),
            FwCommand::ProbeDa(options) => fw::probe_da(&options, session)?,
        },
        Command::Log { command } => match command {
            LogCommand::Info(options) => logcmd::info(&options, session)?,
            LogCommand::Download(options) => logcmd::download(&options, session)?,
            LogCommand::Read(options) => logcmd::read(&options, session)?,
            LogCommand::Erase => return Err(not_implemented("log erase").into()),
            LogCommand::Geotag(options) => logcmd::geotag(&options, session)?,
        },
        Command::Nmea { command } => match command {
            NmeaCommand::Repair(options) => nmea::repair(&options, session)?,
            NmeaCommand::Proxy(options) => nmea::proxy(&options, session)?,
        },
    }
    Ok(())
}

/// The error for a command this build parses but cannot carry out yet.
///
/// The message names the command the user actually typed and says where to find
/// one that works. "not implemented yet" on its own leaves them with no next
/// step, and naming internal work items tells them nothing they can act on.
fn not_implemented(command: &str) -> m241_core::Error {
    m241_core::Error::State(format!(
        "`m241 {command}` is not implemented in this build; \
         see `m241 --help` for the commands that work"
    ))
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
    use super::testing::{AutoConfirm, FakePorts, RecordingConsole};
    use super::*;

    /// Parse a command line, so the stub list is built the way a user builds it.
    fn parsed(args: &[&str]) -> Command {
        use clap::Parser as _;

        let mut argv = vec!["m241"];
        argv.extend_from_slice(args);
        Cli::try_parse_from(argv)
            .expect("the grammar accepts this command line")
            .command
    }

    /// Every command this build still stubs, with the name its message must
    /// carry.
    ///
    /// One entry leaves this list with each commit that wires a handler, so the
    /// list is this build's own record of how far the wiring got. The command
    /// lines are the ones `docs/reference/02-cli.md` documents.
    fn unimplemented_commands() -> Vec<(Command, &'static str)> {
        vec![
            (parsed(&["fw", "flash"]), "fw flash"),
            (parsed(&["log", "erase"]), "log erase"),
        ]
    }

    #[test]
    fn an_unimplemented_command_names_itself_and_a_next_step() {
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let session = Session::new(&links, &console, &AutoConfirm);
        for (command, name) in unimplemented_commands() {
            let err = run_with(session, command).expect_err("a stub command must fail");
            let text = err.to_string();
            assert!(
                text.contains(&format!("`m241 {name}`")),
                "{name}: message does not name the command: {text}"
            );
            assert!(
                text.contains("not implemented in this build"),
                "{name}: does not say what is missing: {text}"
            );
            assert!(
                text.contains("`m241 --help`"),
                "{name}: no next step for the user: {text}"
            );
            assert!(
                !text.contains("vendor") && !text.contains("未実装"),
                "{name}: internal work item, or not in the language of the rest of the CLI: {text}"
            );
        }
    }

    #[test]
    fn the_stub_list_records_how_far_the_wiring_got() {
        // Pins the set: a command that gains an implementation must leave this
        // list, so a half-wired command cannot stay "stubbed but advertised".
        let names: Vec<&str> = unimplemented_commands()
            .iter()
            .map(|(_command, name)| *name)
            .collect();
        assert_eq!(names, vec!["fw flash", "log erase"]);
    }

    #[test]
    fn the_real_timings_are_the_measured_ones() {
        let timings = Timings::real();
        assert_eq!(timings.reply, DEFAULT_REPLY_TIMEOUT);
        assert_eq!(timings.stream, DEFAULT_STREAM_WINDOW);
        assert_eq!(Timings::default().reply, timings.reply);
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
