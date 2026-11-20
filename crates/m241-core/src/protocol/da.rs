//! Download Agent driver — **placeholder pending the BM25 disassembly**.
//!
//! The flash path is not being ported from the previous implementation or from
//! the upstream project. Both are treated as hypotheses until
//! `DA_v3.8.4.bin` has been disassembled and the command semantics documented in
//! `docs/reference/01-da-protocol.md`. This module exists so the rest of the
//! crate can compile against a stable shape.

use crate::error::{Error, Result};

/// Placeholder until the command set is established from the disassembly.
#[derive(Debug)]
pub struct DownloadAgent;

impl DownloadAgent {
    /// Refuse to operate: the behaviour is not established yet.
    ///
    /// # Errors
    ///
    /// Always. There is no partial implementation here on purpose: a plausible
    /// half-correct flash routine is the most dangerous thing this crate could
    /// ship.
    pub fn unsupported() -> Result<()> {
        Err(Error::State(String::from(
            "the download-agent protocol is being re-established from the vendor binary; \
             see docs/reference/01-da-protocol.md",
        )))
    }
}
