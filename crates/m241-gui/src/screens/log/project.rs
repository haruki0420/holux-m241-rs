//! The JSON project document and the `log read` conversion it drives.
//!
//! `m241 log project save` defines a small JSON schema (`format`, `version`,
//! `input`, `log`, `photos`, `utc_offset_days`, `output`, `export_format`);
//! `log project load` reads the named log, applies the stored day correction and
//! writes the export. This module keeps the same schema and the same path rule,
//! so a project written here loads with the CLI and a CLI project loads here:
//!
//! * a relative `LOG` / `PHOTOS` / `OUTPUT` is resolved against the current
//!   directory once, then stored **relative to the project file's directory**
//!   (`02-cli.md` §`log project save`), so the project directory can be moved as
//!   one unit with the files it names;
//! * an absolute path is stored absolute — it already names one machine's file;
//! * stored paths use `/` on every platform, and `load` resolves them against
//!   the project directory.
//!
//! It also owns the GUI's `log read` equivalent: choose a dump or an NMEA
//! capture, a format, a day offset and an output, then read and write once. The
//! `--format json` facts that are not part of the track — the NMEA line counts
//! and the applied day shift — are attached here as the CLI attaches them, so
//! the same input produces the same JSON object from either front end.

use std::path::{Component, Path, PathBuf};

use m241_core::log::{ParseReport, export, import_nmea, parse_dump};
use m241_core::{ExportFormat, Track};
use serde_json::Value;

use super::nmea::{OffsetSpec, apply_to_track};

/// The marker and schema version every project file carries.
const PROJECT_FORMAT: &str = "m241-project";
const PROJECT_VERSION: u64 = 1;

/// The NMEA line counts `log read --nmea` announces.
///
/// Kept after [`m241_core::log::NmeaImport`]'s track has moved out, so a JSON
/// reader still gets the counts the status line showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NmeaCounts {
    /// Sentences whose fields parsed.
    pub imported: usize,
    /// Sentences that produced no point.
    pub dropped: usize,
    /// Lines this reader never turns into a point.
    pub ignored: usize,
}

/// The facts a read announced outside the export.
///
/// `log read` writes these into `--format json` and nowhere else: `offset_days`
/// when something was shifted, `nmea` when the input was a capture. A field is
/// absent when the fact does not apply, exactly as the CLI leaves it out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadFacts {
    /// Days applied to the track; `0` means no field is written.
    pub offset_days: i32,
    /// Line counts for an NMEA capture; `None` for a dump.
    pub nmea: Option<NmeaCounts>,
}

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
        let base = project_base(&path)?;
        let resolve = |text: &str| resolve_stored(&base, text);
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
    /// The facts a JSON export carries beside the track.
    pub facts: ReadFacts,
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
    let (mut track, report, mut note, nmea_counts) = if nmea {
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
        let counts = NmeaCounts {
            imported: imported.imported,
            dropped: imported.dropped,
            ignored: imported.ignored,
        };
        (imported.track, ParseReport::default(), note, Some(counts))
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
        (track, report, note, None)
    };
    let shift = apply_to_track(&mut track, offset);
    if shift != 0 {
        note.push_str(&format!("、日付を {shift:+} 日補正"));
    }
    Ok(LoadedLog {
        track,
        report,
        note,
        facts: ReadFacts {
            offset_days: shift,
            nmea: nmea_counts,
        },
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
    let document = attach_read_report(export(&loaded.track, format), format, loaded.facts);
    write_text(path, &document)?;
    let message = format!(
        "{} 点を {trimmed} に {} で書き出しました",
        loaded.track.points.len(),
        format.extension().to_uppercase()
    );
    Ok((loaded, message))
}

/// Add the announced facts that are not part of the export to `--format json`.
///
/// This mirrors `m241-cli`'s `attach_read_report` (`crates/m241-cli/src/logcmd.rs`)
/// field for field: `offset_days` when something was shifted, `nmea` when the
/// input was a capture, and neither when the fact does not apply. A non-JSON
/// format and a non-object export are returned exactly as the exporter wrote
/// them. It lives here rather than in `m241-core` because core's `export` has no
/// notion of how the track was read; the GUI must not be the second front end
/// with a different JSON schema.
pub fn attach_read_report(exported: String, format: ExportFormat, facts: ReadFacts) -> String {
    if format != ExportFormat::Json || (facts.offset_days == 0 && facts.nmea.is_none()) {
        return exported;
    }
    let Ok(Value::Object(mut object)) = serde_json::from_str(&exported) else {
        return exported;
    };
    if facts.offset_days != 0 {
        object.insert(String::from("offset_days"), Value::from(facts.offset_days));
    }
    if let Some(nmea) = facts.nmea {
        object.insert(
            String::from("nmea"),
            serde_json::json!({
                "imported": nmea.imported,
                "dropped": nmea.dropped,
                "ignored": nmea.ignored,
            }),
        );
    }
    format!("{}\n", Value::Object(object))
}

/// The inputs of [`save_project`].
///
/// A struct rather than seven positional arguments: every path is a `&str`, so a
/// call site could swap the project and the log without the compiler noticing.
#[derive(Debug, Clone, Copy)]
pub struct ProjectSave<'a> {
    /// Where the JSON document goes.
    pub project: &'a str,
    /// The log the document names.
    pub log: &'a str,
    /// Photo folder; empty for none.
    pub photos: &'a str,
    /// Where `load` writes the export.
    pub output: &'a str,
    /// The stored day correction.
    pub utc_offset_days: i32,
    /// `gpx`/`kml`/`csv`/`nmea`/`json`, or `kmz`.
    pub export_format: &'a str,
    /// The log is an NMEA capture.
    pub nmea: bool,
}

