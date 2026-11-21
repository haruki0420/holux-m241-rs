//! End-to-end flows through the real clients and the simulator.
//!
//! The unit tests inside each module check one function. These run the same code
//! the CLI will run, in the order a `m241` command runs it, against a fake chip
//! that speaks the measured protocol. If a protocol detail is wrong, the failure
//! appears here as a timeout or a framing error rather than as a plausible-looking
//! wrong date on someone's unit.
//!
//! Everything here uses the repository's example coordinate or no position at
//! all; no fixture is derived from a real recording.

// Integration tests may panic: a failed assertion is the point.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::Duration;

use m241_core::firmware::image::{
    DEVICE_TYPE_OFFSET, DEVICE_TYPE_SIZE, EPOCH_CONSTANT_OFFSETS, FAMILY, FAMILY_OFFSET,
    FW_SIZE_OFFSET, GPS_EPOCH_UNIX, GPS_EPOCH_UNIX_WNRO,
};
use m241_core::firmware::patch::{EFFECTIVE_EPOCH_OFFSET, INACTIVE_EPOCH_OFFSET};
use m241_core::firmware::profiles::{PROFILE_V1_13, identify};
use m241_core::firmware::{FirmwareImage, patch};
use m241_core::log::{block, export, parse_dump};
use m241_core::protocol::brom::{BASE_ADDR_FIRMWARE, MtkBootRom};
use m241_core::protocol::pmtk::{EntryTiming, PmtkClient, StreamState};
use m241_core::protocol::simulator::{FakeDevice, Mode};
use m241_core::{ExportFormat, Track};

/// Build a structurally valid V1.13 image: the header a real dump has, with the
/// measured release constants and the stock epoch.
fn stock_image() -> Vec<u8> {
    let mut data = vec![0xFFu8; PROFILE_V1_13.size];
    // Header fields are C strings: NUL after the name, not 0xFF.
    data[FAMILY_OFFSET..FAMILY_OFFSET + 0x20].fill(0);
    data[FAMILY_OFFSET..FAMILY_OFFSET + FAMILY.len()].copy_from_slice(FAMILY.as_bytes());
    data[DEVICE_TYPE_OFFSET..DEVICE_TYPE_OFFSET + DEVICE_TYPE_SIZE].fill(0);
    let device = PROFILE_V1_13.device_type.as_bytes();
    let device_end = DEVICE_TYPE_OFFSET + device.len();
    data[DEVICE_TYPE_OFFSET..device_end].copy_from_slice(device);
    let marker = PROFILE_V1_13.name.as_bytes();
    let marker_end = PROFILE_V1_13.version_marker + marker.len();
    data[PROFILE_V1_13.version_marker..marker_end].copy_from_slice(marker);
    for offset in EPOCH_CONSTANT_OFFSETS {
        data[offset..offset + 4].copy_from_slice(&GPS_EPOCH_UNIX.to_le_bytes());
    }
    data[FW_SIZE_OFFSET..FW_SIZE_OFFSET + 4]
        .copy_from_slice(&(PROFILE_V1_13.size as u32).to_le_bytes());
    // The vector table the hardware notes record: `ldr pc, [pc, #-0x18]`.
    let (words, _tail) = data[..0x40].as_chunks_mut::<4>();
    for word in words {
        word.copy_from_slice(&[0x18, 0xf0, 0x9f, 0xe5]);
    }
    data
}

/// A two-record log block with a correct header and checksums.
fn one_block_log() -> Vec<u8> {
    const MASK: u32 = 0x8000_001D; // UTC | LAT | LON | HEIGHT, the measured default
    let mut dump = vec![0xFFu8; block::BLOCK_SIZE];
    dump[0..2].copy_from_slice(&2u16.to_le_bytes());
    dump[2..6].copy_from_slice(&MASK.to_le_bytes());
    dump[6..8].copy_from_slice(&block::POLICY_LOG_ENABLED.to_le_bytes());
    dump[8..12].copy_from_slice(&10u32.to_le_bytes());

    let mut cursor = block::BLOCK_HEADER_SIZE;
    for (index, utc) in [1_700_000_000u32, 1_700_000_001].into_iter().enumerate() {
        let mut body = Vec::new();
        body.extend_from_slice(&utc.to_le_bytes());
        body.extend_from_slice(&35.681_236f32.to_le_bytes());
        body.extend_from_slice(&139.767_13f32.to_le_bytes());
        // 80.0 m as the three high bytes of a little-endian float32.
        body.extend_from_slice(&[0x00, 0xA0, 0x42]);
        let checksum = block::xor_checksum(&body);
        body.push(checksum);
        dump[cursor..cursor + body.len()].copy_from_slice(&body);
        cursor += body.len();
        let _ = index;
    }
    dump
}

