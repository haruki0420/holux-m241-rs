//! `m241 device ports`.
//!
//! An enumeration with a judgement attached: a bare device name answers "is
//! anything there", and the chip name answers "is that the logger". Getting the
//! second answer wrong is the difference between a five-second timeout and
//! reading the identity off the unit on the first try, so the judgement is part
//! of the output rather than left to the reader.

use crate::Result;
use crate::Session;
use crate::cli::PortsOptions;
use crate::link::{Cable, PortEntry};

/// List the serial ports, flagging the ones that look like an M-241 cable.
///
/// # Errors
///
/// The enumeration's failure when the OS cannot list ports at all. An empty list
/// is not an error: `docs/reference/02-cli.md` says so, and a script that runs
/// `device ports` before plugging anything in should not have to special-case 0.
pub fn ports(options: &PortsOptions, session: Session<'_>) -> Result<()> {
    let entries = session.links.ports()?;
    if options.json {
        return session.console.json(&document(&entries));
    }
    if entries.is_empty() {
        session.console.result(
            "no serial ports found. Plug the M-241 in with its USB cable, switch it on, and \
             run `m241 device ports` again.",
        );
        return Ok(());
    }
    session.console.result(&format!(
        "{:<22} {:<8} {:<7} {}",
        "DEVICE", "CABLE", "LIKELY", "DESCRIPTION"
    ));
    for entry in &entries {
        session.console.result(&format!(
            "{:<22} {:<8} {:<7} {}",
            entry.name,
            entry.cable.map_or("-", Cable::label),
            if entry.likely { "*" } else { "" },
            entry.description.as_deref().unwrap_or("-"),
        ));
    }
    session.console.result("");
    let candidates = candidates(&entries);
    if candidates.is_empty() {
        session.console.result(
            "No obvious M-241 cable found. The logger needs its USB cable attached and the \
             unit switched on.",
        );
    } else {
        session
            .console
            .result(&format!("candidate port(s): {}", candidates.join(", ")));
    }
    Ok(())
}

/// The ports a bare `--port auto` would consider.
#[must_use]
pub fn candidates(entries: &[PortEntry]) -> Vec<&str> {
    entries
        .iter()
        .filter(|entry| entry.likely)
        .map(|entry| entry.name.as_str())
        .collect()
}

/// The ports as the one JSON object `--json` promises.
#[must_use]
pub fn document(entries: &[PortEntry]) -> serde_json::Value {
    let ports: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            serde_json::json!({
                "name": entry.name,
                "cable": entry.cable.map(Cable::label),
                "likely": entry.likely,
                "description": entry.description,
            })
        })
        .collect();
    serde_json::json!({
        "ports": ports,
        "candidates": candidates(entries),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::testing::{AutoConfirm, FakePorts, RecordingConsole};

    fn run_ports(
        options: &PortsOptions,
        links: &FakePorts,
        console: &RecordingConsole,
    ) -> Result<()> {
        ports(
            options,
            Session::new(links, console, &AutoConfirm).with_timings(Timings::fast()),
        )
    }

    #[test]
    fn an_empty_port_list_is_success() {
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: false }, &links, &console).expect("empty is not an error");
        assert!(
            console.stdout().contains("no serial ports found"),
            "got {}",
            console.stdout()
        );
        assert_eq!(console.stderr(), "", "nothing to warn about");
    }

    #[test]
    fn a_flagged_port_names_its_chip_and_marks_the_candidate() {
        let links = FakePorts::with_ports(vec![PortEntry {
            name: String::from("/dev/ttyUSB0"),
            cable: Some(Cable::Cp210x),
            likely: true,
            description: Some(String::from("CP2102 USB to UART Bridge")),
        }]);
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: false }, &links, &console).expect("ports");
        let text = console.stdout();
        assert!(text.contains("DEVICE"), "got {text}");
        assert!(text.contains("/dev/ttyUSB0"), "got {text}");
        assert!(text.contains("CP210x"), "got {text}");
        assert!(
            text.contains("candidate port(s): /dev/ttyUSB0"),
            "got {text}"
        );
    }

    #[test]
    fn ports_json_is_one_object_with_the_candidate_list() {
        let links = FakePorts::with_ports(vec![PortEntry {
            name: String::from("COM3"),
            cable: Some(Cable::Ch340),
            likely: true,
            description: None,
        }]);
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: true }, &links, &console).expect("ports");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("the output is one JSON object");
        assert_eq!(document["ports"][0]["name"], "COM3");
        assert_eq!(document["ports"][0]["cable"], "CH340");
        assert_eq!(document["ports"][0]["description"], serde_json::Value::Null);
        assert_eq!(document["candidates"][0], "COM3");
    }

    #[test]
    fn an_unrelated_port_is_listed_but_not_flagged() {
        let links = FakePorts::with_ports(vec![PortEntry {
            name: String::from("/dev/ttyS0"),
            cable: None,
            likely: false,
            description: Some(String::from("pci")),
        }]);
        let console = RecordingConsole::default();
        run_ports(&PortsOptions { json: false }, &links, &console).expect("ports");
        let text = console.stdout();
        assert!(text.contains("/dev/ttyS0"), "got {text}");
        assert!(text.contains("No obvious M-241 cable found"), "got {text}");
    }

    #[test]
    fn the_candidate_helper_returns_only_flagged_ports() {
        let entries = vec![
            PortEntry {
                name: String::from("a"),
                cable: None,
                likely: false,
                description: None,
            },
            PortEntry {
                name: String::from("b"),
                cable: Some(Cable::Ftdi),
                likely: true,
                description: None,
            },
        ];
        assert_eq!(candidates(&entries), vec!["b"]);
        assert_eq!(document(&entries)["candidates"][0], "b");
    }
}
