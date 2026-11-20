//! The vocabulary shared by the CLI, the GUI and every module below them.
//!
//! These types exist so that no layer has to agree with another by convention.
//! The CLI prints a [`DeviceInfo`], the GUI shows the same [`DeviceInfo`], and
//! `protocol::pmtk` produces it: there is one definition, in one place, and a
//! change to it is a compile error everywhere it matters.
//!
//! The types are plain data. Behaviour that needs the device or a file lives in
//! the module that owns that resource, so these can be constructed in tests
//! without a serial port.

use std::fmt;

/// What the logger says about itself, in answer to `$PMTK705`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DeviceInfo {
    /// Release name, e.g. `B-core_1.20`.
    pub release: String,
    /// Build number as the firmware reports it.
    pub build: String,
    /// Model or product id, e.g. `01017-00F` on a V1.13 unit.
    pub model: String,
    /// SDK version, when the firmware answers with one.
    pub sdk: Option<String>,
}

impl DeviceInfo {
    /// Whether the release string names a firmware family this build knows.
    ///
    /// Used for a warning, never for a refusal: a unit that reports an unknown
    /// release may still be repairable, and the patch path decides on the image
    /// rather than on the string.
    #[must_use]
    pub fn is_known_release(&self) -> bool {
        self.release.starts_with("B-core_1.")
    }
}

impl fmt::Display for DeviceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.release, self.model)?;
        if !self.build.is_empty() {
            write!(f, " build {}", self.build)?;
        }
        Ok(())
    }
}

/// One point in the track log.
///
/// Fields are `Option` because the log's block header decides which fields each
/// record carries: a 16-byte record has no speed, and a record truncated by a
/// lost block must still yield the fields it did contain.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TrackPoint {
    /// UTC as Unix seconds.
    pub utc: Option<u32>,
    /// Latitude in degrees, widened from the 4-byte float the device stores.
    pub latitude: Option<f64>,
    /// Longitude in degrees.
    pub longitude: Option<f64>,
    /// Height in metres, decoded from the 3-byte field.
    pub height_m: Option<f32>,
    /// Speed in km/h.
    pub speed_kmh: Option<f32>,
    /// The recorder's reason bitmask; bit 3 marks a manual waypoint.
    pub rcr: Option<u16>,
}

impl TrackPoint {
    /// Whether this point has a position, which is what a map or a GPX file
    /// needs. A record with a time but no fix is kept: it proves the unit was
    /// recording while it had no satellite lock.
    #[must_use]
    pub fn has_position(&self) -> bool {
        self.latitude.is_some() && self.longitude.is_some()
    }

    /// Whether this point is a manual waypoint.
    #[must_use]
    pub fn is_waypoint(&self) -> bool {
        self.rcr.is_some_and(|rcr| rcr & 0x0008 != 0)
    }
}

/// A downloaded or parsed track log.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Track {
    /// Points in the order the device recorded them.
    pub points: Vec<TrackPoint>,
    /// Blocks whose header checksum did not match. Empty is the normal case.
    pub damaged_blocks: Vec<u32>,
}

impl Track {
    /// Number of points that carry a position.
    #[must_use]
    pub fn fix_count(&self) -> usize {
        self.points.iter().filter(|p| p.has_position()).count()
    }

    /// First and last timestamp, when the log has any.
    ///
    /// The log is written oldest-first, so the first point's time is the start
    /// and the last point's time is the end.
    #[must_use]
    pub fn time_span(&self) -> Option<(u32, u32)> {
        let first = self.points.iter().find_map(|p| p.utc)?;
        let last = self.points.iter().rev().find_map(|p| p.utc)?;
        Some((first, last))
    }

    /// Whether the log is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// A summary of a log, for the one-line report the CLI prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogSummary {
    /// Total records, including those without a fix.
    pub records: usize,
    /// Records that carry a position.
    pub fixes: usize,
    /// Blocks whose header checksum did not match.
    pub damaged_blocks: usize,
    /// First timestamp, as Unix seconds.
    pub started_at: Option<u32>,
    /// Last timestamp, as Unix seconds.
    pub ended_at: Option<u32>,
}