#[test]
fn the_identity_and_the_log_download_work_on_a_running_firmware() {
    // The shape of `m241 device info` followed by `m241 log download`.
    let device = FakeDevice::new().with_log(&one_block_log()).with_nmea(2);
    let mut client = PmtkClient::new(device)
        .with_stream_window(Duration::from_millis(50))
        .with_reply_timeout(Duration::from_millis(500));

    let info = client.request_identity().expect("identity");
    assert_eq!(info.model, "01017-00F");
    assert!(info.is_known_release());

    // The real unit emits one sentence per second; ask the fake for one so the
    // probe does not have to wait a second of wall clock.
    client.transport_mut().queue_nmea(1);
    assert_eq!(
        client.probe_stream().expect("probe"),
        StreamState::Streaming
    );

    let mut progress = Vec::new();
    let mut callback = |done: usize, total: usize| progress.push((done, total));
    let log = client.download_log(&mut callback).expect("download");
    assert_eq!(log.len(), block::BLOCK_SIZE);
    assert_eq!(progress.last().copied(), Some((log.len(), log.len())));
    assert!(
        client.transport_mut().logging_enabled(),
        "a finished download must leave the unit recording"
    );

    let (track, report) = parse_dump(&log).expect("parse");
    assert_eq!(track.points.len(), 2);
    assert_eq!(report.bad_checksums, 0);
    assert_eq!(track.time_span(), Some((1_700_000_000, 1_700_000_001)));
}

#[test]
fn pmtk180_then_the_bootrom_sync_then_a_firmware_dump() {
    // The shape of `m241 fw dump`, with the PMTK client handing the transport to
    // the BootROM client exactly as the CLI will.
    let image = stock_image();
    // Two sentences: the probe consumes one, the drain before `$PMTK180` gets
    // the other, and the fake's switch then silences the line for the watch.
    let device = FakeDevice::new().with_firmware(&image).with_nmea(2);
    // `EntryTiming::fast()` for the same reason the unit tests use it: the real
    // 350/200/500 ms windows are exercised on hardware, not in the test suite,
    // and a real window here would make this one test cost a second. The values
    // are not zero; `EntryTiming::fast` says why.
    let mut client = PmtkClient::new(device)
        .with_stream_window(Duration::from_millis(50))
        .with_reply_timeout(Duration::from_millis(500))
        .with_entry_timing(EntryTiming::fast());

    client.enter_bootrom().expect("the stream is live");
    let mut brom = MtkBootRom::new(client.into_transport());
    brom.sync().expect("the documented sync replies");

    // Read the header through the real BootROM client. 0xAF needs a dword count.
    let header = brom.read32(BASE_ADDR_FIRMWARE, 0x200).expect("read header");
    assert_eq!(&header[..4], &[0x18, 0xf0, 0x9f, 0xe5]);
    let image = FirmwareImage::new(&header[..]).expect("the header alone is enough to wrap");
    assert!(image.is_m241(), "family string must survive the transfer");
    assert_eq!(image.device_type(), PROFILE_V1_13.device_type);
}

#[test]
fn the_whole_firmware_dump_round_trips_through_the_bootrom() {
    // Reading 400 KB through the real client and the fake chip is the strongest
    // check that the frame counting and both byte orders are right: any mistake
    // in either turns the image into plausible garbage that fails the header
    // decode.
    let image = stock_image();
    let device = FakeDevice::new().with_firmware(&image).in_bootrom();
    let mut brom = MtkBootRom::new(device);
    brom.sync().expect("sync");
    let dumped = brom
        .read32(BASE_ADDR_FIRMWARE, PROFILE_V1_13.size)
        .expect("dump");
    assert_eq!(dumped, image);

    let firmware = FirmwareImage::new(&dumped).expect("header");
    let found = identify(&firmware).expect("identified by structure");
    assert_eq!(found.profile, &PROFILE_V1_13);
}

