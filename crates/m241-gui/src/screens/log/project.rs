//! The JSON project document and the `log read` conversion it drives.
//!
//! `m241 log project save` defines a small JSON schema (`format`, `version`,
//! `input`, `log`, `photos`, `utc_offset_days`, `output`, `export_format`);
//! `log project load` reads the named log, applies the stored day correction and
//! writes the export. This module keeps the same schema and the same resolution
//! rule — a relative path is resolved against the project file's directory — so
//! a project written here loads with the CLI and a CLI project loads here.
//!
//! It also owns the GUI's `log read` equivalent: choose a dump or an NMEA
//! capture, a format, a day offset and an output, then read and write once.

use std::path::{Path, PathBuf};

use m241_core::log::{ParseReport, export, import_nmea, parse_dump};
use m241_core::{ExportFormat, Track};
use serde_json::Value;

use super::nmea::{OffsetSpec, apply_to_track};

/// The marker and schema version every project file carries.
const PROJECT_FORMAT: &str = "m241-project";
const PROJECT_VERSION: u64 = 1;

/// A project document with every required key checked and its paths resolved.
#[derive(Debug)]
pub struct ProjectDocument {
    /// Whether the log is an NMEA 0183 capture rather than a raw dump.
    pub input_is_nmea: bool,
    /// The log the project names, resolved against the project directory.
    pub log: PathBuf,
    /// Photo folder, when the project carries one.
    pub photos: Option<PathBuf>,
    /// The day correction the project stores.
    pub utc_offset_days: i32,
    /// Where the export goes, resolved against the project directory.
    pub output: PathBuf,
    /// `gpx`/`kml`/`csv`/`nmea`/`json`/`kmz`, as stored.
    pub export_format: String,
}

impl ProjectDocument {
    /// Read and validate a project file.
    ///
    /// # Errors
    ///
    /// A Japanese sentence naming the project file: unreadable, not JSON, the
    /// wrong marker or version, a missing or mistyped key, an unknown input kind,
    /// or an unknown export format.
    pub fn read(project: &str) -> Result<Self, String> {
        let path = PathBuf::from(project.trim());
        if path.as_os_str().is_empty() {
            return Err(String::from("プロジェクトファイルのパスを入力してください"));
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("{} を読めません: {err}", path.display()))?;
        let document: Value = serde_json::from_str(&text).map_err(|err| {
            format!(
                "{}: プロジェクトファイルではありません: {err}",
                path.display()
            )
        })?;
        let marker = document.get("format").and_then(Value::as_str);
        if marker != Some(PROJECT_FORMAT) {
            return Err(format!(
                "{}: プロジェクトファイルではありません（format が {marker:?}）",
                path.display()
            ));
        }
        let version = document.get("version").and_then(Value::as_u64);
        if version != Some(PROJECT_VERSION) {
            return Err(format!(
                "{}: プロジェクトの版 {version:?} は読めません（読めるのは版 {PROJECT_VERSION} のみ）",
                path.display()
            ));
        }
        let text_field = |key: &str| -> Result<String, String> {
            document
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!(
                        "{}: プロジェクトに {key} がありません（文字列でもありません）。既定値は使いません",
                        path.display()
                    )
                })
        };
        let input = text_field("input")?;
        let input_is_nmea = match input.as_str() {
            "nmea" => true,
            "dump" => false,
            other => {
                return Err(format!(
                    "{}: 未知の input {other:?} です（読めるのは \"dump\" か \"nmea\"）",
                    path.display()
                ));
            }
        };
        let export_format = text_field("export_format")?;
        if !export_format.eq_ignore_ascii_case("kmz")
            && ExportFormat::parse(&export_format).is_none()
        {
            return Err(format!(
                "{}: 未知の export_format {export_format:?} です（gpx/kml/csv/nmea/json/kmz）",
                path.display()
            ));
        }
        let offset = document.get("utc_offset_days").ok_or_else(|| {
            format!(
                "{}: プロジェクトに utc_offset_days がありません。既定値は使いません",
                path.display()
            )
        })?;
        let offset = offset.as_i64().ok_or_else(|| {
            format!(
                "{}: utc_offset_days が整数ではありません: {offset}",
                path.display()
            )
        })?;
        let utc_offset_days = i32::try_from(offset)
            .map_err(|_| format!("{}: utc_offset_days {offset} は範囲外です", path.display()))?;
        let photos = match document.get("photos") {
            None => {
                return Err(format!(
                    "{}: プロジェクトに photos がありません。写真フォルダがない場合は null にしてください",
                    path.display()
                ));
            }
            Some(Value::Null) => None,
            Some(Value::String(value)) => Some(PathBuf::from(value)),
            Some(other) => {
                return Err(format!(
                    "{}: photos がパスでも null でもありません: {other}",
                    path.display()
                ));
            }
        };
        let base = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
        let resolve = |text: &str| {
            let stored = PathBuf::from(text);
            if stored.is_absolute() {
                stored
            } else {
                base.join(stored)
            }
        };
        let log = resolve(&text_field("log")?);
        let output = resolve(&text_field("output")?);
        Ok(Self {
            input_is_nmea,
            log,
            photos: photos.map(|photo| resolve(&photo.to_string_lossy())),
            utc_offset_days,
            output,
            export_format,
        })
    }
}

