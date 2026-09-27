//! Time flags: markers that the page embedding Surfer places at specific times, such as
//! failed assertions. Each flag is a yellow line with a small "!" badge to its right.
//! Clicking the badge selects the flag, moves the cursor to it and notifies the host
//! page, which can then show whatever the flag stands for.
//!
//! Hosts set them with `Message::SetTimeFlags` and highlight one with
//! `Message::SelectTimeFlag`; a click is reported as
//! `{"command": "TimeFlagClicked", "id": <id>}` through `host::notify_host`.

use ecolor::Color32;
use egui::CursorIcon;
use emath::{Align2, Pos2, Rect, Vec2};
use epaint::{CornerRadius, FontId, Stroke, StrokeKind};
use num::BigInt;
use serde::{Deserialize, Serialize};

use crate::{
    SystemState, drawing_canvas::draw_vertical_line_at_time, view::DrawingContext,
    wave_data::WaveData,
};

const FLAG_COLOR: Color32 = Color32::from_rgb(0xf2, 0xc0, 0x1e);
const BADGE_TEXT_COLOR: Color32 = Color32::from_rgb(0x2b, 0x22, 0x00);
const BADGE_SIZE: f32 = 14.;
/// Space between the flag's line and its badge, and between the badge and the top of
/// the wave area.
const BADGE_GAP: f32 = 2.;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeFlag {
    /// Chosen by the host and sent back when the flag is clicked.
    pub id: u32,
    pub time: BigInt,
    /// Shown when hovering the badge.
    #[serde(default)]
    pub label: String,
}

impl SystemState {
    /// Canvas-space badge rectangles of the flags within `frame_width`, in drawing order.
    fn time_flag_badges(
        &self,
        waves: &WaveData,
        viewport_idx: usize,
        frame_width: f32,
        top: f32,
    ) -> Vec<(&TimeFlag, Rect)> {
        let range = waves.time_range();
        let viewport = &waves.viewports[viewport_idx];
        self.time_flags
            .iter()
            .filter_map(|flag| {
                let x = viewport.pixel_from_time(&flag.time, frame_width, range);
                let rect = Rect::from_min_size(
                    Pos2::new(x + BADGE_GAP, top + BADGE_GAP),
                    Vec2::splat(BADGE_SIZE),
                );
                (rect.max.x >= 0. && rect.min.x <= frame_width).then_some((flag, rect))
            })
            .collect()
    }

    /// The flag whose badge is under `pos` (canvas coordinates). Badges drawn later are on
    /// top, so they win.
    pub(crate) fn time_flag_at(
        &self,
        waves: &WaveData,
        viewport_idx: usize,
        frame_width: f32,
        top: f32,
        pos: Pos2,
    ) -> Option<&TimeFlag> {
        self.time_flag_badges(waves, viewport_idx, frame_width, top)
            .into_iter()
            .rev()
            .find(|(_, rect)| rect.contains(pos))
            .map(|(flag, _)| flag)
    }

    pub(crate) fn draw_time_flag_lines(
        &self,
        waves: &WaveData,
        ctx: &mut DrawingContext,
        viewport_idx: usize,
    ) {
        let range = waves.time_range();
        let width = self.user.config.theme.cursor.width;
        for flag in &self.time_flags {
            let selected = self.selected_time_flag == Some(flag.id);
            let stroke = Stroke {
                color: FLAG_COLOR,
                width: if selected { width * 2. } else { width },
            };
            draw_vertical_line_at_time(
                &flag.time,
                ctx,
                stroke,
                &waves.viewports[viewport_idx],
                range,
            );
        }
    }

    /// Draws the badges below the timeline at `top`. Call after the timeline so the badges
    /// stay on top of it.
    pub(crate) fn draw_time_flag_badges(
        &self,
        waves: &WaveData,
        ctx: &mut DrawingContext,
        viewport_idx: usize,
        top: f32,
    ) {
        let frame_width = ctx.cfg.canvas_size.x;
        for (flag, rect) in self.time_flag_badges(waves, viewport_idx, frame_width, top) {
            let screen_rect = Rect::from_min_max(
                (ctx.to_screen)(rect.min.x, rect.min.y),
                (ctx.to_screen)(rect.max.x, rect.max.y),
            );
            ctx.painter
                .rect_filled(screen_rect, CornerRadius::same(3), FLAG_COLOR);
            if self.selected_time_flag == Some(flag.id) {
                ctx.painter.rect_stroke(
                    screen_rect.expand(1.5),
                    CornerRadius::same(4),
                    Stroke::new(1.5, self.user.config.theme.foreground),
                    StrokeKind::Outside,
                );
            }
            ctx.painter.text(
                screen_rect.center(),
                Align2::CENTER_CENTER,
                "!",
                FontId::proportional(BADGE_SIZE - 3.),
                BADGE_TEXT_COLOR,
            );
        }
    }

    /// Hover feedback for a badge under the pointer: a hand cursor and the flag's label.
    pub(crate) fn time_flag_hover(
        &self,
        ui: &egui::Ui,
        response: &egui::Response,
        waves: &WaveData,
        viewport_idx: usize,
        frame_width: f32,
        top: f32,
        hover_pos: Option<Pos2>,
    ) {
        let Some(flag) =
            hover_pos.and_then(|pos| self.time_flag_at(waves, viewport_idx, frame_width, top, pos))
        else {
            return;
        };
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
        if !flag.label.is_empty() {
            response.clone().on_hover_text_at_pointer(&flag.label);
        }
    }
}