#[test]
fn a_dumped_image_is_identified_and_the_patch_plan_is_the_measured_three_bytes() {
    // The shape of `m241 fw patch --dry-run`: dump, identify, plan, inspect.
    let image = stock_image();
    let device = FakeDevice::new().with_firmware(&image).in_bootrom();
    let mut brom = MtkBootRom::new(device);
    brom.sync().expect("sync");
    let dumped = brom
        .read32(BASE_ADDR_FIRMWARE, PROFILE_V1_13.size)
        .expect("dump");
    drop(brom);

    let firmware = FirmwareImage::new(&dumped).expect("header");
    let plan = patch::plan(&firmware).expect("a stock image plans");
    let offsets: Vec<usize> = plan.edits.iter().map(|edit| edit.offset).collect();
    // The three bytes the hardware session measured: two of the epoch constant
    // and the last digit of the version marker.
    assert_eq!(
        offsets,
        vec![
            EFFECTIVE_EPOCH_OFFSET + 2,
            EFFECTIVE_EPOCH_OFFSET + 3,
            0x59C28 + 4
        ]
    );
    assert!(
        plan.edits
            .iter()
            .all(|edit| edit.offset < INACTIVE_EPOCH_OFFSET
                || edit.offset >= INACTIVE_EPOCH_OFFSET + 4),
        "the inactive epoch copy must never be touched"
    );

    let patched = plan.apply(&dumped).expect("edits fit");
    let patched_image = FirmwareImage::new(&patched).expect("header");
    assert_eq!(patched_image.effective_epoch(), Some(GPS_EPOCH_UNIX_WNRO));
    assert_eq!(
        patched_image.u32_at(INACTIVE_EPOCH_OFFSET),
        Some(GPS_EPOCH_UNIX)
    );
    assert_eq!(
        patched_image.field(PROFILE_V1_13.version_marker, 5),
        "V1.14"
    );

    // And a second run finds nothing to do instead of patching again.
    let again = patch::plan(&patched_image).expect("identifiable");
    assert!(!again.changes_anything());
    assert!(again.no_op_reason.is_some());
}

#[test]
fn a_log_downloaded_over_pmtk_exports_to_every_format() {
    // The shape of `m241 log read`: download, parse, export.
    let device = FakeDevice::new().with_log(&one_block_log()).with_nmea(1);
    let mut client = PmtkClient::new(device)
        .with_stream_window(Duration::from_millis(50))
        .with_reply_timeout(Duration::from_millis(500));
    let mut callback = |_done: usize, _total: usize| {};
    let log = client.download_log(&mut callback).expect("download");
    let (track, _report) = parse_dump(&log).expect("parse");

    let gpx = export::export(&track, ExportFormat::Gpx);
    // The log stores an `f32`, so the exported digits are the widened `f32`, not
    // the `f64` the example coordinate is written as.
    assert!(gpx.contains("<trkpt lat=\"35.68123627\""), "got {gpx}");
    assert!(
        gpx.contains("<time>2023-11-14T22:13:20Z</time>"),
        "got {gpx}"
    );

    let kml = export::export(&track, ExportFormat::Kml);
    assert!(kml.contains("139.76713562,35.68123627"), "got {kml}");

    let csv = export::export(&track, ExportFormat::Csv);
    assert_eq!(csv.lines().count(), 3, "header plus two points");

    let nmea = export::export(&track, ExportFormat::Nmea);
    for line in nmea.lines() {
        assert!(line.starts_with("$GPRMC,"), "got {line}");
    }
    assert_eq!(nmea.lines().count(), 2);

    let json = export::export(&track, ExportFormat::Json);
    assert!(json.contains("\"records\": 2"), "got {json}");

    // One more property that matters for a real export: nothing in the track is
    // a valid JSON-free XML fragment, so a GPX/KML consumer cannot be tripped by
    // an unescaped character.
    let track_with_name = Track {
        points: track.points.clone(),
        damaged_blocks: Vec::new(),
    };
    assert!(
        export::to_gpx(&track_with_name)
            .trim_end()
            .ends_with("</gpx>")
    );
}

#[test]
fn the_simulator_does_not_pulse_the_control_lines_for_the_pmtk_route() {
    // The safety rule from AGENTS.md §2.1: the PMTK180 route must never touch
    // DTR/RTS, because doing so at a streaming firmware leaves the unit silent
    // until the battery is removed.
    let device = FakeDevice::new().with_nmea(2);
    let mut client = PmtkClient::new(device)
        .with_stream_window(Duration::from_millis(50))
        .with_reply_timeout(Duration::from_millis(500))
        .with_entry_timing(EntryTiming::fast());
    client.enter_bootrom().expect("entry");
    assert!(
        !client.transport_mut().control_lines(),
        "the PMTK route must not assert the reset lines"
    );
    assert_eq!(client.transport_mut().mode(), Mode::BootRom);
}
