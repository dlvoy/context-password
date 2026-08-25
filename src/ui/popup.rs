//! Rendering for each popup content state (plan §9's `Prompting`,
//! `Unlocking`, `ShowingList`). Input handling and state transitions live
//! in `App::logic`; these functions only draw, and `showing_list` reports
//! back which row (if any) was clicked so `App::logic` can act on it.

use eframe::egui;
use egui::emath::GuiRounding;

use crate::bw::model::Entry;

const WARNING_COLOR: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x88, 0x00);
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x33, 0x33);
const BADGE_COLOR: egui::Color32 = egui::Color32::from_rgb(0x2e, 0x6b, 0xd6);
const BADGE_TEXT_COLOR: egui::Color32 = egui::Color32::WHITE;
const TITLE_SIZE: f32 = 13.0;
const HINT_SIZE: f32 = 11.0;
const ITEM_FONT_SIZE: f32 = 16.0;
/// The height of one row in `showing_list`, in points — also used by
/// `app.rs` to size the popup so exactly `max_visible_items` rows fit with
/// no dead space and no overflow.
pub const ITEM_ROW_HEIGHT: f32 = 34.0;
const ITEM_TEXT_INSET: f32 = 34.0;
const ICON_SIZE: f32 = 16.0;
const ICON_LEFT_INSET: f32 = 10.0;
const BADGE_SIZE: f32 = 18.0;
const BADGE_RIGHT_INSET: f32 = 10.0;

/// Which field Enter/a digit would deliver right now, driven by which
/// modifier is held (plain / Shift / Alt) — the render counterpart of
/// `app::DeliveryKind`, computed once per frame in `App::ui()` from
/// `ui.input(|i| i.modifiers)` and passed down, matching how `elevated`
/// already reaches this module as a plain argument rather than being read
/// here directly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    Password,
    Username,
    Otp,
}

/// Common chrome for every content state: a small title, a bottom-pinned
/// hint line (via `TopBottomPanel`, so it stays put regardless of how much
/// the body above it grows), and the body in between.
fn frame<R>(ui: &mut egui::Ui, hint: &str, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // `Panel`/`CentralPanel` replaced the old `TopBottomPanel`/`SidePanel`
    // split in this egui version — `Panel::bottom` is the equivalent.
    egui::Panel::bottom("popup_footer")
        .show_separator_line(true)
        .show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(hint).weak().size(HINT_SIZE));
            ui.add_space(4.0);
        });
    egui::CentralPanel::default()
        .show(ui, |ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("context-password").size(TITLE_SIZE).strong());
            ui.separator();
            ui.add_space(6.0);
            body(ui)
        })
        .inner
}

pub fn prompting(ui: &mut egui::Ui, char_count: usize, error: Option<&str>) {
    frame(ui, "Enter to unlock  ·  Esc to dismiss", |ui| {
        ui.label("Master password:");
        super::password_field::draw(ui, char_count);
        if let Some(err) = error {
            ui.colored_label(ERROR_COLOR, err);
        }
    });
}

/// A spinner plus a message — used for every "waiting on a background `bw`
/// call" state: unlocking, locking, fetching a one-time code.
pub fn busy(ui: &mut egui::Ui, message: &str) {
    frame(ui, "Please wait…", |ui| {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.add_space(6.0);
            ui.label(message);
        });
    });
}

/// The bits of `showing_list` that aren't the list itself — grouped into
/// one struct to keep the function's argument count sane.
pub struct ListInfo<'a> {
    /// Items dropped by the server-side filter (wrong type, no matching
    /// uri, no password) — see `bw::filter::build_entries`.
    pub dropped: usize,
    /// Items beyond `max_visible_items` that were truncated out of
    /// `entries` entirely, not just visually — see `App::ui`.
    pub hidden_by_cap: usize,
    /// An inline reason the last delivery attempt couldn't proceed (no
    /// username, no TOTP configured).
    pub message: Option<&'a str>,
    pub target_elevated: bool,
    pub icon_mode: IconMode,
}

/// Returns the index of a clicked row, if any. `entries` is already
/// truncated to the visible cap by the caller (`App::ui`) — this module
/// only ever renders what it's given, matching its existing pure-function
/// style (see `ListInfo`, likewise computed and passed in rather than read
/// here).
pub fn showing_list(
    ui: &mut egui::Ui,
    entries: &[Entry],
    selected: usize,
    info: ListInfo<'_>,
) -> Option<usize> {
    let hint = match info.icon_mode {
        IconMode::Password => "Up/Down · Enter types password · Esc dismisses",
        IconMode::Username => "Up/Down · Enter types username · Esc dismisses",
        IconMode::Otp => "Up/Down · Enter types one-time code · Esc dismisses",
    };
    frame(ui, hint, |ui| {
        let mut clicked = None;
        if entries.is_empty() {
            ui.label("No items tagged for this app yet.");
        } else {
            for (i, entry) in entries.iter().enumerate() {
                let label = match &entry.username {
                    Some(u) => format!("{} ({u})", entry.name),
                    None => entry.name.clone(),
                };
                let badge = badge_for(i);
                if list_row(ui, i == selected, &label, info.icon_mode, badge).clicked() {
                    clicked = Some(i);
                }
            }
        }
        if info.dropped > 0 {
            ui.label(format!("({} item(s) hidden — see log)", info.dropped));
        }
        if info.hidden_by_cap > 0 {
            ui.label(format!(
                "({} more not shown — raise Max visible items in Settings)",
                info.hidden_by_cap
            ));
        }
        if info.target_elevated {
            ui.colored_label(
                WARNING_COLOR,
                "Target runs elevated — typing would be blocked. Enter will abort.",
            );
        }
        if let Some(msg) = info.message {
            ui.colored_label(ERROR_COLOR, msg);
        }
        clicked
    })
}

