//! Entry point. Everything lives in the library so tests can drive it.
//!
//! `--screenshot` renders every screen to a PNG without a display server; see
//! [`m241_gui::screenshot`]. It exists as a flag rather than a hidden environment
//! variable so the artifact the design review judges (DESIGN.md §8) is produced by a
//! command that can be written down.

// Same reasoning as the CLI: printing is this binary's whole job. The lint exists to
// keep `println!` out of library code, which is why it is allowed here and nowhere
// else in this crate.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

/// Where `--screenshot` writes by default (DESIGN.md §8).
const DEFAULT_SCREENSHOT_DIR: &str = ".design/firmware-wizard/screenshots";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let mut screenshot: Option<Option<String>> = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--screenshot" => screenshot = Some(arguments.next()),
            "--help" | "-h" => {
                print_help();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("知らない引数です: {other}");
                print_help();
                return ExitCode::from(2);
            }
        }
    }

    if let Some(directory) = screenshot {
        return screenshots(directory.unwrap_or_else(|| String::from(DEFAULT_SCREENSHOT_DIR)));
    }

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("m241-gui: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The window.
fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([1024.0, 640.0])
            .with_title("m241"),
        ..Default::default()
    };
    eframe::run_native(
        "m241",
        options,
        Box::new(|cc| {
            m241_gui::theme::install(&cc.egui_ctx);
            Ok(Box::new(m241_gui::App::default()))
        }),
    )
}

/// Render every screen at both documented sizes and write PNGs.
///
/// This is DESIGN.md §8's offscreen path: no display server, no GPU, no window. It is
/// a subcommand rather than a hidden environment variable so the artifact the design
/// review judges is produced by a command that can be put in a document.
fn screenshots(directory: String) -> ExitCode {
    match m241_gui::screenshot::write_all(std::path::Path::new(&directory)) {
        Ok(paths) => {
            for path in &paths {
                println!("{}", path.display());
            }
            println!("{} 枚を {} に書きました", paths.len(), directory);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("スクリーンショットを書けません: {err}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!(
        "m241-gui — the Holux M-241 desktop interface\n\n\
         使い方:\n  \
           m241-gui                      ウィンドウを開く\n  \
           m241-gui --screenshot [DIR]    各画面を PNG に書き出す（既定: {DEFAULT_SCREENSHOT_DIR}）\n"
    );
}
