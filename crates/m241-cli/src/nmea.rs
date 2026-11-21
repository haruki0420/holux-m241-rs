//! `m241 nmea repair` and `m241 nmea proxy`: fix dates in NMEA streams.
//!
//! Pure text in, text out. The M-241's firmware converts GPS time with the 1980
//! epoch constant and never accounts for the second week-number rollover, so
//! every date it emits is 7168 days in the past. `repair` rewrites the date
//! field of `$GPRMC` and `$GPZDA` in a captured file or stdin, recomputes the
//! sentence checksum, and passes every other byte through unchanged. `proxy`
//! reads a live NMEA stream from the serial port and relays repaired lines to
//! stdout and/or a loopback TCP server.
//!
//! # Why the sentence is not re-rendered from scratch
//!
//! A capture can hold sentences this build has never seen, with fields it does
//! not understand. Rebuilding them would silently drop whatever the renderer did
//! not know about. Only the fields that carry a date are replaced, in place, and
//! the checksum is recomputed from the result.

use std::io::Write as _;
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs as _};
use std::path::Path;

use m241_core::protocol::port::Transport;
use m241_core::{Error, Result};

use crate::Session;
use crate::cal::{self, OffsetSpec};
use crate::cli::{ProxyOptions, RepairOptions};
use crate::fileio;
use crate::link;
use crate::output::Console;

/// Longest unterminated fragment the proxy buffers before dropping it.
///
/// This is **not** a length any real sentence approaches, and it deliberately
/// does not reuse `m241_core::protocol::pmtk::MAX_LINE_BYTES`: that constant is
/// sized for a `$PMTK182,8` log reply, which carries 0x800 bytes of data as
/// hexadecimal, and borrowing it would couple the proxy's noise guard to the
/// size of a transfer this command never performs.
///
/// The numbers are the NMEA 0183 limit, 82 characters including `$` and the
/// terminator (NMEA 0183 v4, §7 `Format`), rounded up to the next power of two
/// so a harmless overshoot of a few bytes is not treated as noise. A fragment
/// that grows past this is a baud mismatch or a line that never terminated, and
/// keeping it would only delay the resynchronisation on the next `$`.
const MAX_PROXY_LINE_BYTES: usize = 128;

/// Where the `ddmmyy` date sits in a `$GPRMC` field list that includes the
/// address at index 0.
///
/// `$GPRMC,hhmmss.ss,A,lat,N,lon,E,speed,course,ddmmyy,…`: time is `[1]`,
/// status `[2]`, latitude `[3]`, longitude `[5]`, speed `[7]`, course `[8]`, so
/// the date is `[9]`. It is the *ninth* field after the address, and index 9
/// only because the address occupies index 0 — an off-by-one here would rewrite
/// the course over the date.
const RMC_DATE: usize = 9;

/// The `hhmmss`, `dd`, `mm` and `yyyy` fields of `$GPZDA`, address at index 0.
const ZDA_DAY: usize = 2;
const ZDA_MONTH: usize = 3;
const ZDA_YEAR: usize = 4;

/// Repair a captured stream, or standard input.
///
/// # Errors
///
/// [`m241_core::Error::Io`] when the input cannot be read or the output written,
/// [`m241_core::Error::State`] when the input is not UTF-8 text.
pub fn repair(options: &RepairOptions, session: Session<'_>) -> Result<()> {
    repair_with_stdin(options, session, fileio::read_stdin_text)
}

/// Inner entry point with an injectable stdin reader so `-` can be unit-tested.
fn repair_with_stdin(
    options: &RepairOptions,
    session: Session<'_>,
    read_stdin: impl FnOnce() -> Result<String>,
) -> Result<()> {
    let source = if options.input == "-" {
        read_stdin()?
    } else {
        fileio::read_text(Path::new(&options.input))?
    };

    let mut changed = 0usize;
    let mut total = 0usize;
    let mut out = String::with_capacity(source.len() + 64);
    for line in source.split_inclusive('\n') {
        total += 1;
        let (repaired, did_change) = repair_line(line, options.offset_days);
        if did_change {
            changed += 1;
        }
        out.push_str(&repaired);
    }

    match &options.output {
        Some(path) => fileio::write_text(path, &out)?,
        None => session.console.text(&out),
    }
    if total == 0 {
        session
            .console
            .warn("the input was empty; there was no NMEA stream to repair");
    } else {
        session.console.note(&format!(
            "{changed} of {total} lines carried a date that needed fixing"
        ));
    }
    Ok(())
}

