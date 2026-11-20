//! The application shell: rail on the left, the selected screen filling the rest.
//!
//! `eframe` 0.37 hands the app a `Ui` rather than a bare `Context`, so panels are
//! added to that `Ui` with [`egui::Panel`]. The nesting order matters: panels are
//! added outermost first and the central panel last.

use crate::screens::{self, Screen};
use crate::theme::{self, space};
use crate::widgets::rail;

/// Application state. Deliberately small: the shell owns navigation only, and
/// each screen owns its own state so a screen can be reviewed in isolation.
#[derive(Debug)]
pub struct App {
    current: Screen,
}

impl Default for App {
    fn default() -> Self {
        Self {
            current: Screen::Connect,
        }
    }
}

impl App {
    /// The screen the rail has selected.
    #[must_use]
    pub fn current(&self) -> Screen {
        self.current
    }

    /// Select a screen. Tests use this; the rail uses the return of
    /// [`crate::widgets::rail::rail`].
    pub fn select(&mut self, screen: Screen) {
        self.current = screen;
    }

    /// Draw the whole shell into `ui`.
    ///
    /// Public so integration tests can drive the real layout headlessly. It is
    /// not part of any stable API promise.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        // Title bar: the product name and the one sentence that explains why the
        // user is here. No menu bar until a screen needs one.
        egui::Panel::top("title")
            .frame(
                egui::Frame::new()
                    .fill(theme::GROUND)
                    .inner_margin(space::L),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Holux M-241")
                            .size(20.0)
                            .color(theme::INK)
                            .strong(),
                    );
                    ui.add_space(space::M);
                    ui.label(
                        egui::RichText::new("ファームの日付が 19.6 年ずれています")
                            .size(13.0)
                            .color(theme::MUTE),
                    );
                });
            });

        // The rail is a fixed-width left panel whose contents are custom painted,
        // because the staircase is not a widget egui has.
        egui::Panel::left("rail")
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::RAIL))
            .show(ui, |ui| {
                if let Some(next) = rail::rail(ui, self.current) {
                    self.current = next;
                }
            });

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new()
                .fill(theme::GROUND)
                .inner_margin(space::XL)
                .show(ui, |ui| match self.current {
                    Screen::Connect => screens::connect::ui(ui),
                    Screen::Log => screens::log::ui(ui),
                    Screen::Firmware => screens::firmware::ui(ui),
                    Screen::Settings => screens::settings::ui(ui),
                });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