/// A log read from disk and optionally shifted.
#[derive(Debug)]
pub struct LoadedLog {
    /// The decoded track.
    pub track: Track,
    /// The dump parser's report; default for an NMEA capture.
    pub report: ParseReport,
    /// What the read counted, for the status line.
    pub note: String,
}

/// Read `input` as a dump or an NMEA capture, apply `offset`, and describe it.
///
/// # Errors
///
/// An unreadable file, a dump the parser refuses, a capture whose lines produced
/// no point at all, or a non-UTF-8 capture. An empty capture is not an error:
/// it yields an empty track, exactly as `log read --nmea` does.
pub fn load_log(input: &Path, nmea: bool, offset: OffsetSpec) -> Result<LoadedLog, String> {
    if input.as_os_str().is_empty() {
        return Err(String::from("読み込むログのパスを入力してください"));
    }
    let (mut track, report, mut note) = if nmea {
        let text = std::fs::read_to_string(input).map_err(|err| {
            format!(
                "{} を NMEA テキストとして読めません: {err}",
                input.display()
            )
        })?;
        let imported = import_nmea(&text).map_err(|err| format!("NMEA を読めません: {err}"))?;
        let lines = imported.imported + imported.dropped + imported.ignored;
        if lines > 0 && imported.imported == 0 {
            return Err(format!(
                "{}: {lines} 行読みましたが点を作れませんでした（壊れた文・測位なし・形式違い）。何も書き出していません",
                input.display()
            ));
        }
        let note = format!(
            "NMEA {} 文を読み、{} 文を捨て、{} 文を無視",
            imported.imported, imported.dropped, imported.ignored
        );
        (imported.track, ParseReport::default(), note)
    } else {
        let bytes = std::fs::read(input)
            .map_err(|err| format!("{} を読めません: {err}", input.display()))?;
        let (track, report) = parse_dump(&bytes)
            .map_err(|err| format!("{} をログとして解析できません: {err}", input.display()))?;
        let note = format!(
            "{} 点（破損 {} ブロック）",
            track.points.len(),
            report.damaged_blocks
        );
        (track, report, note)
    };
    let shift = apply_to_track(&mut track, offset);
    if shift != 0 {
        note.push_str(&format!("、日付を {shift:+} 日補正"));
    }
    Ok(LoadedLog {
        track,
        report,
        note,
    })
}

