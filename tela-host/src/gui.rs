// Janela do app: compartilhar (gera código) ou assistir (digita código).
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align, Color32, Layout, RichText};

use crate::session::{self, Config, Handle, SharedStatus, Source, SourceInfo, State};
use crate::theme::{self, ACCENT, DANGER, LIVE, MUTED};
use crate::viewer::{self, SharedFrame};

#[derive(PartialEq)]
enum Mode {
    Home,
    /// Escolhendo tela/janela antes de compartilhar
    Picking,
    Sharing,
    Watching,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Settings {
    site: String,
    fps: u32,
    mbps: u32,
    audio: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { site: String::new(), fps: 60, mbps: 6, audio: true }
    }
}

pub struct App {
    rt: tokio::runtime::Runtime,
    settings: Settings,
    show_settings: bool,
    mode: Mode,
    code_input: String,
    status: SharedStatus,
    frame: SharedFrame,
    handle: Option<Handle>,
    texture: Option<(egui::TextureHandle, u64)>,
    toast: Option<(String, Instant)>,
    sources: Vec<SourceInfo>,
    selected: Option<Source>,
    pick_error: Option<String>,
    /// Última vez que o mouse mexeu (a barra sobre o vídeo some depois de um tempo)
    last_pointer: Instant,
}

