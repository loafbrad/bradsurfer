//! Drawing on signals with the mouse while edit mode is on: user signals (see
//! [`crate::user_signals`]) and bit-vector signals from the file (see
//! [`crate::signal_edits`]).
//!
//! One-bit signals are painted: the height of the pointer in the row picks the level (top half
//! high, bottom half low), a press paints the cell under the pointer and dragging paints every
//! cell the pointer passes. Multi-bit signals get their value from a box shown in the row, for
//! a dragged range of cells or for the clicked segment. Cells come from the timeline ticks or
//! from the transitions of a reference signal, see [`WaveEditSnap`].
use ecolor::Color32;
use egui::{CursorIcon, Id, Key, Order, Stroke, Ui};
use emath::{Pos2, Rect, RectTransform};
use num::{BigInt, BigUint};
use surfer_translation_types::VariableValue;

use crate::SystemState;
use crate::displayed_item::DisplayedItem;
use crate::item_drawing_info::ItemDrawingInfo;
use crate::message::Message;
use crate::user_signals::{UserSignals, WaveEditSnap, tick_cell};
use crate::wave_container::VariableRef;
use crate::wave_data::WaveData;

const NEW_SIGNAL_NAME_ID: &str = "new_user_signal_name";
const NEW_SIGNAL_WIDTH_ID: &str = "new_user_signal_width";
const SIGNAL_VALUE_ID: &str = "user_signal_value";

/// Reports focus changes of a text field so keyboard shortcuts are disabled while typing.
fn track_text_focus(response: &egui::Response, id: &str, msgs: &mut Vec<Message>) {
    if response.gained_focus() {
        msgs.push(Message::SetTextEditFocused(id.to_string(), true));
    }
    if response.lost_focus() {
        msgs.push(Message::SetTextEditFocused(id.to_string(), false));
    }
}

fn release_text_focus(ids: &[&str], msgs: &mut Vec<Message>) {
    for id in ids {
        msgs.push(Message::SetTextEditFocused((*id).to_string(), false));
    }
}

/// A span of time edits snap to, as `(start, end)`. `end` is `None` for a span that lasts
/// until the end of the waveform.
type Cell = (BigInt, Option<BigInt>);

/// The canvas geometry needed to map between the pointer and signal rows and times.
pub(crate) struct EditCanvas<'a> {
    pub waves: &'a WaveData,
    pub to_screen: RectTransform,
    /// Converts canvas y-coordinates to the offset-free row positions in
    /// `waves.drawing_infos`.
    pub row_offset: f32,
    pub frame_width: f32,
    pub viewport_idx: usize,
    /// The timeline ticks of this viewport, as `(label, x, time)`.
    pub ticks: &'a [(String, f32, i64)],
}

impl EditCanvas<'_> {
    fn time_at(&self, x: f32) -> BigInt {
        self.waves.viewports[self.viewport_idx]
            .as_time_bigint(x, self.frame_width, self.waves.time_range())
            .max(BigInt::ZERO)
    }

    fn x_of(&self, time: &BigInt) -> f32 {
        self.waves.viewports[self.viewport_idx].pixel_from_time(
            time,
            self.frame_width,
            self.waves.time_range(),
        )
    }

    /// The span of constant value of `variable` containing `time`.
    fn segment_at(&self, variable: &VariableRef, time: &BigInt) -> Option<Cell> {
        let result = self
            .waves
            .inner
            .as_waves()?
            .query_variable(variable, &time.to_biguint()?)
            .ok()??;
        let start = result
            .current
            .map(|(t, _)| BigInt::from(t))
            .unwrap_or_default();
        Some((start, result.next.map(BigInt::from)))
    }

    /// Screen rectangle covering `cell` in the row between `top` and `bottom`.
    fn cell_rect(&self, cell: &Cell, top: f32, bottom: f32) -> Rect {
        let x0 = self.x_of(&cell.0).max(-1.);
        let x1 = cell
            .1
            .as_ref()
            .map_or(self.frame_width, |end| self.x_of(end))
            .min(self.frame_width + 1.);
        Rect::from_two_pos(
            self.to_screen
                .transform_pos(Pos2::new(x0, top + self.row_offset)),
            self.to_screen
                .transform_pos(Pos2::new(x1, bottom + self.row_offset)),
        )
    }
}