/// Repair one line, preserving its terminator.
///
/// Returns the line and whether it changed. A line that is not a date-bearing
/// sentence is returned untouched, so the caller does not have to know which
/// sentences matter.
#[must_use]
pub fn repair_line(line: &str, offset: OffsetSpec) -> (String, bool) {
    let (text, ending) = split_ending(line);
    let Some(body) = text.strip_prefix('$') else {
        return (line.to_owned(), false);
    };
    let Some((payload, _carried)) = body.split_once('*') else {
        // A sentence with no checksum cannot be repaired without inventing one,
        // and inventing one would change a line whose framing is already lost.
        return (line.to_owned(), false);
    };

    let mut fields: Vec<String> = payload.split(',').map(str::to_owned).collect();
    let Some(address) = fields.first() else {
        return (line.to_owned(), false);
    };
    let upper = address.to_ascii_uppercase();

    // The talker is deliberately not checked: `$GPRMC`, `$GNRMC` (a combined
    // multi-GNSS receiver), `$INRMC` (integrated navigation) and `$PGRMC` (a
    // proprietary sentence) all carry the date in the same position, so the
    // sentence type alone — the last three characters — decides what to repair.
    // Requiring `$GP` would silently pass a `$GNRMC` capture through unrepaired.
    let changed = if upper.ends_with("RMC") {
        repair_rmc(&mut fields, offset)
    } else if upper.ends_with("ZDA") {
        repair_zda(&mut fields, offset)
    } else {
        false
    };
    if !changed {
        return (line.to_owned(), false);
    }

    let rebuilt = fields.join(",");
    (
        format!("${rebuilt}*{:02X}{ending}", xor_checksum(&rebuilt)),
        true,
    )
}

/// Replace the `ddmmyy` field of an RMC sentence.
fn repair_rmc(fields: &mut [String], offset: OffsetSpec) -> bool {
    let Some(text) = fields.get(RMC_DATE) else {
        return false;
    };
    let Some((day, month, year)) = parse_ddmmyy(text) else {
        return false;
    };
    let days = cal::days_from_civil(year, month, day);
    let shift = offset.shift_for(days);
    if shift == 0 {
        return false;
    }
    let (year, month, day) = cal::civil_from_days(days + i64::from(shift));
    // The NMEA field holds two digits; `% 100` is what "last two digits of the
    // year" means, and `rem_euclid` keeps it correct for a year before 2000
    // instead of producing a negative remainder.
    let short_year = year.rem_euclid(100);
    let replacement = format!("{day:02}{month:02}{short_year:02}");
    let Some(slot) = fields.get_mut(RMC_DATE) else {
        return false;
    };
    *slot = replacement;
    true
}

/// Replace the day, month and year fields of a ZDA sentence.
fn repair_zda(fields: &mut [String], offset: OffsetSpec) -> bool {
    let (Some(day), Some(month), Some(year)) = (
        fields.get(ZDA_DAY),
        fields.get(ZDA_MONTH),
        fields.get(ZDA_YEAR),
    ) else {
        return false;
    };
    let (Ok(day), Ok(month), Ok(year)) = (
        day.parse::<u32>(),
        month.parse::<u32>(),
        year.parse::<i32>(),
    ) else {
        return false;
    };
    if !cal::is_valid_date(year, month, day) {
        return false;
    }
    let days = cal::days_from_civil(year, month, day);
    let shift = offset.shift_for(days);
    if shift == 0 {
        return false;
    }
    let (year, month, day) = cal::civil_from_days(days + i64::from(shift));
    let replacements = [
        format!("{day:02}"),
        format!("{month:02}"),
        format!("{year:04}"),
    ];
    for (index, text) in [ZDA_DAY, ZDA_MONTH, ZDA_YEAR].into_iter().zip(replacements) {
        let Some(slot) = fields.get_mut(index) else {
            return false;
        };
        *slot = text;
    }
    true
}