const MAX_CONTENT: f32 = 760.0;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, "settings")).unwrap_or_default();
        let show_settings = settings.site.trim().is_empty();
        theme::apply(&cc.egui_ctx);
        Self {
            rt: tokio::runtime::Runtime::new().expect("runtime tokio"),
            settings,
            show_settings,
            mode: Mode::Home,
            code_input: String::new(),
            status: Arc::new(Mutex::new(Default::default())),
            frame: Arc::new(Mutex::new(None)),
            handle: None,
            texture: None,
            toast: None,
            sources: Vec::new(),
            selected: None,
            pick_error: None,
            last_pointer: Instant::now(),
        }
    }

    fn repaint(ctx: &egui::Context) -> session::Repaint {
        let ctx = ctx.clone();
        Arc::new(move || ctx.request_repaint())
    }

    fn pick(&mut self) {
        #[cfg(any(windows, target_os = "macos"))]
        let result = crate::capture::sources();
        #[cfg(not(any(windows, target_os = "macos")))]
        let result: anyhow::Result<Vec<SourceInfo>> = Err(anyhow::anyhow!("sistema não suportado"));
        match result {
            Ok(list) => {
                if !self.selected.as_ref().is_some_and(|s| list.iter().any(|i| &i.source == s)) {
                    self.selected = list.first().map(|i| i.source.clone());
                }
                self.sources = list;
                self.pick_error = None;
            }
            Err(e) => {
                self.sources.clear();
                self.pick_error = Some(format!("{e:#}"));
            }
        }
        self.mode = Mode::Picking;
    }

    fn share(&mut self, ctx: &egui::Context) {
        let Some(source) = self.selected.clone() else { return };
        let cfg = Config {
            source,
            audio: self.settings.audio,
            site: self.settings.site.clone(),
            fps: self.settings.fps,
            mbps: self.settings.mbps,
            max_width: 1920,
        };
        self.handle = Some(session::start(&self.rt, cfg, self.status.clone(), Self::repaint(ctx)));
        self.mode = Mode::Sharing;
    }

    fn watch(&mut self, ctx: &egui::Context) {
        self.handle = Some(viewer::start(
            &self.rt,
            self.settings.site.clone(),
            self.code_input.clone(),
            self.status.clone(),
            self.frame.clone(),
            Self::repaint(ctx),
        ));
        self.mode = Mode::Watching;
    }

    fn stop(&mut self, ctx: &egui::Context) {
        if let Some(h) = self.handle.take() {
            h.stop();
        }
        self.mode = Mode::Home;
        self.texture = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }

    fn copy(&mut self, ctx: &egui::Context, text: String, what: &str) {
        ctx.copy_text(text);
        self.toast = Some((format!("{what} copiado"), Instant::now()));
    }

    // ── Topo ──────────────────────────────────────────────────────────────

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
            ui.painter().circle_filled(rect.center(), 6.0, LIVE);
            ui.label(RichText::new("Tela P2P").size(18.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let gear = egui::Button::new(RichText::new("⚙  Configurações").size(14.0)).frame(false);
                if ui.add(gear).clicked() {
                    self.show_settings = true;
                }
            });
        });
        ui.add_space(4.0);
    }

    // ── Início ────────────────────────────────────────────────────────────

    fn home(&mut self, ui: &mut egui::Ui, error: Option<String>) {
        let ctx = ui.ctx().clone();
        let ready = !self.settings.site.trim().is_empty();

        ui.add_space(28.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("Compartilhe sua tela em segundos").size(30.0).strong());
            ui.label(RichText::new("Gere um código, mande para quem vai assistir e pronto.").size(16.0).color(MUTED));
        });
        ui.add_space(24.0);

        if let Some(error) = error {
            theme::banner(ui, &error, DANGER);
            ui.add_space(8.0);
        }
        if !ready {
            theme::banner(ui, "Configure o endereço do servidor para começar (⚙ Configurações).", Color32::from_rgb(0xf5, 0xa5, 0x24));
            ui.add_space(8.0);
        }

        let mut share = false;
        let mut watch = false;
        let wide = ui.available_width() > 600.0;
        let mut cards = |ui: &mut egui::Ui, which: usize, this: &mut Self| {
            theme::card().show(ui, |ui| {
                ui.set_min_height(210.0);
                ui.set_width(ui.available_width());
                if which == 0 {
                    ui.label(RichText::new("🖥").size(30.0));
                    ui.label(RichText::new("Compartilhar").size(20.0).strong());
                    ui.label(RichText::new("Escolha uma tela ou janela e envie o código gerado.").color(MUTED));
                    ui.add_space(ui.available_height() - 50.0);
                    let b = theme::primary("Compartilhar minha tela").min_size(egui::vec2(ui.available_width(), 46.0));
                    share = ui.add_enabled(ready, b).clicked();
                } else {
                    ui.label(RichText::new("▶").size(30.0).color(ACCENT));
                    ui.label(RichText::new("Assistir").size(20.0).strong());
                    ui.label(RichText::new("Digite o código que você recebeu.").color(MUTED));
                    ui.add_space(6.0);
                    let input = ui.add(
                        egui::TextEdit::singleline(&mut this.code_input)
                            .hint_text("CÓDIGO")
                            .char_limit(6)
                            .font(egui::FontId::monospace(26.0))
                            .align(egui::Align2::CENTER_CENTER)
                            .margin(egui::vec2(10.0, 10.0))
                            .desired_width(f32::INFINITY),
                    );
                    this.code_input = this.code_input.to_uppercase();
                    let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let can = ready && this.code_input.trim().len() >= 4;
                    ui.add_space((ui.available_height() - 50.0).max(0.0));
                    let b = theme::primary("Assistir").min_size(egui::vec2(ui.available_width(), 46.0));
                    watch = ui.add_enabled(can, b).clicked() || (enter && can);
                }
            });
        };
        if wide {
            ui.columns(2, |cols| {
                cards(&mut cols[0], 0, self);
                cards(&mut cols[1], 1, self);
            });
        } else {
            cards(ui, 0, self);
            cards(ui, 1, self);
        }

        if share {
            self.pick();
        }
        if watch {
            self.watch(&ctx);
        }
    }

    // ── Escolher tela/janela ──────────────────────────────────────────────

    fn picking(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui.add(theme::secondary("‹  Voltar")).clicked() {
                self.mode = Mode::Home;
            }
            ui.label(RichText::new("O que você quer compartilhar?").size(22.0).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::secondary("⟳  Atualizar")).clicked() {
                    self.pick();
                }
            });
        });
        ui.add_space(8.0);
        if let Some(e) = &self.pick_error {
            theme::banner(ui, e, DANGER);
        }

        let mut clicked = None;
        let mut start = false;
        let footer = 74.0;
        egui::ScrollArea::vertical()
            .max_height((ui.available_height() - footer).max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (title, screens) in [("TELAS", true), ("JANELAS", false)] {
                    let items: Vec<&SourceInfo> = self
                        .sources
                        .iter()
                        .filter(|s| matches!(s.source, Source::Display(_)) == screens)
                        .collect();
                    if items.is_empty() {
                        continue;
                    }
                    ui.label(RichText::new(title).size(12.5).strong().color(MUTED));
                    // Largura calculada aqui fora: dentro de uma linha o espaço disponível encolhe
                    let gap = 10.0;
                    let full = ui.available_width();
                    let cols = ((full / 240.0).floor() as usize).max(1);
                    let w = (full - gap * (cols as f32 - 1.0)) / cols as f32;
                    for row in items.chunks(cols) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for item in row {
                                let selected = self.selected.as_ref() == Some(&item.source);
                                let r = source_tile(ui, item, selected, w);
                                if r.clicked() {
                                    clicked = Some(item.source.clone());
                                }
                                if r.double_clicked() {
                                    clicked = Some(item.source.clone());
                                    start = true;
                                }
                            }
                        });
                    }
                    ui.add_space(14.0);
                }
            });

        if clicked.is_some() {
            self.selected = clicked;
        }
        if start {
            self.share(&ctx);
            return;
        }

        ui.add_space(6.0);
        theme::card().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.settings.audio, RichText::new("🔊  Compartilhar o som do computador").size(15.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let b = theme::primary("Compartilhar").min_size(egui::vec2(170.0, 44.0));
                    if ui.add_enabled(self.selected.is_some(), b).clicked() {
                        self.share(&ctx);
                    }
                });
            });
        });
    }

    // ── Compartilhando ────────────────────────────────────────────────────

    fn sharing(&mut self, ui: &mut egui::Ui, status: &session::Status) {
        let ctx = ui.ctx().clone();
        ui.add_space(20.0);
        theme::card().inner_margin(egui::Margin::same(32)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| match &status.state {
                State::Live { code, link } => {
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 110.0).max(0.0) / 2.0);
                        theme::pulse_dot(ui, LIVE);
                        ui.label(RichText::new("AO VIVO").size(13.0).strong().color(LIVE));
                    });
                    ui.add_space(10.0);
                    ui.label(RichText::new("Seu código").color(MUTED));
                    let spaced: String = code.chars().flat_map(|c| [c, ' ']).collect();
                    ui.label(RichText::new(spaced.trim_end()).size(60.0).monospace().strong());
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 390.0).max(0.0) / 2.0);
                        if ui.add(theme::primary("Copiar código").min_size(egui::vec2(170.0, 42.0))).clicked() {
                            self.copy(&ctx, code.clone(), "Código");
                        }
                        if ui.add(theme::secondary("Copiar link do navegador").min_size(egui::vec2(200.0, 42.0))).clicked() {
                            self.copy(&ctx, link.clone(), "Link");
                        }
                    });
                }
                State::Connecting => {
                    ui.add_space(30.0);
                    ui.spinner();
                    ui.label(RichText::new("Criando sala…").color(MUTED));
                    ui.add_space(30.0);
                }
                _ => {}
            });
        });

        if let State::Live { .. } = status.state {
            ui.add_space(14.0);
            ui.columns(2, |cols| {
                theme::card().show(&mut cols[0], |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("ESPECTADORES").size(12.5).strong().color(MUTED));
                    let n = status.viewers;
                    let text = match n {
                        0 => "Ninguém ainda".to_string(),
                        1 => "1 pessoa".to_string(),
                        n => format!("{n} pessoas"),
                    };
                    ui.label(RichText::new(text).size(22.0).strong());
                });
                theme::card().show(&mut cols[1], |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("TRANSMISSÃO").size(12.5).strong().color(MUTED));
                    ui.horizontal_wrapped(|ui| {
                        theme::chip(ui, format!("{:.0} fps", status.fps), ACCENT);
                        theme::chip(ui, format!("{:.1} Mbps", status.mbps), ACCENT);
                        theme::chip(ui, format!("{:.0} ms/quadro", status.encode_ms), MUTED);
                        if self.settings.audio {
                            theme::chip(ui, "com som", LIVE);
                        }
                    });
                });
            });
        }

        ui.add_space(18.0);
        ui.vertical_centered(|ui| {
            if ui.add(theme::danger("Parar compartilhamento").min_size(egui::vec2(240.0, 44.0))).clicked() {
                self.stop(&ctx);
            }
        });
    }

    // ── Assistindo ────────────────────────────────────────────────────────

    fn watching(&mut self, ui: &mut egui::Ui, status: &session::Status) {
        let ctx = ui.ctx().clone();

        // Sobe o quadro novo para a GPU só quando muda
        if let Some(f) = self.frame.lock().unwrap().as_ref() {
            if self.texture.as_ref().map(|t| t.1) != Some(f.serial) {
                let image = egui::ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.rgba);
                match &mut self.texture {
                    Some((tex, serial)) => {
                        tex.set(image, egui::TextureOptions::LINEAR);
                        *serial = f.serial;
                    }
                    None => {
                        let tex = ctx.load_texture("tela", image, egui::TextureOptions::LINEAR);
                        self.texture = Some((tex, f.serial));
                    }
                }
            }
        }

        let fullscreen = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        }
        if ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO || i.pointer.any_down()) {
            self.last_pointer = Instant::now();
        }

        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, egui::Sense::click());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::BLACK);

        let live = status.state == State::Watching && self.texture.is_some();
        if let (true, Some((tex, _))) = (live, &self.texture) {
            let size = tex.size_vec2();
            let scale = (area.width() / size.x).min(area.height() / size.y);
            let rect = egui::Rect::from_center_size(area.center(), size * scale);
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            painter.image(tex.id(), rect, uv, Color32::WHITE);
            if response.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
            }
        } else {
            let text = match status.state {
                State::Connecting => "Conectando…",
                _ => "Esperando a transmissão começar…",
            };
            let c = area.center();
            painter.text(c + egui::vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(17.0), MUTED);
            let t = ui.input(|i| i.time) as f32;
            for k in 0..3 {
                let a = ((t * 2.0 - k as f32 * 0.4).sin() * 0.5 + 0.5).clamp(0.2, 1.0);
                painter.circle_filled(c + egui::vec2(-14.0 + 14.0 * k as f32, -10.0), 4.0, ACCENT.gamma_multiply(a));
            }
            ctx.request_repaint_after(Duration::from_millis(33));
        }

        // Barra por cima do vídeo: some depois de 2,5 s sem mexer o mouse
        let show_bar = !live || self.last_pointer.elapsed() < Duration::from_millis(2500);
        if show_bar {
            let bar = egui::Rect::from_min_size(area.min, egui::vec2(area.width(), 56.0));
            painter.rect_filled(bar, 0.0, Color32::from_black_alpha(170));
            let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(14.0, 10.0))));
            bar_ui.horizontal_centered(|ui| {
                if ui.add(theme::secondary("‹  Sair")).clicked() {
                    self.stop(&ctx);
                }
                ui.label(RichText::new(format!("Sala {}", self.code_input)).strong());
                if live {
                    theme::pulse_dot(ui, LIVE);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let label = if fullscreen { "Sair da tela cheia" } else { "⛶  Tela cheia" };
                    if ui.add(theme::secondary(label)).clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                    }
                    if live {
                        theme::chip(ui, format!("{:.1} Mbps", status.mbps), MUTED);
                        theme::chip(ui, format!("{:.0} fps", status.fps), ACCENT);
                    }
                });
            });
        } else {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }

    // ── Configurações ─────────────────────────────────────────────────────

    fn settings_modal(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let modal = egui::Modal::new(egui::Id::unique("config")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new("Configurações").size(20.0).strong());
            ui.add_space(8.0);

            ui.label(RichText::new("Servidor").strong());
            ui.add(
                egui::TextEdit::singleline(&mut self.settings.site)
                    .hint_text("https://….trycloudflare.com")
                    .margin(egui::vec2(10.0, 8.0))
                    .desired_width(f32::INFINITY),
            );
            ui.label(RichText::new("O mesmo endereço para quem compartilha e para quem assiste.").size(12.5).color(MUTED));
            ui.add_space(10.0);

            ui.label(RichText::new("Quadros por segundo").strong());
            ui.horizontal(|ui| {
                for fps in [30, 60] {
                    let selected = self.settings.fps == fps;
                    let text = RichText::new(format!("{fps} fps")).size(15.0);
                    if ui.add(egui::Button::selectable(selected, text).min_size(egui::vec2(90.0, 36.0))).clicked() {
                        self.settings.fps = fps;
                    }
                }
            });
            ui.add_space(10.0);

            ui.label(RichText::new("Qualidade máxima").strong());
            ui.add(egui::Slider::new(&mut self.settings.mbps, 2..=20).suffix(" Mbps"));
            ui.label(RichText::new("Mais qualidade usa mais internet de quem compartilha (por espectador).").size(12.5).color(MUTED));
            ui.add_space(14.0);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(theme::primary("Pronto").min_size(egui::vec2(120.0, 40.0))).clicked() {
                    ui.close();
                }
            });
        });
        if modal.should_close() {
            self.show_settings = false;
        }
    }

    fn toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else { return };
        if at.elapsed() > Duration::from_millis(1800) {
            self.toast = None;
            return;
        }
        egui::Area::new(egui::Id::unique("toast"))
            .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -24.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x2a, 0x2f, 0x3a))
                    .corner_radius(egui::CornerRadius::same(255))
                    .inner_margin(egui::Margin::symmetric(16, 8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(format!("✔  {text}")).strong());
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(200));
    }
}

