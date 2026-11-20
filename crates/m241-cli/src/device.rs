//! `m241 device ports` and `m241 device info`.
//!
//! `ports` is an enumeration with a judgement attached: a bare device name
//! answers "is anything there", and the chip name answers "is that the logger".
//! Getting the second answer wrong is the difference between a five-second
//! timeout and reading the identity off the unit on the first try, so the
//! judgement is part of the output rather than left to the reader.
//!
//! `info` is one request/reply round trip, and the only command whose whole job
//! is to prove the wire works. It is also where the release string comes from:
//! the patch path does not trust that string — it decides on the image's bytes —
//! so an unknown release is a warning here, never a refusal.

use m241_core::protocol::pmtk::{NMEA_BAUD, PmtkClient};

use crate::Result;
use crate::Session;
use crate::cli::{InfoOptions, PortsOptions};
use crate::link::{self, Cable, PortEntry};

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

/// Ask the unit who it is: `$PMTK605` → `$PMTK705`.
///
/// # Errors
///
/// [`m241_core::Error::Port`] when the port cannot be opened or no plausible
/// port exists, [`m241_core::Error::Timeout`] when the unit does not answer,
/// [`m241_core::Error::Nmea`] when the reply is malformed.
pub fn info(options: &InfoOptions, session: Session<'_>) -> Result<()> {
    let port = link::resolve(session.links, &options.port)?;
    let transport = session.links.open(&port, NMEA_BAUD)?;
    let mut client = PmtkClient::new(transport).with_reply_timeout(session.timings.reply);
    let identity = client.request_identity()?;

    if !is_known_release(&identity.release) {
        session.console.warn(&format!(
            "the release {:?} is not one this build knows; the patch path decides on the \
             image's bytes rather than on this string",
            identity.release
        ));
    }

    if options.json {
        return session.console.json(&serde_json::json!({
            "port": port,
            "release": identity.release,
            "build": identity.build,
            "model": identity.model,
            "sdk": identity.sdk,
            "known_release": is_known_release(&identity.release),
        }));
    }
    session.console.result(&format!("{:<10}: {port}", "port"));
    session
        .console
        .result(&format!("{:<10}: {}", "release", identity.release));
    session
        .console
        .result(&format!("{:<10}: {}", "build", identity.build));
    session
        .console
        .result(&format!("{:<10}: {}", "model", identity.model));
    session.console.result(&format!(
        "{:<10}: {}",
        "sdk",
        identity.sdk.as_deref().unwrap_or("-")
    ));
    Ok(())
}

/// Whether a release string names a firmware family this build knows.
///
/// A warning, never a refusal: a unit that reports an unknown release may still
/// be repairable, and the patch path decides on the image rather than on the
/// string.
#[must_use]
pub fn is_known_release(release: &str) -> bool {
    release.starts_with("B-core_1.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Timings;
    use crate::testing::{AutoConfirm, FAKE_PORT, FakePorts, RecordingConsole};
    use m241_core::protocol::simulator::FakeDevice;

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

    fn run_info(
        options: &InfoOptions,
        links: &FakePorts,
        console: &RecordingConsole,
    ) -> Result<()> {
        info(
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
    fn info_reads_the_identity_the_simulated_unit_reports() {
        let links = FakePorts::single(FakeDevice::new);
        let console = RecordingConsole::default();
        run_info(&InfoOptions::default(), &links, &console).expect("identity");
        let text = console.stdout();
        assert!(text.contains("B-core_1.20"), "got {text}");
        assert!(text.contains("01017-00F"), "got {text}");
        assert!(text.contains("0043"), "got {text}");
    }
    #[test]
    fn info_json_is_one_object() {
        let links = FakePorts::single(|| {
            FakeDevice::with_identity("B-core_1.20", "0043", "01017-00F", "1.0")
        });
        let console = RecordingConsole::default();
        run_info(
            &InfoOptions {
                json: true,
                ..InfoOptions::default()
            },
            &links,
            &console,
        )
        .expect("identity");
        let document: serde_json::Value =
            serde_json::from_str(&console.stdout()).expect("one JSON object");
        assert_eq!(document["release"], "B-core_1.20");
        assert_eq!(document["sdk"], "1.0");
        assert_eq!(document["known_release"], true);
        assert_eq!(document["port"], FAKE_PORT);
    }
    #[test]
    fn an_unknown_release_is_reported_but_not_refused() {
        let links =
            FakePorts::single(|| FakeDevice::with_identity("X-core_9.99", "0001", "99999-00Z", ""));
        let console = RecordingConsole::default();
        run_info(&InfoOptions::default(), &links, &console).expect("still answerable");
        assert!(
            console.stderr().contains("not one this build knows"),
            "got {}",
            console.stderr()
        );
        assert!(console.stdout().contains("X-core_9.99"));
    }
    #[test]
    fn opening_a_device_that_never_answers_is_a_hardware_error() {
        // A unit already in the BootROM answers no PMTK sentence, so the
        // identity request has nothing to read and times out.
        let links = FakePorts::single(|| FakeDevice::new().in_bootrom());
        let console = RecordingConsole::default();
        let err = run_info(&InfoOptions::default(), &links, &console).expect_err("no reply");
        assert!(matches!(err, m241_core::Error::Timeout { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 3);
    }
    #[test]
    fn an_ambiguous_auto_ports_needs_an_explicit_choice() {
        let links = FakePorts::with_ports(vec![
            PortEntry {
                name: String::from("/dev/ttyUSB0"),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: None,
            },
            PortEntry {
                name: String::from("/dev/ttyUSB1"),
                cable: Some(Cable::Cp210x),
                likely: true,
                description: None,
            },
        ]);
        let console = RecordingConsole::default();
        let err = run_info(&InfoOptions::default(), &links, &console).expect_err("ambiguous");
        assert!(matches!(err, m241_core::Error::Port { .. }), "got {err}");
        assert!(format!("{err}").contains("--port"), "got {err}");
        assert_eq!(console.stdout(), "", "nothing is printed on a failure");
    }
    #[test]
    fn the_known_release_check_accepts_the_family_and_nothing_else() {
        assert!(is_known_release("B-core_1.20"));
        assert!(!is_known_release("X-core_9.99"));
        assert!(!is_known_release(""));
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