/// The row of an editable signal on the canvas.
#[derive(Clone)]
struct EditTarget {
    variable: VariableRef,
    width: u32,
    top: f32,
    bottom: f32,
    /// Where the trace of a one-bit signal is drawn when high and when low, in the same
    /// offset-free coordinates as `top` and `bottom`.
    high_y: f32,
    low_y: f32,
}

impl EditTarget {
    /// One-bit signals are set high when the pointer is in the top half of the row.
    fn is_high(&self, pos: Pos2, row_offset: f32) -> bool {
        pos.y - row_offset < (self.top + self.bottom) / 2.
    }
}

/// An edit in progress while the primary button is held, kept in egui's temporary memory.
#[derive(Clone)]
enum EditGesture {
    /// Painting a one-bit signal. `last` is the most recently painted cell.
    Paint {
        target: EditTarget,
        last: Cell,
        high: bool,
    },
    /// Choosing a range of a multi-bit signal, starting at cell `start`.
    Range { target: EditTarget, start: Cell },
}

/// A translucent hint of what an edit will change.
/// Color of the edit a press would make, and of the marker on edited signals.
pub(crate) const EDIT_COLOR: Color32 = Color32::from_rgb(255, 204, 0);

/// A preview of what an edit will change, in screen coordinates.
#[derive(Default)]
pub(crate) struct EditPreview {
    /// Highlighted in yellow: the area under a one-bit signal's new high level, or the range
    /// of a multi-bit edit.
    pub fill: Option<Rect>,
    /// Parts of the current waveform the edit removes, drawn darkened.
    pub erased: Vec<Rect>,
    /// The new waveform including its edges, drawn dashed.
    pub ghost: Vec<Pos2>,
}

impl EditPreview {
    pub(crate) fn draw(&self, painter: &egui::Painter) {
        for rect in &self.erased {
            painter.rect_filled(*rect, 0., Color32::from_black_alpha(170));
        }
        if let Some(fill) = self.fill {
            painter.rect_filled(fill, 0., EDIT_COLOR.gamma_multiply(0.25));
        }
        if self.ghost.len() >= 2 {
            painter.extend(egui::Shape::dashed_line(
                &self.ghost,
                Stroke::new(1.5, EDIT_COLOR),
                5.,
                3.,
            ));
        }
    }
}

/// Pixels between the diagonal stripes marking edited stretches.
const STRIPE_SPACING: f32 = 10.;
/// How fast the stripes move, in pixels per second.
const STRIPE_SPEED: f32 = 12.;

/// Result of [`SystemState::handle_wave_edit_input`].
#[derive(Default)]
pub(crate) struct WaveEditInput {
    /// The pointer interaction belongs to the editor, so the cursor should not move.
    pub consumed: bool,
    pub preview: Option<EditPreview>,
}

fn paint_message(variable: &VariableRef, cell: Cell, high: bool, continue_stroke: bool) -> Message {
    Message::SetSignalValue {
        variable: variable.clone(),
        start: cell.0,
        end: cell.1,
        value: VariableValue::BigUint(BigUint::from(u8::from(high))),
        continue_stroke,
    }
}

/// The cells from `last` to `current`, not including `last`, so a fast drag leaves no gaps.
/// If both are the same cell, that cell.
fn cells_between(last: &Cell, current: &Cell) -> Cell {
    if current.0 > last.0 {
        (
            last.1.clone().unwrap_or_else(|| current.0.clone()),
            current.1.clone(),
        )
    } else if current.0 < last.0 {
        (current.0.clone(), Some(last.0.clone()))
    } else {
        current.clone()
    }
}

/// The smallest range covering both cells.
fn cells_union(a: &Cell, b: &Cell) -> Cell {
    let (first, second) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let end = match (&first.1, &second.1) {
        (Some(x), Some(y)) => Some(x.max(y).clone()),
        _ => None,
    };
    (first.0.clone(), end)
}

impl SystemState {
    /// The row of an editable signal at canvas position `pos`.
    fn edit_target_at(&self, c: &EditCanvas, pos: Pos2) -> Option<EditTarget> {
        let (item_ref, info) = c.waves.item_and_drawing_info_at_y(pos.y - c.row_offset)?;
        let Some(DisplayedItem::Variable(displayed)) = c.waves.displayed_items.get(&item_ref)
        else {
            return None;
        };
        // User signals, and loaded bit-vector signals from the file.
        let width = c
            .waves
            .inner
            .as_waves()?
            .editable_width(&displayed.variable_ref)?;
        // The same geometry `draw_wave_data` uses for one-bit traces.
        let layout = &self.user.config.layout;
        let high_y = info.top() + layout.waveforms_gap;
        let low_y =
            high_y + layout.waveforms_line_height * displayed.height_scaling_factor.unwrap_or(1.);
        Some(EditTarget {
            variable: displayed.variable_ref.clone(),
            width,
            top: info.top(),
            bottom: info.bottom(),
            high_y,
            low_y,
        })
    }