/// Parse a `ddmmyy` field. Two-digit years follow the NMEA convention: 70–99 are
/// 1970–1999, 00–69 are 2000–2069.
fn parse_ddmmyy(text: &str) -> Option<(u32, u32, i32)> {
    if text.len() != 6 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let day: u32 = text.get(0..2)?.parse().ok()?;
    let month: u32 = text.get(2..4)?.parse().ok()?;
    let short: i32 = text.get(4..6)?.parse().ok()?;
    if day == 0 || month == 0 || month > 12 || day > 31 {
        return None;
    }
    let year = if short < 70 {
        2000 + short
    } else {
        1900 + short
    };
    Some((day, month, year))
}

/// Split the line terminator from the sentence body.
fn split_ending(line: &str) -> (&str, &str) {
    let body = line.trim_end_matches(['\r', '\n']);
    let ending = line.get(body.len()..).unwrap_or("");
    (body, ending)
}

/// XOR of every byte between `$` and `*`.
fn xor_checksum(body: &str) -> u8 {
    body.as_bytes().iter().fold(0u8, |acc, byte| acc ^ byte)
}

/// Relay a live NMEA stream from the logger with repaired dates.
///
/// Defaults to relaying to stdout when `--bind` is omitted. When `--bind` is
/// given, resolves the bind address and refuses any non-loopback IP with
/// [`Error::Safety`] (exit code 5) before binding or opening the serial port,
/// unless `--allow-non-loopback` is passed.
///
/// # Errors
///
/// * [`Error::Safety`] when `--bind` resolves to a non-loopback address without
///   `--allow-non-loopback` (exit code 5);
/// * [`Error::State`] when `--bind` is invalid;
/// * [`Error::Io`] when binding or accepting on the TCP socket fails;
/// * [`Error::Port`] when the serial port cannot be resolved or opened.
pub fn proxy(options: &ProxyOptions, session: Session<'_>) -> Result<()> {
    proxy_with_hook(options, session, |_| {})
}

/// Inner entry point with a post-bind hook so tests can connect to `127.0.0.1:0`.
fn proxy_with_hook(
    options: &ProxyOptions,
    session: Session<'_>,
    on_bound: impl FnOnce(SocketAddr),
) -> Result<()> {
    let listener = match &options.bind {
        Some(spec) => Some(bind_checked_listener(spec, options.allow_non_loopback)?),
        None => None,
    };
    if let Some(listener) = &listener {
        let local_addr = listener
            .local_addr()
            .map_err(|source| Error::io("tcp listener local_addr", source))?;
        session
            .console
            .note(&format!("relaying repaired NMEA on tcp://{local_addr}"));
        on_bound(local_addr);
    }

    let relay_stdout = options.stdout || options.bind.is_none();
    let port = link::resolve(session.links, &options.port)?;
    let mut transport = session.links.open(&port, options.baud)?;
    session.console.note(&format!(
        "reading live NMEA from {port} at {} baud",
        options.baud
    ));

    proxy_transport(
        &mut transport,
        listener.as_ref(),
        relay_stdout,
        options.offset_days,
        options.max_lines,
        session.console,
    )
}

/// Resolve `bind_spec`, enforce the loopback-only safety gate, and bind a non-blocking TCP listener.
fn bind_checked_listener(bind_spec: &str, allow_non_loopback: bool) -> Result<TcpListener> {
    let addrs: Vec<SocketAddr> = bind_spec
        .to_socket_addrs()
        .map_err(|err| Error::State(format!("invalid --bind address {bind_spec:?}: {err}")))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::State(format!(
            "--bind address {bind_spec:?} resolved to no socket addresses"
        )));
    }
    if !allow_non_loopback
        && let Some(non_loopback) = addrs.iter().find(|addr| !addr.ip().is_loopback())
    {
        return Err(Error::Safety {
            reason: format!(
                "refusing to bind NMEA proxy to non-loopback address {non_loopback} ({bind_spec}); \
                 pass --allow-non-loopback to expose the stream on a network interface"
            ),
        });
    }
    let listener = TcpListener::bind(addrs.as_slice())
        .map_err(|source| Error::io(format!("bind {bind_spec}"), source))?;
    listener
        .set_nonblocking(true)
        .map_err(|source| Error::io(format!("configure {bind_spec}"), source))?;
    Ok(listener)
}

