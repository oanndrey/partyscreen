// Tema visual do app (escuro, mesmas cores da página web) e componentes reutilizáveis.
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Shadow, Stroke, TextStyle,
};

pub const BG: Color32 = Color32::from_rgb(0x0f, 0x11, 0x15);
pub const SURFACE: Color32 = Color32::from_rgb(0x18, 0x1b, 0x22);
pub const SURFACE_HI: Color32 = Color32::from_rgb(0x21, 0x25, 0x2e);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x2f, 0x3a);
pub const TEXT: Color32 = Color32::from_rgb(0xe8, 0xea, 0xf0);
pub const MUTED: Color32 = Color32::from_rgb(0x90, 0x97, 0xa6);
pub const ACCENT: Color32 = Color32::from_rgb(0x4f, 0x7c, 0xff);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0x6a, 0x90, 0xff);
pub const LIVE: Color32 = Color32::from_rgb(0x30, 0xa4, 0x6c);
pub const DANGER: Color32 = Color32::from_rgb(0xe5, 0x48, 0x4d);

pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.text_styles = [
            (TextStyle::Heading, FontId::new(24.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(15.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(15.0, FontFamily::Monospace)),
        ]
        .into();

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(10.0, 10.0);
        s.button_padding = egui::vec2(16.0, 9.0);
        s.interact_size.y = 36.0;

        let v = &mut style.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.extreme_bg_color = BG;
        v.faint_bg_color = SURFACE;
        v.override_text_color = Some(TEXT);
        v.hyperlink_color = ACCENT_HI;
        v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
        v.selection.stroke = Stroke::new(1.0, ACCENT_HI);
        v.window_corner_radius = CornerRadius::same(16);
        v.window_stroke = Stroke::new(1.0, BORDER);
        v.window_shadow = Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(140) };
        v.popup_shadow = v.window_shadow;

        let radius = CornerRadius::same(10);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = SURFACE;
        w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
        w.inactive.bg_fill = SURFACE_HI;
        w.inactive.weak_bg_fill = SURFACE_HI;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.bg_fill = Color32::from_rgb(0x2a, 0x2f, 0x3a);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x2a, 0x2f, 0x3a);
        w.hovered.bg_stroke = Stroke::new(1.0, MUTED);
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.bg_fill = Color32::from_rgb(0x33, 0x39, 0x46);
        w.active.weak_bg_fill = Color32::from_rgb(0x33, 0x39, 0x46);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.open = w.active;
        for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            state.corner_radius = radius;
            state.expansion = 0.0;
        }
    });
}

/// Cartão com fundo, borda e cantos arredondados
pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::same(22))
}

/// Botão principal (azul)
pub fn primary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(16.0).strong().color(Color32::WHITE))
        .fill(ACCENT)
        .stroke(Stroke::NONE)
        .min_size(egui::vec2(0.0, 44.0))
}

/// Botão de ação destrutiva (vermelho)
pub fn danger(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(15.0).strong().color(Color32::WHITE))
        .fill(DANGER)
        .stroke(Stroke::NONE)
        .min_size(egui::vec2(0.0, 40.0))
}

/// Botão secundário (contorno)
pub fn secondary(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).size(15.0)).min_size(egui::vec2(0.0, 40.0))
}

/// Etiqueta pequena com fundo (ex.: "AO VIVO", "60 fps")
pub fn chip(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.18))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(10, 4))
        .show(ui, |ui| {
            ui.label(RichText::new(text.into()).size(12.5).strong().color(color));
        });
}

/// Faixa de aviso (erro ou atenção)
pub fn banner(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).color(color.lerp_to_gamma(Color32::WHITE, 0.45)));
        });
}

/// Bolinha pulsando (indicador de ao vivo)
pub fn pulse_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
    let t = ui.input(|i| i.time) as f32;
    let halo = 4.0 + 3.0 * (t * 3.0).sin().abs();
    ui.painter().circle_filled(rect.center(), halo, color.gamma_multiply(0.25));
    ui.painter().circle_filled(rect.center(), 4.0, color);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}
