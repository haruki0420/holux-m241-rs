//! End-to-end check that `m241 nmea repair` reads standard input by default.

#![allow(clippy::expect_used)]

use std::io::Write as _;
use std::process::{Command, Stdio};

#[test]
fn nmea_repair_defaults_to_stdin_and_repairs_broken_dates() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_m241"))
        .args(["nmea", "repair"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn m241");

    let broken = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N*7F\r\n";
    let expected = "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,030526,,,N*74\r\n";

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(broken.as_bytes())
        .expect("write stdin");

    let output = child.wait_with_output().expect("wait for m241");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("1 of 1 lines"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