/// Read lines from `transport`, repair dates via [`repair_line`], and relay to stdout/TCP.
///
/// # When it stops
///
/// Only two things end the relay:
///
/// * `max_lines`: it returns once that many lines have been relayed
///   (`--max-lines`, already 0 → return at once);
/// * the transport fails: a read error is returned to the caller, which prints it
///   and exits 1. There is no end-of-stream for a serial port, so nothing else
///   stops it; a human ends a live relay with an interrupt.
///
/// A read that returns 0 bytes means *nothing has arrived yet*, not that the
/// stream ended — a serial port is idle between the sentences, which arrive at
/// 1 Hz. The loop yields its slice and reads again rather than treating 0 as EOF;
/// treating it as EOF would end `nmea proxy` after the first quiet millisecond.
fn proxy_transport(
    transport: &mut dyn Transport,
    listener: Option<&TcpListener>,
    relay_stdout: bool,
    offset: OffsetSpec,
    max_lines: Option<usize>,
    console: &dyn Console,
) -> Result<()> {
    if max_lines == Some(0) {
        return Ok(());
    }

    let mut clients: Vec<TcpStream> = Vec::new();
    if let Some(listener) = listener {
        accept_pending_clients(listener, &mut clients);
    }

    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 256];
    let mut lines_relayed = 0usize;

    loop {
        let count = transport.read(&mut chunk)?;
        if let Some(listener) = listener {
            accept_pending_clients(listener, &mut clients);
        }
        if count == 0 {
            std::thread::yield_now();
            continue;
        }
        if let Some(received) = chunk.get(..count) {
            pending.extend_from_slice(received);
        }

        while let Some(newline_pos) = pending.iter().position(|&byte| byte == b'\n') {
            let raw: Vec<u8> = pending.drain(..=newline_pos).collect();
            let line = String::from_utf8_lossy(&raw);
            // The count of repaired lines is deliberately dropped here: stdout
            // carries the relayed stream and nothing else, so a tally would have
            // to go to stderr, which would then grow without bound on a relay
            // that runs for hours. `nmea repair` reports the count because its
            // input is a file with an end.
            let (repaired, _changed) = repair_line(&line, offset);

            if relay_stdout {
                console.text(&repaired);
            }
            broadcast_line(&mut clients, repaired.as_bytes());

            lines_relayed += 1;
            if max_lines.is_some_and(|limit| lines_relayed >= limit) {
                return Ok(());
            }
        }

        if pending.len() >= MAX_PROXY_LINE_BYTES {
            // Drop an unterminated fragment so baud mismatch or noise cannot
            // grow the line buffer without bound.
            pending.clear();
        }
    }
}

