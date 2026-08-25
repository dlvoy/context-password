//! Rendering for each popup content state (plan §9's `Prompting`,
//! `Unlocking`, `ShowingList`). Input handling and state transitions live
//! in `App::logic`; these functions only draw, and `showing_list` reports
//! back which row (if any) was clicked so `App::logic` can act on it.

use eframe::egui;
use egui::emath::GuiRounding;

use crate::bw::model::Entry;

const WARNING_COLOR: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x88, 0x00);
const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(0xcc, 0x33, 0x33);
const TITLE_SIZE: f32 = 13.0;
const HINT_SIZE: f32 = 11.0;
const ITEM_FONT_SIZE: f32 = 16.0;
const ITEM_ROW_HEIGHT: f32 = 34.0;
const ITEM_TEXT_INSET: f32 = 10.0;

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

pub fn unlocking(ui: &mut egui::Ui) {
    frame(ui, "Please wait…", |ui| {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spinner();
            ui.add_space(6.0);
            ui.label("Unlocking vault…");
        });
    });
}

/// Returns the index of a clicked row, if any.
pub fn showing_list(
    ui: &mut egui::Ui,
    entries: &[Entry],
    selected: usize,
    dropped: usize,
    target_elevated: bool,
) -> Option<usize> {
    frame(
        ui,
        "Up/Down to navigate  ·  Enter to type  ·  Esc to dismiss",
        |ui| {
            let mut clicked = None;
            if entries.is_empty() {
                ui.label("No items tagged for this app yet.");
            } else {
                for (i, entry) in entries.iter().enumerate() {
                    let label = match &entry.username {
                        Some(u) => format!("{} ({u})", entry.name),
                        None => entry.name.clone(),
                    };
                    if list_row(ui, i == selected, &label).clicked() {
                        clicked = Some(i);
                    }
                }
            }
            if dropped > 0 {
                ui.label(format!("({dropped} item(s) hidden — see log)"));
            }
            if target_elevated {
                ui.colored_label(
                    WARNING_COLOR,
                    "Target runs elevated — typing would be blocked. Enter will abort.",
                );
            }
            clicked
        },
    )
}

/// A full-width, click-and-hover-highlighted list row, painted directly
/// (rather than via `Button`/`selectable_label`) so it can be sized to fill
/// the popup and carry its own left text inset — a stock `SelectableLabel`
/// sizes to its text and centers it, neither of which suits a list row.
/// Colors come from `Style::interact_selectable`, so this stays correctly
/// themed in both light and dark mode without any hardcoded colors here.
fn list_row(ui: &mut egui::Ui, selected: bool, text: &str) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), ITEM_ROW_HEIGHT);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() {
            ui.painter()
                .rect_filled(rect, visuals.corner_radius, visuals.weak_bg_fill);
        }
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
    }

    response
}
