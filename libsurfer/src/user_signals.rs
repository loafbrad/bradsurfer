//! Signals created and drawn by the user, stored next to the waveform data loaded from a file.
//!
//! User signals live in a virtual top-level scope named [`USER_SCOPE`] and are resolved by
//! [`crate::wellen::WellenContainer`] before it looks at the wellen hierarchy, so the rest of
//! Surfer (drawing, translators, the value column, cursor snapping, ...) treats them like any
//! other variable.
use eyre::{Result, bail};
use num::{BigInt, BigUint, Integer as _, One as _, Zero as _};
use serde::{Deserialize, Serialize};
use surfer_translation_types::{VariableEncoding, VariableType, VariableValue};
use tracing::error;

use crate::SystemState;
use crate::displayed_item::{DisplayedFieldRef, DisplayedItem};
use crate::signal_edits::SignalEdits;
use crate::wave_container::{
    QueryResult, ScopeRef, ScopeRefExt as _, VarId, VariableMeta, VariableRef, VariableRefExt as _,
};
use crate::wave_container::{SignalId, WaveContainer};
use crate::wave_data::WaveData;

/// Name of the virtual top-level scope holding all user signals.
pub const USER_SCOPE: &str = "user";

/// Theme color given to newly created signals. Themes without it use their default color.
pub const USER_SIGNAL_COLOR: &str = "Yellow";

/// A multi-bit edit drawn on the canvas, waiting for the user to type a value into the box
/// shown inside the signal's row.
#[derive(Debug, Clone)]
pub struct PendingUserSignalValue {
    pub variable: VariableRef,
    pub start: BigInt,
    pub end: Option<BigInt>,
    pub viewport_idx: usize,
    /// Radix used for input without a prefix, from the signal's display format.
    pub radix: u32,
    pub text: String,
    pub error: Option<String>,
    /// Keyboard focus has been given to the box once. Losing it afterwards cancels the edit.
    pub focused: bool,
}

/// What edits on the canvas snap to.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub enum WaveEditSnap {
    /// Cells between the timeline's tick marks.
    #[default]
    Ticks,
    /// Cells between the transitions of another signal, e.g. a clock.
    Signal(VariableRef),
}

/// The cell of a uniform grid through the timeline `ticks` (`(label, x, time)`) that contains
/// `time`, as `(start, end)`. `None` if there are fewer than two ticks.
#[must_use]
pub fn tick_cell(ticks: &[(String, f32, i64)], time: &BigInt) -> Option<(BigInt, BigInt)> {
    let [(_, _, first), (_, _, second), ..] = ticks else {
        return None;
    };
    let step = BigInt::from(second - first);
    if step <= BigInt::zero() {
        return None;
    }
    let origin = BigInt::from(*first);
    let start = &origin + (time - &origin).div_floor(&step) * &step;
    let end = &start + &step;
    Some((start, end))
}

/// The radix used for typed values of a signal shown with the translator `format`.
#[must_use]
pub fn radix_for_format(format: &str) -> u32 {
    if format.starts_with("Hex") {
        16
    } else if format.starts_with("Binary") {
        2
    } else if format.starts_with("Octal") {
        8
    } else {
        10
    }
}

/// Formats `value` in `radix`, the inverse of [`parse_user_value`] with the same radix.
#[must_use]
pub fn format_in_radix(value: &VariableValue, radix: u32) -> String {
    match value {
        VariableValue::BigUint(v) => v.to_str_radix(radix),
        VariableValue::String(s) => s.clone(),
    }
}

/// Text fields of the "New signal" dialog.
#[derive(Debug, Clone)]
pub struct NewUserSignalDialog {
    pub name: String,
    pub width: String,
    pub error: Option<String>,
}

impl Default for NewUserSignalDialog {
    fn default() -> Self {
        Self {
            name: String::new(),
            width: String::from("1"),
            error: None,
        }
    }
}

