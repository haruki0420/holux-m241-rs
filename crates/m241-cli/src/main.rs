//! `m241` — the command line interface.
//!
//! This file is the whole binary: parse the arguments, run one command, map the
//! result onto a documented exit code. The commands themselves are in the
//! library beside it, so they can be tested without a terminal.
//!
//! ```text
//!   m241 device  ports | info | doctor
//!   m241 fw      dump  | patch | flash
//!   m241 log     info  | download | read | erase | geotag
//!   m241 nmea    repair | proxy
//! ```

// The CLI's job is to print; see the note in `lib.rs`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use clap::Parser;
use m241_cli::{Cli, exit, run};

// `m241-flash` is a dependency of this package because `fw flash` (the Download
// Agent) and `log erase` (`$PMTK182,6`) write to the device. The library wires
// `fw flash`; `log erase` is still a stub in `m241-cli/src/lib.rs`.

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // `--help` and `--version` belong on stdout with exit 0; a bad
            // argument belongs on stderr with exit 2. clap already decides which
            // is which, so this only maps its decision onto the table.
            let _ = err.print();
            return ExitCode::from(exit::exit_code_for_parse_error(&err));
        }
    };
    m241_cli::init_tracing(cli.verbose);
    let result = run(cli.command);
    if let Err(err) = &result {
        eprintln!("m241: error: {err}");
        if let Some(hint) = err.hint() {
            eprintln!("hint: {hint}");
        }
    }
    ExitCode::from(exit::exit_code_of(&result))
}
