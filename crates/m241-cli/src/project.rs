//! Saving and reloading an ezTour-style project: ezTour stores one in a SQLite
//! file (`.itm`) but specifies no schema, so this module defines a minimal JSON
//! one. It is not an `.itm` reader and does not interoperate with ezTour itself.
//!
//! A project carries eight keys: `format`, `version`, `input`, `log`, `photos`,
//! `utc_offset_days`, `output` and `export_format` (`docs/reference/02-cli.md`).
//! `log`, `photos` and `output` are resolved against the **project file's
//! directory**, never the current directory. `save` therefore stores a relative
//! path relative to that directory — the same string means the same file on
//! load, from any cwd, and the project can be moved together with what it
//! names. An absolute path is kept as an absolute path, because its meaning is
//! already specific to the machine it names.
//!
//! Stored paths use `/` as their separator on every platform: the file outlives
//! the machine that wrote it, and `Path` accepts `/` everywhere this tool builds
//! for, so `load` turns the string straight back into a path.
//!
//! `input` records whether the log is a raw dump (`log download`) or an NMEA
//! 0183 capture (`log read --nmea`), so `load` reads it back the same way.

use std::path::{Component, Path, PathBuf};

use m241_core::log::geotag::DEFAULT_MAX_GAP_SECONDS;
use m241_core::{Error, ExportFormat, Result};
use serde_json::Value;

use crate::Session;
use crate::cal::OffsetSpec;
use crate::cli::{
    CoordFormatArg, LogKmzOptions, LogProjectLoadOptions, LogProjectSaveOptions, LogReadOptions,
};
use crate::fileio;
use crate::logcmd;

/// The marker and schema version every project file carries.
const PROJECT_FORMAT: &str = "m241-project";
const PROJECT_VERSION: u64 = 1;

/// How a project's log is read back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    /// A raw dump written by `log download`.
    Dump,
    /// An NMEA 0183 capture, as `log read --nmea`.
    Nmea,
}

impl InputKind {
    /// The value stored under `input`.
    fn label(self) -> &'static str {
        match self {
            Self::Dump => "dump",
            Self::Nmea => "nmea",
        }
    }

    /// The stored value for `save`'s `--nmea` flag.
    fn from_flag(nmea: bool) -> Self {
        if nmea { Self::Nmea } else { Self::Dump }
    }

    /// The kind a stored value names, or `None` when this build does not know it.
    fn parse(text: &str) -> Option<Self> {
        match text {
            "dump" => Some(Self::Dump),
            "nmea" => Some(Self::Nmea),
            _ => None,
        }
    }

    /// Does `log read` need `--nmea` for this kind?
    fn is_nmea(self) -> bool {
        matches!(self, Self::Nmea)
    }
}

/// `m241 log project save`: write a project file without opening the log.
///
/// # Errors
///
/// [`Error::State`] when a `--kmz` project has no `--photos` folder or is asked
/// to read an NMEA capture; [`Error::Safety`] when the project path is also the
/// log or the output path (the project would overwrite the file it names);
/// [`Error::Io`] naming the project file when it cannot be written.
pub fn save(options: &LogProjectSaveOptions, session: Session<'_>) -> Result<()> {
    if options.kmz && options.photos.is_none() {
        return Err(Error::State(String::from("a kmz project needs --photos")));
    }
    if options.kmz && options.nmea {
        return Err(Error::State(String::from(
            "a kmz project needs a raw dump, not an NMEA capture",
        )));
    }
    // The project file is JSON text. Writing it over the dump would destroy the
    // only copy this tool can offer of the user's track; the same check as
    // `fw patch`, for the same reason.
    fileio::ensure_distinct(&options.project, &options.log)?;
    fileio::ensure_distinct(&options.project, &options.output)?;
    let base = project_base(&options.project)?;
    let photos = options
        .photos
        .as_deref()
        .map(|path| stored_path(&base, path))
        .transpose()?;
    let format = if options.kmz {
        "kmz"
    } else {
        options.format.extension()
    };
    let document = serde_json::json!({
        "format": PROJECT_FORMAT,
        "version": PROJECT_VERSION,
        "input": InputKind::from_flag(options.nmea).label(),
        "log": stored_path(&base, &options.log)?,
        "photos": photos,
        "utc_offset_days": options.utc_offset_days,
        "output": stored_path(&base, &options.output)?,
        "export_format": format,
    });
    let text = serde_json::to_string_pretty(&document).map_err(|err| {
        Error::State(format!(
            "could not serialise project {}: {err}",
            options.project.display()
        ))
    })?;
    fileio::write_text(&options.project, &format!("{text}\n"))?;
    session.console.note(&format!(
        "saved project {} ({format})",
        options.project.display()
    ));
    Ok(())
}