/// Parses a value typed by the user, in `default_radix` unless it has a `0x`, `0b`, `0o` or
/// `0d` prefix. Underscores are ignored.
pub fn parse_user_value(text: &str, width: u32, default_radix: u32) -> Result<VariableValue> {
    let text: String = text.trim().chars().filter(|c| *c != '_').collect();
    let lower = text.to_lowercase();
    let (digits, radix) = if let Some(d) = lower.strip_prefix("0x") {
        (d, 16)
    } else if let Some(d) = lower.strip_prefix("0b") {
        (d, 2)
    } else if let Some(d) = lower.strip_prefix("0o") {
        (d, 8)
    } else if let Some(d) = lower.strip_prefix("0d") {
        (d, 10)
    } else {
        (lower.as_str(), default_radix)
    };
    let Some(value) = BigUint::parse_bytes(digits.as_bytes(), radix) else {
        bail!("'{text}' is not a number");
    };
    if value.bits() > u64::from(width) {
        bail!("{text} does not fit in {width} bits");
    }
    Ok(VariableValue::BigUint(value))
}

/// Values of a signal over time, as changes sorted by time with no two consecutive changes
/// having the same value. Used for user signals and for edits of loaded signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeList<V> {
    changes: Vec<(u64, V)>,
}

impl<V: Clone + PartialEq> ChangeList<V> {
    /// A list holding `initial` from time 0.
    pub fn new(initial: V) -> Self {
        Self {
            changes: vec![(0, initial)],
        }
    }

    #[must_use]
    pub fn changes(&self) -> &[(u64, V)] {
        &self.changes
    }

    /// Index of the first change strictly after `time`.
    fn index_after(&self, time: u64) -> usize {
        self.changes.partition_point(|(t, _)| *t <= time)
    }

    /// The change in effect at `time`, as `(change time, value)`.
    #[must_use]
    pub fn entry_at(&self, time: u64) -> Option<(u64, &V)> {
        let idx = self.index_after(time);
        (idx > 0).then(|| (self.changes[idx - 1].0, &self.changes[idx - 1].1))
    }

    #[must_use]
    pub fn value_at(&self, time: u64) -> Option<&V> {
        self.entry_at(time).map(|(_, v)| v)
    }

    /// The time of the first change strictly after `time`.
    #[must_use]
    pub fn next_after(&self, time: u64) -> Option<u64> {
        self.changes.get(self.index_after(time)).map(|(t, _)| *t)
    }

    /// Set the value in `[start, end)`, or from `start` onwards if `end` is `None`.
    /// The value that was in effect at `end` resumes there.
    pub fn set_range(&mut self, start: u64, end: Option<u64>, value: V) {
        if end.is_some_and(|end| end <= start) {
            return;
        }
        let resume = end.and_then(|end| self.value_at(end).cloned().map(|v| (end, v)));
        self.changes
            .retain(|(t, _)| *t < start || end.is_some_and(|end| *t >= end));
        let insert_at = self.changes.partition_point(|(t, _)| *t < start);
        self.changes.insert(insert_at, (start, value));
        if let Some((end, resume_value)) = resume
            && self.changes.get(insert_at + 1).map(|(t, _)| *t) != Some(end)
        {
            self.changes.insert(insert_at + 1, (end, resume_value));
        }
        self.changes.dedup_by(|next, prev| next.1 == prev.1);
    }

