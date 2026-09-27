//! Toolbar handling.
use egui::{Button, Color32, CursorIcon, Label, Layout, Panel, Rect, RichText, Sense, Stroke, Ui};
use egui_remixicon::icons;
use emath::{Align, Vec2};

use std::collections::HashSet;

use crate::message::MessageTarget;
use crate::mousegestures::AnnotationKind;
use crate::wave_container::SimulationStatus;
use crate::wave_source::LoadOptions;
use crate::{
    SystemState,
    file_dialog::OpenMode,
    message::Message,
    wave_data::{PER_SCROLL_EVENT, SCROLL_EVENTS_PER_PAGE},
};

pub(crate) const TOOLBAR_TIME_ID: &str = "toolbar-time";

#[derive(Clone, Copy)]
pub(crate) struct ToolbarGroupSpec {
    pub id: &'static str,
    pub label: &'static str,
}

const TOOLBAR_GROUP_SPECS: [ToolbarGroupSpec; 13] = [
    ToolbarGroupSpec {
        id: "menu",
        label: "Menu",
    },
    ToolbarGroupSpec {
        id: "files",
        label: "Files",
    },
    ToolbarGroupSpec {
        id: "copy",
        label: "Copy",
    },
    ToolbarGroupSpec {
        id: "zoom",
        label: "Zoom",
    },
    ToolbarGroupSpec {
        id: "navigation",
        label: "Navigation",
    },
    ToolbarGroupSpec {
        id: "transitions",
        label: "Transitions",
    },
    ToolbarGroupSpec {
        id: "add_items",
        label: "Add items",
    },
    ToolbarGroupSpec {
        id: "viewports",
        label: "Viewports",
    },
    ToolbarGroupSpec {
        id: "undo",
        label: "Undo/Redo",
    },
    ToolbarGroupSpec {
        id: "cxxrtl",
        label: "CXXRTL",
    },
    ToolbarGroupSpec {
        id: "time",
        label: "Time",
    },
    ToolbarGroupSpec {
        id: "annotations",
        label: "Annotations",
    },
    ToolbarGroupSpec {
        id: "edit",
        label: "Edit signals",
    },
];

#[derive(Clone)]
struct RenderedGroup {
    row: usize,
    visible_index: usize,
    rect: Rect,
}

pub(crate) fn toolbar_group_specs() -> &'static [ToolbarGroupSpec] {
    &TOOLBAR_GROUP_SPECS
}

/// Helper function to add a new toolbar button, setting up icon, hover text etc.
fn add_toolbar_button(
    ui: &mut Ui,
    msgs: &mut Vec<Message>,
    icon_string: &str,
    hover_text: &str,
    message: Message,
    enabled: bool,
) {
    let button = Button::new(RichText::new(icon_string).heading()).frame(false);
    if ui
        .add_enabled(enabled, button)
        .on_hover_text(hover_text)
        .clicked()
    {
        msgs.push(message);
    }
}

impl SystemState {
    /// Add panel and draw toolbar.
    pub(crate) fn add_toolbar_panel(&mut self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        Panel::top("toolbar").show(ui, |ui| {
            self.draw_toolbar(ui, msgs);
        });
    }

    pub(crate) fn toolbar_group_enabled(&self, id: &str) -> bool {
        self.user
            .toolbar_group_enabled
            .get(id)
            .copied()
            .flatten()
            .or_else(|| self.user.config.layout.toolbar_group_visibility(id))
            .unwrap_or(true)
    }

    fn default_toolbar_rows(&self) -> Vec<Vec<String>> {
        let mut rows: Vec<Vec<String>> = Vec::new();

        for spec in TOOLBAR_GROUP_SPECS {
            let row = usize::from(
                self.user
                    .config
                    .layout
                    .toolbar_group_row(spec.id)
                    .unwrap_or(0),
            );
            while rows.len() <= row {
                rows.push(Vec::new());
            }
            rows[row].push(spec.id.to_string());
        }

        if rows.is_empty() {
            rows.push(Vec::new());
        }

        rows
    }

