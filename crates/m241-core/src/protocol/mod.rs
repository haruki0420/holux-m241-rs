//! The serial wire: MTK BootROM frames, PMTK sentences, and the port itself.
//!
//! # Layers
//!
//! ```text
//!   PMTK sentences  ── NMEA mode (38400 8N1, the firmware is running)
//!        │
//!        └── $PMTK180 switches the chip into BootROM mode
//!
//!   BootROM frames  ── 9-byte opcode+address+count frames, every byte echoed
//! ```
//!
//! Every high-level routine in this module is read-only. Log erasure and the
//! Download Agent — the half of the protocol that erases and programs flash —
//! live in `m241-flash`, so a binary that links only `m241-core` contains no
//! write or erase routines to call. Low-level transport and framing primitives
//! ([`Port`], [`port::Transport::write_all`], [`pmtk::line`], [`brom::Frame`])
//! remain public because `m241-flash` builds on them.
//!
//! Only [`Port`] touches the operating system. Everything above it is pure
//! byte-in, byte-out so the simulator can stand in for the chip.

pub mod brom;
pub mod frame;
pub mod pmtk;
pub mod port;
pub mod simulator;

pub use brom::{BASE_ADDR_DA, BASE_ADDR_FIRMWARE, MtkBootRom};
pub use frame::{checksum_xor16, reverse_words32, swap_pairs};
pub use pmtk::{PmtkClient, StreamState};
pub use port::Port;