    /// Preview of painting `cell` of a one-bit signal high or low: the new waveform as a
    /// dashed line with edges to the current waveform on both sides, a yellow fill under a new
    /// high level, and the current high stretches darkened when painting low erases them.
    fn paint_preview(
        &self,
        c: &EditCanvas,
        target: &EditTarget,
        cell: &Cell,
        high: bool,
    ) -> EditPreview {
        let screen = |x: f32, y: f32| c.to_screen.transform_pos(Pos2::new(x, y + c.row_offset));
        let level_y = |high: bool| if high { target.high_y } else { target.low_y };
        let query = |time: &BigInt| {
            c.waves
                .inner
                .as_waves()?
                .query_variable(&target.variable, &time.to_biguint()?)
                .ok()?
        };
        let is_high = |value: &VariableValue| matches!(value, VariableValue::BigUint(v) if *v == BigUint::from(1u8));
        let high_at = |time: &BigInt| {
            query(time)
                .and_then(|r| r.current)
                .map(|(_, value)| is_high(&value))
        };

        let x_start = c.x_of(&cell.0).max(-1.);
        let x_end = cell
            .1
            .as_ref()
            .map_or(c.frame_width, |end| c.x_of(end))
            .min(c.frame_width + 1.);
        let before = (cell.0 > BigInt::ZERO)
            .then(|| high_at(&(&cell.0 - 1)))
            .flatten();
        let after = cell.1.as_ref().and_then(high_at);

        let mut ghost = vec![];
        if let Some(before) = before.filter(|b| *b != high) {
            ghost.push(screen(x_start, level_y(before)));
        }
        ghost.push(screen(x_start, level_y(high)));
        ghost.push(screen(x_end, level_y(high)));
        if let Some(after) = after.filter(|a| *a != high) {
            ghost.push(screen(x_end, level_y(after)));
        }

        let mut erased = vec![];
        if !high {
            // Walk the stretches of constant value inside the cell.
            let mut time = cell.0.clone();
            for _ in 0..1000 {
                let Some(result) = query(&time) else {
                    break;
                };
                let next = result.next.map(BigInt::from);
                let span_end = match (&next, &cell.1) {
                    (Some(n), Some(e)) => Some(n.min(e).clone()),
                    (Some(n), None) => Some(n.clone()),
                    (None, e) => e.clone(),
                };
                if result.current.is_some_and(|(_, value)| is_high(&value)) {
                    let x1 = span_end.as_ref().map_or(x_end, |t| c.x_of(t).min(x_end));
                    erased.push(Rect::from_two_pos(
                        screen(c.x_of(&time).max(x_start), target.high_y - 2.),
                        screen(x1, target.low_y - 1.),
                    ));
                }
                match (next, &cell.1) {
                    (Some(n), Some(end)) if &n < end => time = n,
                    (Some(n), None) => time = n,
                    _ => break,
                }
                if c.x_of(&time) > x_end {
                    break;
                }
            }
        }

        EditPreview {
            fill: high.then(|| {
                Rect::from_two_pos(screen(x_start, target.high_y), screen(x_end, target.low_y))
            }),
            erased,
            ghost,
        }
    }

    /// Preview of editing `range` of a multi-bit signal.
    fn range_preview(c: &EditCanvas, target: &EditTarget, range: &Cell) -> EditPreview {
        EditPreview {
            fill: Some(c.cell_rect(range, target.top, target.bottom)),
            ..Default::default()
        }
    }

    /// The cell an edit of `variable` at `time` snaps to.
    fn edit_cell_at(&self, c: &EditCanvas, variable: &VariableRef, time: &BigInt) -> Cell {
        let snapped = match &self.wave_edit_snap {
            WaveEditSnap::Ticks => {
                tick_cell(c.ticks, time).map(|(start, end)| (start.max(BigInt::ZERO), Some(end)))
            }
            WaveEditSnap::Signal(reference) => c.segment_at(reference, time),
        };
        snapped
            .or_else(|| c.segment_at(variable, time))
            .unwrap_or_else(|| (time.clone(), None))
    }