impl LogSummary {
    /// Build a summary from a parsed track.
    #[must_use]
    pub fn of(track: &Track) -> Self {
        let span = track.time_span();
        Self {
            records: track.points.len(),
            fixes: track.fix_count(),
            damaged_blocks: track.damaged_blocks.len(),
            started_at: span.map(|(start, _)| start),
            ended_at: span.map(|(_, end)| end),
        }
    }
}

/// One byte that a patch would change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchEdit {
    /// Offset from the start of the image file.
    pub offset: usize,
    /// The byte as it is in the input image.
    pub before: u8,
    /// The byte as the patch would leave it.
    pub after: u8,
}

/// What patching one image would do.
///
/// Produced by the patch planner and applied by the patcher, so the plan that a
/// person reviews in `--dry-run` is byte-for-byte the plan that gets written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PatchPlan {
    /// Every changed byte, in ascending offset order.
    pub edits: Vec<PatchEdit>,
    /// The profile the image was identified as.
    pub profile: Option<FirmwareProfile>,
    /// Why the plan is empty, when it is.
    pub no_op_reason: Option<String>,
}

impl PatchPlan {
    /// Whether applying this plan would change the image.
    #[must_use]
    pub fn changes_anything(&self) -> bool {
        !self.edits.is_empty()
    }

    /// Apply the plan to a copy of `image`.
    ///
    /// Each edit is checked against the image before it is written: the byte the
    /// plan says it replaces must actually be there. That is what makes a
    /// reviewed plan safe to apply — a plan built for a different image is
    /// refused instead of overwriting whatever the offset happens to hold.
    ///
    /// # Errors
    ///
    /// [`crate::Error::Firmware`] when an edit lands outside the image or when
    /// the byte at its offset is not [`PatchEdit::before`]; either means the
    /// plan was built against a different file.
    pub fn apply(&self, image: &[u8]) -> crate::Result<Vec<u8>> {
        let mut out = image.to_vec();
        for edit in &self.edits {
            let Some(byte) = out.get_mut(edit.offset) else {
                return Err(crate::Error::Firmware {
                    message: format!(
                        "patch offset 0x{:X} is outside a {}-byte image",
                        edit.offset,
                        image.len()
                    ),
                    offset: Some(edit.offset),
                });
            };
            let found = *byte;
            if found != edit.before {
                return Err(crate::Error::Firmware {
                    message: format!(
                        "patch expects 0x{expect:02X} at offset 0x{offset:X}, but the image has \
                         0x{found:02X}; the plan was built for a different image",
                        expect = edit.before,
                        offset = edit.offset,
                    ),
                    offset: Some(edit.offset),
                });
            }
            *byte = edit.after;
        }
        Ok(out)
    }
}

/// A known firmware image, identified by its measured facts.
///
/// A profile is what makes "is this the image I think it is" answerable without
/// trusting the filename. Every field was measured on a real image; see
/// `docs/dev/03-hardware-facts.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirmwareProfile {
    /// Human name, e.g. `V1.13`.
    pub name: &'static str,
    /// Exact image length in bytes.
    pub size: usize,
    /// SHA-256 of the whole image, lowercase hex.
    pub sha256: &'static str,
    /// Offset of the ASCII version marker.
    pub version_marker: usize,
    /// Device type string, e.g. `01017-00F`.
    pub device_type: &'static str,
}

impl fmt::Display for FirmwareProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}, {} bytes)",
            self.name, self.device_type, self.size
        )
    }
}

/// Which serialisation an exporter writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// GPX 1.1 track.
    Gpx,
    /// KML 2.2 document.
    Kml,
    /// Comma-separated rows, one per point.
    Csv,
    /// NMEA sentences regenerated from the log.
    Nmea,
    /// One JSON object.
    Json,
}

impl ExportFormat {
    /// Every format an exporter can write.
    pub const ALL: [Self; 5] = [Self::Gpx, Self::Kml, Self::Csv, Self::Nmea, Self::Json];