/// `m241 log project load`: read the log the project names and write its export.
///
/// # Errors
///
/// [`Error::State`] naming the project file when it is not a project this build
/// reads, has no required field, names an input kind or export format this build
/// does not know, stores a `utc_offset_days` that is not a whole number in
/// range, or asks a `kmz` project to read an NMEA capture; [`Error::Io`] naming
/// the file that cannot be read or written; whatever `log read` / `log kmz`
/// returns for the log itself.
pub fn load(options: &LogProjectLoadOptions, session: Session<'_>) -> Result<()> {
    let document = ProjectDocument::read(&options.project)?;
    let base = project_base(&options.project)?;
    let log = base.join(Path::new(&document.log));
    let output = base.join(Path::new(&document.output));
    let utc_offset_days = OffsetSpec::Days(document.utc_offset_days);
    if document.export_format.eq_ignore_ascii_case("kmz") {
        if document.input.is_nmea() {
            return Err(Error::State(format!(
                "{}: a kmz project needs a raw dump, not an NMEA capture",
                options.project.display()
            )));
        }
        let photos = document.photos.as_deref().ok_or_else(|| {
            Error::State(format!(
                "{}: a kmz project needs a photos folder, but the project has null",
                options.project.display()
            ))
        })?;
        let kmz = LogKmzOptions {
            log,
            photos: vec![base.join(photos)],
            output,
            max_photo_edge: None,
            max_gap: DEFAULT_MAX_GAP_SECONDS,
            utc_offset_days,
            tz_offset_minutes: 0,
            // A project records no thumbnail setting; a KMZ loaded from a
            // project is the same archive `log kmz` writes without `--icons`.
            icons: None,
        };
        return logcmd::kmz(&kmz, session);
    }
    let format = document.format(&options.project)?;
    let read = LogReadOptions {
        nmea: document.input.is_nmea(),
        gpx: false,
        file: log,
        format,
        csv_report: false,
        coord_format: CoordFormatArg::Decimal,
        simplify: None,
        cache: None,
        output: Some(output),
        utc_offset_days,
    };
    logcmd::read(&read, session)
}

/// One project document, with every required key checked.
#[derive(Debug)]
struct ProjectDocument {
    input: InputKind,
    log: String,
    photos: Option<String>,
    utc_offset_days: i32,
    output: String,
    export_format: String,
}

