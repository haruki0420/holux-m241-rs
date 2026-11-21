//! Proof that a person answered the geotag confirmation prompt.
//!
//! The token lives in a child module of the Log screen, and its field is private
//! to that module, so [`super::LogScreen`] itself cannot write
//! `GeotagConfirmation(())`. The only way to obtain one is
//! [`super::LogScreen::confirm_geotag_write`], which refuses unless the prompt is
//! open, and which is called from exactly one place: the 「書き込む」 button. A
//! buffer inside `super` can name the type; it cannot make a value.
//!
//! Why a child module and not a field in `log.rs`: the Stage 3 review of
//! `fix/gui-eztour-live` showed the earlier version — a private tuple field on a
//! type declared in `log.rs` — let a one-line addition to the 「写真に位置」
//! handler (`apply_geotags(GeotagConfirmation(()))`) write EXIF tags without the
//! prompt, and every test still passed. Privacy below the type's own module is
//! what makes that line fail to compile.

/// Proof that the user answered the geotag confirmation prompt.
///
/// Constructed only by [`super::LogScreen::confirm_geotag_write`], which hands it
/// straight to [`super::LogScreen::apply_geotags`].
///
/// The field is private with no `pub` at all: `pub(super)` would re-open it to
/// exactly the module the token is meant to protect.
#[derive(Debug, Clone, Copy)]
pub(super) struct GeotagConfirmation(());

impl GeotagConfirmation {
    /// Mint the token for `LogScreen::confirm_geotag_write`.
    ///
    /// This constructor is the module's only door. It is `pub(super)`, so the
    /// screen can call it — but calling it is a visible line in `log.rs` under a
    /// name that says what it means, which is the difference between "the write
    /// button asked for a token" and "somebody pasted `GeotagConfirmation(())`
    /// into a second handler". The constructor lives here so the field stays
    /// private and the review has exactly one name to grep.
    pub(super) const fn answered_confirmation() -> Self {
        Self(())
    }
}
