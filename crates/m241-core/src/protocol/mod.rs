//! The serial wire: MTK BootROM frames, the Download Agent, and PMTK sentences.
//!
//! # Layers
//!
//! ```text
//!   PmTK sentences  ── NMEA mode (38400 8N1, the firmware is running)
//!        │
//!        └── $PMTK180 switches the chip into BootROM mode
//!
//!   Brom frames     ── 9-byte opcode+address+count frames, every byte echoed
//!        │
//!        └── JUMP loads the Download Agent into RAM
//!
//!   Da commands     ── 0xD3 range, 0xD5 write, 0x100-byte packets
//! ```
//!
//! Only [`Port`] touches the operating system. Everything above it is pure
//! byte-in, byte-out so the simulator can stand in for the chip.

pub mod brom;
pub mod da;
pub mod frame;
pub mod pmtk;
pub mod port;

pub use brom::{BASE_ADDR_DA, BASE_ADDR_FIRMWARE, MtkBootRom};
pub use da::DownloadAgent;
pub use frame::{checksum_xor16, reverse_words32, swap_pairs};
pub use port::Port;
