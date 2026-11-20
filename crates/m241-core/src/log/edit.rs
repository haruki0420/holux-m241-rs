//! Pure track-editing operations and multi-step Undo/Redo history.
//!
//! Splitting, merging, and point deletion operate on [`Track`] values without
//! mutating files or touching the device. [`TrackEditor`] wraps a `Vec<Track>`
//! with an undo/redo stack so UI or CLI layers can compose edits safely.

use std::collections::HashSet;

use crate::error::{Error, Result};
use crate::types::{Track, TrackPoint};

/// Split `track` into two non-empty halves at point index `split_at`.
///
/// The first returned track holds points `0..split_at` and the second holds
/// `split_at..len`.
///
/// # Errors
///
/// Returns [`Error::Log`] when `split_at == 0` or `split_at >= track.points.len()`,
/// because either case would leave an empty half or index out of bounds.
pub fn split_track(track: &Track, split_at: usize) -> Result<(Track, Track)> {
    let len = track.points.len();
    if split_at == 0 || split_at >= len {
        return Err(Error::Log {
            message: format!(
                "cannot split track of {len} point(s) at index {split_at} (must be 1..{len})"
            ),
            offset: None,
        });
    }
    let Some((left, right)) = track.points.split_at_checked(split_at) else {
        return Err(Error::Log {
            message: format!("split index {split_at} is out of bounds for {len} point(s)"),
            offset: None,
        });
    };
    Ok((
        Track {
            points: left.to_vec(),
            damaged_blocks: track.damaged_blocks.clone(),
        },
        Track {
            points: right.to_vec(),
            damaged_blocks: track.damaged_blocks.clone(),
        },
    ))
}

/// Merge `tracks` into a single [`Track`], preserving point order.
///
/// Damaged block indices are preserved in order without duplicates so
/// downstream summaries reflect the combined tracks.
#[must_use]
pub fn merge_tracks(tracks: &[Track]) -> Track {
    let total_points: usize = tracks.iter().map(|t| t.points.len()).sum();
    let mut points: Vec<TrackPoint> = Vec::with_capacity(total_points);
    let mut damaged_blocks: Vec<u32> = Vec::new();
    let mut seen_blocks: HashSet<u32> = HashSet::new();

    for track in tracks {
        points.extend_from_slice(&track.points);
        for &block in &track.damaged_blocks {
            if seen_blocks.insert(block) {
                damaged_blocks.push(block);
            }
        }
    }

    Track {
        points,
        damaged_blocks,
    }
}

/// Return a copy of `track` with the specified point `indices` removed.
///
/// Duplicate and out-of-range indices are ignored safely without panicking.
#[must_use]
pub fn delete_points(track: &Track, indices: &[usize]) -> Track {
    let to_remove: HashSet<usize> = indices
        .iter()
        .copied()
        .filter(|&i| i < track.points.len())
        .collect();
    if to_remove.is_empty() {
        return track.clone();
    }
    let points = track
        .points
        .iter()
        .enumerate()
        .filter_map(|(idx, &pt)| (!to_remove.contains(&idx)).then_some(pt))
        .collect();
    Track {
        points,
        damaged_blocks: track.damaged_blocks.clone(),
    }
}

/// Stateful track collection with multi-step Undo/Redo history.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TrackEditor {
    tracks: Vec<Track>,
    undo_stack: Vec<Vec<Track>>,
    redo_stack: Vec<Vec<Track>>,
}