/// Bloco clicável de uma tela/janela na lista de escolha
fn source_tile(ui: &mut egui::Ui, item: &SourceInfo, selected: bool, width: f32) -> egui::Response {
    let (icon, kind) = match item.source {
        Source::Display(_) => ("🖥", "Tela inteira"),
        Source::Window(_) => ("🗖", "Janela"),
    };
    let (title, detail) = match item.label.split_once(" — ") {
        Some((app, title)) => (title.to_string(), app.to_string()),
        None => (item.label.clone(), kind.to_string()),
    };

    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 64.0), egui::Sense::click());
    let hovered = response.hovered();
    let (fill, stroke) = match (selected, hovered) {
        (true, _) => (ACCENT.gamma_multiply(0.16), egui::Stroke::new(2.0, ACCENT)),
        (false, true) => (theme::SURFACE_HI, egui::Stroke::new(1.0, MUTED)),
        _ => (theme::SURFACE, egui::Stroke::new(1.0, theme::BORDER)),
    };
    let painter = ui.painter_at(rect);
    painter.rect(rect, 12.0, fill, stroke, egui::StrokeKind::Inside);
    painter.text(
        rect.left_center() + egui::vec2(18.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon,
        egui::FontId::proportional(24.0),
        if selected { ACCENT } else { MUTED },
    );

    let text_rect = egui::Rect::from_min_max(rect.min + egui::vec2(54.0, 10.0), rect.max - egui::vec2(12.0, 10.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(text_rect).layout(Layout::top_down(Align::Min)));
    child.spacing_mut().item_spacing.y = 2.0;
    child.add(egui::Label::new(RichText::new(title).strong()).truncate().selectable(false));
    child.add(egui::Label::new(RichText::new(detail).size(12.5).color(MUTED)).truncate().selectable(false));
    response.on_hover_text(&item.label)
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let status = self.status.lock().unwrap().clone();

        // Sessão terminou sozinha (erro ou servidor caiu): volta para o início
        let error = match &status.state {
            State::Error(e) => Some(e.clone()),
            _ => None,
        };
        if matches!(self.mode, Mode::Sharing | Mode::Watching)
            && matches!(status.state, State::Error(_) | State::Idle)
            && self.handle.is_some()
        {
            self.handle = None;
            self.stop(&ctx);
        }

        if self.mode == Mode::Watching {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(Color32::BLACK))
                .show(ui, |ui| self.watching(ui, &status));
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin::symmetric(24, 18)))
                .show(ui, |ui| {
                    self.header(ui);
                    // Conteúdo centralizado com largura máxima
                    let w = ui.available_width().min(MAX_CONTENT);
                    let pad = (ui.available_width() - w) / 2.0;
                    ui.horizontal_top(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(w);
                            match self.mode {
                                Mode::Home => self.home(ui, error),
                                Mode::Picking => self.picking(ui),
                                Mode::Sharing => self.sharing(ui, &status),
                                Mode::Watching => {}
                            }
                        });
                    });
                });
        }

        self.settings_modal(&ctx);
        self.toast(&ctx);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "settings", &self.settings);
    }
}