/// Accept any queued TCP connections without blocking the serial stream loop.
fn accept_pending_clients(listener: &TcpListener, clients: &mut Vec<TcpStream>) {
    loop {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = stream.set_nonblocking(false);
                clients.push(stream);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

/// Write `bytes` to every connected TCP client, dropping clients that disconnected.
fn broadcast_line(clients: &mut Vec<TcpStream>, bytes: &[u8]) {
    clients.retain_mut(|stream| stream.write_all(bytes).is_ok() && stream.flush().is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::DeviceHandle;
    use crate::testing::{
        AutoConfirm, FAKE_PORT, FakePorts, RecordingConsole, Scratch, ScriptedLinks,
        ScriptedTransport,
    };
    use m241_core::protocol::simulator::FakeDevice;

    /// A no-fix sentence dated 2006-09-17 — the input side of the worked example.
    ///
    /// The date is a plain arithmetic input, not an observation: it is what
    /// `cal::days_from_civil(2006, 9, 17)` says, and 7,168 days are added to it
    /// by `OffsetSpec::Auto` because it falls inside the stale window. The
    /// sentence as a whole is not a recording from a unit; see
    /// `docs/dev/05-verification.md` for the one date the hardware did show.
    const BROKEN_RMC: &str = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N*7F";

    /// The expected repair of [`BROKEN_RMC`]: 2006-09-17 + 7,168 days =
    /// 2026-05-03, checksum recomputed. Calculated, not captured.
    const FIXED_RMC: &str = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,030526,,,N*74";

    #[test]
    fn the_broken_example_becomes_its_expected_7168_days_later_date() {
        // Not a hardware observation: 2006-09-17 plus one rollover cycle is
        // 2026-05-03 (see `cal::tests::the_rollover_shift_is_7168_days`), and
        // this pins the whole line, checksum included.
        let (repaired, changed) = repair_line(BROKEN_RMC, OffsetSpec::Auto);
        assert!(changed);
        assert_eq!(repaired, FIXED_RMC);
    }

    #[test]
    fn a_modern_date_is_left_alone_by_auto() {
        // 2020-11-17 is past the stale window, so `auto` has no reason to move
        // it. `*68` is the real XOR of this body: a wrong checksum would still
        // be repaired by the command, so the line is written correctly to make
        // "left alone" the only thing under test.
        let modern = "$GPRMC,022127.542,A,3540.87416,N,13946.02750,E,0.00,0.00,201126,,,A*68";
        let (repaired, changed) = repair_line(modern, OffsetSpec::Auto);
        assert!(!changed, "auto must not touch a date that is not stale");
        assert_eq!(repaired, modern);
    }

    #[test]
    fn an_explicit_offset_is_applied_to_a_modern_date() {
        // Same 2020-11-17 line: outside the stale window, so this proves the
        // explicit offset is applied rather than auto-detection having fired.
        let (repaired, changed) = repair_line(
            "$GPRMC,022127.542,A,3540.87416,N,13946.02750,E,0.00,0.00,201126,,,A*68",
            OffsetSpec::Days(-2),
        );
        assert!(changed, "an explicit offset is never second-guessed");
        assert!(repaired.contains(",181126,"), "got {repaired}");
    }

    #[test]
    fn the_two_test_sentences_carry_their_real_checksums() {
        for line in [BROKEN_RMC, FIXED_RMC] {
            let body = line
                .strip_prefix('$')
                .and_then(|rest| rest.split_once('*'))
                .expect("a sentence with a checksum");
            assert_eq!(
                format!("{:02X}", xor_checksum(body.0)),
                body.1,
                "{line:?} carries the wrong checksum"
            );
        }
    }

    #[test]
    fn a_zda_sentence_has_all_three_date_fields_rewritten() {
        let (repaired, changed) =
            repair_line("$GPZDA,022127.54,17,09,2006,00,00*00", OffsetSpec::Auto);
        assert!(changed);
        assert!(repaired.contains(",03,05,2026,"), "got {repaired}");
        let (body, carried) = repaired.split_once('*').expect("checksum");
        let body = body.strip_prefix('$').expect("a sentence body");
        assert_eq!(format!("{:02X}", xor_checksum(body)), carried);
    }

    #[test]
    fn lines_that_are_not_date_bearing_pass_through_byte_for_byte() {
        for line in [
            "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n",
            "======Setting_ProcessKeys========\r\n",
            "",
            "\r\n",
            "$PMTK705,B-core_1.20,0043,01017-00F,1.0*72",
        ] {
            let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
            assert!(!changed, "{line:?} must not change");
            assert_eq!(repaired, line);
        }
    }

    #[test]
    fn a_sentence_without_a_checksum_is_not_invented() {
        let line = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N";
        let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
        assert!(!changed);
        assert_eq!(repaired, line);
    }

    #[test]
    fn a_malformed_date_field_is_left_alone() {
        for text in ["", "1709", "1709067", "aa0906", "17090a"] {
            let line =
                format!("$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,{text},,,N*7F");
            let (repaired, changed) = repair_line(&line, OffsetSpec::Auto);
            assert!(!changed, "{text:?} must not be treated as a date");
            assert_eq!(repaired, line);
        }
    }

    #[test]
    fn a_malformed_zda_that_is_not_numeric_is_left_alone() {
        for line in [
            "$GPZDA,022127.54,,09,2006,00,00*00",
            "$GPZDA,022127.54,17,09*00",
            "$GPZDA,022127.54,17,abc,2006,00,00*00",
        ] {
            let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
            assert!(!changed, "{line:?} must not change");
            assert_eq!(repaired, line);
        }
    }

    #[test]
    fn a_zda_sentence_with_an_out_of_range_date_is_left_alone() {
        for line in [
            "$GPZDA,022127.54,00,00,2006,00,00*00",
            "$GPZDA,022127.54,99,99,2006,00,00*00",
            "$GPZDA,022127.54,32,01,2006,00,00*00",
            "$GPZDA,022127.54,17,13,2006,00,00*00",
            "$GPZDA,022127.54,31,02,2006,00,00*00",
            "$GPZDA,022127.54,17,09,-2006,00,00*00",
        ] {
            let (repaired, changed) = repair_line(line, OffsetSpec::Auto);
            assert!(!changed, "{line:?} must not be rewritten");
            assert_eq!(repaired, line);
        }
    }

    #[test]
    fn the_two_digit_year_uses_the_nmea_century_rule() {
        assert_eq!(parse_ddmmyy("010170"), Some((1, 1, 1970)));
        assert_eq!(parse_ddmmyy("010169"), Some((1, 1, 2069)));
        assert_eq!(parse_ddmmyy("290200"), Some((29, 2, 2000)));
        assert_eq!(parse_ddmmyy("290201"), Some((29, 2, 2001)));
        assert_eq!(parse_ddmmyy("000101"), None, "day zero is not a date");
    }

    #[test]
    fn the_lines_terminator_survives_a_repair() {
        let with_crlf = format!("{BROKEN_RMC}\r\n");
        let (repaired, changed) = repair_line(&with_crlf, OffsetSpec::Auto);
        assert!(changed);
        assert!(repaired.ends_with("\r\n"), "got {repaired:?}");
        let without = repair_line(BROKEN_RMC, OffsetSpec::Auto).0;
        assert!(!without.ends_with('\n'), "got {without:?}");
    }

    #[test]
    fn repair_reads_a_file_and_writes_a_file() {
        let scratch = Scratch::new("file");
        let input = scratch.join("capture.nmea");
        let output = scratch.join("fixed.nmea");
        fileio::write_text(
            &input,
            &format!(
                "{BROKEN_RMC}\n$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\n"
            ),
        )
        .expect("write input");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        repair(
            &RepairOptions {
                input: input.display().to_string(),
                output: Some(output.clone()),
                offset_days: OffsetSpec::Auto,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("repair");
        let text = fileio::read_text(&output).expect("read output");
        assert!(text.contains(",030526,"), "got {text}");
        assert!(
            text.contains("$GPGGA,022127.54"),
            "the GGA line survives: {text}"
        );
        assert!(
            console.stderr().contains("1 of 2 lines"),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn repair_without_an_output_writes_to_the_console() {
        let scratch = Scratch::new("stdout");
        let input = scratch.join("capture.nmea");
        fileio::write_text(&input, BROKEN_RMC).expect("write input");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        repair(
            &RepairOptions {
                input: input.display().to_string(),
                output: None,
                offset_days: OffsetSpec::Auto,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("repair");
        assert_eq!(console.stdout(), FIXED_RMC);
    }

    #[test]
    fn an_empty_input_is_warned_about_and_not_refused() {
        let scratch = Scratch::new("empty");
        let input = scratch.join("empty.nmea");
        fileio::write_text(&input, "").expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        repair(
            &RepairOptions {
                input: input.display().to_string(),
                output: None,
                offset_days: OffsetSpec::Auto,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("empty is not an error");
        assert!(
            console.stderr().contains("empty"),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn a_file_without_a_final_newline_keeps_ending_without_one() {
        let scratch = Scratch::new("no-newline");
        let input = scratch.join("capture.nmea");
        fileio::write_text(
            &input,
            "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00",
        )
        .expect("write");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        repair(
            &RepairOptions {
                input: input.display().to_string(),
                output: None,
                offset_days: OffsetSpec::Auto,
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("repair");
        assert!(
            !console.stdout().ends_with('\n'),
            "got {:?}",
            console.stdout()
        );
    }

    #[test]
    fn repair_reads_standard_input_when_input_is_dash() {
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        repair_with_stdin(
            &RepairOptions {
                input: String::from("-"),
                output: None,
                offset_days: OffsetSpec::Auto,
            },
            Session::new(&links, &console, &confirm),
            || Ok(format!("{BROKEN_RMC}\r\n")),
        )
        .expect("stdin repair");
        assert_eq!(console.stdout(), format!("{FIXED_RMC}\r\n"));
        assert!(
            console.stderr().contains("1 of 1 lines"),
            "got {}",
            console.stderr()
        );
    }

    /// A streamed capture: the proxy reads these bytes, then the port goes idle.
    fn stream_links(lines: &[String]) -> ScriptedLinks {
        let lines = lines.to_vec();
        ScriptedLinks::new(move || ScriptedTransport::replaying(&lines))
    }

    #[test]
    fn proxy_drops_a_fragment_that_never_terminates_and_still_relays_the_next_line() {
        // S7: the cap is a resynchronisation guard, so the property is a bound,
        // not a byte-for-byte result. A baud mismatch produces a run of bytes
        // with no newline; whatever survives the drain stays within one read
        // chunk plus a cap, and the next complete sentence is still relayed
        // repaired.
        //
        // The bound is written down because it is not obvious from the code: the
        // loop appends one 256-byte chunk and only then tests the length, so the
        // first check can see up to `chunk + cap - 1` bytes, and clearing at that
        // point leaves nothing else. The slack is why this is a noise guard and
        // not a hard cap, which is exactly what `MAX_PROXY_LINE_BYTES` documents.
        const READ_CHUNK: usize = 256;
        let noise_len = MAX_PROXY_LINE_BYTES * 8;
        assert_eq!(MAX_PROXY_LINE_BYTES, 128, "two NMEA-0183 sentences' worth");
        let console = RecordingConsole::default();
        let mut transport =
            ScriptedTransport::replaying(&["x".repeat(noise_len), format!("{BROKEN_RMC}\r\n")]);
        proxy_transport(
            &mut transport,
            None,
            true,
            OffsetSpec::Auto,
            Some(1),
            &console,
        )
        .expect("relay");
        let out = console.stdout();
        let (before, relayed) = out
            .split_once('$')
            .unwrap_or_else(|| panic!("no sentence relayed: {out:?}"));
        assert!(
            before.len() < READ_CHUNK + MAX_PROXY_LINE_BYTES,
            "{} bytes of noise survived the drain: {out:?}",
            before.len()
        );
        assert!(
            before.len() < noise_len,
            "the run was not kept in full: {} bytes survived",
            before.len()
        );
        assert_eq!(
            format!("${relayed}"),
            format!("{FIXED_RMC}\r\n"),
            "the sentence was relayed repaired"
        );
    }

    #[test]
    fn proxy_relays_repaired_rmc_and_zda_and_untouched_gga_to_stdout() {
        let gga = "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n";
        let broken_zda = "$GPZDA,022127.54,17,09,2006,00,00*00\r\n";
        let (fixed_zda, zda_changed) = repair_line(broken_zda, OffsetSpec::Auto);
        assert!(zda_changed);

        let links = stream_links(&[
            format!("{BROKEN_RMC}\r\n"),
            String::from(gga),
            String::from(broken_zda),
        ]);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let mut transport = ScriptedTransport::replaying(&[
            format!("{BROKEN_RMC}\r\n"),
            String::from(gga),
            String::from(broken_zda),
        ]);
        proxy_transport(
            &mut transport,
            None,
            true,
            OffsetSpec::Auto,
            Some(3),
            &console,
        )
        .expect("relay from the stream");
        assert_eq!(console.stdout(), format!("{FIXED_RMC}\r\n{gga}{fixed_zda}"));

        // `proxy` itself is unchanged: `--stdout` is the documented way to ask
        // for both channels, and with `--bind` omitted it relays to stdout.
        let console = RecordingConsole::default();
        proxy(
            &ProxyOptions {
                max_lines: Some(3),
                ..ProxyOptions::default()
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("proxy to stdout");
        assert_eq!(console.stdout(), format!("{FIXED_RMC}\r\n{gga}{fixed_zda}"));
    }

    #[test]
    fn proxy_with_bind_and_stdout_relays_to_both_channels() {
        // S5: `--bind` alone sends the stream to the socket only; `--stdout`
        // added alongside it must not be ignored, because a user relaying to a
        // program and to the terminal at once asked for two destinations.
        let gga = "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n";
        let links = stream_links(&[format!("{BROKEN_RMC}\r\n"), String::from(gga)]);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut client_slot: Option<TcpStream> = None;

        proxy_with_hook(
            &ProxyOptions {
                bind: Some(String::from("127.0.0.1:0")),
                stdout: true,
                max_lines: Some(2),
                ..ProxyOptions::default()
            },
            Session::new(&links, &console, &confirm),
            |addr| {
                let stream = TcpStream::connect(addr).expect("connect to loopback proxy");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .expect("set read timeout");
                client_slot = Some(stream);
            },
        )
        .expect("tcp and stdout proxy");

        let expected = format!("{FIXED_RMC}\r\n{gga}");
        assert_eq!(console.stdout(), expected, "stdout carries the stream");
        let mut received = String::new();
        std::io::Read::read_to_string(
            client_slot.as_mut().expect("client connected in hook"),
            &mut received,
        )
        .expect("read tcp stream");
        assert_eq!(received, expected, "the socket carries the stream too");
    }

    #[test]
    fn proxy_opens_the_port_at_the_requested_nmea_baud() {
        // A proxy that spoke the boot rate instead of the NMEA rate would hear
        // nothing from a live unit. The simulated port records the rate it was
        // opened at and hands out its own state, so the handle is the proof.
        let handle = DeviceHandle::new(FakeDevice::new);
        let links = FakePorts::shared(handle.clone());
        handle.set_stream(&[format!("{BROKEN_RMC}\r\n")]);
        let console = RecordingConsole::default();
        proxy(
            &ProxyOptions {
                baud: 4_800,
                max_lines: Some(1),
                ..ProxyOptions::default()
            },
            Session::new(&links, &console, &AutoConfirm),
        )
        .expect("proxy");
        assert_eq!(
            handle.open_baud(),
            Some(4_800),
            "the port was opened at the rate --baud asked for"
        );
        assert!(
            console.stderr().contains("at 4800 baud"),
            "the note names the rate the port was opened at: {}",
            console.stderr()
        );
    }

    #[test]
    fn proxy_allows_a_non_loopback_bind_when_the_flag_is_given() {
        // S6: `--allow-non-loopback` must actually change the outcome. Binding
        // 0.0.0.0 is how a user relays to another machine, and a gate that
        // refused regardless would pass every refusal test while breaking the
        // documented escape hatch.
        let bound =
            bind_checked_listener("0.0.0.0:0", true).expect("the flag allows a non-loopback bind");
        let local = bound.local_addr().expect("bound address");
        assert!(
            match local {
                SocketAddr::V4(addr) => addr.ip().is_unspecified(),
                SocketAddr::V6(addr) => addr.ip().is_unspecified(),
            },
            "bound to the requested wildcard address, got {local}"
        );
        drop(bound);

        // Without the flag the very same spec is refused, with the documented
        // exit code, so the test cannot pass on an always-permit gate either.
        let err = bind_checked_listener("0.0.0.0:0", false).expect_err("refused by default");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5);
    }

    #[test]
    fn proxy_reports_the_simulated_port_and_its_stream_on_stderr() {
        // The notes are diagnostics, and the port name is the one the link layer
        // hands out for the simulator (`crate::testing::FAKE_PORT`), not a
        // measurement of a real device.
        let links = stream_links(&[format!("{BROKEN_RMC}\r\n")]);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        proxy(
            &ProxyOptions {
                max_lines: Some(1),
                ..ProxyOptions::default()
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("proxy");
        assert!(
            console.stderr().contains(&format!("from {FAKE_PORT} at")),
            "got {}",
            console.stderr()
        );
    }

    #[test]
    fn proxy_refuses_non_loopback_bind_without_allow_flag() {
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let err = proxy(
            &ProxyOptions {
                bind: Some(String::from("0.0.0.0:10110")),
                allow_non_loopback: false,
                max_lines: Some(0),
                ..ProxyOptions::default()
            },
            Session::new(&links, &console, &confirm),
        )
        .expect_err("non-loopback bind must be refused");
        assert!(matches!(err, Error::Safety { .. }), "got {err}");
        assert_eq!(crate::exit::exit_code_for(&err), 5);
    }

    #[test]
    fn proxy_relays_repaired_lines_over_loopback_tcp_socket() {
        use std::io::Read as _;

        let gga = "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n";
        let links = stream_links(&[format!("{BROKEN_RMC}\r\n"), String::from(gga)]);
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut client_slot: Option<TcpStream> = None;

        proxy_with_hook(
            &ProxyOptions {
                bind: Some(String::from("127.0.0.1:0")),
                max_lines: Some(2),
                ..ProxyOptions::default()
            },
            Session::new(&links, &console, &confirm),
            |addr| {
                let stream = TcpStream::connect(addr).expect("connect to loopback proxy");
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .expect("set read timeout");
                client_slot = Some(stream);
            },
        )
        .expect("tcp proxy");

        // When --bind is set without --stdout, stdout stays empty.
        assert_eq!(console.stdout(), "");

        let mut client = client_slot.expect("client connected in hook");
        let mut received = String::new();
        client
            .read_to_string(&mut received)
            .expect("read tcp stream");
        assert_eq!(received, format!("{FIXED_RMC}\r\n{gga}"));
    }
}