impl TrackEditor {
    /// Create an editor starting with `tracks` and empty history.
    #[must_use]
    pub fn new(tracks: Vec<Track>) -> Self {
        Self {
            tracks,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Create an editor holding a single [`Track`].
    #[must_use]
    pub fn from_track(track: Track) -> Self {
        Self::new(vec![track])
    }

    /// Current tracks in editing order.
    #[must_use]
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Look up one track by index.
    #[must_use]
    pub fn track(&self, index: usize) -> Option<&Track> {
        self.tracks.get(index)
    }

    /// Whether [`Self::undo`] can revert to an earlier state.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Whether [`Self::redo`] can replay an undone edit.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Split `tracks[track_index]` at `split_at`, replacing it with both halves.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Log`] if `track_index` or `split_at` is out of bounds.
    pub fn split(&mut self, track_index: usize, split_at: usize) -> Result<()> {
        let Some(track) = self.tracks.get(track_index) else {
            return Err(Error::Log {
                message: format!(
                    "track index {track_index} is out of bounds (have {} track(s))",
                    self.tracks.len()
                ),
                offset: None,
            });
        };
        let (left, right) = split_track(track, split_at)?;

        let mut next = Vec::with_capacity(self.tracks.len() + 1);
        for (idx, existing) in self.tracks.iter().enumerate() {
            if idx == track_index {
                next.push(left.clone());
                next.push(right.clone());
            } else {
                next.push(existing.clone());
            }
        }
        self.commit(next);
        Ok(())
    }

    /// Merge the tracks at `track_indices` (in the order listed) into one track,
    /// placed at the lowest selected index while removing the remaining ones.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Log`] if fewer than two indices are given, any index is
    /// duplicated, or any index is out of bounds.
    pub fn merge(&mut self, track_indices: &[usize]) -> Result<()> {
        if track_indices.len() < 2 {
            return Err(Error::Log {
                message: String::from("merging requires at least two track indices"),
                offset: None,
            });
        }

        let mut seen = HashSet::with_capacity(track_indices.len());
        let mut selected = Vec::with_capacity(track_indices.len());
        let mut min_index = usize::MAX;
        for &idx in track_indices {
            let Some(track) = self.tracks.get(idx) else {
                return Err(Error::Log {
                    message: format!(
                        "track index {idx} is out of bounds (have {} track(s))",
                        self.tracks.len()
                    ),
                    offset: None,
                });
            };
            if !seen.insert(idx) {
                return Err(Error::Log {
                    message: format!("duplicate track index {idx} in merge"),
                    offset: None,
                });
            }
            if idx < min_index {
                min_index = idx;
            }
            selected.push(track.clone());
        }

        let merged = merge_tracks(&selected);
        let mut next = Vec::with_capacity(self.tracks.len() - selected.len() + 1);
        for (idx, existing) in self.tracks.iter().enumerate() {
            if idx == min_index {
                next.push(merged.clone());
            } else if !seen.contains(&idx) {
                next.push(existing.clone());
            }
        }
        self.commit(next);
        Ok(())
    }

    /// Delete `point_indices` from `tracks[track_index]`.
    ///
    /// Duplicate and out-of-range point indices are ignored cleanly.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Log`] if `track_index` is out of bounds.
    pub fn delete_points(&mut self, track_index: usize, point_indices: &[usize]) -> Result<()> {
        let Some(track) = self.tracks.get(track_index) else {
            return Err(Error::Log {
                message: format!(
                    "track index {track_index} is out of bounds (have {} track(s))",
                    self.tracks.len()
                ),
                offset: None,
            });
        };
        let updated = delete_points(track, point_indices);
        if updated == *track {
            return Ok(());
        }

        let mut next = self.tracks.clone();
        if let Some(slot) = next.get_mut(track_index) {
            *slot = updated;
        }
        self.commit(next);
        Ok(())
    }

    /// Revert the most recent edit, returning `false` when nothing can be undone.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.tracks, previous);
        self.redo_stack.push(current);
        true
    }

    /// Reapply the most recently undone edit, returning `false` when the redo
    /// stack is empty.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.tracks, next);
        self.undo_stack.push(current);
        true
    }