impl ProjectDocument {
    /// Read `project` and refuse anything this build cannot honour.
    ///
    /// Every refusal names the project file: "a project is malformed" without
    /// the path leaves the user guessing which of several project files is
    /// wrong (`fileio`'s rule for I/O failures, applied here too).
    fn read(project: &Path) -> Result<Self> {
        let text = fileio::read_text(project)?;
        let document: Value = serde_json::from_str(&text).map_err(|err| {
            Error::State(format!("{}: not a project file: {err}", project.display()))
        })?;
        let marker = document.get("format").and_then(Value::as_str);
        if marker != Some(PROJECT_FORMAT) {
            return Err(Error::State(format!(
                "{}: not a project file: format marker is {marker:?}, expected {PROJECT_FORMAT:?}",
                project.display()
            )));
        }
        let version = document.get("version").and_then(Value::as_u64);
        if version != Some(PROJECT_VERSION) {
            return Err(Error::State(format!(
                "{}: project version {version:?} is not one this build reads (it reads version \
                 {PROJECT_VERSION})",
                project.display()
            )));
        }
        let text_field = |key: &str| -> Result<String> {
            document
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    Error::State(format!(
                        "{}: project has no {key} (or it is not a string); nothing is defaulted",
                        project.display()
                    ))
                })
        };
        let input_text = text_field("input")?;
        let input = InputKind::parse(&input_text).ok_or_else(|| {
            Error::State(format!(
                "{}: unknown input {input_text:?}; this build reads \"dump\" or \"nmea\"",
                project.display()
            ))
        })?;
        let export_format = text_field("export_format")?;
        // The day correction is the reason a project exists; a missing or
        // misspelt key must not silently become 0, which would shift every
        // timestamp by a different number of days than the user saved.
        let offset = document.get("utc_offset_days").ok_or_else(|| {
            Error::State(format!(
                "{}: project has no utc_offset_days; nothing is defaulted",
                project.display()
            ))
        })?;
        let offset = offset.as_i64().ok_or_else(|| {
            Error::State(format!(
                "{}: utc_offset_days is not a whole number: {offset}",
                project.display()
            ))
        })?;
        let utc_offset_days = i32::try_from(offset).map_err(|_| {
            Error::State(format!(
                "{}: utc_offset_days {offset} is out of range",
                project.display()
            ))
        })?;
        let photos = match document.get("photos") {
            None => {
                return Err(Error::State(format!(
                    "{}: project has no photos key; use null when there is no photo folder",
                    project.display()
                )));
            }
            Some(Value::Null) => None,
            Some(Value::String(path)) => Some(path.clone()),
            Some(other) => {
                return Err(Error::State(format!(
                    "{}: photos is neither a path nor null: {other}",
                    project.display()
                )));
            }
        };
        Ok(Self {
            input,
            log: text_field("log")?,
            photos,
            utc_offset_days,
            output: text_field("output")?,
            export_format,
        })
    }

    /// The export format, refused when this build has no writer for it.
    fn format(&self, project: &Path) -> Result<ExportFormat> {
        ExportFormat::parse(&self.export_format).ok_or_else(|| {
            Error::State(format!(
                "{}: unknown export_format {:?}; this build reads gpx, kml, csv, nmea, json or kmz",
                project.display(),
                self.export_format
            ))
        })
    }
}

/// The directory a project's paths are stored against: the project file's own
/// parent, made absolute and lexically normalised.
fn project_base(project: &Path) -> Result<PathBuf> {
    let parent = project
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if parent.is_absolute() {
        return Ok(normalize(parent));
    }
    let cwd = std::env::current_dir().map_err(|source| Error::io(".", source))?;
    Ok(normalize(&cwd.join(parent)))
}

/// Drop `.` components and resolve `..` lexically.
///
/// `path` is always absolute here, so a leading `..` cannot climb past the root.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The string `save` stores for `path`.
///
/// An absolute path is stored as an absolute path: it already means the same
/// file from any directory on the machine it names. A relative path is resolved
/// against the current directory once — that is what it meant when the user
/// typed it — and stored relative to `base`, which is where `load` will resolve
/// it. Both spellings use `/` separators (see [`stored_spelling`]).
fn stored_path(base: &Path, path: &Path) -> Result<String> {
    if path.is_absolute() {
        return Ok(stored_spelling(path));
    }
    let cwd = std::env::current_dir().map_err(|source| Error::io(".", source))?;
    let absolute = normalize(&cwd.join(path));
    Ok(stored_relative(base, &absolute))
}

/// `path` as `save` stores it, relative to `base`, with `/` separators.
fn stored_relative(base: &Path, path: &Path) -> String {
    stored_spelling(&relative_from(base, path))
}

