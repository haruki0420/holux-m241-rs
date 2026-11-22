//! Test-only fixtures shared by the `log` submodules.
//!
//! The core's dump builders are `#[cfg(test)]` inside `m241-core`, so a GUI test
//! cannot reuse them. [`synthetic_dump`] builds the smallest valid 64 KiB block
//! `m241_core::log::parse_dump` reads; the points are arithmetic, never a
//! recording.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A 64 KiB dump holding `points` as `(utc, latitude, longitude, speed)`.
pub fn synthetic_dump(points: &[(u32, f32, f32, f32)]) -> Vec<u8> {
    use m241_core::log::block::{
        BLOCK_HEADER_SIZE, BLOCK_SIZE, HOLUX_EXTRA_BIT, POLICY_LOG_ENABLED, bit,
    };
    let mask = HOLUX_EXTRA_BIT
        | (1u32 << u32::from(bit::UTC))
        | (1u32 << u32::from(bit::LATITUDE))
        | (1u32 << u32::from(bit::LONGITUDE))
        | (1u32 << u32::from(bit::HEIGHT))
        | (1u32 << u32::from(bit::SPEED));
    let mut block = vec![0xFFu8; BLOCK_SIZE];
    let count = points.len() as u16;
    block[0..2].copy_from_slice(&count.to_le_bytes());
    block[2..6].copy_from_slice(&mask.to_le_bytes());
    block[6..8].copy_from_slice(&POLICY_LOG_ENABLED.to_le_bytes());

    let mut offset = BLOCK_HEADER_SIZE;
    for (utc, lat, lon, speed) in points {
        let mut record = Vec::with_capacity(20);
        record.extend_from_slice(&utc.to_le_bytes());
        record.extend_from_slice(&lat.to_le_bytes());
        record.extend_from_slice(&lon.to_le_bytes());
        record.extend_from_slice(&[0x00, 0x28, 0x42]); // height 42.0 m
        record.extend_from_slice(&speed.to_le_bytes());
        let checksum = record.iter().fold(0u8, |acc, byte| acc ^ byte);
        record.push(checksum);
        block[offset..offset + record.len()].copy_from_slice(&record);
        offset += record.len();
    }
    block
}

/// A unique temporary directory, removed when it falls out of scope.
#[derive(Debug)]
pub struct ScratchDir(PathBuf);

impl ScratchDir {
    /// Create a scratch directory labelled `tag`.
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "m241-gui-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the scratch directory");
        Self(path)
    }

    /// The directory path.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// A path inside the directory.
    pub fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
