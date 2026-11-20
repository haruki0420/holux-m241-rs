//! `m241` — the command line interface, as a library.
//!
//! The binary in `main.rs` only parses the arguments, runs one command and turns
//! the result into an exit code. Everything else lives here, in modules that can
//! be tested without a terminal and — through the [`link::Links`] seam — without
//! a serial port.
//!
//! ```text
//!   cli      the argument grammar
//!   exit     the documented exit-code table
//!   link     finding and opening a port (the seam the tests replace)
//!   output   stdout for data, stderr for progress (the seam the tests replace)
//!   fileio   reading and writing the files a command works on
//!   cal      civil dates, so the offset flags need no date library
//! ```
//!
//! The handlers land one verb at a time; a command whose handler is not here yet
//! fails with a message naming itself and pointing at `m241 --help`.
//!
//! Machine output goes to stdout; progress and warnings go to stderr. Exit codes
//! are documented in `docs/reference/02-cli.md` and map one-to-one onto
//! [`exit::exit_code_for`].

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

pub mod cal;
pub mod cli;
pub mod device;
pub mod exit;
pub mod fileio;
pub mod fw;
pub mod link;
pub mod logcmd;
pub mod nmea;
pub mod output;

#[cfg(test)]
// A fixture library: each command's tests take the doubles they need, so a
// double can be unused while the branch does not reach it yet.
#[allow(dead_code)]
mod testing;

use std::time::Duration;

use m241_core::Result;
use m241_core::protocol::pmtk::{DEFAULT_REPLY_TIMEOUT, DEFAULT_STREAM_WINDOW};

pub use cli::{Cli, Command, DeviceCommand, FwCommand, LogCommand, NmeaCommand};
pub use link::{Cable, Links, PortEntry};
pub use output::Console;

/// How long the wire commands wait.
///
/// Two numbers, not one: a reply may take seconds because the device has to
/// parse and answer, while the stream probe watches for one sentence per second.
/// Both are exposed so a test can run the same code paths without spending five
/// seconds proving a timeout.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
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

/// The two prompts a command may need to ask.
///
/// `fw dump` is the only caller today: it stops between the two reads and asks
/// the user to power-cycle the logger. Making that a seam is what lets the
/// command be tested without a terminal, and keeps "wait for a human" out of the
/// command body.
pub trait Confirm: std::fmt::Debug {
    /// Show `message` on stderr and wait for the user to acknowledge it.
    ///
    /// # Errors
    ///
    /// [`m241_core::Error::Io`] when the terminal cannot be read.
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
pub struct Session<'a> {
    /// The machine's serial ports, or a simulator in a test.
    pub links: &'a dyn Links,
    /// Where results and progress go.
    pub console: &'a dyn Console,
    /// How the user is asked to power-cycle the logger.
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

/// A prompt that reads the terminal, and explains itself when there is not one.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemConfirm;

impl Confirm for SystemConfirm {
    fn pause(&self, message: &str) -> Result<()> {
        use std::io::{BufRead as _, IsTerminal as _};

        eprintln!("{message}");
        if !std::io::stdin().is_terminal() {
            // Piped stdin cannot answer, and blocking on a closed pipe would
            // look like a hang. Say so, then carry on: the read is read-only
            // either way, and its two passes are still compared.
            eprintln!(
                "m241: note: standard input is not a terminal, so the power-cycle prompt was \
                 not awaited; the second read starts now"
            );
            return Ok(());
        }
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|source| m241_core::Error::io("<stdin>", source))?;
        Ok(())
    }
}

/// Carry out one command against the real machine.
///
/// This is the entry point the binary uses. Tests call [`run_with`] with their
/// own seams instead.
///
/// # Errors
///
/// Whatever the command failed with; [`exit::exit_code_for`] turns it into the
/// documented exit code.
pub fn run(command: Command) -> Result<()> {
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
pub fn run_with(session: Session<'_>, command: Command) -> Result<()> {
    match command {
        Command::Device { command } => match command {
            DeviceCommand::Ports(options) => device::ports(&options, session),
            DeviceCommand::Info(options) => device::info(&options, session),
            DeviceCommand::Doctor => Err(not_implemented("device doctor")),
        },
        Command::Fw { command } => match command {
            FwCommand::Dump(options) => fw::dump(&options, session),
            FwCommand::Patch(options) => fw::patch(&options, session),
            FwCommand::Flash => Err(not_implemented("fw flash")),
        },
        Command::Log { command } => match command {
            LogCommand::Info(options) => logcmd::info(&options, session),
            LogCommand::Download(options) => logcmd::download(&options, session),
            LogCommand::Read(options) => logcmd::read(&options, session),
            LogCommand::Erase => Err(not_implemented("log erase")),
            LogCommand::Geotag => Err(not_implemented("log geotag")),
        },
        Command::Nmea { command } => match command {
            NmeaCommand::Repair(options) => nmea::repair(&options, session),
            NmeaCommand::Proxy => Err(not_implemented("nmea proxy")),
        },
    }
}

/// The error for a command this build parses but cannot carry out yet.
///
/// The message names the command the user actually typed and says where to find
/// one that works. "not implemented yet" on its own leaves them with no next
/// step, and naming internal work items tells them nothing they can act on.
fn not_implemented(command: &str) -> m241_core::Error {
    m241_core::Error::State(format!(
        "このビルドでは `m241 {command}` は未実装です。\
         `m241 --help` で使えるコマンドを確認してください。"
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
            (parsed(&["device", "doctor"]), "device doctor"),
            (parsed(&["fw", "flash"]), "fw flash"),
            (parsed(&["log", "erase"]), "log erase"),
            (parsed(&["log", "geotag"]), "log geotag"),
            (parsed(&["nmea", "proxy"]), "nmea proxy"),
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
            assert!(text.contains("未実装"), "{name}: not in Japanese: {text}");
            assert!(
                text.contains("`m241 --help`"),
                "{name}: no next step for the user: {text}"
            );
            assert!(
                !text.contains("not implemented") && !text.contains("vendor"),
                "{name}: still an internal work item: {text}"
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
        assert_eq!(
            names,
            vec![
                "device doctor",
                "fw flash",
                "log erase",
                "log geotag",
                "nmea proxy",
            ]
        );
    }

    #[test]
    fn the_real_timings_are_the_measured_ones() {
        let timings = Timings::real();
        assert_eq!(timings.reply, DEFAULT_REPLY_TIMEOUT);
        assert_eq!(timings.stream, DEFAULT_STREAM_WINDOW);
        assert_eq!(Timings::default().reply, timings.reply);
    }
}