/// `log read` in one call: read `input`, convert to `format`, write `output`.
///
/// # Errors
///
/// Whatever [`load_log`] refuses, an empty output, an existing output without
/// `force`, or an I/O failure.
pub fn read_and_export(
    input: &str,
    nmea: bool,
    format: ExportFormat,
    offset: OffsetSpec,
    output: &str,
    force: bool,
) -> Result<(LoadedLog, String), String> {
    let loaded = load_log(Path::new(input.trim()), nmea, offset)?;
    let trimmed = output.trim();
    if trimmed.is_empty() {
        return Err(String::from("書き出し先を入力してください"));
    }
    let path = Path::new(trimmed);
    if path.exists() && !force {
        return Err(format!(
            "{trimmed} はすでに存在します。上書きする場合は「上書き」を入れてください"
        ));
    }
    write_text(path, &export(&loaded.track, format))?;
    let message = format!(
        "{} 点を {trimmed} に {} で書き出しました",
        loaded.track.points.len(),
        format.extension().to_uppercase()
    );
    Ok((loaded, message))
}

/// Write a project document without reading the log it names.
///
/// # Errors
///
/// An empty path, a project path that is also its log or output, a KMZ project
/// without a photo folder or over an NMEA capture, an unknown format, or an I/O
/// failure.
pub fn save_project(
    project: &str,
    log: &str,
    photos: &str,
    output: &str,
    utc_offset_days: i32,
    export_format: &str,
    nmea: bool,
) -> Result<PathBuf, String> {
    let project_path = PathBuf::from(project.trim());
    if project_path.as_os_str().is_empty() {
        return Err(String::from("プロジェクトファイルのパスを入力してください"));
    }
    let log_path = PathBuf::from(log.trim());
    if log_path.as_os_str().is_empty() {
        return Err(String::from("ログのパスを入力してください"));
    }
    let output_path = PathBuf::from(output.trim());
    if output_path.as_os_str().is_empty() {
        return Err(String::from("書き出し先を入力してください"));
    }
    let photos_trimmed = photos.trim();
    if export_format.eq_ignore_ascii_case("kmz") {
        if photos_trimmed.is_empty() {
            return Err(String::from("KMZ のプロジェクトには写真フォルダが必要です"));
        }
        if nmea {
            return Err(String::from(
                "KMZ のプロジェクトは NMEA 入力ではなく生ダンプが必要です",
            ));
        }
    } else if ExportFormat::parse(export_format).is_none() {
        return Err(format!(
            "{export_format:?} は書き出し形式ではありません（gpx/kml/csv/nmea/json/kmz）"
        ));
    }
    if same_path(&project_path, &log_path) || same_path(&project_path, &output_path) {
        return Err(String::from(
            "プロジェクトファイルがログや書き出し先と同じパスです。別の名前にしてください",
        ));
    }
    let document = serde_json::json!({
        "format": PROJECT_FORMAT,
        "version": PROJECT_VERSION,
        "input": if nmea { "nmea" } else { "dump" },
        "log": stored_path(&log_path),
        "photos": if photos_trimmed.is_empty() {
            Value::Null
        } else {
            Value::String(stored_path(Path::new(photos_trimmed)))
        },
        "utc_offset_days": utc_offset_days,
        "output": stored_path(&output_path),
        "export_format": export_format,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|err| format!("プロジェクトを JSON にできません: {err}"))?;
    write_text(&project_path, &format!("{text}\n"))?;
    Ok(project_path)
}

/// The stored spelling of a path: absolute, with `/` separators on every OS.
///
/// The project file outlives the directory it was written from, so a relative
/// path is resolved against the current directory once, when saving. `load`
/// resolves a stored relative path against the project directory, which is the
/// CLI's rule.
fn stored_path(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    absolute
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// Whether two paths name the same file, resolving relative spellings.
fn same_path(left: &Path, right: &Path) -> bool {
    match (std::path::absolute(left), std::path::absolute(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Write text, creating its parent directory.
fn write_text(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("{} を作れません: {err}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|err| format!("{} に書けません: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{ScratchDir, synthetic_dump};
    use super::*;

    fn one_day_dump() -> Vec<u8> {
        synthetic_dump(&[(1_700_000_000, 35.68, 139.76, 10.0)])
    }

    #[test]
    fn a_saved_project_carries_the_cli_schema_and_round_trips() {
        let scratch = ScratchDir::new("project-save");
        let log = scratch.join("track.bin");
        std::fs::write(&log, one_day_dump()).expect("write the dump");
        let output = scratch.join("track.gpx");
        let project = scratch.join("trip.json");
        save_project(
            project.to_str().unwrap_or_default(),
            log.to_str().unwrap_or_default(),
            "",
            output.to_str().unwrap_or_default(),
            1,
            "gpx",
            false,
        )
        .expect("save the project");

        let text = std::fs::read_to_string(&project).expect("read the project");
        let document: Value = serde_json::from_str(&text).expect("one JSON object");
        assert_eq!(
            document.get("format").and_then(Value::as_str),
            Some(PROJECT_FORMAT)
        );
        assert_eq!(document["version"], 1);
        assert_eq!(document["input"], "dump");
        assert_eq!(document["utc_offset_days"], 1);
        assert_eq!(document["export_format"], "gpx");
        assert_eq!(document["photos"], Value::Null);

        let loaded = ProjectDocument::read(project.to_str().unwrap_or_default())
            .expect("read the project back");
        assert_eq!(loaded.log, log);
        assert_eq!(loaded.output, output);
        assert!(!loaded.input_is_nmea);
        assert_eq!(loaded.utc_offset_days, 1);
    }

    #[test]
    fn a_project_load_applies_the_stored_day_correction() {
        let scratch = ScratchDir::new("project-shift");
        let log = scratch.join("track.bin");
        std::fs::write(&log, one_day_dump()).expect("write the dump");
        let project = scratch.join("trip.json");
        save_project(
            project.to_str().unwrap_or_default(),
            log.to_str().unwrap_or_default(),
            "",
            scratch.join("out.json").to_str().unwrap_or_default(),
            1,
            "json",
            false,
        )
        .expect("save");
        let document = ProjectDocument::read(project.to_str().unwrap_or_default()).expect("read");
        let loaded = load_log(
            &document.log,
            document.input_is_nmea,
            OffsetSpec::Days(document.utc_offset_days),
        )
        .expect("load the log");
        assert_eq!(
            loaded.track.points.first().and_then(|point| point.utc),
            Some(1_700_000_000 + 86_400)
        );
        assert!(loaded.note.contains("+1 日"), "got {}", loaded.note);
    }

    #[test]
    fn a_relative_path_is_resolved_against_the_project_directory() {
        let scratch = ScratchDir::new("project-relative");
        std::fs::write(scratch.join("track.bin"), one_day_dump()).expect("write the dump");
        let project = scratch.join("trip.json");
        let document = serde_json::json!({
            "format": PROJECT_FORMAT,
            "version": PROJECT_VERSION,
            "input": "dump",
            "log": "track.bin",
            "photos": Value::Null,
            "utc_offset_days": 0,
            "output": "track.gpx",
            "export_format": "gpx",
        });
        std::fs::write(&project, document.to_string()).expect("write the project");
        let loaded = ProjectDocument::read(project.to_str().unwrap_or_default()).expect("read");
        assert_eq!(loaded.log, scratch.join("track.bin"));
        assert_eq!(loaded.output, scratch.join("track.gpx"));
    }

    #[test]
    fn a_foreign_or_incomplete_project_is_refused_by_name() {
        let scratch = ScratchDir::new("project-refuse");
        let project = scratch.join("trip.json");
        for text in [
            "not json",
            "{\"format\": \"something-else\", \"version\": 1}",
            "{\"format\": \"m241-project\", \"version\": 99}",
            "{\"format\": \"m241-project\", \"version\": 1, \"input\": \"dump\"}",
        ] {
            std::fs::write(&project, text).expect("write the bad project");
            let error = ProjectDocument::read(project.to_str().unwrap_or_default())
                .expect_err("must be refused");
            assert!(
                error.contains("trip.json"),
                "{text}: the refusal names the file: {error}"
            );
        }
    }

    #[test]
    fn an_unknown_export_format_in_a_project_is_refused() {
        let scratch = ScratchDir::new("project-format");
        let project = scratch.join("trip.json");
        let document = serde_json::json!({
            "format": PROJECT_FORMAT,
            "version": PROJECT_VERSION,
            "input": "dump",
            "log": "track.bin",
            "photos": Value::Null,
            "utc_offset_days": 0,
            "output": "track.pdf",
            "export_format": "pdf",
        });
        std::fs::write(&project, document.to_string()).expect("write the project");
        let error = ProjectDocument::read(project.to_str().unwrap_or_default())
            .expect_err("pdf is not a format");
        assert!(error.contains("pdf"), "got {error}");
    }

    #[test]
    fn a_saved_project_refuses_to_overwrite_the_log_it_names() {
        let scratch = ScratchDir::new("project-clobber");
        let project = scratch.join("trip.json");
        let error = save_project(
            project.to_str().unwrap_or_default(),
            project.to_str().unwrap_or_default(),
            "",
            scratch.join("out.gpx").to_str().unwrap_or_default(),
            0,
            "gpx",
            false,
        )
        .expect_err("the project is also the log");
        assert!(error.contains("同じパス"), "got {error}");
    }

    #[test]
    fn a_kmz_project_needs_a_photo_folder_and_a_dump() {
        let scratch = ScratchDir::new("project-kmz");
        let project = scratch.join("trip.json");
        let error = save_project(
            project.to_str().unwrap_or_default(),
            scratch.join("track.bin").to_str().unwrap_or_default(),
            "",
            scratch.join("track.kmz").to_str().unwrap_or_default(),
            0,
            "kmz",
            false,
        )
        .expect_err("kmz without photos");
        assert!(error.contains("写真"), "got {error}");
        let error = save_project(
            project.to_str().unwrap_or_default(),
            scratch.join("track.bin").to_str().unwrap_or_default(),
            scratch.path().to_str().unwrap_or_default(),
            scratch.join("track.kmz").to_str().unwrap_or_default(),
            0,
            "kmz",
            true,
        )
        .expect_err("kmz over nmea");
        assert!(error.contains("NMEA"), "got {error}");
    }

    #[test]
    fn read_and_export_writes_the_chosen_format() {
        let scratch = ScratchDir::new("read-export");
        let input = scratch.join("track.bin");
        std::fs::write(&input, one_day_dump()).expect("write the dump");
        let output = scratch.join("track.gpx");
        let (loaded, message) = read_and_export(
            input.to_str().unwrap_or_default(),
            false,
            ExportFormat::Gpx,
            OffsetSpec::Auto,
            output.to_str().unwrap_or_default(),
            false,
        )
        .expect("read and export");
        assert_eq!(loaded.track.points.len(), 1);
        let written = std::fs::read_to_string(&output).expect("read the export");
        assert_eq!(written.matches("<trkpt").count(), 1);
        assert!(message.contains("GPX"), "got {message}");
    }

    #[test]
    fn an_nmea_capture_is_read_as_sentences() {
        let scratch = ScratchDir::new("read-nmea");
        let input = scratch.join("capture.nmea");
        let line = m241_core::protocol::pmtk::sentence(
            "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
        );
        std::fs::write(&input, format!("{line}\r\n")).expect("write the capture");
        let loaded = load_log(&input, true, OffsetSpec::Auto).expect("load the capture");
        assert_eq!(loaded.track.points.len(), 1);
        assert!(loaded.note.contains("NMEA 1 文"), "got {}", loaded.note);
    }

    #[test]
    fn an_nmea_capture_that_yields_no_point_is_an_error() {
        let scratch = ScratchDir::new("read-nmea-empty");
        let input = scratch.join("capture.nmea");
        std::fs::write(&input, "$GPGSA,A,3,01,02,03*00\r\n").expect("write the capture");
        let error = load_log(&input, true, OffsetSpec::Auto).expect_err("no point");
        assert!(error.contains("点を作れませんでした"), "got {error}");
    }
}
