//! Saving and reloading an ezTour-style project: ezTour stores one in a SQLite
//! file (`.itm`) but specifies no schema, so this module defines a minimal JSON
//! one; paths resolve against the project file's directory on load.

use std::path::{Path, PathBuf};

use m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS;
use m241_core::{Error, ExportFormat, Result};
use serde_json::Value;

use crate::Session;
use crate::cal::OffsetSpec;
use crate::cli::{LogKmzOptions, LogProjectLoadOptions, LogProjectSaveOptions, LogReadOptions};
use crate::fileio;
use crate::logcmd;

/// The marker and schema version every project file carries.
pub const PROJECT_FORMAT: &str = "m241-project";
pub const PROJECT_VERSION: u64 = 1;

/// `m241 log project save`: write a project file without opening the log.
pub fn save(options: &LogProjectSaveOptions, session: Session<'_>) -> Result<()> {
    if options.kmz && options.photos.is_none() {
        return Err(Error::State(String::from("a kmz project needs --photos")));
    }
    let photos = options
        .photos
        .as_ref()
        .map(|path| path.display().to_string());
    let format = if options.kmz {
        "kmz"
    } else {
        options.format.extension()
    };
    let document = serde_json::json!({
        "format": PROJECT_FORMAT,
        "version": PROJECT_VERSION,
        "log": options.log.display().to_string(),
        "photos": photos,
        "utc_offset_days": options.utc_offset_days,
        "output": options.output.display().to_string(),
        "export_format": format,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|err| Error::State(format!("could not write the project: {err}")))?;
    fileio::write_text(&options.project, &format!("{text}\n"))?;
    session.console.note(&format!(
        "saved project {} ({format})",
        options.project.display()
    ));
    Ok(())
}

/// `m241 log project load`: read the log the project names and write its export.
pub fn load(options: &LogProjectLoadOptions, session: Session<'_>) -> Result<()> {
    let text = fileio::read_text(&options.project)?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|err| Error::State(format!("not a project file: {err}")))?;
    let marker = document.get("format").and_then(Value::as_str);
    let version = document.get("version").and_then(Value::as_u64);
    if marker != Some(PROJECT_FORMAT) || version != Some(PROJECT_VERSION) {
        return Err(Error::State(String::from(
            "not a project file this build reads",
        )));
    }
    let text_field = |key: &str| document.get(key).and_then(Value::as_str);
    let path = |key: &str| {
        text_field(key)
            .map(PathBuf::from)
            .ok_or_else(|| Error::State(format!("project has no {key} path")))
    };
    let format_text = text_field("export_format")
        .ok_or_else(|| Error::State(String::from("project has no export_format")))?;
    let offset = document
        .get("utc_offset_days")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let offset =
        i32::try_from(offset).map_err(|_| Error::State(format!("bad utc_offset_days {offset}")))?;
    let utc_offset_days = OffsetSpec::Days(offset);
    let photos = text_field("photos").map(PathBuf::from);
    let base = options
        .project
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    let base = base.unwrap_or_else(|| Path::new("."));
    let log = base.join(path("log")?);
    let output = base.join(path("output")?);
    if format_text.eq_ignore_ascii_case("kmz") {
        let photos = photos
            .ok_or_else(|| Error::State(String::from("a kmz project needs a photo folder")))?;
        let kmz = LogKmzOptions {
            log,
            photos: vec![base.join(photos)],
            output,
            max_photo_edge: None,
            max_gap: DEFAULT_MAX_GAP_SECONDS,
            utc_offset_days,
            tz_offset_minutes: 0,
        };
        return logcmd::kmz(&kmz, session);
    }
    let format = ExportFormat::parse(format_text)
        .ok_or_else(|| Error::State(format!("bad export_format {format_text:?}")))?;
    let read = LogReadOptions {
        nmea: false,
        file: log,
        format,
        output: Some(output),
        utc_offset_days,
    };
    logcmd::read(&read, session)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AutoConfirm, FakePorts, RecordingConsole, Scratch, synthetic_dump};

    fn options(project: PathBuf, log: PathBuf, output: PathBuf) -> LogProjectSaveOptions {
        LogProjectSaveOptions {
            project,
            log,
            output,
            format: ExportFormat::Csv,
            kmz: false,
            photos: None,
            utc_offset_days: 0,
        }
    }

    #[test]
    fn a_project_round_trips_its_settings_and_writes_the_named_export() {
        let scratch = Scratch::new("project-round-trip");
        let log = scratch.join("track.bin");
        fileio::write(&log, &synthetic_dump(3)).expect("write a dump");
        let output = scratch.join("track.csv");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let log_text = log.display().to_string();
        save(
            &options(project.clone(), log, output.clone()),
            Session::new(&links, &console, &confirm),
        )
        .expect("save the project");
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&project).expect("read the project"))
                .expect("the project is one JSON object");
        assert_eq!(
            document.get("format").and_then(Value::as_str),
            Some(PROJECT_FORMAT)
        );
        assert_eq!(
            document.get("export_format").and_then(Value::as_str),
            Some("csv")
        );
        assert_eq!(
            document.get("log").and_then(Value::as_str),
            Some(log_text.as_str())
        );

        load(
            &LogProjectLoadOptions {
                project: project.clone(),
            },
            Session::new(&links, &console, &confirm),
        )
        .expect("load the project");
        let written = std::fs::read_to_string(&output).expect("read the export");
        assert_eq!(
            written.lines().next().expect("a header"),
            "INDEX,TRACK NUMBER,UTC DATE,UTC TIME,LOCAL DATE,LOCAL TIME,LATITUDE,N/S,LONGITUDE,E/W,ALTITUDE,SPEED"
        );
        assert_eq!(written.lines().count(), 4, "header plus three points");
    }

    #[test]
    fn a_project_that_is_not_one_is_refused_not_guessed() {
        let scratch = Scratch::new("project-foreign");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        for text in [
            "not json",
            "{\"format\": \"something-else\", \"version\": 1}",
            "{\"format\": \"m241-project\", \"version\": 99}",
        ] {
            fileio::write_text(&project, text).expect("write the bad project");
            let err = load(
                &LogProjectLoadOptions {
                    project: project.clone(),
                },
                Session::new(&links, &console, &confirm),
            )
            .expect_err("a foreign document is not a project");
            assert!(matches!(err, Error::State(_)), "{text}: got {err:?}");
        }
    }

    #[test]
    fn a_kmz_project_without_a_photo_folder_is_refused() {
        let scratch = Scratch::new("project-kmz");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut save_options = options(
            scratch.join("trip.json"),
            scratch.join("track.bin"),
            scratch.join("track.kmz"),
        );
        save_options.kmz = true;
        let err = save(&save_options, Session::new(&links, &console, &confirm))
            .expect_err("a kmz project needs photos");
        assert!(matches!(err, Error::State(_)), "got {err:?}");
    }
}