    /// Handles primary-button presses and drags on user signal rows while edit mode is on,
    /// and returns a preview of the edit under the pointer.
    pub(crate) fn handle_wave_edit_input(
        &self,
        ui: &Ui,
        response: &egui::Response,
        c: &EditCanvas,
        msgs: &mut Vec<Message>,
    ) -> WaveEditInput {
        if !self.wave_edit_mode {
            return WaveEditInput::default();
        }
        let gesture_id = Id::new("wave_edit_gesture").with(c.viewport_idx);
        let pointer = ui
            .input(|i| i.pointer.interact_pos())
            .map(|p| c.to_screen.inverse().transform_pos(p));
        let (pressed, down) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down()));
        let gesture = ui.data(|d| d.get_temp::<EditGesture>(gesture_id));
        let consumed = |preview| WaveEditInput {
            consumed: true,
            preview,
        };

        match gesture {
            None => {
                // While a value is being typed, the box highlights its own range instead.
                if self.pending_user_signal_value.borrow().is_some() {
                    return WaveEditInput::default();
                }
                let Some(pos) = pointer.filter(|_| response.hovered()) else {
                    return WaveEditInput::default();
                };
                let Some(target) = self.edit_target_at(c, pos) else {
                    return WaveEditInput::default();
                };
                ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                let time = c.time_at(pos.x);
                let cell = self.edit_cell_at(c, &target.variable, &time);

                if pressed {
                    let gesture = if target.width == 1 {
                        let high = target.is_high(pos, c.row_offset);
                        msgs.push(paint_message(&target.variable, cell.clone(), high, false));
                        EditGesture::Paint {
                            target,
                            last: cell,
                            high,
                        }
                    } else {
                        EditGesture::Range {
                            target,
                            start: cell,
                        }
                    };
                    ui.data_mut(|d| d.insert_temp(gesture_id, gesture));
                    return consumed(None);
                }

                // Hover: show what a press would change.
                let preview = if target.width == 1 {
                    self.paint_preview(c, &target, &cell, target.is_high(pos, c.row_offset))
                } else {
                    let segment = c.segment_at(&target.variable, &time).unwrap_or(cell);
                    Self::range_preview(c, &target, &segment)
                };
                WaveEditInput {
                    consumed: false,
                    preview: Some(preview),
                }
            }

            Some(EditGesture::Paint { target, last, high }) => {
                if !down {
                    ui.data_mut(|d| d.remove::<EditGesture>(gesture_id));
                    return consumed(None);
                }
                if let Some(pos) = pointer {
                    let cell = self.edit_cell_at(c, &target.variable, &c.time_at(pos.x));
                    let now_high = target.is_high(pos, c.row_offset);
                    if cell != last || now_high != high {
                        msgs.push(paint_message(
                            &target.variable,
                            cells_between(&last, &cell),
                            now_high,
                            true,
                        ));
                        ui.data_mut(|d| {
                            d.insert_temp(
                                gesture_id,
                                EditGesture::Paint {
                                    target,
                                    last: cell,
                                    high: now_high,
                                },
                            );
                        });
                    }
                }
                consumed(None)
            }

            Some(EditGesture::Range { target, start }) => {
                let current = pointer.map(|pos| {
                    let time = c.time_at(pos.x);
                    (self.edit_cell_at(c, &target.variable, &time), time)
                });
                if !down {
                    ui.data_mut(|d| d.remove::<EditGesture>(gesture_id));
                    if let Some((cell, time)) = current {
                        // Staying within one cell edits the clicked segment, as a click does.
                        let (range_start, range_end) = if cell == start {
                            c.segment_at(&target.variable, &time).unwrap_or(cell)
                        } else {
                            cells_union(&start, &cell)
                        };
                        msgs.push(Message::OpenUserSignalValueEditor {
                            variable: target.variable,
                            start: range_start,
                            end: range_end,
                            viewport_idx: c.viewport_idx,
                        });
                    }
                    return consumed(None);
                }
                let preview = current
                    .map(|(cell, _)| Self::range_preview(c, &target, &cells_union(&start, &cell)));
                consumed(preview)
            }
        }
    }

    /// Marks the stretches where edits make signals from the file differ from the file with
    /// moving diagonal stripes. Edits that match the file's values are not marked.
    pub(crate) fn draw_edited_spans(&self, ui: &Ui, c: &EditCanvas, painter: &egui::Painter) {
        let Some(container) = c.waves.inner.as_waves() else {
            return;
        };
        if container
            .signal_edits()
            .is_none_or(|(_, edits)| edits.is_empty())
        {
            return;
        }
        let to_u64 = |t: BigInt| u64::try_from(t).unwrap_or(0);
        let from = to_u64(c.time_at(0.));
        let to = to_u64(c.time_at(c.frame_width)).saturating_add(1);
        let phase = (ui.input(|i| i.time) as f32 * STRIPE_SPEED).rem_euclid(STRIPE_SPACING);
        let stroke = Stroke::new(2., EDIT_COLOR.gamma_multiply(0.45));
        let canvas_height = c.to_screen.to().height();

        let mut drawn = false;
        for info in c
            .waves
            .visible_drawing_infos(-c.row_offset, canvas_height - c.row_offset)
        {
            // Only the row of the whole signal, not rows of its translated fields.
            let ItemDrawingInfo::Variable(row) = info else {
                continue;
            };
            if !row.field_ref.field.is_empty() {
                continue;
            }
            for (start, end) in container.changed_spans(&row.field_ref.root, from, to) {
                let cell = (BigInt::from(start), Some(BigInt::from(end)));
                let mut rect = c.cell_rect(&cell, row.top, row.bottom);
                // Keep very short changes visible.
                if rect.width() < 2. {
                    rect = Rect::from_center_size(rect.center(), emath::vec2(2., rect.height()));
                }
                drawn = true;
                let clipped = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
                clipped.rect_filled(rect, 0., EDIT_COLOR.gamma_multiply(0.08));
                // Lines rising to the right, on a grid shared by all stretches so the pattern
                // lines up across them and moves smoothly.
                let height = rect.height();
                let first = rect.left() - height;
                let mut x = first - (first - phase).rem_euclid(STRIPE_SPACING);
                while x < rect.right() {
                    clipped.line_segment(
                        [
                            Pos2::new(x, rect.bottom()),
                            Pos2::new(x + height, rect.top()),
                        ],
                        stroke,
                    );
                    x += STRIPE_SPACING;
                }
            }
        }
        if drawn {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
    }

    /// Draws the value box of a pending multi-bit edit inside the signal's row.
    pub(crate) fn draw_user_signal_value_box(
        &self,
        ui: &Ui,
        c: &EditCanvas,
        msgs: &mut Vec<Message>,
    ) {
        let mut guard = self.pending_user_signal_value.borrow_mut();
        let Some(pending) = guard.as_mut() else {
            return;
        };
        if pending.viewport_idx != c.viewport_idx {
            return;
        }
        let close = |msgs: &mut Vec<Message>| {
            release_text_focus(&[SIGNAL_VALUE_ID], msgs);
            msgs.push(Message::CloseUserSignalValueEditor);
        };
        let row = c.waves.drawing_infos.iter().find_map(|info| {
            let node = c.waves.items_tree.get_visible(info.vidx())?;
            match c.waves.displayed_items.get(&node.item_ref)? {
                DisplayedItem::Variable(v) if v.variable_ref == pending.variable => {
                    Some((info.top(), info.bottom()))
                }
                _ => None,
            }
        });
        let Some((row_top, row_bottom)) = row else {
            // The signal is no longer shown.
            close(msgs);
            return;
        };

        // Highlight the range that will be set, like the preview before the box opened.
        ui.painter().rect_filled(
            c.cell_rect(
                &(pending.start.clone(), pending.end.clone()),
                row_top,
                row_bottom,
            ),
            0.,
            EDIT_COLOR.gamma_multiply(0.25),
        );

        let x = c
            .x_of(&pending.start)
            .clamp(0., (c.frame_width - 130.).max(0.))
            + 2.;
        let pos = c
            .to_screen
            .transform_pos(Pos2::new(x, row_top + c.row_offset));
        let radix_name = match pending.radix {
            16 => "hex",
            2 => "binary",
            8 => "octal",
            _ => "decimal",
        };
        let area = egui::Area::new(Id::new("user_signal_value_box").with(c.viewport_idx))
            .order(Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                let stroke = if pending.error.is_some() {
                    Stroke::new(1.5, ui.visuals().error_fg_color)
                } else {
                    ui.visuals().selection.stroke
                };
                egui::Frame::popup(ui.style())
                    .inner_margin(2.)
                    .stroke(stroke)
                    .show(ui, |ui| {
                        let text = ui.add(
                            egui::TextEdit::singleline(&mut pending.text)
                                .desired_width(110.)
                                .font(egui::TextStyle::Monospace)
                                .hint_text(radix_name),
                        );
                        if let Some(error) = &pending.error {
                            ui.colored_label(ui.visuals().error_fg_color, error);
                        }
                        text.on_hover_text(format!(
                            "Value in {radix_name}, or with a 0x, 0b, 0o or 0d prefix. \
                             Enter sets it, Escape cancels."
                        ))
                    })
                    .inner
            });
        let text = area.inner;
        track_text_focus(&text, SIGNAL_VALUE_ID, msgs);

        if !pending.focused {
            text.request_focus();
            pending.focused = true;
        } else if text.lost_focus() {
            if ui.input(|i| i.key_pressed(Key::Enter)) {
                release_text_focus(&[SIGNAL_VALUE_ID], msgs);
                msgs.push(Message::CommitUserSignalValue);
            } else {
                // Escape or clicking elsewhere.
                close(msgs);
            }
        }
    }

    /// Draws the "New signal" dialog, if open.
    pub(crate) fn draw_new_user_signal_dialog(&self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        let mut guard = self.new_user_signal_dialog.borrow_mut();
        let Some(dialog) = guard.as_mut() else {
            return;
        };
        let close = |msgs: &mut Vec<Message>| {
            release_text_focus(&[NEW_SIGNAL_NAME_ID, NEW_SIGNAL_WIDTH_ID], msgs);
            msgs.push(Message::SetNewUserSignalDialogVisible(false));
        };
        let mut open = true;
        egui::Window::new("New signal")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui, |ui| {
                let mut submit = false;
                egui::Grid::new("new_user_signal_grid")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("Name");
                        let name = ui.text_edit_singleline(&mut dialog.name);
                        if dialog.name.is_empty() && !name.has_focus() && dialog.error.is_none() {
                            name.request_focus();
                        }
                        track_text_focus(&name, NEW_SIGNAL_NAME_ID, msgs);
                        ui.end_row();

                        ui.label("Width (bits)");
                        let width = ui.text_edit_singleline(&mut dialog.width);
                        track_text_focus(&width, NEW_SIGNAL_WIDTH_ID, msgs);
                        submit = (name.lost_focus() || width.lost_focus())
                            && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.end_row();
                    });
                if let Some(error) = &dialog.error {
                    ui.colored_label(ui.visuals().error_fg_color, error);
                }
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() || submit {
                        match (
                            UserSignals::validate_name(dialog.name.trim()),
                            dialog.width.trim().parse::<u32>(),
                        ) {
                            (Err(e), _) => dialog.error = Some(format!("{e:#}")),
                            (_, Ok(width)) if width > 0 => {
                                msgs.push(Message::CreateUserSignal {
                                    name: dialog.name.trim().to_string(),
                                    width,
                                });
                                close(msgs);
                            }
                            _ => dialog.error = Some("Width must be at least 1 bit".to_string()),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close(msgs);
                    }
                });
            });
        if !open {
            close(msgs);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(start: i64, end: Option<i64>) -> Cell {
        (BigInt::from(start), end.map(BigInt::from))
    }

    #[test]
    fn cells_between_fills_gaps_in_both_directions() {
        assert_eq!(
            cells_between(&cell(0, Some(10)), &cell(30, Some(40))),
            cell(10, Some(40))
        );
        assert_eq!(
            cells_between(&cell(30, Some(40)), &cell(0, Some(10))),
            cell(0, Some(30))
        );
        assert_eq!(
            cells_between(&cell(10, Some(20)), &cell(10, Some(20))),
            cell(10, Some(20))
        );
        assert_eq!(
            cells_between(&cell(0, Some(10)), &cell(20, None)),
            cell(10, None)
        );
    }

    #[test]
    fn cells_union_orders_and_extends() {
        assert_eq!(
            cells_union(&cell(30, Some(40)), &cell(0, Some(10))),
            cell(0, Some(40))
        );
        assert_eq!(
            cells_union(&cell(0, Some(10)), &cell(20, None)),
            cell(0, None)
        );
    }
}