    /// The span of constant value containing `time`, as `(start, end)` where `end` is `None`
    /// for the last span.
    #[must_use]
    pub fn segment_at(&self, time: u64) -> (u64, Option<u64>) {
        (
            self.entry_at(time).map_or(0, |(t, _)| t),
            self.next_after(time),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserSignal {
    pub name: String,
    pub width: u32,
    changes: ChangeList<VariableValue>,
}

impl UserSignal {
    fn new(name: String, width: u32) -> Self {
        Self {
            name,
            width,
            changes: ChangeList::new(VariableValue::BigUint(BigUint::zero())),
        }
    }

    fn query(&self, time: u64) -> QueryResult {
        QueryResult {
            current: self
                .changes
                .entry_at(time)
                .map(|(t, v)| (BigUint::from(t), v.clone())),
            next: self.changes.next_after(time).map(BigUint::from),
        }
    }

    fn fits(&self, value: &VariableValue) -> bool {
        value_fits(value, self.width)
    }
}

/// Whether `value` can be stored in a signal `width` bits wide.
#[must_use]
pub fn value_fits(value: &VariableValue, width: u32) -> bool {
    match value {
        VariableValue::BigUint(v) => v.bits() <= u64::from(width),
        VariableValue::String(s) => s.len() == width as usize,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserSignals {
    signals: Vec<UserSignal>,
}

impl UserSignals {
    #[must_use]
    pub fn is_user_scope(scope: &ScopeRef) -> bool {
        scope.strs() == [USER_SCOPE]
    }

    #[must_use]
    pub fn scope() -> ScopeRef {
        ScopeRef::from_strs(&[USER_SCOPE])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    fn variable_ref(idx: usize, signal: &UserSignal) -> VariableRef {
        VariableRef::new_with_id_and_index(
            Self::scope(),
            signal.name.clone(),
            VarId::User(idx as u32),
            None,
        )
    }

    /// Checks that `name` can be used in commands and hierarchy paths.
    pub fn validate_name(name: &str) -> Result<()> {
        if name.is_empty() {
            bail!("Enter a name for the signal");
        }
        if name.contains(|c: char| c.is_whitespace() || c == '.') {
            bail!("Signal names cannot contain spaces or '.'");
        }
        Ok(())
    }

    /// Adds a new signal, initially 0 from time 0, and returns a reference to it.
    pub fn create(&mut self, name: String, width: u32) -> Result<VariableRef> {
        Self::validate_name(&name)?;
        if width == 0 {
            bail!("User signals must be at least one bit wide");
        }
        if self.signals.iter().any(|s| s.name == name) {
            bail!("A user signal named '{name}' already exists");
        }
        self.signals.push(UserSignal::new(name, width));
        let idx = self.signals.len() - 1;
        Ok(Self::variable_ref(idx, &self.signals[idx]))
    }

    /// The index of the signal `variable` refers to, if it is a user signal.
    #[must_use]
    pub fn index_of(&self, variable: &VariableRef) -> Option<usize> {
        match variable.id {
            VarId::User(idx) => Some(idx as usize).filter(|i| *i < self.signals.len()),
            _ if Self::is_user_scope(&variable.path) => {
                self.signals.iter().position(|s| s.name == variable.name)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn get(&self, idx: usize) -> Option<&UserSignal> {
        self.signals.get(idx)
    }

    #[must_use]
    pub fn variables(&self) -> Vec<VariableRef> {
        self.signals
            .iter()
            .enumerate()
            .map(|(idx, s)| Self::variable_ref(idx, s))
            .collect()
    }

    #[must_use]
    pub fn update_variable_ref(&self, variable: &VariableRef) -> Option<VariableRef> {
        let idx = self.index_of(variable)?;
        Some(Self::variable_ref(idx, &self.signals[idx]))
    }

    #[must_use]
    pub fn query(&self, idx: usize, time: &BigUint) -> Option<QueryResult> {
        let time = u64::try_from(time).unwrap_or(u64::MAX);
        self.signals.get(idx).map(|s| s.query(time))
    }

    /// All value changes of a signal, sorted by time.
    #[must_use]
    pub fn changes(&self, idx: usize) -> Vec<(u64, VariableValue)> {
        self.signals
            .get(idx)
            .map(|s| s.changes.changes().to_vec())
            .unwrap_or_default()
    }

    pub fn meta(&self, variable: &VariableRef) -> Result<VariableMeta> {
        let Some(signal) = self.index_of(variable).map(|idx| &self.signals[idx]) else {
            bail!("Unknown user signal {}", variable.full_path_string());
        };
        Ok(VariableMeta {
            var: variable.clone(),
            num_bits: Some(signal.width),
            variable_type: Some(if signal.width == 1 {
                VariableType::Bit
            } else {
                VariableType::VCDWire
            }),
            variable_type_name: None,
            index: None,
            direction: None,
            enum_map: Default::default(),
            encoding: VariableEncoding::BitVector,
        })
    }

    /// Sets the value of a user signal in `[start, end)`, or from `start` onwards if `end`
    /// is `None`.
    pub fn set_range(
        &mut self,
        variable: &VariableRef,
        start: &BigUint,
        end: Option<&BigUint>,
        value: VariableValue,
    ) -> Result<()> {
        let Some(signal) = self.index_of(variable).map(|idx| &mut self.signals[idx]) else {
            bail!("Unknown user signal {}", variable.full_path_string());
        };
        if !signal.fits(&value) {
            bail!(
                "Value {value} does not fit in {} ({} bits)",
                signal.name,
                signal.width
            );
        }
        let to_u64 = |t: &BigUint| u64::try_from(t).unwrap_or(u64::MAX);
        signal
            .changes
            .set_range(to_u64(start), end.map(to_u64), value);
        Ok(())
    }

    /// Flips a one-bit user signal over the span of constant value containing `time`.
    pub fn toggle_at(&mut self, variable: &VariableRef, time: &BigUint) -> Result<()> {
        let Some(signal) = self.index_of(variable).map(|idx| &mut self.signals[idx]) else {
            bail!("Unknown user signal {}", variable.full_path_string());
        };
        if signal.width != 1 {
            bail!("Only one-bit signals can be toggled");
        }
        let time = u64::try_from(time).unwrap_or(u64::MAX);
        let (start, end) = signal.changes.segment_at(time);
        let current_is_one =
            matches!(signal.changes.value_at(time), Some(VariableValue::BigUint(v)) if v.is_one());
        let new = if current_is_one {
            BigUint::zero()
        } else {
            BigUint::one()
        };
        signal
            .changes
            .set_range(start, end, VariableValue::BigUint(new));
        Ok(())
    }
}

/// Restores user signals and edits of file signals saved in an undo snapshot. Edits saved for
/// an earlier load of the file are skipped, since their signal references may point at other
/// signals now.
pub(crate) fn restore_edited_signals(
    waves: &mut WaveData,
    user_signals: Option<UserSignals>,
    signal_edits: Option<(u64, SignalEdits)>,
) {
    if let (Some(saved), Some(current)) = (user_signals, waves.user_signals_mut()) {
        *current = saved;
    }
    let Some(container) = waves.inner.as_waves_mut() else {
        return;
    };
    let current_id = container.signal_edits().map(|(id, _)| id);
    if let Some((saved_id, saved)) = signal_edits
        && Some(saved_id) == current_id
        && let Some(current) = container.signal_edits_mut()
    {
        *current = saved;
    }
}

impl SystemState {
    /// Applies `edit` to copies of the user signals and of the edits of file signals and, if
    /// it succeeds, stores the results. An undo step named `undo_msg` is created unless it is
    /// `None`, which is used for the later parts of a paint stroke so the whole stroke is
    /// undone at once.
    fn edit_signals<T>(
        &mut self,
        undo_msg: Option<String>,
        edit: impl FnOnce(&WaveContainer, &mut UserSignals, &mut SignalEdits) -> Result<T>,
    ) -> Option<T> {
        let Some(container) = self.user.waves.as_ref()?.inner.as_waves() else {
            error!("Signals can only be edited in waveforms loaded from a file");
            return None;
        };
        let (Some(user_signals), Some((_, signal_edits))) =
            (container.user_signals(), container.signal_edits())
        else {
            error!("Signals can only be edited in waveforms loaded from a file");
            return None;
        };
        // Cheap: edits of file signals are shared until changed.
        let (mut user_signals, mut signal_edits) = (user_signals.clone(), signal_edits.clone());
        let result = match edit(container, &mut user_signals, &mut signal_edits) {
            Ok(result) => result,
            Err(e) => {
                error!("{e:#}");
                return None;
            }
        };
        if let Some(undo_msg) = undo_msg {
            self.save_current_canvas(undo_msg);
        }
        let waves = self.user.waves.as_mut()?;
        *waves.user_signals_mut()? = user_signals;
        *waves.inner.as_waves_mut()?.signal_edits_mut()? = signal_edits;
        self.invalidate_signal_data_caches();
        Some(result)
    }

    /// Whether `variable` is a signal from the file with edits (possibly through an alias).
    pub(crate) fn is_edited(&self, variable: &VariableRef) -> bool {
        self.user
            .waves
            .as_ref()
            .and_then(|w| w.inner.as_waves())
            .is_some_and(|w| w.is_edited(variable))
    }

    /// Makes everything showing signal values read them again after they were edited.
    pub(crate) fn invalidate_signal_data_caches(&mut self) {
        if let Some(waves) = self.user.waves.as_mut() {
            // Analog caches are keyed by signal, so rebuild them from the edited data.
            waves.cache_generation += 1;
        }
        self.frame_buffer_array_cache = None;
        self.frame_buffer_pixel_cache = None;
        self.memory_viewer_cache = None;
        self.invalidate_draw_commands();
    }

    pub(crate) fn create_user_signal(&mut self, name: String, width: u32) {
        let Some(variable) = self
            .edit_signals(Some(format!("Create signal {name}")), |_, signals, _| {
                signals.create(name.clone(), width)
            })
        else {
            return;
        };
        if let Some(waves) = self.user.waves.as_mut() {
            waves.add_variables(
                &self.translators,
                vec![variable.clone()],
                None,
                true,
                false,
                None,
            );
            // Created signals are yellow by default, to tell them apart from the file's.
            for item in waves.displayed_items.values_mut() {
                if let DisplayedItem::Variable(displayed) = item
                    && displayed.variable_ref == variable
                    && displayed.color.is_none()
                {
                    displayed.color = Some(USER_SIGNAL_COLOR.to_string());
                }
            }
        }
        self.invalidate_draw_commands();
    }

    /// Sets the value of a user signal or of a signal from the file in `[start, end)`.
    pub(crate) fn set_signal_value(
        &mut self,
        variable: &VariableRef,
        start: &BigInt,
        end: Option<&BigInt>,
        value: VariableValue,
        continue_stroke: bool,
    ) {
        let to_unsigned = |t: &BigInt| t.to_biguint().unwrap_or_default();
        let start = to_unsigned(start);
        let end = end.map(to_unsigned);
        let undo_msg = (!continue_stroke).then(|| format!("Edit {}", variable.name));
        self.edit_signals(undo_msg, |container, user_signals, signal_edits| {
            if user_signals.index_of(variable).is_some() {
                return user_signals.set_range(variable, &start, end.as_ref(), value);
            }
            let Some(width) = container.editable_width(variable) else {
                bail!(
                    "{} cannot be edited: only loaded bit-vector signals can",
                    variable.full_path_string()
                );
            };
            // Only two-state values can be written for now.
            if !matches!(value, VariableValue::BigUint(_)) || !value_fits(&value, width) {
                bail!(
                    "{value} is not a 0/1 value that fits in {} ({width} bits)",
                    variable.full_path_string()
                );
            }
            let SignalId::Wellen(signal_ref) = container.signal_id(variable)? else {
                bail!(
                    "{} is not a signal from the file",
                    variable.full_path_string()
                );
            };
            let to_u64 = |t: &BigUint| u64::try_from(t).unwrap_or(u64::MAX);
            signal_edits.set_range(signal_ref, to_u64(&start), end.as_ref().map(to_u64), value);
            Ok(())
        });
    }

    pub(crate) fn toggle_user_signal(&mut self, variable: &VariableRef, time: &BigInt) {
        let time = time.to_biguint().unwrap_or_default();
        self.edit_signals(Some(format!("Edit {}", variable.name)), |_, signals, _| {
            signals.toggle_at(variable, &time)
        });
    }

    /// Restores the file's values of `variable` (and its aliases), or of all signals.
    pub(crate) fn revert_signal_edits(&mut self, variable: Option<&VariableRef>) {
        let undo_msg = variable.map_or_else(
            || "Revert all edits".to_string(),
            |v| format!("Revert edits of {}", v.name),
        );
        self.edit_signals(Some(undo_msg), |container, _, signal_edits| {
            match variable {
                Some(variable) => {
                    let SignalId::Wellen(signal_ref) = container.signal_id(variable)? else {
                        bail!(
                            "{} is not a signal from the file",
                            variable.full_path_string()
                        );
                    };
                    if !signal_edits.revert(signal_ref) {
                        bail!("{} has no edits", variable.full_path_string());
                    }
                }
                None => {
                    if signal_edits.is_empty() {
                        bail!("No signals have been edited");
                    }
                    signal_edits.revert_all();
                }
            }
            Ok(())
        });
    }

    /// The radix for typing values of a user signal, following how it is displayed.
    fn user_signal_radix(&self, variable: &VariableRef) -> u32 {
        let Some(waves) = self.user.waves.as_ref() else {
            return 10;
        };
        let displayed = waves
            .displayed_items
            .iter()
            .find_map(|(item_ref, item)| match item {
                DisplayedItem::Variable(v) if &v.variable_ref == variable => Some(*item_ref),
                _ => None,
            });
        let meta = waves
            .inner
            .as_waves()
            .and_then(|w| w.variable_meta(variable).ok());
        match (displayed, meta) {
            (Some(item_ref), Some(meta)) => {
                let field: DisplayedFieldRef = item_ref.into();
                radix_for_format(
                    &waves
                        .variable_translator_with_meta(&field, &self.translators, &meta)
                        .name(),
                )
            }
            _ => 10,
        }
    }

    pub(crate) fn open_user_signal_value_editor(
        &mut self,
        variable: VariableRef,
        start: BigInt,
        end: Option<BigInt>,
        viewport_idx: usize,
    ) {
        let radix = self.user_signal_radix(&variable);
        // Start from the value currently at the start of the range.
        let text = self
            .user
            .waves
            .as_ref()
            .and_then(|w| w.inner.as_waves())
            .zip(start.to_biguint())
            .and_then(|(w, t)| w.query_variable(&variable, &t).ok().flatten())
            .and_then(|r| r.current)
            .map(|(_, value)| format_in_radix(&value, radix))
            .unwrap_or_default();
        *self.pending_user_signal_value.borrow_mut() = Some(PendingUserSignalValue {
            variable,
            start,
            end,
            viewport_idx,
            radix,
            text,
            error: None,
            focused: false,
        });
    }

    /// Applies the value typed into the pending multi-bit edit, keeping the edit open with an
    /// error message if the value is invalid.
    pub(crate) fn commit_pending_user_signal_value(&mut self) {
        let Some(pending) = self.pending_user_signal_value.borrow_mut().take() else {
            return;
        };
        let width = self
            .user
            .waves
            .as_ref()
            .and_then(|w| w.inner.as_waves())
            .and_then(|w| w.editable_width(&pending.variable))
            .unwrap_or(0);
        match parse_user_value(&pending.text, width, pending.radix) {
            Ok(value) => self.set_signal_value(
                &pending.variable,
                &pending.start,
                pending.end.as_ref(),
                value,
                false,
            ),
            Err(e) => {
                // Keep the box open, and give it focus again, to fix the value.
                *self.pending_user_signal_value.borrow_mut() = Some(PendingUserSignalValue {
                    error: Some(format!("{e:#}")),
                    focused: false,
                    ..pending
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: u32) -> VariableValue {
        VariableValue::BigUint(BigUint::from(x))
    }

    fn signal(changes: &[(u64, u32)]) -> UserSignal {
        UserSignal {
            name: "s".to_string(),
            width: 8,
            changes: ChangeList {
                changes: changes.iter().map(|(t, x)| (*t, v(*x))).collect(),
            },
        }
    }

    fn changes(s: &UserSignal) -> Vec<(u64, u32)> {
        s.changes
            .changes()
            .iter()
            .map(|(t, val)| match val {
                VariableValue::BigUint(b) => (*t, u32::try_from(b).unwrap()),
                VariableValue::String(_) => panic!("unexpected string"),
            })
            .collect()
    }

    #[test]
    fn set_range_inside_constant_span_resumes_old_value() {
        let mut s = signal(&[(0, 0)]);
        s.changes.set_range(10, Some(20), v(5));
        assert_eq!(changes(&s), [(0, 0), (10, 5), (20, 0)]);
    }

    #[test]
    fn set_range_replaces_changes_inside_range() {
        let mut s = signal(&[(0, 0), (12, 1), (15, 2), (30, 3)]);
        s.changes.set_range(10, Some(20), v(7));
        assert_eq!(changes(&s), [(0, 0), (10, 7), (20, 2), (30, 3)]);
    }

    #[test]
    fn set_range_keeps_existing_change_at_end() {
        let mut s = signal(&[(0, 0), (20, 1)]);
        s.changes.set_range(10, Some(20), v(7));
        assert_eq!(changes(&s), [(0, 0), (10, 7), (20, 1)]);
    }

    #[test]
    fn set_range_merges_equal_neighbours() {
        let mut s = signal(&[(0, 0), (10, 5), (20, 0)]);
        s.changes.set_range(10, Some(20), v(0));
        assert_eq!(changes(&s), [(0, 0)]);

        let mut s = signal(&[(0, 0), (10, 5), (20, 0)]);
        s.changes.set_range(20, Some(30), v(5));
        assert_eq!(changes(&s), [(0, 0), (10, 5), (30, 0)]);
    }

    #[test]
    fn set_range_open_ended_and_empty() {
        let mut s = signal(&[(0, 0), (10, 1), (50, 2)]);
        s.changes.set_range(20, None, v(9));
        assert_eq!(changes(&s), [(0, 0), (10, 1), (20, 9)]);

        s.changes.set_range(30, Some(30), v(4));
        assert_eq!(changes(&s), [(0, 0), (10, 1), (20, 9)]);
    }

    #[test]
    fn set_range_past_last_change() {
        let mut s = signal(&[(0, 0)]);
        s.changes.set_range(100, Some(200), v(1));
        assert_eq!(changes(&s), [(0, 0), (100, 1), (200, 0)]);
    }

    #[test]
    fn query_returns_current_and_next() {
        let s = signal(&[(5, 1), (10, 2)]);
        let r = s.query(3);
        assert!(r.current.is_none());
        assert_eq!(r.next, Some(BigUint::from(5u32)));

        let r = s.query(7);
        assert_eq!(r.current, Some((BigUint::from(5u32), v(1))));
        assert_eq!(r.next, Some(BigUint::from(10u32)));

        let r = s.query(10);
        assert_eq!(r.current, Some((BigUint::from(10u32), v(2))));
        assert_eq!(r.next, None);
    }

    #[test]
    fn create_rejects_duplicates_and_zero_width() {
        let mut signals = UserSignals::default();
        let r = signals.create("a".to_string(), 1).unwrap();
        assert_eq!(signals.index_of(&r), Some(0));
        assert!(signals.create("a".to_string(), 1).is_err());
        assert!(signals.create("b".to_string(), 0).is_err());
    }

    #[test]
    fn toggle_flips_segment() {
        let mut signals = UserSignals::default();
        let r = signals.create("clk".to_string(), 1).unwrap();
        signals
            .set_range(&r, &BigUint::from(10u32), Some(&BigUint::from(20u32)), v(1))
            .unwrap();
        signals.toggle_at(&r, &BigUint::from(15u32)).unwrap();
        assert_eq!(signals.changes(0), vec![(0, v(0))]);
        signals.toggle_at(&r, &BigUint::from(15u32)).unwrap();
        assert_eq!(signals.changes(0), vec![(0, v(1))]);
    }

    #[test]
    fn parse_user_value_formats() {
        assert_eq!(parse_user_value("10", 8, 10).unwrap(), v(10));
        assert_eq!(parse_user_value("0xFF", 8, 10).unwrap(), v(255));
        assert_eq!(parse_user_value("0b1010_1010", 8, 10).unwrap(), v(170));
        assert_eq!(parse_user_value(" 0o17 ", 8, 10).unwrap(), v(15));
        assert!(parse_user_value("0x100", 8, 10).is_err());
        assert!(parse_user_value("abc", 8, 10).is_err());
        assert!(parse_user_value("", 8, 10).is_err());
    }

    #[test]
    fn parse_user_value_default_radix() {
        assert_eq!(parse_user_value("ff", 8, 16).unwrap(), v(255));
        assert_eq!(parse_user_value("101", 8, 2).unwrap(), v(5));
        assert_eq!(parse_user_value("0d10", 8, 16).unwrap(), v(10));
        assert_eq!(parse_user_value("0b11", 8, 16).unwrap(), v(3));
        assert!(parse_user_value("12", 8, 2).is_err());
    }

    #[test]
    fn radix_and_formatting_round_trip() {
        assert_eq!(radix_for_format("Hexadecimal"), 16);
        assert_eq!(radix_for_format("Binary (with groups)"), 2);
        assert_eq!(radix_for_format("Octal"), 8);
        assert_eq!(radix_for_format("Unsigned"), 10);
        for radix in [2, 8, 10, 16] {
            let text = format_in_radix(&v(0xab), radix);
            assert_eq!(parse_user_value(&text, 8, radix).unwrap(), v(0xab));
        }
    }

    /// The messages the embed playground injects to choose the snap target.
    #[test]
    fn snap_messages_deserialize_from_json() {
        let ticks: crate::message::Message =
            serde_json::from_str(r#"{"SetWaveEditSnap": "Ticks"}"#).unwrap();
        assert!(matches!(
            ticks,
            crate::message::Message::SetWaveEditSnap(WaveEditSnap::Ticks)
        ));
        let signal: crate::message::Message = serde_json::from_str(
            r#"{"SetWaveEditSnap": {"Signal": {"path": {"strs": ["tb", "dut"], "id": "None"},
                "name": "clk", "id": "None", "index": null}}}"#,
        )
        .unwrap();
        let crate::message::Message::SetWaveEditSnap(WaveEditSnap::Signal(variable)) = signal
        else {
            panic!("expected a signal snap target");
        };
        assert_eq!(variable.full_path_string(), "tb.dut.clk");
    }

    fn ticks(times: &[i64]) -> Vec<(String, f32, i64)> {
        times.iter().map(|t| (String::new(), 0.0, *t)).collect()
    }

    #[test]
    fn tick_cells_extend_beyond_visible_ticks() {
        let t = ticks(&[100, 150, 200]);
        let cell = |time: i64| tick_cell(&t, &BigInt::from(time)).unwrap();
        assert_eq!(cell(120), (BigInt::from(100), BigInt::from(150)));
        assert_eq!(cell(150), (BigInt::from(150), BigInt::from(200)));
        assert_eq!(cell(730), (BigInt::from(700), BigInt::from(750)));
        assert_eq!(cell(10), (BigInt::from(0), BigInt::from(50)));
        assert_eq!(cell(-10), (BigInt::from(-50), BigInt::from(0)));
        assert!(tick_cell(&ticks(&[100]), &BigInt::from(5)).is_none());
        assert!(tick_cell(&ticks(&[100, 100]), &BigInt::from(5)).is_none());
    }

    #[test]
    fn set_range_rejects_too_wide_values() {
        let mut signals = UserSignals::default();
        let r = signals.create("b".to_string(), 2).unwrap();
        assert!(signals.set_range(&r, &BigUint::zero(), None, v(4)).is_err());
        assert!(signals.set_range(&r, &BigUint::zero(), None, v(3)).is_ok());
    }
}
