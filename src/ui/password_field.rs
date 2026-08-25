//! Renders a password field's visual state (a boxed row of bullets).
//!
//! Input handling lives in `App::logic`, not here — see plan F6:
//! `egui::TextEdit` is never used for the master password, since it keeps
//! cloned plaintext in an undo buffer inside `ctx.data` regardless of
//! `.password(true)`. Since there's no `TextEdit`, there's no egui widget
//! focus/undo machinery to hook into here either; this function only draws
//! what `App::logic` has already computed (the character count).

use eframe::egui;

pub fn draw(ui: &mut egui::Ui, char_count: usize) {
    // `widgets.inactive.bg_stroke` is nearly invisible by design (it's
    // meant for widgets that already stand out some other way) — against
    // this dialog's near-white/near-black background it read as no border
    // at all. Pick an explicit mid-gray instead, adapted for the theme so
    // it stays visible either way without being harsh.
    let border_gray = if ui.visuals().dark_mode { 90 } else { 150 };
    egui::Frame::new()
        .fill(ui.visuals().extreme_bg_color)
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(border_gray)))
        .inner_margin(egui::Margin::symmetric(8, 6))
        // Vertical breathing room from the label above and whatever
        // follows (the error line, or the footer) — part of the widget
        // itself so every call site gets it for free.
        .outer_margin(egui::Margin::symmetric(0, 6))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            // A single space when empty, so the box doesn't collapse to
            // zero height before anything is typed.
            let masked = if char_count == 0 {
                " ".to_string()
            } else {
                "•".repeat(char_count)
            };
            ui.monospace(masked);
        });
}
