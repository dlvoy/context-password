//! Renders a password field's visual state (a boxed row of bullets, or the
//! plaintext when revealed) plus its reveal-toggle eye button.
//!
//! Input handling lives in `App::logic`, not here — see plan F6:
//! `egui::TextEdit` is never used for the master password, since it keeps
//! cloned plaintext in an undo buffer inside `ctx.data` regardless of
//! `.password(true)`. Since there's no `TextEdit`, there's no egui widget
//! focus/undo machinery to hook into here either; this function only draws
//! what `App::logic` has already computed. Revealing the password is safe
//! precisely because it still never touches a `TextEdit` or any other
//! widget with its own hidden storage — it's a plain painted label, exactly
//! like the masked bullets it replaces.

use eframe::egui;

const EYE_BUTTON_SIZE: f32 = 22.0;

/// Draws the field and its reveal-toggle eye button. Returns `true` if the
/// eye was clicked this frame — the caller owns the `revealed` bool (it's
/// part of `Content::Prompting`), this just reports the click.
pub fn draw(ui: &mut egui::Ui, password: &str, revealed: bool) -> bool {
    // `widgets.inactive.bg_stroke` is nearly invisible by design (it's
    // meant for widgets that already stand out some other way) — against
    // this dialog's near-white/near-black background it read as no border
    // at all. Pick an explicit mid-gray instead, adapted for the theme so
    // it stays visible either way without being harsh.
    let border_gray = if ui.visuals().dark_mode { 90 } else { 150 };
    let mut toggled = false;
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
            // Without an explicit height, `Sides` centers each side within
            // `Spacing::interact_size.y` and lets taller content (the eye
            // button) overflow that afterward — which left the shorter
            // text side centered on the wrong, smaller reference height,
            // reading as "more padding below the text than above." Setting
            // it to the button's own size makes both sides center on the
            // same, already-correct height from the start.
            egui::Sides::new().height(EYE_BUTTON_SIZE).show(
                ui,
                |ui| {
                    // A single space when empty, so the box doesn't collapse
                    // to zero height before anything is typed.
                    let shown = if password.is_empty() {
                        " ".to_string()
                    } else if revealed {
                        password.to_string()
                    } else {
                        "•".repeat(password.chars().count())
                    };
                    ui.monospace(shown);
                },
                |ui| {
                    let (rect, response) = ui.allocate_exact_size(
                        egui::vec2(EYE_BUTTON_SIZE, EYE_BUTTON_SIZE),
                        egui::Sense::click(),
                    );
                    if ui.is_rect_visible(rect) {
                        let color = if response.hovered() {
                            ui.visuals().strong_text_color()
                        } else {
                            ui.visuals().text_color()
                        };
                        paint_eye(ui.painter(), rect, revealed, color);
                    }
                    if response.clicked() {
                        toggled = true;
                    }
                    response.on_hover_text(if revealed { "Hide password" } else { "Show password" });
                },
            );
        });
    toggled
}

/// A handful of points along a shallow arc — `upper` bulges up, otherwise
/// down — combined into a lens/almond outline for the open eye, or used
/// alone as the closed-eye eyelid curve. A parabola, not a true circular
/// arc, but indistinguishable at icon size and simpler to sample.
fn arc_points(center: egui::Pos2, half_width: f32, half_height: f32, upper: bool, n: usize) -> Vec<egui::Pos2> {
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32 * 2.0 - 1.0; // -1..1
            let bulge = 1.0 - t * t; // 0 at the ends, 1 in the middle
            let y_offset = if upper { -bulge * half_height } else { bulge * half_height };
            egui::pos2(center.x + t * half_width, center.y + y_offset)
        })
        .collect()
}

/// A hand-painted eye glyph — open (lens outline plus a filled pupil) or
/// closed (a single eyelid curve) — rather than an image asset or emoji
/// font glyph, consistent with the app's no-asset-pipeline style (see
/// `ui::popup::paint_icon`).
fn paint_eye(painter: &egui::Painter, rect: egui::Rect, open: bool, color: egui::Color32) {
    let center = rect.center();
    let half_width = rect.width() * 0.36;
    let half_height = rect.height() * 0.26;
    let stroke = egui::Stroke::new(1.4, color);
    if open {
        let mut points = arc_points(center, half_width, half_height, true, 10);
        let mut lower = arc_points(center, half_width, half_height, false, 10);
        lower.reverse();
        points.extend(lower);
        painter.add(egui::Shape::convex_polygon(points, egui::Color32::TRANSPARENT, stroke));
        painter.circle_filled(center, half_height * 0.55, color);
    } else {
        let curve = arc_points(center, half_width, half_height * 0.75, false, 10);
        painter.add(egui::Shape::line(curve, stroke));
    }
}
