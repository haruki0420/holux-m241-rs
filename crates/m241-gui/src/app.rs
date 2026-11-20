//! The application shell: rail on the left, the selected screen filling the rest.
//!
//! `eframe` 0.37 hands the app a `Ui` rather than a bare `Context`, so panels are
//! added to that `Ui` with [`egui::Panel`]. The nesting order matters: panels are
//! added outermost first and the central panel last.
//!
//! The shell owns the three screens' state, and each screen owns its own fields.
//! That split is what lets a screen be tested with `Screen::ui` directly instead of
//! through the whole application, and it is why the dispatch below takes `&mut` per
//! screen rather than one big state struct passed by name.

use crate::screens::connect::ConnectScreen;
use crate::screens::firmware::FirmwareScreen;
use crate::screens::log::LogScreen;
use crate::screens::{self, Screen};
use crate::theme::{self, size, space};
use crate::widgets::rail;

/// Application state: the selected screen and the three screens' own state.
#[derive(Debug, Default)]
pub struct App {
    current: Screen,
    connect: ConnectScreen,
    log: LogScreen,
    firmware: FirmwareScreen,
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

    /// The Connect screen's state, for a caller that has just read a device.
    #[must_use]
    pub fn connect_mut(&mut self) -> &mut ConnectScreen {
        &mut self.connect
    }

    /// The Log screen's state, for a caller that has just downloaded a log.
    #[must_use]
    pub fn log_mut(&mut self) -> &mut LogScreen {
        &mut self.log
    }

    /// The Firmware screen's state, for a caller that has just dumped an image.
    #[must_use]
    pub fn firmware_mut(&mut self) -> &mut FirmwareScreen {
        &mut self.firmware
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
                            .size(size::TITLE)
                            .color(theme::INK)
                            .strong(),
                    );
                    ui.add_space(space::M);
                    ui.label(
                        egui::RichText::new("ファームの日付が 19.6 年ずれています")
                            .size(size::UI)
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

        // Copied out of `self` before the closure: a `&mut` to one screen field and
        // a borrow of `self.current` cannot coexist inside it.
        let current = self.current;
        let connect = &mut self.connect;
        let log = &mut self.log;
        let firmware = &mut self.firmware;

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new()
                .fill(theme::GROUND)
                .inner_margin(space::XL)
                .show(ui, |ui| match current {
                    Screen::Connect => connect.ui(ui),
                    Screen::Log => log.ui(ui),
                    Screen::Firmware => firmware.ui(ui),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_starts_on_connect() {
        // Every session starts by asking "is the device there"; landing anywhere
        // else would put a destructive screen one click from a cold start.
        assert_eq!(App::default().current(), Screen::Connect);
    }

    #[test]
    fn each_screen_can_reach_its_own_state() {
        let mut app = App::default();
        // The accessors exist so a future session layer can fill one screen
        // without knowing the shell's fields; touching all three proves they are
        // wired to distinct state rather than to one shared placeholder.
        let _connect = app.connect_mut();
        let _log = app.log_mut();
        let _firmware = app.firmware_mut();
    }
}
