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
use crate::screens::settings::AppSettings;
use crate::screens::{self, Screen};
use crate::theme::{self, size, space};
use crate::widgets::rail;

/// Application state: the selected screen, preferences, and the screens' own state.
#[derive(Debug, Default)]
pub struct App {
    current: Screen,
    settings: AppSettings,
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
    /// [`crate::widgets::rail::rail_with_transition`].
    pub fn select(&mut self, screen: Screen) {
        self.current = screen;
    }

    /// Application display and motion preferences.
    #[must_use]
    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    /// Mutable access to application display and motion preferences.
    #[must_use]
    pub fn settings_mut(&mut self) -> &mut AppSettings {
        &mut self.settings
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
                // The one elevation the product declares: a `rule` hairline
                // under the title bar (DESIGN.md §3 — no shadows).
                ui.add_space(space::S);
                screens::common::hairline(ui);
            });

        // The rail is a fixed-width left panel whose contents are custom painted,
        // because the staircase is not a widget egui has. The width is pinned
        // rather than content-sized: otherwise the current screen's deepest tab
        // decides the rail width, and the heading shifts sideways when you navigate.
        let transition_secs = rail::rail_transition_secs_for(self);
        egui::Panel::left("rail")
            .exact_size(rail::WIDTH)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::RAIL))
            .show(ui, |ui| {
                if let Some(next) = rail::rail_with_transition(ui, self.current, transition_secs) {
                    self.current = next;
                }
            });

        // Copied out of `self` before the closure: a `&mut` to one screen field and
        // a borrow of `self.current` cannot coexist inside it.
        let current = self.current;
        let settings = &mut self.settings;
        let connect = &mut self.connect;
        let log = &mut self.log;
        let firmware = &mut self.firmware;

        egui::CentralPanel::no_frame().show(ui, |ui| {
            egui::Frame::new()
                .fill(theme::GROUND)
                .inner_margin(space::XL)
                .show(ui, |ui| match current {
                    Screen::Connect => connect.ui(ui),
                    Screen::Log => log.ui_with_settings(ui, settings),
                    Screen::Firmware => firmware.ui(ui),
                    Screen::Settings => screens::settings::ui(ui, settings),
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
    fn the_shell_hands_the_reduced_motion_setting_to_the_rail() {
        // Stage 3 review, N5: the settings getter was tested and the rail's
        // duration parameter was tested, but nothing checked what `App::draw`
        // actually passes — hard-coding 0.15 at the call site (M11b) left every
        // test green.
        //
        // So this draws the real shell and asks the rail what it was handed. The
        // draw runs on its own thread because the rail records per thread: the
        // parallel render tests draw the rail too, and this thread must read its
        // own frame rather than whichever one finished last.
        let draw_and_read = |reduced_motion: bool| -> Option<f32> {
            std::thread::spawn(move || {
                let ctx = egui::Context::default();
                crate::theme::install(&ctx);
                let mut app = App::default();
                app.settings_mut().reduced_motion = reduced_motion;
                let output = ctx.run_ui(egui::RawInput::default(), |ui| app.draw(ui));
                output.drop_without_applying_deltas();
                rail::last_transition_secs()
            })
            .join()
            .ok()
            .flatten()
        };

        let reduced = draw_and_read(true);
        assert_eq!(
            reduced,
            Some(0.0),
            "with reduced motion on the shell handed the rail {reduced:?}s, expected 0"
        );
        let motion = draw_and_read(false);
        assert_eq!(
            motion,
            Some(rail::TRANSITION_MOTION_SECS),
            "with reduced motion off the shell handed the rail {motion:?}s, expected {}",
            rail::TRANSITION_MOTION_SECS
        );
    }

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