/// Write a project document without reading the log it names.
///
/// # Errors
///
/// An empty path, a project path that is also its log or output, a KMZ project
/// without a photo folder or over an NMEA capture, an unknown format, or an I/O
/// failure.
pub fn save_project(options: &ProjectSave<'_>) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .map_err(|err| format!("現在のディレクトリを取得できません: {err}"))?;
    save_project_in(options, &cwd)
}

/// [`save_project`] with the directory relative inputs are typed in made
/// explicit, so the CLI's rule can be tested without changing the process cwd.
///
/// A relative `LOG` / `PHOTOS` / `OUTPUT` is resolved against `cwd` once (what it
/// meant when the user typed it) and stored relative to the project's directory
/// ([`stored_path`]); `load` resolves it there. That is what makes the project
/// directory movable as one unit (`02-cli.md` §`log project save`).
fn save_project_in(options: &ProjectSave<'_>, cwd: &Path) -> Result<PathBuf, String> {
    let ProjectSave {
        project,
        log,
        photos,
        output,
        utc_offset_days,
        export_format,
        nmea,
    } = *options;
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
    if same_path_in(&project_path, &log_path, cwd) || same_path_in(&project_path, &output_path, cwd)
    {
        return Err(String::from(
            "プロジェクトファイルがログや書き出し先と同じパスです。別の名前にしてください",
        ));
    }
    // The project file itself is written where the typed path points when the
    // caller's cwd is the process cwd; making that explicit is what lets the
    // test drive a different cwd without changing the process one.
    let project_file = if project_path.is_absolute() {
        normalize(&project_path)
    } else {
        normalize(&cwd.join(&project_path))
    };
    let base = project_base_in(&project_file, cwd);
    let document = serde_json::json!({
        "format": PROJECT_FORMAT,
        "version": PROJECT_VERSION,
        "input": if nmea { "nmea" } else { "dump" },
        "log": stored_path(&base, &log_path, cwd),
        "photos": if photos_trimmed.is_empty() {
            Value::Null
        } else {
            Value::String(stored_path(&base, Path::new(photos_trimmed), cwd))
        },
        "utc_offset_days": utc_offset_days,
        "output": stored_path(&base, &output_path, cwd),
        "export_format": export_format,
    });
    let text = serde_json::to_string_pretty(&document)
        .map_err(|err| format!("プロジェクトを JSON にできません: {err}"))?;
    write_text(&project_file, &format!("{text}\n"))?;
    Ok(project_path)
}

/// The directory a project file's stored paths are relative to.
fn project_base(project: &Path) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir()
        .map_err(|err| format!("現在のディレクトリを取得できません: {err}"))?;
    Ok(project_base_in(project, &cwd))
}

