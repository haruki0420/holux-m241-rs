//! The four screens and the shell that holds them.

pub mod connect;
pub mod firmware;
pub mod log;
pub mod settings;

/// Which screen the rail has selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Connect,
    Log,
    Firmware,
    Settings,
}

impl Screen {
    /// Rail order, top to bottom. The staircase indents by position.
    pub const ALL: [Self; 4] = [Self::Connect, Self::Log, Self::Firmware, Self::Settings];

    /// The label on the rail.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Connect => "接続",
            Self::Log => "ログ",
            Self::Firmware => "ファーム",
            Self::Settings => "設定",
        }
    }

    /// Position in the rail, used for the 8px staircase.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}