    fn ensure_toolbar_rows(&mut self) {
        if self.user.toolbar_group_rows.is_empty() {
            self.user.toolbar_group_rows = self.default_toolbar_rows();
        }
    }

    fn visible_toolbar_groups(&self) -> HashSet<&'static str> {
        let mut visible = HashSet::from([
            "files",
            "copy",
            "zoom",
            "navigation",
            "transitions",
            "add_items",
            "viewports",
            "undo",
            "annotations",
        ]);
        if !self.show_menu() {
            visible.insert("menu");
        }
        if self
            .user
            .waves
            .as_ref()
            .and_then(|waves| waves.inner.simulation_status())
            .is_some()
        {
            visible.insert("cxxrtl");
        }
        if self.user.waves.is_some() {
            visible.insert("time");
        }
        if self
            .user
            .waves
            .as_ref()
            .and_then(|waves| waves.user_signals())
            .is_some()
        {
            visible.insert("edit");
        }
        visible
    }

    fn active_toolbar_rows(&mut self) -> Vec<Vec<String>> {
        self.ensure_toolbar_rows();

        let visible = self.visible_toolbar_groups();
        let mut rows = self.user.toolbar_group_rows.clone();
        let mut present: HashSet<String> = HashSet::new();

        for row in &mut rows {
            row.retain(|id| {
                let is_visible = visible.contains(id.as_str());
                let is_enabled = self.toolbar_group_enabled(id);
                let keep = is_visible && is_enabled;
                if keep {
                    present.insert(id.clone());
                }
                keep
            });
        }

        rows.retain(|row| !row.is_empty());

        if rows.is_empty() {
            rows.push(Vec::new());
        }

        for spec in TOOLBAR_GROUP_SPECS {
            let id = spec.id.to_string();
            if visible.contains(spec.id)
                && self.toolbar_group_enabled(spec.id)
                && !present.contains(&id)
            {
                rows[0].push(id);
            }
        }

        rows
    }

    fn simulate_group_drop(
        rows: &[Vec<String>],
        dragged: &str,
        target_row: usize,
        target_visible_index: usize,
        new_row: bool,
    ) -> Vec<Vec<String>> {
        let mut next = rows.to_vec();
        for row in &mut next {
            row.retain(|id| id != dragged);
        }
        next.retain(|row| !row.is_empty());

        let insert_row = if new_row {
            target_row.min(next.len())
        } else {
            target_row.min(next.len().saturating_sub(1))
        };

        if new_row || next.is_empty() {
            next.insert(insert_row, vec![dragged.to_string()]);
        } else {
            let row = &mut next[insert_row];
            let index = target_visible_index.min(row.len());
            row.insert(index, dragged.to_string());
        }

        next
    }

    pub(crate) fn set_toolbar_group_row(&mut self, group_id: &str, row: u8) {
        if !TOOLBAR_GROUP_SPECS.iter().any(|spec| spec.id == group_id) {
            tracing::warn!(
                "Unknown toolbar group id '{group_id}' provided to SetToolbarGroupRow (row {row})"
            );
            return;
        }

        self.ensure_toolbar_rows();

        for existing_row in &mut self.user.toolbar_group_rows {
            existing_row.retain(|id| id != group_id);
        }
        self.user
            .toolbar_group_rows
            .retain(|existing_row| !existing_row.is_empty());

        let target_row = usize::from(row);
        while self.user.toolbar_group_rows.len() <= target_row {
            self.user.toolbar_group_rows.push(Vec::new());
        }
        self.user.toolbar_group_rows[target_row].push(group_id.to_string());
    }

    fn draw_group_container(
        &mut self,
        ui: &mut Ui,
        group_id: &str,
        msgs: &mut Vec<Message>,
        wave_loaded: bool,
        item_and_cursor: bool,
    ) -> Rect {
        let stroke = Stroke::new(0.5, self.user.config.theme.border_color);
        let inner = egui::Frame::default().stroke(stroke).show(ui, |ui| {
            ui.horizontal(|ui| {
                let handle = ui
                    .add(Label::new(RichText::new("||").monospace().small()).sense(Sense::drag()));
                let is_dragging_handle = handle.dragged() || handle.drag_started();
                if is_dragging_handle {
                    ui.output_mut(|o| o.cursor_icon = CursorIcon::Grabbing);
                } else if handle.hovered() {
                    ui.output_mut(|o| o.cursor_icon = CursorIcon::Grab);
                }
                if handle.drag_started() {
                    self.toolbar_dragging_group = Some(group_id.to_string());
                }

                match group_id {
                    "menu" => self.draw_toolbar_group_menu(ui, msgs),
                    "files" => self.draw_toolbar_group_files(ui, msgs, wave_loaded),
                    "copy" => self.draw_toolbar_group_copy(ui, msgs, item_and_cursor),
                    "zoom" => self.draw_toolbar_group_zoom(ui, msgs, wave_loaded),
                    "navigation" => self.draw_toolbar_group_navigation(ui, msgs, wave_loaded),
                    "transitions" => self.draw_toolbar_group_transitions(ui, msgs, item_and_cursor),
                    "add_items" => self.draw_toolbar_group_add_items(ui, msgs, wave_loaded),
                    "viewports" => self.draw_toolbar_group_viewports(ui, msgs, wave_loaded),
                    "undo" => self.draw_toolbar_group_history(ui, msgs),
                    "cxxrtl" => self.draw_toolbar_group_simulation(ui, msgs),
                    "time" => self.draw_toolbar_group_time_input(ui, msgs),
                    "annotations" => self.draw_annotation_group(ui, msgs, wave_loaded),
                    "edit" => self.draw_toolbar_group_edit(ui, msgs),
                    _ => {
                        unreachable!("Unknown toolbar group id {group_id:?}")
                    }
                }
            });
        });

        inner.response.rect
    }

    fn draw_toolbar_group_menu(&self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        if !self.show_menu() {
            ui.menu_button(RichText::new(icons::MENU_FILL).heading(), |ui| {
                self.menu_contents(ui, msgs);
            });
        }
    }

    fn draw_toolbar_group_files(&self, ui: &mut Ui, msgs: &mut Vec<Message>, wave_loaded: bool) {
        add_toolbar_button(
            ui,
            msgs,
            icons::FOLDER_OPEN_FILL,
            "Open file...",
            Message::OpenFileDialog(OpenMode::Open),
            true,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::DOWNLOAD_CLOUD_FILL,
            "Open URL...",
            Message::SetUrlEntryVisible(
                true,
                Some(Box::new(|url: String| {
                    Message::LoadWaveformFileFromUrl(url.clone(), LoadOptions::Clear)
                })),
            ),
            true,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::REFRESH_LINE,
            "Reload",
            Message::ReloadWaveform(self.user.config.behavior.keep_during_reload),
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::RUN_LINE,
            "Run command file...",
            Message::OpenCommandFileDialog,
            true,
        );
        if self.user.surver_url.is_some() {
            add_toolbar_button(
                ui,
                msgs,
                icons::FILE_LIST_FILL,
                "Select Surver file",
                Message::SetSurverFileWindowVisible(true),
                true,
            );
        }
    }

    fn draw_toolbar_group_copy(&self, ui: &mut Ui, msgs: &mut Vec<Message>, enabled: bool) {
        add_toolbar_button(
            ui,
            msgs,
            icons::FILE_COPY_FILL,
            "Copy variable value",
            Message::VariableValueToClipbord(MessageTarget::CurrentSelection),
            enabled,
        );
    }

    fn draw_toolbar_group_zoom(&self, ui: &mut Ui, msgs: &mut Vec<Message>, wave_loaded: bool) {
        let viewport_idx = self
            .user
            .waves
            .as_ref()
            .map_or(0, |waves| waves.last_active_viewport_idx);
        let cursor_set = self
            .user
            .waves
            .as_ref()
            .is_some_and(|waves| waves.cursor.is_some());
        add_toolbar_button(
            ui,
            msgs,
            icons::ZOOM_IN_FILL,
            "Zoom in",
            Message::CanvasZoom {
                mouse_ptr: None,
                delta: 0.5,
                viewport_idx,
            },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::ZOOM_OUT_FILL,
            "Zoom out",
            Message::CanvasZoom {
                mouse_ptr: None,
                delta: 2.0,
                viewport_idx,
            },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::TARGET_FILL,
            "Zoom in on cursor",
            Message::ZoomToCursor {
                delta: 0.5,
                viewport_idx,
            },
            wave_loaded && cursor_set,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::ASPECT_RATIO_FILL,
            "Zoom to fit",
            Message::ZoomToFit { viewport_idx },
            wave_loaded,
        );
    }

    fn draw_toolbar_group_navigation(
        &self,
        ui: &mut Ui,
        msgs: &mut Vec<Message>,
        wave_loaded: bool,
    ) {
        let viewport_idx = self
            .user
            .waves
            .as_ref()
            .map_or(0, |waves| waves.last_active_viewport_idx);
        add_toolbar_button(
            ui,
            msgs,
            icons::REWIND_START_FILL,
            "Go to start",
            Message::GoToStart { viewport_idx },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::REWIND_FILL,
            "Go one page left",
            Message::CanvasScroll {
                delta: Vec2 {
                    y: PER_SCROLL_EVENT * SCROLL_EVENTS_PER_PAGE,
                    x: 0.,
                },
                viewport_idx,
            },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::PLAY_REVERSE_FILL,
            "Go left",
            Message::CanvasScroll {
                delta: Vec2 {
                    y: PER_SCROLL_EVENT,
                    x: 0.,
                },
                viewport_idx,
            },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::PLAY_FILL,
            "Go right",
            Message::CanvasScroll {
                delta: Vec2 {
                    y: -PER_SCROLL_EVENT,
                    x: 0.,
                },
                viewport_idx,
            },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::SPEED_FILL,
            "Go one page right",
            Message::CanvasScroll {
                delta: Vec2 {
                    y: -PER_SCROLL_EVENT * SCROLL_EVENTS_PER_PAGE,
                    x: 0.,
                },
                viewport_idx,
            },
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::FORWARD_END_FILL,
            "Go to end",
            Message::GoToEnd { viewport_idx },
            wave_loaded,
        );
    }

    fn draw_toolbar_group_transitions(&self, ui: &mut Ui, msgs: &mut Vec<Message>, enabled: bool) {
        add_toolbar_button(
            ui,
            msgs,
            icons::CONTRACT_LEFT_FILL,
            "Set cursor on previous transition of focused variable",
            Message::MoveCursorToTransition {
                next: false,
                variable: None,
                skip_zero: false,
            },
            enabled,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::CONTRACT_RIGHT_FILL,
            "Set cursor on next transition of focused variable",
            Message::MoveCursorToTransition {
                next: true,
                variable: None,
                skip_zero: false,
            },
            enabled,
        );
    }

    fn draw_toolbar_group_add_items(
        &self,
        ui: &mut Ui,
        msgs: &mut Vec<Message>,
        wave_loaded: bool,
    ) {
        add_toolbar_button(
            ui,
            msgs,
            icons::SPACE,
            "Add divider",
            Message::AddDivider(None, None),
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::TIME_FILL,
            "Add timeline",
            Message::AddTimeLine(None),
            wave_loaded,
        );
    }

    fn draw_toolbar_group_viewports(
        &self,
        ui: &mut Ui,
        msgs: &mut Vec<Message>,
        wave_loaded: bool,
    ) {
        let multiple_viewports = self
            .user
            .waves
            .as_ref()
            .is_some_and(|waves| waves.viewports.len() > 1);

        add_toolbar_button(
            ui,
            msgs,
            icons::ADD_BOX_FILL,
            "Add viewport",
            Message::AddViewport,
            wave_loaded,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::CHECKBOX_INDETERMINATE_FILL,
            "Remove viewport",
            Message::RemoveViewport,
            wave_loaded && multiple_viewports,
        );
    }

    fn draw_toolbar_group_history(&self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        let undo_available = !self.undo_stack.is_empty();
        let redo_available = !self.redo_stack.is_empty();

        let undo_tooltip = if let Some(undo_op) = self.undo_stack.last() {
            format!("Undo: {}", undo_op.message)
        } else {
            "Undo".into()
        };
        let redo_tooltip = if let Some(redo_op) = self.redo_stack.last() {
            format!("Redo: {}", redo_op.message)
        } else {
            "Redo".into()
        };
        add_toolbar_button(
            ui,
            msgs,
            icons::ARROW_GO_BACK_FILL,
            &undo_tooltip,
            Message::Undo(1),
            undo_available,
        );
        add_toolbar_button(
            ui,
            msgs,
            icons::ARROW_GO_FORWARD_FILL,
            &redo_tooltip,
            Message::Redo(1),
            redo_available,
        );
    }

    fn draw_toolbar_group_simulation(&self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        let Some(waves) = &self.user.waves else {
            return;
        };
        let Some(status) = waves.inner.simulation_status() else {
            return;
        };

        ui.label("Simulation");
        match status {
            SimulationStatus::Paused => add_toolbar_button(
                ui,
                msgs,
                icons::PLAY_CIRCLE_FILL,
                "Run simulation",
                Message::UnpauseSimulation,
                true,
            ),
            SimulationStatus::Running => add_toolbar_button(
                ui,
                msgs,
                icons::PAUSE_CIRCLE_FILL,
                "Pause simulation",
                Message::PauseSimulation,
                true,
            ),
            SimulationStatus::Finished => {
                ui.label("Finished");
            }
        }
    }

    fn draw_toolbar_group_time_input(&self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        if let Some(waves) = &self.user.waves {
            self.time_input_widget(ui, TOOLBAR_TIME_ID, waves, msgs);
        }
    }

    /// Helper function to help the annotation buttons know what icon to display and message to send.
    fn annotation_helper<'a>(
        &self,
        hover_text: &'a str,
        icon_unselected: &'a str,
        icon_selected: &'a str,
        annotation_kind: AnnotationKind,
    ) -> (&'a str, Option<AnnotationKind>, &'a str) {
        if self.annotation_kind == Some(annotation_kind) {
            (icon_selected, None, "Cancel Action")
        } else {
            (icon_unselected, Some(annotation_kind), hover_text)
        }
    }

    fn draw_toolbar_group_edit(&self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        add_toolbar_button(
            ui,
            msgs,
            icons::ADD_BOX_LINE,
            "New signal",
            Message::SetNewUserSignalDialogVisible(true),
            true,
        );
        let (icon, text) = if self.wave_edit_mode {
            (icons::EDIT_2_FILL, "Stop editing signals")
        } else {
            (
                icons::EDIT_2_LINE,
                "Edit signals: drag on a signal to set its value",
            )
        };
        add_toolbar_button(
            ui,
            msgs,
            icon,
            text,
            Message::SetWaveEditMode(!self.wave_edit_mode),
            true,
        );
    }

    fn draw_annotation_group(&mut self, ui: &mut Ui, msgs: &mut Vec<Message>, wave_loaded: bool) {
        // Implementation for drawing the annotation group
        let (rect_icon, rect_kind, rect_text) = self.annotation_helper(
            "Add Rectangle",
            icons::EDIT_BOX_LINE,
            icons::EDIT_BOX_FILL,
            AnnotationKind::Rectangle,
        );
        add_toolbar_button(
            ui,
            msgs,
            rect_icon,
            rect_text,
            Message::SetMouseGestureAnnotation(rect_kind),
            wave_loaded,
        );
        let (arrow_icon, arrow_kind, arrow_text) = self.annotation_helper(
            "Add Arrow",
            icons::ARROW_RIGHT_UP_BOX_LINE,
            icons::ARROW_RIGHT_UP_BOX_FILL,
            AnnotationKind::ArrowSingleHead,
        );
        add_toolbar_button(
            ui,
            msgs,
            arrow_icon,
            arrow_text,
            Message::SetMouseGestureAnnotation(arrow_kind),
            wave_loaded,
        );

        let (double_arrow_icon, double_arrow_kind, double_arrow_text) = self.annotation_helper(
            "Add Double Headed Arrow",
            icons::ARROW_LEFT_RIGHT_FILL,
            icons::CLOSE_LARGE_LINE,
            AnnotationKind::ArrowDoubleHead,
        );
        add_toolbar_button(
            ui,
            msgs,
            double_arrow_icon,
            double_arrow_text,
            Message::SetMouseGestureAnnotation(double_arrow_kind),
            wave_loaded,
        );

        add_toolbar_button(
            ui,
            msgs,
            icons::LIST_CHECK,
            "Annotations list",
            Message::ToggleAnnotationlistVisibility(),
            wave_loaded,
        );
    }

    fn compute_drop_target(
        pointer: egui::Pos2,
        rows: &[Vec<String>],
        row_rects: &[Rect],
        rendered: &[RenderedGroup],
    ) -> Option<(usize, usize, bool)> {
        for group in rendered {
            if group.rect.contains(pointer) {
                let idx = if pointer.x > group.rect.center().x {
                    group.visible_index + 1
                } else {
                    group.visible_index
                };
                return Some((group.row, idx, false));
            }
        }

        // Explicitly detect drop targets in horizontal gaps between groups.
        for (row_idx, row_rect) in row_rects.iter().enumerate() {
            if !row_rect.contains(pointer) {
                continue;
            }

            let mut row_groups: Vec<&RenderedGroup> =
                rendered.iter().filter(|g| g.row == row_idx).collect();
            row_groups.sort_by_key(|g| g.visible_index);
            if row_groups.is_empty() {
                continue;
            }

            if pointer.x <= row_groups[0].rect.left() {
                return Some((row_idx, 0, false));
            }

            for pair in row_groups.windows(2) {
                let left = pair[0];
                let right = pair[1];
                if pointer.x >= left.rect.right() && pointer.x <= right.rect.left() {
                    return Some((row_idx, right.visible_index, false));
                }
            }

            if pointer.x >= row_groups[row_groups.len() - 1].rect.right() {
                return Some((row_idx, row_groups.len(), false));
            }
        }

        if row_rects.is_empty() {
            return Some((0, 0, true));
        }
        if pointer.y < row_rects[0].top() {
            return Some((0, 0, true));
        }
        if pointer.y > row_rects[row_rects.len() - 1].bottom() {
            return Some((rows.len(), 0, true));
        }
        for (i, pair) in row_rects.windows(2).enumerate() {
            if pointer.y > pair[0].bottom() && pointer.y < pair[1].top() {
                return Some((i + 1, 0, true));
            }
        }

        let nearest_row = row_rects
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (a.center().y - pointer.y)
                    .abs()
                    .total_cmp(&(b.center().y - pointer.y).abs())
            })
            .map_or(0, |(idx, _)| idx);

        let mut centers: Vec<f32> = rendered
            .iter()
            .filter(|g| g.row == nearest_row)
            .map(|g| g.rect.center().x)
            .collect();
        centers.sort_by(f32::total_cmp);
        let visible_index = centers.iter().take_while(|x| pointer.x > **x).count();

        Some((nearest_row, visible_index, false))
    }

    fn apply_toolbar_drop(&mut self, rows: &[Vec<String>]) {
        let Some(dragged) = self.toolbar_dragging_group.clone() else {
            return;
        };
        let Some(target_row) = self.toolbar_drop_row else {
            return;
        };
        let Some(target_index) = self.toolbar_drop_index else {
            return;
        };

        self.user.toolbar_group_rows = Self::simulate_group_drop(
            rows,
            &dragged,
            target_row,
            target_index,
            self.toolbar_drop_new_row,
        );
    }

    fn clear_toolbar_drag_state(&mut self) {
        self.toolbar_dragging_group = None;
        self.toolbar_drop_row = None;
        self.toolbar_drop_index = None;
        self.toolbar_drop_new_row = false;
    }

    fn draw_drop_indicator(
        &self,
        ui: &Ui,
        row_rects: &[Rect],
        rendered: &[RenderedGroup],
        row: usize,
        visible_index: usize,
        new_row: bool,
    ) {
        let indicator_stroke = Stroke::new(3.0, Color32::from_white_alpha(210));

        if new_row {
            let y = if row == 0 {
                row_rects.first().map_or(0.0, Rect::top)
            } else if row >= row_rects.len() {
                row_rects.last().map_or(0.0, Rect::bottom)
            } else {
                row_rects[row - 1].bottom().midpoint(row_rects[row].top())
            };

            let x_min = row_rects
                .iter()
                .map(Rect::left)
                .min_by(f32::total_cmp)
                .unwrap_or(0.0);
            let x_max = row_rects
                .iter()
                .map(Rect::right)
                .max_by(f32::total_cmp)
                .unwrap_or(x_min + 40.0);

            ui.painter().line_segment(
                [egui::pos2(x_min, y), egui::pos2(x_max, y)],
                indicator_stroke,
            );
            return;
        }

        let Some(row_rect) = row_rects.get(row) else {
            return;
        };

        let mut row_groups: Vec<&RenderedGroup> =
            rendered.iter().filter(|g| g.row == row).collect();
        row_groups.sort_by_key(|g| g.visible_index);
        if row_groups.is_empty() {
            return;
        }

        let x = if visible_index == 0 {
            row_groups[0].rect.left()
        } else if visible_index >= row_groups.len() {
            row_groups[row_groups.len() - 1].rect.right()
        } else {
            f32::midpoint(
                row_groups[visible_index - 1].rect.right(),
                row_groups[visible_index].rect.left(),
            )
        };

        ui.painter().line_segment(
            [
                egui::pos2(x, row_rect.top()),
                egui::pos2(x, row_rect.bottom()),
            ],
            indicator_stroke,
        );
    }

    fn draw_toolbar(&mut self, ui: &mut Ui, msgs: &mut Vec<Message>) {
        let wave_loaded = self.user.waves.is_some();

        let (item_selected, cursor_set) = if let Some(waves) = &self.user.waves {
            (waves.focused_item.is_some(), waves.cursor.is_some())
        } else {
            (false, false)
        };
        let item_and_cursor = item_selected && cursor_set;

        let active_rows = self.active_toolbar_rows();
        let mut rendered_groups = Vec::<RenderedGroup>::new();
        let mut row_rects = Vec::<Rect>::new();

        ui.with_layout(Layout::top_down(Align::Min), |ui| {
            for (row_idx, row) in active_rows.iter().enumerate() {
                let response = ui.horizontal(|ui| {
                    for (visible_idx, group_id) in row.iter().enumerate() {
                        let rect = self.draw_group_container(
                            ui,
                            group_id,
                            msgs,
                            wave_loaded,
                            item_and_cursor,
                        );
                        rendered_groups.push(RenderedGroup {
                            row: row_idx,
                            visible_index: visible_idx,
                            rect,
                        });
                    }
                });
                row_rects.push(response.response.rect);
            }
        });

        let dragging_group_visible = self
            .toolbar_dragging_group
            .as_deref()
            .is_some_and(|dragging_id| active_rows.iter().flatten().any(|id| id == dragging_id));

        if dragging_group_visible {
            if let Some(pointer) = ui.ctx().pointer_hover_pos()
                && let Some((row, idx, new_row)) =
                    Self::compute_drop_target(pointer, &active_rows, &row_rects, &rendered_groups)
            {
                self.toolbar_drop_row = Some(row);
                self.toolbar_drop_index = Some(idx);
                self.toolbar_drop_new_row = new_row;

                self.draw_drop_indicator(ui, &row_rects, &rendered_groups, row, idx, new_row);
            }

            if ui.input(|i| i.pointer.any_released()) {
                self.apply_toolbar_drop(&active_rows);
                self.clear_toolbar_drag_state();
            }
        }
    }
}
