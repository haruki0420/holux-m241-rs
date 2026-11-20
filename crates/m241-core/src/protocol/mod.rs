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
//! Everything in this module reads. The Download Agent — the half of the
//! protocol that erases and programs flash — is in `m241-flash`, so a binary
//! that links only this crate cannot write to a device.
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