/// The quick-select digit for row `i`: `'1'..'9'` then `'0'` for the 10th —
/// `None` beyond that (unreachable in practice since `max_visible_items` is
/// clamped to at most 10, but the row-painting code has no reason to assume
/// that from the outside).
fn badge_for(i: usize) -> Option<char> {
    match i {
        0..=8 => char::from_digit(i as u32 + 1, 10),
        9 => Some('0'),
        _ => None,
    }
}

/// A full-width, click-and-hover-highlighted list row, painted directly
/// (rather than via `Button`/`selectable_label`) so it can be sized to fill
/// the popup and carry its own left icon/text inset and right-aligned badge
/// — a stock `SelectableLabel` sizes to its text and centers it, none of
/// which suits a list row. Colors come from `Style::interact_selectable`,
/// so this stays correctly themed in both light and dark mode without any
/// hardcoded colors here (the badge is the one deliberate exception, per
/// the request for a specifically blue label).
fn list_row(
    ui: &mut egui::Ui,
    selected: bool,
    text: &str,
    icon_mode: IconMode,
    badge: Option<char>,
) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), ITEM_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }

        paint_icon(ui.painter(), rect, icon_mode, visuals.text_color());

        // A hand-picked position like this isn't pixel-aligned the way
        // normal widget text (routed through egui's own layout code) is —
        // left as a raw fractional coordinate it rasterizes noticeably
        // softer than everything else in the popup.
        let text_pos = (rect.left_center() + egui::vec2(ITEM_TEXT_INSET, 0.0))
            .round_to_pixels(ui.pixels_per_point());
        ui.painter().text(
            text_pos,
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(ITEM_FONT_SIZE),
            visuals.text_color(),
        );

        if let Some(digit) = badge {
            paint_badge(ui.painter(), rect, digit);
        }
    }

    response
}

/// A small hand-painted glyph — key (password), person (username), or
/// clock (one-time code) — rather than an image asset or emoji font glyph,
/// consistent with the app's no-asset-pipeline style (`tray.rs`'s
/// `placeholder_icon`) and immune to inconsistent emoji glyph coverage
/// across systems.
fn paint_icon(painter: &egui::Painter, row_rect: egui::Rect, mode: IconMode, color: egui::Color32) {
    let center = row_rect.left_center() + egui::vec2(ICON_LEFT_INSET + ICON_SIZE / 2.0, 0.0);
    let stroke = egui::Stroke::new(1.5, color);
    match mode {
        IconMode::Password => {
            // A key: a ring for the bow, a shaft, two teeth.
            let bow_r = ICON_SIZE * 0.28;
            let bow_center = center + egui::vec2(-ICON_SIZE * 0.22, 0.0);
            painter.circle_stroke(bow_center, bow_r, stroke);
            let shaft_start = bow_center + egui::vec2(bow_r, 0.0);
            let shaft_end = center + egui::vec2(ICON_SIZE * 0.42, 0.0);
            painter.line_segment([shaft_start, shaft_end], stroke);
            for dx in [-0.14, -0.02] {
                let tooth_x = shaft_end.x + ICON_SIZE * dx;
                painter.line_segment(
                    [
                        egui::pos2(tooth_x, shaft_end.y),
                        egui::pos2(tooth_x, shaft_end.y + ICON_SIZE * 0.22),
                    ],
                    stroke,
                );
            }
        }
        IconMode::Username => {
            // A person: a head (circle) over shoulders (an arc-ish curve
            // approximated with a stroked circle clipped by the row — kept
            // simple as a filled circle plus a wider filled circle below,
            // which reads as a person silhouette at this size).
            let head_r = ICON_SIZE * 0.2;
            let head_center = center + egui::vec2(0.0, -ICON_SIZE * 0.18);
            painter.circle_filled(head_center, head_r, color);
            let shoulders = egui::Rect::from_center_size(
                center + egui::vec2(0.0, ICON_SIZE * 0.22),
                egui::vec2(ICON_SIZE * 0.62, ICON_SIZE * 0.34),
            );
            painter.rect_filled(
                shoulders,
                egui::CornerRadius {
                    nw: (ICON_SIZE * 0.3) as u8,
                    ne: (ICON_SIZE * 0.3) as u8,
                    sw: 0,
                    se: 0,
                },
                color,
            );
        }
        IconMode::Otp => {
            // A clock: a ring with two hands.
            let r = ICON_SIZE * 0.34;
            painter.circle_stroke(center, r, stroke);
            painter.line_segment([center, center + egui::vec2(0.0, -r * 0.6)], stroke);
            painter.line_segment([center, center + egui::vec2(r * 0.45, r * 0.15)], stroke);
        }
    }
}

/// The blue quick-select digit badge, anchored near the row's right edge.
fn paint_badge(painter: &egui::Painter, row_rect: egui::Rect, digit: char) {
    let center = row_rect.right_center() + egui::vec2(-BADGE_RIGHT_INSET - BADGE_SIZE / 2.0, 0.0);
    painter.circle_filled(center, BADGE_SIZE / 2.0, BADGE_COLOR);
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        digit,
        egui::FontId::proportional(ITEM_FONT_SIZE * 0.7),
        BADGE_TEXT_COLOR,
    );
}
