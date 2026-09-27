//! Edits of signals loaded from a waveform file.
//!
//! The file's data is never changed: wellen signals are immutable and index into a time table
//! shared by all signals. Instead each edited signal gets an overlay, a [`ChangeList`] whose
//! entries either replace the file's value (`Some`) or fall back to it (`None`), and reads
//! combine the two. Overlays are keyed by [`SignalRef`], so editing a variable also edits every
//! other name (alias) for the same stored signal.
use std::collections::HashMap;
use std::sync::Arc;

use num::BigUint;
use surfer_translation_types::VariableValue;
use wellen::SignalRef;

use crate::user_signals::ChangeList;
use crate::wave_container::QueryResult;

type Overlay = ChangeList<Option<VariableValue>>;

#[derive(Debug, Clone, Default)]
pub struct SignalEdits {
    /// Shared with undo snapshots, and copied only when a snapshot's overlay is edited.
    overlays: HashMap<SignalRef, Arc<Overlay>>,
}

impl SignalEdits {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.overlays.is_empty()
    }

    #[must_use]
    pub fn is_edited(&self, signal_ref: SignalRef) -> bool {
        self.overlays.contains_key(&signal_ref)
    }

    /// Sets the value of `signal_ref` in `[start, end)`, or from `start` onwards if `end` is
    /// `None`. Whatever was in effect at `end`, an edit or the file's value, resumes there.
    pub fn set_range(
        &mut self,
        signal_ref: SignalRef,
        start: u64,
        end: Option<u64>,
        value: VariableValue,
    ) {
        let overlay = Arc::make_mut(
            self.overlays
                .entry(signal_ref)
                .or_insert_with(|| Arc::new(ChangeList::new(None))),
        );
        overlay.set_range(start, end, Some(value));
    }

    /// Removes all edits of `signal_ref`. Returns whether it had any.
    pub fn revert(&mut self, signal_ref: SignalRef) -> bool {
        self.overlays.remove(&signal_ref).is_some()
    }

    pub fn revert_all(&mut self) {
        self.overlays.clear();
    }

    /// The value of `signal_ref` at `time` with edits applied. `original` gives the file's
    /// value at a time, or `None` if the signal is not loaded yet.
    ///
    /// Where an edit starts or ends without changing the value, for example where a painted 0
    /// meets a 0 in the file, no change is reported: the drawing code marks every reported
    /// change of an unchanged value as a glitch.
    pub fn query(
        &self,
        signal_ref: SignalRef,
        time: u64,
        original: impl Fn(u64) -> Option<QueryResult>,
    ) -> Option<QueryResult> {
        let Some(overlay) = self.overlays.get(&signal_ref) else {
            return original(time);
        };
        let at = |time: u64| Self::query_overlay(overlay, time, &original);
        let mut result = at(time)?;
        let is_edit_boundary = |t: u64| overlay.entry_at(t).is_some_and(|(start, _)| start == t);
        // Bounded, as each step only skips one edit boundary.
        for _ in 0..overlay.changes().len() {
            let Some((change, value)) = &result.current else {
                break;
            };
            let change = u64::try_from(change).unwrap_or(u64::MAX);
            if change == 0 || !is_edit_boundary(change) {
                break;
            }
            match at(change - 1)?.current {
                Some((earlier, before)) if &before == value => {
                    result.current = Some((earlier, before));
                }
                _ => break,
            }
        }
        for _ in 0..overlay.changes().len() {
            let (Some(next), Some((_, value))) = (&result.next, &result.current) else {
                break;
            };
            let next = u64::try_from(next).unwrap_or(u64::MAX);
            if !is_edit_boundary(next) {
                break;
            }
            let after = at(next)?;
            if after.current.as_ref().map(|(_, v)| v) != Some(value) {
                break;
            }
            result.next = after.next;
        }
        Some(result)
    }

    /// The value at `time` combining `overlay` with the file, reporting a change at every
    /// boundary between edits and the file.
    fn query_overlay(
        overlay: &Overlay,
        time: u64,
        original: impl Fn(u64) -> Option<QueryResult>,
    ) -> Option<QueryResult> {
        let next_edit = overlay.next_after(time).map(BigUint::from);
        match overlay.entry_at(time) {
            Some((edit_time, Some(value))) => Some(QueryResult {
                current: Some((BigUint::from(edit_time), value.clone())),
                next: next_edit,
            }),
            entry => {
                // The file's value, which may have been hidden by an edit until `edit_start`.
                let edit_start = BigUint::from(entry.map_or(0, |(t, _)| t));
                let mut result = original(time)?;
                result.current = result.current.map(|(t, value)| (t.max(edit_start), value));
                result.next = match (result.next, next_edit) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                Some(result)
            }
        }
    }

    /// The time ranges within `[from, to)` where the edited value of `signal_ref` differs from
    /// the file's, merged where they touch. `original` gives the file's value at a time.
    pub fn changed_spans(
        &self,
        signal_ref: SignalRef,
        from: u64,
        to: u64,
        original: impl Fn(u64) -> Option<QueryResult>,
    ) -> Vec<(u64, u64)> {
        let Some(overlay) = self.overlays.get(&signal_ref) else {
            return vec![];
        };
        let entries = overlay.changes();
        let mut spans: Vec<(u64, u64)> = vec![];
        for (i, (start, value)) in entries.iter().enumerate() {
            let Some(value) = value else {
                continue;
            };
            let edit_end = entries.get(i + 1).map_or(u64::MAX, |(t, _)| *t).min(to);
            let mut time = (*start).max(from);
            // Walk the file's changes under this edit. Bounded in case of very busy signals.
            for _ in 0..100_000 {
                if time >= edit_end {
                    break;
                }
                let Some(file) = original(time) else {
                    break;
                };
                let next = file
                    .next
                    .and_then(|t| u64::try_from(t).ok())
                    .unwrap_or(u64::MAX)
                    .min(edit_end);
                if file.current.as_ref().map(|(_, v)| v) != Some(value) {
                    match spans.last_mut() {
                        Some(last) if last.1 == time => last.1 = next,
                        _ => spans.push((time, next)),
                    }
                }
                time = next;
            }
        }
        spans
    }

    /// All value changes of `signal_ref` with edits applied, given the file's changes sorted
    /// by time.
    pub fn merged_changes(
        &self,
        signal_ref: SignalRef,
        original: impl Iterator<Item = (u64, VariableValue)>,
    ) -> Vec<(u64, VariableValue)> {
        let original: Vec<_> = original.collect();
        let Some(overlay) = self.overlays.get(&signal_ref) else {
            return original;
        };
        let entries = overlay.changes();
        let mut merged: Vec<(u64, VariableValue)> = vec![];
        for (i, (start, value)) in entries.iter().enumerate() {
            let end = entries.get(i + 1).map(|(t, _)| *t);
            match value {
                Some(value) => merged.push((*start, value.clone())),
                None => {
                    // The file's value in effect at `start`, then its changes until `end`.
                    let first = original.partition_point(|(t, _)| *t <= *start);
                    if first > 0 {
                        merged.push((*start, original[first - 1].1.clone()));
                    }
                    merged.extend(
                        original[first..]
                            .iter()
                            .take_while(|(t, _)| end.is_none_or(|end| *t < end))
                            .cloned(),
                    );
                }
            }
        }
        merged.dedup_by(|next, prev| next.1 == prev.1);
        merged
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: u32) -> VariableValue {
        VariableValue::BigUint(BigUint::from(x))
    }

    fn signal() -> SignalRef {
        SignalRef::from_index(0).unwrap()
    }

    /// A file signal that is 0 until 10, 1 until 30, then 0.
    const FILE: [(u64, u32); 3] = [(0, 0), (10, 1), (30, 0)];

    fn file_query(time: u64) -> QueryResult {
        let idx = FILE.partition_point(|(t, _)| *t <= time);
        QueryResult {
            current: (idx > 0).then(|| (BigUint::from(FILE[idx - 1].0), v(FILE[idx - 1].1))),
            next: FILE.get(idx).map(|(t, _)| BigUint::from(*t)),
        }
    }

    fn query(edits: &SignalEdits, time: u64) -> (Option<(u64, u32)>, Option<u64>) {
        let result = edits
            .query(signal(), time, |t| Some(file_query(t)))
            .unwrap();
        (
            result.current.map(|(t, value)| {
                let VariableValue::BigUint(value) = value else {
                    panic!("unexpected string")
                };
                (u64::try_from(t).unwrap(), u32::try_from(value).unwrap())
            }),
            result.next.map(|t| u64::try_from(t).unwrap()),
        )
    }

    fn merged(edits: &SignalEdits) -> Vec<(u64, VariableValue)> {
        edits.merged_changes(signal(), FILE.iter().map(|(t, x)| (*t, v(*x))))
    }

    #[test]
    fn unedited_signals_read_the_file() {
        let edits = SignalEdits::default();
        assert_eq!(query(&edits, 15), (Some((10, 1)), Some(30)));
        assert!(!edits.is_edited(signal()));
    }

    #[test]
    fn edit_inside_a_stretch_resumes_the_file() {
        let mut edits = SignalEdits::default();
        edits.set_range(signal(), 15, Some(20), v(0));
        assert!(edits.is_edited(signal()));
        // Before the edit: the file's value, with the next change capped at the edit.
        assert_eq!(query(&edits, 12), (Some((10, 1)), Some(15)));
        // Inside the edit.
        assert_eq!(query(&edits, 17), (Some((15, 0)), Some(20)));
        // After the edit the file's value continues, starting at the end of the edit.
        assert_eq!(query(&edits, 25), (Some((20, 1)), Some(30)));
        assert_eq!(query(&edits, 35), (Some((30, 0)), None));
        assert_eq!(
            merged(&edits),
            vec![(0, v(0)), (10, v(1)), (15, v(0)), (20, v(1)), (30, v(0))]
        );
    }

    #[test]
    fn overlapping_edits_merge_and_revert_restores_the_file() {
        let mut edits = SignalEdits::default();
        edits.set_range(signal(), 15, Some(20), v(0));
        edits.set_range(signal(), 18, Some(40), v(0));
        // The file is 0 from 30 on too, so nothing changes where the edit ends.
        assert_eq!(query(&edits, 25), (Some((15, 0)), None));
        assert_eq!(merged(&edits), vec![(0, v(0)), (10, v(1)), (15, v(0))]);

        assert!(edits.revert(signal()));
        assert_eq!(query(&edits, 25), (Some((10, 1)), Some(30)));
        assert!(!edits.revert(signal()));
    }

    /// Edits that match the value around them must not look like changes, or they are drawn
    /// as glitches.
    #[test]
    fn edits_equal_to_surrounding_values_report_no_change() {
        let mut edits = SignalEdits::default();
        // A 0 painted where the file is already 0, as left after painting a pulse and then
        // painting it low again.
        edits.set_range(signal(), 35, Some(40), v(0));
        assert_eq!(query(&edits, 32), (Some((30, 0)), None));
        assert_eq!(query(&edits, 37), (Some((30, 0)), None));
        assert_eq!(query(&edits, 45), (Some((30, 0)), None));

        // A 1 painted where the file is already 1, next to a real edit.
        edits.set_range(signal(), 15, Some(20), v(1));
        edits.set_range(signal(), 20, Some(25), v(0));
        assert_eq!(query(&edits, 12), (Some((10, 1)), Some(20)));
        assert_eq!(query(&edits, 17), (Some((10, 1)), Some(20)));
        assert_eq!(query(&edits, 22), (Some((20, 0)), Some(25)));
        assert_eq!(query(&edits, 27), (Some((25, 1)), Some(30)));
    }

    fn spans(edits: &SignalEdits, from: u64, to: u64) -> Vec<(u64, u64)> {
        edits.changed_spans(signal(), from, to, |t| Some(file_query(t)))
    }

    #[test]
    fn changed_spans_cover_only_differences_from_the_file() {
        let mut edits = SignalEdits::default();
        assert!(spans(&edits, 0, 100).is_empty());

        // 0 over 5..35: differs from the file (1) only in 10..30.
        edits.set_range(signal(), 5, Some(35), v(0));
        assert_eq!(spans(&edits, 0, 100), [(10, 30)]);
        // Clipped to the requested range.
        assert_eq!(spans(&edits, 20, 25), [(20, 25)]);

        // A 1 from 40 onwards differs from the file's 0; it joins nothing before it.
        edits.set_range(signal(), 40, None, v(1));
        assert_eq!(spans(&edits, 0, 100), [(10, 30), (40, 100)]);

        // Painting the file's own value back leaves no difference.
        edits.set_range(signal(), 0, None, v(0));
        edits.set_range(signal(), 10, Some(30), v(1));
        assert!(spans(&edits, 0, 100).is_empty());
    }

    #[test]
    fn open_ended_edit_hides_later_file_changes() {
        let mut edits = SignalEdits::default();
        edits.set_range(signal(), 5, None, v(1));
        assert_eq!(query(&edits, 50), (Some((5, 1)), None));
        assert_eq!(merged(&edits), vec![(0, v(0)), (5, v(1))]);
    }

    #[test]
    fn unloaded_signals_stay_unloaded() {
        let mut edits = SignalEdits::default();
        edits.set_range(signal(), 15, Some(20), v(0));
        assert!(edits.query(signal(), 25, |_| None).is_none());
    }

    #[test]
    fn snapshots_are_not_changed_by_later_edits() {
        let mut edits = SignalEdits::default();
        edits.set_range(signal(), 15, Some(20), v(0));
        let snapshot = edits.clone();
        edits.set_range(signal(), 0, None, v(1));
        assert_eq!(query(&snapshot, 17), (Some((15, 0)), Some(20)));
        assert_eq!(query(&edits, 17), (Some((0, 1)), None));
    }
}
