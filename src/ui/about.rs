//! The About screen: version, license, and copyright. A screen in the same
//! popup window as Settings, for the same reason — see `Content::Settings`'s
//! doc in `app.rs`: a separate viewport panics when opened while the root is
//! hidden, which is exactly when the tray's About item needs it.

use eframe::egui;

/// Embedded at compile time so it ships with the executable regardless of
/// what else is (or isn't) installed alongside it — see `LICENSE.txt`.
const LICENSE_TEXT: &str = include_str!("../../LICENSE.txt");

// Matches `config_window`'s own constants so the OK button here looks
// identical to Settings' Save button (font, padding) — duplicated rather
// than shared since the two screens are otherwise independent, and it's
// two small values.
const FONT_SCALE: f32 = 1.25;
const BUTTON_PADDING: egui::Vec2 = egui::Vec2::new(12.0, 8.0);

/// Returns `true` once OK is clicked or Escape is pressed.
pub fn draw(ui: &mut egui::Ui) -> bool {
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return true;
    }

    let style = ui.style_mut();
    for font_id in style.text_styles.values_mut() {
        font_id.size *= FONT_SCALE;
    }
    style.spacing.button_padding = BUTTON_PADDING;

    let mut close = false;

    egui::Panel::bottom("about_footer")
        .show_separator_line(true)
        .show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                // Right-aligned, same as Save on the Settings screen (which
                // uses this same `Sides` mechanism with Cancel on the left).
                egui::Sides::new().show(
                    ui,
                    |_ui| {},
                    |ui| {
                        if ui.button("OK").clicked() {
                            close = true;
                        }
                    },
                );
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(16)))
        .show(ui, |ui| {
            ui.heading("Context Password for Bitwarden");
            ui.separator();
            ui.add_space(8.0);
            ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
            ui.label("MIT License");
            ui.label("Copyright \u{a9} 2026 Dominik Dzienia");
            // A plain, selectable label rather than a hyperlink — opening a
            // browser would need a new dependency for this one line.
            ui.label(env!("CARGO_PKG_REPOSITORY"));
            ui.add_space(12.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.monospace(LICENSE_TEXT);
            });
        });

    close
}