    /// The file extension this format normally uses.
    #[must_use]
    pub fn extension(self) -> &'static str {
        match self {
            Self::Gpx => "gpx",
            Self::Kml => "kml",
            Self::Csv => "csv",
            Self::Nmea => "nmea",
            Self::Json => "json",
        }
    }

    /// Parse from a command-line argument.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "gpx" => Some(Self::Gpx),
            "kml" => Some(Self::Kml),
            "csv" => Some(Self::Csv),
            "nmea" | "nmea0183" => Some(Self::Nmea),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_without_coordinates_is_not_a_fix() {
        let point = TrackPoint {
            utc: Some(1_700_000_000),
            ..TrackPoint::default()
        };
        assert!(!point.has_position());
        assert!(!point.is_waypoint());
    }

    #[test]
    fn rcr_bit_three_marks_a_waypoint() {
        let point = TrackPoint {
            latitude: Some(35.681_236),
            longitude: Some(139.767_125),
            rcr: Some(0x0008),
            ..TrackPoint::default()
        };
        assert!(point.has_position());
        assert!(point.is_waypoint());
    }

    #[test]
    fn the_time_span_uses_the_first_and_last_timed_point() {
        let track = Track {
            points: vec![
                TrackPoint {
                    utc: Some(100),
                    ..TrackPoint::default()
                },
                // A point with a time but no fix still bounds the log.
                TrackPoint {
                    utc: Some(200),
                    ..TrackPoint::default()
                },
            ],
            damaged_blocks: Vec::new(),
        };
        assert_eq!(track.time_span(), Some((100, 200)));
    }

    #[test]
    fn a_track_without_timestamps_has_no_span() {
        let track = Track::default();
        assert_eq!(track.time_span(), None);
        assert!(track.is_empty());
        assert_eq!(LogSummary::of(&track), LogSummary::default());
    }

    #[test]
    fn a_plan_applies_only_the_bytes_it_names() {
        let plan = PatchPlan {
            edits: vec![PatchEdit {
                offset: 2,
                before: 0x00,
                after: 0xFF,
            }],
            profile: None,
            no_op_reason: None,
        };
        let patched = plan.apply(&[1, 2, 0, 4]).expect("offset is inside");
        assert_eq!(patched, vec![1, 2, 0xFF, 4]);
    }

    #[test]
    fn a_plan_built_for_another_image_is_refused() {
        let plan = PatchPlan {
            edits: vec![PatchEdit {
                offset: 99,
                before: 0,
                after: 1,
            }],
            profile: None,
            no_op_reason: None,
        };
        let err = plan.apply(&[0; 4]).expect_err("offset is outside");
        assert!(format!("{err}").contains("0x63"), "got {err}");
    }

    #[test]
    fn a_plan_whose_before_byte_is_absent_from_the_image_is_refused() {
        // The plan was built from a stock image whose byte 1 is 0x00; this image
        // has 0x2A there, so it is a different file and must not be touched.
        let plan = PatchPlan {
            edits: vec![PatchEdit {
                offset: 1,
                before: 0x00,
                after: 0xFF,
            }],
            profile: None,
            no_op_reason: None,
        };
        let err = plan
            .apply(&[0x11, 0x2A, 0x33])
            .expect_err("before does not match");
        assert!(matches!(err, crate::Error::Firmware { .. }), "got {err}");
        let text = format!("{err}");
        assert!(text.contains("0x2A"), "the found byte is named: {text}");
        assert!(text.contains("0x00"), "the expected byte is named: {text}");
        assert!(
            text.contains("different image"),
            "the cause is named: {text}"
        );
    }

    #[test]
    fn an_empty_plan_reports_why_it_is_empty() {
        let plan = PatchPlan {
            no_op_reason: Some(String::from("already patched")),
            ..PatchPlan::default()
        };
        assert!(!plan.changes_anything());
        assert!(plan.no_op_reason.is_some());
    }

    #[test]
    fn formats_round_trip_through_their_names() {
        for format in ExportFormat::ALL {
            assert_eq!(ExportFormat::parse(format.extension()), Some(format));
        }
        assert_eq!(ExportFormat::parse("GPX"), Some(ExportFormat::Gpx));
        assert_eq!(ExportFormat::parse("yaml"), None);
    }

    #[test]
    fn the_release_check_accepts_the_known_family_and_nothing_else() {
        let known = DeviceInfo {
            release: String::from("B-core_1.20"),
            ..DeviceInfo::default()
        };
        assert!(known.is_known_release());
        assert!(!DeviceInfo::default().is_known_release());
    }
}