/// The stored spelling of a path: `/` separators on every platform.
///
/// The project file is meant to move with the files it names, possibly to
/// another OS, so it must not encode `std::path::MAIN_SEPARATOR`. Every platform
/// this tool builds for accepts a `/` in a path, so `load` passes the string to
/// [`std::path::Path`] unchanged.
fn stored_spelling(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

/// `path` written relative to `base`, with `..` components when it sits above it.
///
/// Both arguments are absolute; when they share no component at all (a different
/// drive on Windows) `path` is returned unchanged.
fn relative_from(base: &Path, path: &Path) -> PathBuf {
    let base_components: Vec<Component<'_>> = base.components().collect();
    let path_components: Vec<Component<'_>> = path.components().collect();
    let common = base_components
        .iter()
        .zip(&path_components)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return path.to_path_buf();
    }
    let mut out = PathBuf::new();
    for _ in common..base_components.len() {
        out.push("..");
    }
    for component in path_components.iter().skip(common) {
        out.push(component.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
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
            nmea: false,
        }
    }

    fn load_options(project: PathBuf) -> LogProjectLoadOptions {
        LogProjectLoadOptions { project }
    }

    /// A project file with every required key, as `save` would write it.
    fn a_valid_document() -> Value {
        serde_json::json!({
            "format": PROJECT_FORMAT,
            "version": PROJECT_VERSION,
            "input": "dump",
            "log": "track.bin",
            "photos": Value::Null,
            "utc_offset_days": 0,
            "output": "track.gpx",
            "export_format": "gpx",
        })
    }

    /// Run `body` with the process working directory set to `dir`.
    ///
    /// The working directory is process-wide, so this locks out the other tests
    /// that need it and restores it even when `body` panics.
    fn in_directory<T>(dir: &Path, body: impl FnOnce() -> T) -> T {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::current_dir().expect("current directory");
        std::env::set_current_dir(dir).expect("enter the scratch directory");
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.0);
            }
        }
        let _restore = Restore(previous);
        body()
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

        save(
            &options(project.clone(), log.clone(), output.clone()),
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
        assert_eq!(document["input"], "dump", "the input kind is recorded");
        assert_eq!(
            document.get("export_format").and_then(Value::as_str),
            Some("csv")
        );
        // Compared as a path, not as the platform's spelling: the stored form
        // uses `/` on every OS, and `Path` treats that as the same file.
        let stored_log = document
            .get("log")
            .and_then(Value::as_str)
            .expect("the log path is stored");
        assert_eq!(
            Path::new(stored_log),
            log.as_path(),
            "an absolute path is stored as a path naming the same file"
        );
        assert_eq!(document["photos"], Value::Null, "no photo folder is null");

        load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect("load the project");
        let written = std::fs::read_to_string(&output).expect("read the export");
        let header = m241_core::log::export::CSV_COLUMNS.join(",");
        assert_eq!(written.lines().next().expect("a header"), header);
        assert_eq!(written.lines().count(), 4, "header plus three points");
    }

    #[test]
    fn a_project_in_a_subdirectory_round_trips_relative_paths() {
        // The reviewer's reproduction: the user types the paths at the cwd the
        // project is created from, while `load` resolves them against the
        // project directory. Storing "track.bin" verbatim would make load look
        // for `proj/track.bin`; `../track.bin` is the same file on load.
        let scratch = Scratch::new("project-subdir");
        fileio::write(&scratch.join("track.bin"), &synthetic_dump(3)).expect("write a dump");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let session = || Session::new(&links, &console, &confirm);

        in_directory(scratch.path(), || {
            save(
                &options(
                    PathBuf::from("proj/trip.json"),
                    PathBuf::from("track.bin"),
                    PathBuf::from("proj/track.csv"),
                ),
                session(),
            )
            .expect("save the project");
            let document: Value = serde_json::from_str(
                &std::fs::read_to_string(scratch.join("proj/trip.json")).expect("read the project"),
            )
            .expect("the project is one JSON object");
            assert_eq!(document["log"], "../track.bin", "stored from `proj/`");
            assert_eq!(document["output"], "track.csv", "inside `proj/` already");

            load(&load_options(PathBuf::from("proj/trip.json")), session())
                .expect("load from the directory that saved it");
        });

        // Nothing in `load` depends on the cwd: the same project also loads
        // from outside, given a path to the file.
        load(&load_options(scratch.join("proj/trip.json")), session())
            .expect("load from another directory");
        let written = std::fs::read_to_string(scratch.join("proj/track.csv")).expect("read it");
        assert_eq!(written.lines().count(), 4, "header plus three points");
    }

    #[test]
    fn a_relative_path_is_stored_against_the_project_directory_with_slashes() {
        // The case from the review, as pure path arithmetic: cwd `/work`,
        // project `/work/proj/trip.json`, log `track.bin` typed at that cwd.
        // The expected strings use `/` on every platform, because that is what
        // `save` writes into the (portable) project file.
        assert_eq!(
            stored_relative(Path::new("/work/proj"), Path::new("/work/track.bin")),
            "../track.bin"
        );
        assert_eq!(
            stored_relative(Path::new("/work/proj"), Path::new("/work/proj/track.csv")),
            "track.csv"
        );
        assert_eq!(
            stored_relative(Path::new("/work/proj"), Path::new("/other/track.bin")),
            "../../other/track.bin"
        );
        assert_eq!(
            stored_relative(Path::new("/work"), Path::new("/work")),
            ".",
            "the directory itself is `.`, not an empty path"
        );
    }

    #[test]
    fn a_project_records_the_input_kind_and_load_reads_it_back() {
        use m241_core::protocol::pmtk::sentence;

        let scratch = Scratch::new("project-nmea");
        let capture = scratch.join("capture.nmea");
        fileio::write_text(
            &capture,
            &format!(
                "{}\r\n",
                sentence("GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,")
            ),
        )
        .expect("write a capture");
        let output = scratch.join("track.gpx");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut save_options = options(project.clone(), capture, output.clone());
        save_options.nmea = true;
        save_options.format = ExportFormat::Gpx;

        save(&save_options, Session::new(&links, &console, &confirm)).expect("save the project");
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&project).expect("read the project"))
                .expect("one JSON object");
        assert_eq!(document["input"], "nmea", "the input kind is stored");

        load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect("load reads the capture as NMEA");
        let written = std::fs::read_to_string(&output).expect("read the export");
        assert_eq!(written.matches("<trkpt").count(), 1);
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
                &load_options(project.clone()),
                Session::new(&links, &console, &confirm),
            )
            .expect_err("a foreign document is not a project");
            assert!(matches!(err, Error::State(_)), "{text}: got {err:?}");
            assert!(
                format!("{err}").contains("trip.json"),
                "{text}: the refusal names the file: {err}"
            );
        }
    }

    #[test]
    fn a_project_error_names_the_file_and_the_version_it_found() {
        let scratch = Scratch::new("project-version");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        fileio::write_text(&project, "{\"format\": \"m241-project\", \"version\": 99}")
            .expect("write the project");

        let err = load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("another version is not one this build reads");
        let text = format!("{err}");
        assert!(text.contains("trip.json"), "wrong file is named: {text}");
        assert!(text.contains("99"), "the found version is named: {text}");
        assert!(
            text.contains("version 1"),
            "the supported version is named: {text}"
        );
    }

    #[test]
    fn every_required_field_is_refused_when_missing_and_the_file_is_named() {
        let scratch = Scratch::new("project-missing");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        for key in [
            "format",
            "version",
            "input",
            "log",
            "photos",
            "utc_offset_days",
            "output",
            "export_format",
        ] {
            let mut document = a_valid_document();
            document.as_object_mut().expect("a JSON object").remove(key);
            fileio::write_text(&project, &document.to_string()).expect("write the project");
            let err = load(
                &load_options(project.clone()),
                Session::new(&links, &console, &confirm),
            )
            .expect_err("a missing required field is an error");
            let text = format!("{err}");
            assert!(text.contains("trip.json"), "{key}: {text}");
            assert!(
                text.contains(key),
                "{key}: the missing key is named: {text}"
            );
        }

        // A present-but-wrong type is refused too: `"0"` is not a day count, and
        // accepting it would guess the very correction the project exists to
        // store.
        let mut document = a_valid_document();
        document["utc_offset_days"] = serde_json::json!("0");
        fileio::write_text(&project, &document.to_string()).expect("write the project");
        let err = load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("a string is not a day count");
        let text = format!("{err}");
        assert!(text.contains("trip.json"), "{text}");
        assert!(text.contains("utc_offset_days"), "{text}");
    }

    #[test]
    fn an_unknown_export_format_in_a_project_is_refused() {
        let scratch = Scratch::new("project-format");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut document = a_valid_document();
        document["export_format"] = serde_json::json!("pdf");
        fileio::write_text(&project, &document.to_string()).expect("write the project");

        let err = load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("pdf is not an export format");
        let text = format!("{err}");
        assert!(text.contains("trip.json"), "{text}");
        assert!(text.contains("pdf"), "the value is quoted: {text}");
    }

    #[test]
    fn a_kmz_project_without_a_photo_folder_is_refused_on_load() {
        let scratch = Scratch::new("project-kmz-load");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut document = a_valid_document();
        document["export_format"] = serde_json::json!("kmz");
        fileio::write_text(&project, &document.to_string()).expect("write the project");

        let err = load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("kmz needs photos");
        let text = format!("{err}");
        assert!(text.contains("trip.json"), "{text}");
        assert!(
            text.contains("photos"),
            "the missing input is named: {text}"
        );
    }

    #[test]
    fn a_kmz_project_cannot_read_an_nmea_capture() {
        let scratch = Scratch::new("project-kmz-nmea");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut document = a_valid_document();
        document["export_format"] = serde_json::json!("kmz");
        document["input"] = serde_json::json!("nmea");
        document["photos"] = serde_json::json!("photos");
        fileio::write_text(&project, &document.to_string()).expect("write the project");

        let err = load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("kmz reads a dump");
        let text = format!("{err}");
        assert!(text.contains("trip.json"), "{text}");
        assert!(text.contains("NMEA"), "the mismatch is named: {text}");
    }

    #[test]
    fn saving_a_project_does_not_open_the_log() {
        let scratch = Scratch::new("project-lazy");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let project = scratch.join("trip.json");
        save(
            &options(
                project.clone(),
                scratch.join("never-written.bin"),
                scratch.join("out.gpx"),
            ),
            Session::new(&links, &console, &confirm),
        )
        .expect("save names the log, it does not read it");
        assert!(project.exists());
    }

    #[test]
    fn saving_a_project_replaces_an_existing_file() {
        let scratch = Scratch::new("project-replace");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        save(
            &options(
                project.clone(),
                scratch.join("track.bin"),
                scratch.join("out.csv"),
            ),
            Session::new(&links, &console, &confirm),
        )
        .expect("first save");
        let mut second = options(
            project.clone(),
            scratch.join("track.bin"),
            scratch.join("out.json"),
        );
        second.format = ExportFormat::Json;
        save(&second, Session::new(&links, &console, &confirm)).expect("second save");
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&project).expect("read the project"))
                .expect("one JSON object");
        assert_eq!(document["export_format"], "json", "the file was replaced");
    }

    #[test]
    fn a_project_load_applies_the_stored_day_correction_and_announces_it() {
        let scratch = Scratch::new("project-shift");
        let log = scratch.join("track.bin");
        fileio::write(&log, &synthetic_dump(1)).expect("write a dump");
        let output = scratch.join("track.json");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut save_options = options(project.clone(), log, output.clone());
        save_options.format = ExportFormat::Json;
        save_options.utc_offset_days = 1;
        save(&save_options, Session::new(&links, &console, &confirm)).expect("save the project");

        load(
            &load_options(project),
            Session::new(&links, &console, &confirm),
        )
        .expect("load the project");
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read the export"))
                .expect("one JSON object");
        // The synthetic dump starts at 2023-11-14T22:13:20Z; +1 day is the
        // correction the project stored.
        assert_eq!(
            document["points"][0]["utc"],
            1_700_000_000u32 + 86_400,
            "the stored day correction was applied"
        );
        assert!(
            console.stderr().contains("+1 days"),
            "the shift is announced as `log read` announces it: {}",
            console.stderr()
        );
    }

    #[test]
    fn a_kmz_project_records_the_kmz_export_format_and_photo_folder() {
        let scratch = Scratch::new("project-kmz-save");
        let project = scratch.join("trip.json");
        let photos = scratch.join("photos");
        std::fs::create_dir_all(&photos).expect("photo folder");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;
        let mut save_options = options(
            project.clone(),
            scratch.join("track.bin"),
            scratch.join("track.kmz"),
        );
        save_options.kmz = true;
        save_options.photos = Some(photos.clone());
        save(&save_options, Session::new(&links, &console, &confirm))
            .expect("save the kmz project");

        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&project).expect("read the project"))
                .expect("one JSON object");
        assert_eq!(document["export_format"], "kmz");
        // The stored spelling is `/`-separated on every platform; compare it as
        // a path so the assertion does not encode the host's separator.
        let stored_photos = document
            .get("photos")
            .and_then(Value::as_str)
            .expect("the photo folder is stored");
        assert_eq!(Path::new(stored_photos), photos.as_path());
    }

    #[test]
    fn saving_a_project_over_the_files_it_names_is_refused() {
        let scratch = Scratch::new("project-clobber");
        let project = scratch.join("trip.json");
        let links = FakePorts::empty();
        let console = RecordingConsole::default();
        let confirm = AutoConfirm;

        let log_err = save(
            &options(project.clone(), project.clone(), scratch.join("out.gpx")),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("the project must not overwrite its log");
        assert!(matches!(log_err, Error::Safety { .. }), "got {log_err}");

        let output_err = save(
            &options(project.clone(), scratch.join("track.bin"), project.clone()),
            Session::new(&links, &console, &confirm),
        )
        .expect_err("the project must not overwrite its output");
        assert!(
            matches!(output_err, Error::Safety { .. }),
            "got {output_err}"
        );
    }
}