/// [`project_base`] against an explicit cwd.
///
/// The project's parent is made absolute and lexically normalised, so a project
/// typed as `proj/trip.json` and one typed as `./proj/../proj/trip.json` share the
/// same base and therefore the same stored spellings.
fn project_base_in(project: &Path, cwd: &Path) -> PathBuf {
    let parent = project
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let absolute = if parent.is_absolute() {
        parent.to_path_buf()
    } else {
        cwd.join(parent)
    };
    normalize(&absolute)
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

/// The string `save` stores for `path`, given the project's `base` and the
/// directory relative inputs are typed in.
///
/// An absolute path is stored as an absolute path: it already means the same
/// file from any directory on the machine it names. A relative path is resolved
/// against `cwd` once — that is what it meant when the user typed it — and
/// stored relative to `base`, which is where `load` will resolve it. Both
/// spellings use `/` separators (see [`stored_spelling`]).
fn stored_path(base: &Path, path: &Path, cwd: &Path) -> String {
    if path.is_absolute() {
        return stored_spelling(path);
    }
    stored_relative(base, &normalize(&cwd.join(path)))
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
    if let Some(rest) = path_components.get(common..) {
        for component in rest {
            out.push(component.as_os_str());
        }
    }
    out
}

/// Turn a stored path string into the file it names, against `base`.
fn resolve_stored(base: &Path, text: &str) -> PathBuf {
    let stored = PathBuf::from(text);
    if stored.is_absolute() {
        normalize(&stored)
    } else {
        normalize(&base.join(stored))
    }
}

/// Whether two paths name the same file, resolving relative spellings against
/// `cwd`.
fn same_path_in(left: &Path, right: &Path, cwd: &Path) -> bool {
    let absolute = |path: &Path| {
        if path.is_absolute() {
            normalize(path)
        } else {
            normalize(&cwd.join(path))
        }
    };
    absolute(left) == absolute(right)
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
        save_project(&ProjectSave {
            project: project.to_str().unwrap_or_default(),
            log: log.to_str().unwrap_or_default(),
            photos: "",
            output: output.to_str().unwrap_or_default(),
            utc_offset_days: 1,
            export_format: "gpx",
            nmea: false,
        })
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
        save_project(&ProjectSave {
            project: project.to_str().unwrap_or_default(),
            log: log.to_str().unwrap_or_default(),
            photos: "",
            output: scratch.join("out.json").to_str().unwrap_or_default(),
            utc_offset_days: 1,
            export_format: "json",
            nmea: false,
        })
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
    fn a_project_in_a_subdirectory_stores_paths_relative_to_its_directory() {
        // The reviewer's reproduction: the paths are typed at the cwd the
        // project is created from, while `load` resolves them against the
        // project directory. Storing "track.bin" verbatim would make load look
        // for `proj/track.bin`; `../track.bin` is the same file on load, and the
        // whole `proj/` directory can be moved with the files it names.
        let scratch = ScratchDir::new("project-subdir");
        std::fs::write(scratch.join("track.bin"), one_day_dump()).expect("write the dump");
        std::fs::create_dir_all(scratch.join("proj")).expect("create the project directory");
        save_project_in(
            &ProjectSave {
                project: "proj/trip.json",
                log: "track.bin",
                photos: "",
                output: "proj/track.gpx",
                utc_offset_days: 0,
                export_format: "gpx",
                nmea: false,
            },
            scratch.path(),
        )
        .expect("save the project from the parent directory");

        let project = scratch.join("proj/trip.json");
        let text = std::fs::read_to_string(&project).expect("read the project");
        let document: Value = serde_json::from_str(&text).expect("one JSON object");
        assert_eq!(
            document["log"], "../track.bin",
            "a relative log is stored relative to the project's directory"
        );
        assert_eq!(
            document["output"], "track.gpx",
            "a relative output beside the project is stored by name"
        );

        // The other half of the round trip: `load` resolves those strings
        // against the project's directory, not the cwd the save was typed at.
        let loaded = ProjectDocument::read(project.to_str().unwrap_or_default())
            .expect("read the project back");
        assert_eq!(loaded.log, scratch.join("track.bin"));
        assert_eq!(loaded.output, scratch.join("proj/track.gpx"));
        let log = load_log(&loaded.log, false, OffsetSpec::Days(0)).expect("load the log");
        assert_eq!(log.track.points.len(), 1);
    }

    #[test]
    fn an_absolute_path_is_stored_absolute_and_read_back_as_the_same_file() {
        let scratch = ScratchDir::new("project-absolute");
        std::fs::write(scratch.join("track.bin"), one_day_dump()).expect("write the dump");
        let log = scratch.join("track.bin");
        save_project_in(
            &ProjectSave {
                project: scratch.join("proj/trip.json").to_str().unwrap_or_default(),
                log: log.to_str().unwrap_or_default(),
                photos: "",
                output: scratch.join("proj/out.gpx").to_str().unwrap_or_default(),
                utc_offset_days: 0,
                export_format: "gpx",
                nmea: false,
            },
            scratch.path(),
        )
        .expect("save the project");
        let text =
            std::fs::read_to_string(scratch.join("proj/trip.json")).expect("read the project");
        let document: Value = serde_json::from_str(&text).expect("one JSON object");
        assert_eq!(
            document["log"],
            log.to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
            "an absolute path has one meaning and is kept as written"
        );
    }

    #[test]
    fn a_json_read_export_carries_the_nmea_counts_and_the_day_shift() {
        // `02-cli.md` §`log read`: `--format json` carries `nmea` for a capture
        // and `offset_days` when something was shifted, so a script does not
        // have to parse the human message. The GUI writes the same object.
        let scratch = ScratchDir::new("read-json-facts");
        let input = scratch.join("capture.nmea");
        let line = m241_core::protocol::pmtk::sentence(
            "GPRMC,221320.00,A,3540.8742,N,13946.0275,E,10.00,,141123,,",
        );
        std::fs::write(&input, format!("$GPGSA,A,3,01,02,03*00\r\n{line}\r\n"))
            .expect("write the capture");
        let output = scratch.join("track.json");
        let (loaded, _message) = read_and_export(
            input.to_str().unwrap_or_default(),
            true,
            ExportFormat::Json,
            OffsetSpec::Days(1),
            output.to_str().unwrap_or_default(),
            false,
        )
        .expect("read and export");
        assert_eq!(loaded.facts.offset_days, 1);
        assert_eq!(
            loaded.facts.nmea.map(|counts| counts.imported),
            Some(1),
            "one sentence produced a point; the GSA line was ignored"
        );
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read the export"))
                .expect("one JSON object");
        assert_eq!(document["offset_days"], 1);
        assert_eq!(document["nmea"]["imported"], 1);
        assert_eq!(document["nmea"]["dropped"], 0);
        assert_eq!(document["nmea"]["ignored"], 1);
    }

    #[test]
    fn a_dump_json_export_has_neither_read_report_field() {
        let scratch = ScratchDir::new("read-json-plain");
        let input = scratch.join("track.bin");
        std::fs::write(&input, one_day_dump()).expect("write the dump");
        let output = scratch.join("track.json");
        read_and_export(
            input.to_str().unwrap_or_default(),
            false,
            ExportFormat::Json,
            OffsetSpec::Auto,
            output.to_str().unwrap_or_default(),
            false,
        )
        .expect("read and export");
        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&output).expect("read the export"))
                .expect("one JSON object");
        assert!(document.get("offset_days").is_none(), "got {document}");
        assert!(document.get("nmea").is_none(), "got {document}");
    }

    #[test]
    fn the_read_report_is_only_added_to_json() {
        let scratch = ScratchDir::new("read-report-formats");
        let input = scratch.join("track.bin");
        std::fs::write(&input, one_day_dump()).expect("write the dump");
        let output = scratch.join("track.gpx");
        read_and_export(
            input.to_str().unwrap_or_default(),
            false,
            ExportFormat::Gpx,
            OffsetSpec::Days(1),
            output.to_str().unwrap_or_default(),
            false,
        )
        .expect("read and export");
        let written = std::fs::read_to_string(&output).expect("read the export");
        assert!(
            !written.contains("offset_days") && !written.contains("nmea"),
            "a non-JSON export must be exactly what the exporter wrote"
        );
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
        let error = save_project(&ProjectSave {
            project: project.to_str().unwrap_or_default(),
            log: project.to_str().unwrap_or_default(),
            photos: "",
            output: scratch.join("out.gpx").to_str().unwrap_or_default(),
            utc_offset_days: 0,
            export_format: "gpx",
            nmea: false,
        })
        .expect_err("the project is also the log");
        assert!(error.contains("同じパス"), "got {error}");
    }

    #[test]
    fn a_kmz_project_needs_a_photo_folder_and_a_dump() {
        let scratch = ScratchDir::new("project-kmz");
        let project = scratch.join("trip.json");
        let error = save_project(&ProjectSave {
            project: project.to_str().unwrap_or_default(),
            log: scratch.join("track.bin").to_str().unwrap_or_default(),
            photos: "",
            output: scratch.join("track.kmz").to_str().unwrap_or_default(),
            utc_offset_days: 0,
            export_format: "kmz",
            nmea: false,
        })
        .expect_err("kmz without photos");
        assert!(error.contains("写真"), "got {error}");
        let error = save_project(&ProjectSave {
            project: project.to_str().unwrap_or_default(),
            log: scratch.join("track.bin").to_str().unwrap_or_default(),
            photos: scratch.path().to_str().unwrap_or_default(),
            output: scratch.join("track.kmz").to_str().unwrap_or_default(),
            utc_offset_days: 0,
            export_format: "kmz",
            nmea: true,
        })
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