    fn commit(&mut self, next: Vec<Track>) {
        let previous = std::mem::replace(&mut self.tracks, next);
        self.undo_stack.push(previous);
        self.redo_stack.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LogSummary;

    fn pt(utc: u32, lat: f64, lon: f64) -> TrackPoint {
        TrackPoint {
            utc: Some(utc),
            latitude: Some(lat),
            longitude: Some(lon),
            height_m: Some(10.0),
            speed_kmh: None,
            rcr: None,
        }
    }

    fn sample_track(count: u32) -> Track {
        Track {
            points: (0..count)
                .map(|i| {
                    pt(
                        1_700_000_000 + i * 10,
                        35.681_236 + f64::from(i) * 0.001,
                        139.767_125 + f64::from(i) * 0.001,
                    )
                })
                .collect(),
            damaged_blocks: Vec::new(),
        }
    }

    #[test]
    fn split_track_splits_non_empty_halves_and_rejects_bad_indices() {
        let track = sample_track(4);
        let (left, right) = split_track(&track, 2).expect("split at 2");
        assert_eq!(left.points.len(), 2);
        assert_eq!(right.points.len(), 2);
        assert_eq!(left.points[0].utc, Some(1_700_000_000));
        assert_eq!(right.points[0].utc, Some(1_700_000_020));

        for bad in [0usize, 4, 99] {
            let err = split_track(&track, bad).expect_err("invalid split must fail");
            assert!(matches!(err, Error::Log { .. }));
        }
        let empty = Track::default();
        assert!(matches!(split_track(&empty, 0), Err(Error::Log { .. })));
    }

    #[test]
    fn merge_tracks_combines_points_and_deduplicates_damaged_blocks() {
        let (a, b) = split_track(&sample_track(4), 2).expect("split");
        let a = Track {
            damaged_blocks: vec![1, 3],
            ..a
        };
        let b = Track {
            damaged_blocks: vec![3, 5],
            ..b
        };
        let merged = merge_tracks(&[a, b]);
        assert_eq!(merged.points.len(), 4);
        assert_eq!(merged.damaged_blocks, vec![1, 3, 5]);
        let summary = LogSummary::of(&merged);
        assert_eq!(summary.records, 4);
        assert_eq!(summary.started_at, Some(1_700_000_000));
        assert_eq!(summary.ended_at, Some(1_700_000_030));
    }

    #[test]
    fn delete_points_handles_duplicates_and_out_of_range_safely() {
        let track = sample_track(4);
        let edited = delete_points(&track, &[1, 1, 3, 99]);
        assert_eq!(edited.points.len(), 2);
        assert_eq!(edited.points[0].utc, Some(1_700_000_000));
        assert_eq!(edited.points[1].utc, Some(1_700_000_020));

        let unchanged = delete_points(&track, &[99, 100]);
        assert_eq!(unchanged, track);
    }

    #[test]
    fn track_editor_supports_multi_step_undo_and_redo() {
        let initial = sample_track(4);
        let mut editor = TrackEditor::from_track(initial.clone());
        assert!(!editor.can_undo());
        assert!(!editor.can_redo());

        // Step 1: split into 2 tracks of 2 points each.
        editor.split(0, 2).expect("split");
        assert_eq!(editor.tracks().len(), 2);
        assert!(editor.can_undo());
        assert!(!editor.can_redo());

        // Step 2: delete first point of second track.
        editor.delete_points(1, &[0]).expect("delete point");
        assert_eq!(editor.tracks()[1].points.len(), 1);

        // Step 3: merge back into one 3-point track.
        editor.merge(&[0, 1]).expect("merge");
        assert_eq!(editor.tracks().len(), 1);
        assert_eq!(editor.tracks()[0].points.len(), 3);

        // Undo step 3 -> 2 tracks (2 pts, 1 pt)
        assert!(editor.undo());
        assert_eq!(editor.tracks().len(), 2);
        assert_eq!(editor.tracks()[1].points.len(), 1);
        assert!(editor.can_redo());

        // Undo step 2 -> 2 tracks (2 pts, 2 pts)
        assert!(editor.undo());
        assert_eq!(editor.tracks().len(), 2);
        assert_eq!(editor.tracks()[1].points.len(), 2);

        // Undo step 1 -> original 1 track of 4 pts
        assert!(editor.undo());
        assert_eq!(editor.tracks(), &[initial]);
        assert!(!editor.can_undo());
        assert!(!editor.undo());

        // Redo step 1 & step 2, then make a new edit which clears redo stack.
        assert!(editor.redo());
        assert_eq!(editor.tracks().len(), 2);
        assert!(editor.redo());
        assert_eq!(editor.tracks()[1].points.len(), 1);
        assert!(editor.can_redo());

        editor.delete_points(0, &[0]).expect("branching edit");
        assert!(!editor.can_redo(), "new edit must clear redo stack");
        assert!(!editor.redo());
    }

    #[test]
    fn track_editor_errors_leave_state_and_history_unchanged() {
        let mut editor = TrackEditor::from_track(sample_track(2));
        assert!(editor.split(5, 1).is_err());
        assert!(editor.split(0, 0).is_err());
        assert!(editor.merge(&[0]).is_err());
        assert!(editor.delete_points(9, &[0]).is_err());
        assert!(!editor.can_undo());
        assert_eq!(editor.tracks()[0].points.len(), 2);
    }
}
