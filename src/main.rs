mod emu;
mod patch;
mod smwc;

use eframe::egui::{self, Color32, FontId, Key, Rect, RichText, Sense, Stroke, TextureOptions, pos2, vec2};
use smwc::{Hack, Page, Query};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, Sender, channel};

const BG: Color32 = Color32::from_rgb(0x10, 0x10, 0x28);
const PANEL: Color32 = Color32::from_rgb(0x20, 0x20, 0x48);
const BLUE: Color32 = Color32::from_rgb(0x30, 0x40, 0xa0);
const YELLOW: Color32 = Color32::from_rgb(0xf8, 0xd8, 0x00);
const RED: Color32 = Color32::from_rgb(0xd8, 0x28, 0x28);
const GREEN: Color32 = Color32::from_rgb(0x38, 0xb8, 0x48);
const TEXT: Color32 = Color32::from_rgb(0xf0, 0xf0, 0xf0);
const DIM: Color32 = Color32::from_rgb(0x98, 0x98, 0xc0);

/// Idioma de la interfaz (índice en LANGS).
static LANG: AtomicUsize = AtomicUsize::new(0);
const LANGS: [&str; 2] = ["Español", "English"];

/// Texto en el idioma elegido.
pub fn t(es: &'static str, en: &'static str) -> &'static str {
    if LANG.load(Relaxed) == 1 { en } else { es }
}

const TILE_W: f32 = 240.;
const TILE_IMG: f32 = TILE_W * 224. / 256.;

/// Teclado → botones de SNES mientras se juega.
const KEYMAP: [(Key, u32); 12] = [
    (Key::Z, emu::B),
    (Key::A, emu::Y),
    (Key::Backspace, emu::SELECT),
    (Key::Enter, emu::START),
    (Key::ArrowUp, emu::UP),
    (Key::ArrowDown, emu::DOWN),
    (Key::ArrowLeft, emu::LEFT),
    (Key::ArrowRight, emu::RIGHT),
    (Key::X, emu::A),
    (Key::S, emu::X),
    (Key::Q, emu::L),
    (Key::W, emu::R),
];

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Config {
    dest: Option<PathBuf>,
    rom: Option<PathBuf>,
    /// Núcleo libretro del emulador integrado.
    core: PathBuf,
    /// Emulador externo opcional; si no está vacío se usa en lugar del integrado.
    external: String,
    lang: usize,
    library: Vec<Entry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Entry {
    hack: Hack,
    path: PathBuf,
}

enum Msg {
    Results(u64, Result<Page, String>),
    Installed(Box<Hack>, Result<PathBuf, String>),
    Pad(gilrs::Button),
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Library,
    Search,
    Settings,
}

struct App {
    cfg: Config,
    tab: Tab,
    query: Query,
    page: Option<Page>,
    search_id: u64,
    loading: bool,
    installing: HashSet<u64>,
    /// Página "ver más" abierta y el widget al que volver con B.
    details: Option<Hack>,
    back_focus: Option<egui::Id>,
    /// El primer widget enfocable que se dibuje pide el foco (navegación con mando).
    focus_first: bool,
    focus_results: bool,
    osk: bool,
    /// Dirección pedida con el mando; se aplica al foco al empezar el frame.
    pad_dir: Option<egui::FocusDirection>,
    status: String,
    rom_error: String,
    emu: Option<emu::Emu>,
    paused: bool,
    tex: Option<egui::TextureHandle>,
    play_t: f64,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,
}

fn find_program(cmd: &str) -> Option<PathBuf> {
    let prog = Path::new(cmd.split_whitespace().next()?);
    if prog.components().count() > 1 {
        return prog.is_file().then(|| prog.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(prog)).find(|p| p.is_file())
}

fn default_core() -> PathBuf {
    let own = eframe::storage_dir("smw-launcher").unwrap_or_default().join("cores");
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    [own.clone(), "/usr/lib/libretro".into(), home.join(".config/retroarch/cores")]
        .into_iter()
        .map(|d| d.join("snes9x_libretro.so"))
        .find(|p| p.is_file())
        .unwrap_or(own.join("snes9x_libretro.so"))
}

fn pretty(p: &Option<PathBuf>) -> String {
    p.as_ref().map_or(t("(sin elegir)", "(not set)").into(), |p| p.display().to_string())
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "smw".into(),
        std::sync::Arc::new(egui::FontData::from_static(include_bytes!("../assets/smw.ttf"))),
    );
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.get_mut(&fam).unwrap().insert(0, "smw".into());
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        use egui::TextStyle::*;
        // La fuente mide 8px por em: múltiplos de 8 se ven nítidos.
        s.text_styles = [(Heading, 32.), (Body, 16.), (Button, 24.), (Small, 16.), (Monospace, 16.)]
            .into_iter()
            .map(|(t, z)| (t, FontId::proportional(z)))
            .collect();
        s.spacing.button_padding = vec2(14., 8.);
        s.spacing.item_spacing = vec2(12., 10.);
        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = PANEL;
        v.extreme_bg_color = Color32::BLACK;
        v.faint_bg_color = PANEL;
        v.window_corner_radius = 0.into();
        v.menu_corner_radius = 0.into();
        v.window_shadow = egui::Shadow::NONE;
        v.popup_shadow = egui::Shadow::NONE;
        v.window_stroke = Stroke::new(3., YELLOW);
        v.selection.bg_fill = RED;
        v.selection.stroke = Stroke::new(2., Color32::WHITE);
        v.hyperlink_color = YELLOW;
        let w = &mut v.widgets;
        for (wv, fill, fg) in [
            (&mut w.noninteractive, BG, TEXT),
            (&mut w.inactive, PANEL, TEXT),
            (&mut w.hovered, BLUE, YELLOW),
            // `active` también es el estilo del widget enfocado: amarillo para verlo con el mando.
            (&mut w.active, YELLOW, Color32::BLACK),
            (&mut w.open, BLUE, YELLOW),
        ] {
            wv.corner_radius = 0.into();
            wv.expansion = 0.;
            wv.bg_fill = fill;
            wv.weak_bg_fill = fill;
            wv.fg_stroke = Stroke::new(2., fg);
        }
        w.noninteractive.bg_stroke = Stroke::new(2., BLUE);
        w.inactive.bg_stroke = Stroke::new(2., Color32::BLACK);
        w.hovered.bg_stroke = Stroke::new(2., YELLOW);
    });
}

/// Lee el mando en segundo plano: publica el estado para el emulador y los botones pulsados para la UI.
fn gamepad(tx: Sender<Msg>, ctx: egui::Context) {
    use gilrs::{Axis, Button::*};
    const MAP: [(gilrs::Button, u32); 12] = [
        (South, emu::B),
        (West, emu::Y),
        (Select, emu::SELECT),
        (Start, emu::START),
        (DPadUp, emu::UP),
        (DPadDown, emu::DOWN),
        (DPadLeft, emu::LEFT),
        (DPadRight, emu::RIGHT),
        (East, emu::A),
        (North, emu::X),
        (LeftTrigger, emu::L),
        (RightTrigger, emu::R),
    ];
    let Ok(mut gilrs) = gilrs::Gilrs::new() else { return };
    let mut stick_prev = 0u32;
    loop {
        // ponytail: con `None` gilrs no despierta en Linux; un timeout sí funciona.
        let Some(ev) = gilrs.next_event_blocking(Some(std::time::Duration::from_millis(100))) else { continue };
        let gp = gilrs.gamepad(ev.id);
        let (x, y) = (gp.value(Axis::LeftStickX), gp.value(Axis::LeftStickY));
        let stick = [(y > 0.5, emu::UP), (y < -0.5, emu::DOWN), (x < -0.5, emu::LEFT), (x > 0.5, emu::RIGHT)]
            .iter()
            .filter(|(on, _)| *on)
            .fold(0, |a, (_, b)| a | 1 << b);
        let bits = MAP.iter().filter(|(b, _)| gp.is_pressed(*b)).fold(stick, |a, (_, i)| a | 1 << i);
        emu::PAD.store(bits, Relaxed);
        // El stick también navega por los menús, como la cruceta.
        let mut pressed: Vec<gilrs::Button> =
            [(emu::UP, DPadUp), (emu::DOWN, DPadDown), (emu::LEFT, DPadLeft), (emu::RIGHT, DPadRight)]
                .into_iter()
                .filter(|(b, _)| (stick & !stick_prev) >> b & 1 == 1)
                .map(|(_, d)| d)
                .collect();
        stick_prev = stick;
        if let gilrs::EventType::ButtonPressed(b, _) = ev.event {
            pressed.push(b);
        }
        for b in pressed {
            tx.send(Msg::Pad(b)).ok();
            ctx.request_repaint();
        }
    }
}

fn autofocus(flag: &mut bool, r: &egui::Response) {
    if std::mem::take(flag) {
        r.request_focus();
    }
}

/// Botón que se desplaza a la vista al recibir foco (navegación con mando).
fn btn(ui: &mut egui::Ui, b: impl egui::Widget) -> egui::Response {
    let r = ui.add(b);
    if r.gained_focus() {
        r.scroll_to_me(None);
    }
    r
}

fn big(ui: &mut egui::Ui, text: &str) -> egui::Response {
    btn(ui, egui::Button::new(text).min_size(vec2(340., 52.)))
}

/// Selector que cambia de opción al pulsarlo (clic derecho = anterior).
fn cycle(ui: &mut egui::Ui, label: &str, sel: &mut usize, opts: &[&str], width: f32) -> bool {
    ui.label(RichText::new(label).color(DIM));
    let r = btn(ui, egui::Button::new(format!("{} →", opts[*sel])).min_size(vec2(width, 48.)));
    let n = opts.len();
    if r.clicked() {
        *sel = (*sel + 1) % n;
    } else if r.secondary_clicked() {
        *sel = (*sel + n - 1) % n;
    } else {
        return false;
    }
    true
}

fn chip(ui: &mut egui::Ui, text: &str) {
    egui::Frame::new()
        .stroke(Stroke::new(2., DIM))
        .inner_margin(vec2(6., 2.))
        .show(ui, |ui| ui.label(RichText::new(text).color(DIM)));
}

fn hint(ui: &mut egui::Ui, key: &str, color: Color32, text: &str) {
    let (r, _) = ui.allocate_exact_size(vec2(26., 26.), Sense::hover());
    ui.painter().circle_filled(r.center(), 13., color);
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, key, FontId::proportional(16.), Color32::WHITE);
    ui.label(text);
    ui.add_space(18.);
}

/// Portada de un hack en la cuadrícula.
fn tile(ui: &mut egui::Ui, h: &Hack, badge: Option<(&str, Color32)>) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(TILE_W, TILE_IMG + 52.), Sense::click());
    if r.gained_focus() {
        r.scroll_to_me(None);
    }
    let on = r.has_focus() || r.hovered();
    let p = ui.painter().clone();
    p.rect_filled(rect, 0., if on { BLUE } else { PANEL });
    let img = Rect::from_min_size(rect.min, vec2(TILE_W, TILE_IMG)).shrink(4.);
    match h.images.first() {
        Some(u) => egui::Image::new(u.as_str()).texture_options(TextureOptions::NEAREST).paint_at(ui, img),
        None => _ = p.rect_filled(img, 0., Color32::BLACK),
    }
    let mut job = egui::text::LayoutJob::simple_singleline(
        h.name.clone(),
        FontId::proportional(16.),
        if on { YELLOW } else { TEXT },
    );
    job.wrap = egui::text::TextWrapping { max_width: TILE_W - 12., max_rows: 2, ..Default::default() };
    p.galley(pos2(rect.min.x + 6., img.max.y + 6.), p.layout_job(job), TEXT);
    if let Some((t, c)) = badge {
        let g = p.layout_no_wrap(t.into(), FontId::proportional(16.), Color32::WHITE);
        let b = Rect::from_min_size(img.min + vec2(4., 4.), g.size() + vec2(8., 4.));
        p.rect_filled(b, 0., c);
        p.galley(b.min + vec2(4., 2.), g, Color32::WHITE);
    }
    let stroke = if on { Stroke::new(4., YELLOW) } else { Stroke::new(2., Color32::BLACK) };
    p.rect_stroke(rect, 0., stroke, egui::StrokeKind::Inside);
    r
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        setup_style(&cc.egui_ctx);
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let mut cfg: Config = cc.storage.and_then(|s| eframe::get_value(s, eframe::APP_KEY)).unwrap_or_default();
        if !cfg.core.is_file() {
            cfg.core = default_core();
        }
        cfg.library.retain(|e| e.path.exists());
        LANG.store(cfg.lang, Relaxed);
        let (tx, rx) = channel();
        let (pad_tx, ctx) = (tx.clone(), cc.egui_ctx.clone());
        std::thread::spawn(move || gamepad(pad_tx, ctx));
        Self {
            cfg,
            tab: Tab::Library,
            query: Query { sort: 1, page: 1, ..Default::default() },
            page: None,
            search_id: 0,
            loading: false,
            installing: HashSet::new(),
            details: None,
            back_focus: None,
            focus_first: true,
            focus_results: false,
            osk: false,
            pad_dir: None,
            status: String::new(),
            rom_error: String::new(),
            emu: None,
            paused: false,
            tex: None,
            play_t: 0.,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
        }
    }

    fn spawn(&self, job: impl FnOnce() -> Msg + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            tx.send(job()).ok();
            ctx.request_repaint();
        });
    }

    fn go(&mut self, tab: Tab) {
        self.tab = tab;
        self.details = None;
        self.osk = false;
        self.focus_first = true;
    }

    fn open_details(&mut self, h: Hack, from: egui::Id) {
        self.details = Some(h);
        self.back_focus = Some(from);
        self.focus_first = true;
    }

    fn close_details(&mut self) {
        self.details = None;
        if let Some(id) = self.back_focus.take() {
            self.ctx.memory_mut(|m| m.request_focus(id));
        }
    }

    fn search(&mut self) {
        self.search_id += 1;
        self.loading = true;
        let (id, q) = (self.search_id, self.query.clone());
        self.spawn(move || Msg::Results(id, smwc::search(&q)));
    }

    fn install(&mut self, h: Hack) {
        let (Some(rom), Some(dest)) = (self.cfg.rom.clone(), self.cfg.dest.clone()) else { return };
        if !self.installing.insert(h.id) {
            return;
        }
        self.status = format!("{} {}...", t("Descargando", "Downloading"), h.name);
        self.spawn(move || {
            let r = smwc::install(&h, &rom, &dest);
            Msg::Installed(Box::new(h), r)
        });
    }

    fn play(&mut self, rom: &Path) {
        let mut parts = self.cfg.external.split_whitespace();
        if let Some(prog) = parts.next() {
            match std::process::Command::new(prog).args(parts).arg(rom).spawn() {
                Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
                Err(e) => self.status = format!("{} '{prog}': {e}", t("No se pudo abrir el emulador", "Could not open the emulator")),
            }
            return;
        }
        match emu::Emu::start(self.cfg.core.clone(), rom.into(), self.ctx.clone()) {
            Ok(e) => {
                self.emu = Some(e);
                self.paused = false;
                self.play_t = self.ctx.input(|i| i.time);
            }
            Err(e) => self.status = e,
        }
    }

    fn set_paused(&mut self, p: bool) {
        if let Some(e) = &self.emu {
            e.set_paused(p);
        }
        self.paused = p;
        self.focus_first = p;
        emu::KEYS.store(0, Relaxed);
    }

    fn stop_game(&mut self) {
        self.emu = None; // Drop guarda la partida y cierra el núcleo
        self.tex = None;
        self.paused = false;
        self.focus_first = true;
        emu::KEYS.store(0, Relaxed);
    }

    /// Mando en menús: cruceta = mover foco, A = Enter, B = Escape, LB/RB = pestañas.
    fn pad(&mut self, b: gilrs::Button, input: &mut egui::RawInput) {
        use gilrs::Button::*;
        if self.emu.is_some() && !self.paused {
            let held = emu::PAD.load(Relaxed);
            let combo = (b == Start && held >> emu::SELECT & 1 == 1) || (b == Select && held >> emu::START & 1 == 1);
            if b == Mode || combo {
                self.set_paused(true);
            }
            return;
        }
        use egui::FocusDirection as Dir;
        // La cruceta mueve el foco directamente (no como flechas): así un campo de texto no la atrapa.
        let dir = match b {
            DPadUp => Some(Dir::Up),
            DPadDown => Some(Dir::Down),
            DPadLeft => Some(Dir::Left),
            DPadRight => Some(Dir::Right),
            _ => None,
        };
        if dir.is_some() {
            self.pad_dir = dir;
            return;
        }
        let key = match b {
            South => Key::Enter,
            East => Key::Escape,
            Mode | Start if self.emu.is_some() => return self.set_paused(false),
            LeftTrigger | RightTrigger if self.emu.is_none() => {
                let tabs = [Tab::Library, Tab::Search, Tab::Settings];
                let i = tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
                return self.go(tabs[(i + if b == RightTrigger { 1 } else { 2 }) % 3]);
            }
            _ => return,
        };
        for pressed in [true, false] {
            input.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: Default::default() });
        }
    }

    fn handle(&mut self, msg: Msg, input: &mut egui::RawInput) {
        match msg {
            // Ignora el mando si la ventana no tiene foco (p. ej. con un emulador externo).
            Msg::Pad(b) if input.focused => self.pad(b, input),
            Msg::Pad(_) => {}
            Msg::Results(id, _) if id != self.search_id => {}
            Msg::Results(_, r) => {
                self.loading = false;
                match r {
                    Ok(p) => {
                        self.page = Some(p);
                        self.focus_first |= std::mem::take(&mut self.focus_results);
                    }
                    Err(e) => self.status = format!("{}: {e}", t("Error al buscar", "Search failed")),
                }
            }
            Msg::Installed(h, r) => {
                self.installing.remove(&h.id);
                match r {
                    Ok(path) => {
                        self.status = format!("{} {}", h.name, t("añadido a la biblioteca", "added to the library"));
                        self.cfg.library.retain(|e| e.hack.id != h.id);
                        self.cfg.library.insert(0, Entry { hack: *h, path });
                    }
                    Err(e) => self.status = format!("{} {}: {e}", t("Error con", "Error with"), h.name),
                }
            }
        }
    }

    fn library(&mut self, ui: &mut egui::Ui) {
        if self.cfg.library.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(80.);
                ui.label(RichText::new(t("TU BIBLIOTECA ESTÁ VACÍA", "YOUR LIBRARY IS EMPTY")).heading().color(YELLOW));
                ui.label(t("Busca hacks en SMWCentral y añádelos aquí.", "Find hacks on SMWCentral and add them here."));
                ui.add_space(16.);
                let r = big(ui, t("IR A BUSCAR", "GO TO SEARCH"));
                autofocus(&mut self.focus_first, &r);
                if r.clicked() {
                    self.go(Tab::Search);
                }
            });
            return;
        }
        let mut open = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for e in &self.cfg.library {
                    let r = tile(ui, &e.hack, None);
                    autofocus(&mut self.focus_first, &r);
                    if r.clicked() {
                        open = Some((e.hack.clone(), r.id));
                    }
                }
            });
        });
        if let Some((h, id)) = open {
            self.open_details(h, id);
        }
    }

    /// Teclado en pantalla para escribir con el mando.
    fn osk(&mut self, ui: &mut egui::Ui) -> bool {
        let mut go = false;
        egui::Frame::new().fill(PANEL).stroke(Stroke::new(2., Color32::BLACK)).inner_margin(10).show(ui, |ui| {
            for row in ["1234567890", "QWERTYUIOP", "ASDFGHJKL'", "ZXCVBNM-!?"] {
                ui.horizontal(|ui| {
                    for c in row.chars() {
                        let r = btn(ui, egui::Button::new(c.to_string()).min_size(vec2(52., 52.)));
                        autofocus(&mut self.focus_first, &r);
                        if r.clicked() {
                            self.query.name.push(c.to_ascii_lowercase());
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if btn(ui, egui::Button::new(t("ESPACIO", "SPACE")).min_size(vec2(200., 52.))).clicked() {
                    self.query.name.push(' ');
                }
                if btn(ui, egui::Button::new(t("BORRAR", "DELETE"))).clicked() {
                    self.query.name.pop();
                }
                if btn(ui, egui::Button::new(t("LIMPIAR", "CLEAR"))).clicked() {
                    self.query.name.clear();
                }
                if btn(ui, egui::Button::new(t("BUSCAR", "SEARCH"))).clicked() {
                    self.osk = false;
                    go = true;
                }
            });
        });
        go
    }

    fn search_tab(&mut self, ui: &mut egui::Ui) {
        if self.page.is_none() && !self.loading {
            self.search();
        }
        let mut go = false;
        // Filtros en columna a la izquierda y resultados a la derecha: arriba/abajo recorre
        // los filtros y derecha entra en la cuadrícula, sin saltos raros del foco.
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(300.);
                go |= self.filters(ui);
            });
            ui.add_space(8.);
            ui.vertical(|ui| {
                if self.osk && self.osk(ui) {
                    go = true;
                    self.focus_results = true;
                }
                self.results(ui);
            });
        });
        if go {
            self.query.page = 1;
            self.search();
        }
    }

    fn filters(&mut self, ui: &mut egui::Ui) -> bool {
        let w = ui.available_width();
        let mut go = false;
        ui.label(RichText::new(t("NOMBRE", "NAME")).color(DIM));
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.query.name)
                .hint_text(t("Nombre...", "Name..."))
                .font(FontId::proportional(24.))
                .desired_width(w),
        );
        go |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if btn(ui, egui::Button::new(t("TECLADO", "KEYBOARD")).min_size(vec2(w, 48.))).clicked() {
            self.osk = !self.osk;
            self.focus_first = self.osk;
        }
        let any = t("Cualquiera", "Any");
        let mut diffs = smwc::DIFFICULTIES;
        diffs[0] = any;
        let mut kinds = smwc::KINDS.map(|k| k.0);
        kinds[0] = any;
        let sorts = [t("Recientes", "Newest"), t("Populares", "Popular"), t("Mejor valorados", "Top rated"), t("Nombre", "Name")];
        go |= cycle(ui, t("DIFICULTAD", "DIFFICULTY"), &mut self.query.difficulty, &diffs, w);
        go |= cycle(ui, t("TIPO", "TYPE"), &mut self.query.kind, &kinds, w);
        go |= cycle(ui, t("ORDEN", "SORT"), &mut self.query.sort, &sorts, w);
        ui.add_space(8.);
        if btn(ui, egui::Button::new(t("BUSCAR", "SEARCH")).min_size(vec2(w, 48.))).clicked() {
            go = true;
            self.focus_results = true;
        }
        go
    }

    fn results(&mut self, ui: &mut egui::Ui) {
        let Some(page) = self.page.take() else {
            ui.spinner();
            return;
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} {}", page.total, t("RESULTADOS", "RESULTS"))).color(DIM));
            if self.loading {
                ui.spinner();
            }
        });
        let (mut open, mut goto) = (None, None);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for h in &page.data {
                    let badge = if self.cfg.library.iter().any(|e| e.hack.id == h.id) {
                        Some((t("EN BIBLIOTECA", "IN LIBRARY"), GREEN))
                    } else if self.installing.contains(&h.id) {
                        Some((t("DESCARGANDO", "DOWNLOADING"), BLUE))
                    } else {
                        None
                    };
                    let r = tile(ui, h, badge);
                    autofocus(&mut self.focus_first, &r);
                    if r.clicked() {
                        open = Some((h.clone(), r.id));
                    }
                }
            });
            ui.add_space(8.);
            ui.horizontal(|ui| {
                let n = self.query.page;
                if ui.add_enabled_ui(n > 1, |ui| btn(ui, egui::Button::new(t("← ANTERIOR", "← PREVIOUS")))).inner.clicked() {
                    goto = Some(n - 1);
                }
                ui.label(format!("{} {n}/{}", t("PÁGINA", "PAGE"), page.last_page.max(1)));
                if ui.add_enabled_ui(n < page.last_page, |ui| btn(ui, egui::Button::new(t("SIGUIENTE →", "NEXT →")))).inner.clicked() {
                    goto = Some(n + 1);
                }
            });
        });
        self.page = Some(page);
        if let Some((h, id)) = open {
            self.open_details(h, id);
        }
        if let Some(n) = goto {
            self.query.page = n;
            self.focus_results = true;
            self.search();
        }
    }

    fn details_page(&mut self, ui: &mut egui::Ui, h: Hack) {
        let entry = self.cfg.library.iter().position(|e| e.hack.id == h.id);
        let (mut play, mut add, mut remove, mut back) = (false, false, false, false);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_top(|ui| {
                let n = h.images.len();
                if n > 0 {
                    // Carrusel automático: no necesita controles.
                    let i = (ui.input(|i| i.time) / 3.) as usize % n;
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                    let w = (ui.available_width() * 0.5).clamp(256., 768.);
                    ui.vertical(|ui| {
                        ui.add(
                            egui::Image::new(h.images[i].as_str())
                                .texture_options(TextureOptions::NEAREST)
                                .fit_to_exact_size(vec2(w, w * 224. / 256.)),
                        );
                        ui.horizontal(|ui| {
                            for k in 0..n {
                                let (r, _) = ui.allocate_exact_size(vec2(14., 14.), Sense::hover());
                                ui.painter().rect_filled(r, 0., if k == i { YELLOW } else { PANEL });
                            }
                        });
                    });
                }
                ui.add_space(12.);
                ui.vertical(|ui| {
                    ui.label(RichText::new(&h.name).heading().color(YELLOW));
                    ui.label(format!("{} {}", t("por", "by"), h.authors()));
                    ui.add_space(8.);
                    let f = &h.fields;
                    let rating = h.rating.map_or("-".into(), |r| format!("{r:.1} / 5"));
                    egui::Grid::new("info").num_columns(2).show(ui, |ui| {
                        for (k, v) in [
                            (t("VERSIÓN", "VERSION"), f.version.clone().unwrap_or_default()),
                            (t("DIFICULTAD", "DIFFICULTY"), f.difficulty.clone().unwrap_or_default()),
                            (t("TIPO", "TYPE"), f.kind.clone().unwrap_or_default()),
                            (t("LONGITUD", "LENGTH"), f.length.clone().unwrap_or_default()),
                            (t("DESCARGAS", "DOWNLOADS"), h.downloads.to_string()),
                            (t("VALORACIÓN", "RATING"), rating),
                        ] {
                            ui.label(RichText::new(k).color(DIM));
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                    ui.add_space(16.);
                    let first = if entry.is_some() {
                        let r = big(ui, t("JUGAR", "PLAY"));
                        play = r.clicked();
                        remove = big(ui, t("ELIMINAR", "DELETE")).clicked();
                        r
                    } else if self.installing.contains(&h.id) {
                        ui.add_enabled_ui(false, |ui| big(ui, t("DESCARGANDO...", "DOWNLOADING..."))).inner
                    } else {
                        let r = big(ui, t("AÑADIR A BIBLIOTECA", "ADD TO LIBRARY"));
                        add = r.clicked();
                        r
                    };
                    autofocus(&mut self.focus_first, &first);
                    if big(ui, t("VER EN SMWCENTRAL", "VIEW ON SMWCENTRAL")).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(h.url()));
                    }
                    back = big(ui, t("VOLVER", "BACK")).clicked();
                });
            });
            ui.add_space(12.);
            ui.separator();
            ui.label(h.description());
        });
        if play && let Some(i) = entry {
            let path = self.cfg.library[i].path.clone();
            self.play(&path);
        }
        if add {
            self.install(h);
        }
        if remove && let Some(i) = entry {
            let e = self.cfg.library.remove(i);
            std::fs::remove_file(&e.path).ok();
            self.status = format!("{} {}", e.hack.name, t("eliminado", "deleted"));
            self.focus_first = true;
        }
        if back {
            self.close_details();
        }
    }

    fn pickers(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new(t("CARPETA DONDE SE GUARDAN LAS ROMS", "FOLDER WHERE ROMS ARE SAVED")).color(DIM));
        ui.horizontal(|ui| {
            let r = btn(ui, egui::Button::new(t("ELEGIR...", "CHOOSE...")));
            autofocus(&mut self.focus_first, &r);
            if r.clicked()
                && let Some(p) = rfd::FileDialog::new().pick_folder()
            {
                self.cfg.dest = Some(p);
            }
            ui.label(pretty(&self.cfg.dest));
        });
        ui.add_space(8.);
        ui.label(RichText::new(t("ROM ORIGINAL DE SUPER MARIO WORLD (USA)", "ORIGINAL SUPER MARIO WORLD (USA) ROM")).color(DIM));
        ui.horizontal(|ui| {
            if btn(ui, egui::Button::new(t("ELEGIR...", "CHOOSE..."))).clicked()
                && let Some(p) = rfd::FileDialog::new().add_filter(t("ROM de SNES", "SNES ROM"), &["sfc", "smc"]).pick_file()
            {
                if smwc::is_clean_smw(&p) {
                    self.cfg.rom = Some(p);
                    self.rom_error.clear();
                } else {
                    self.rom_error = t("Esa ROM no es Super Mario World (USA) sin modificar", "That ROM is not an unmodified Super Mario World (USA)").into();
                }
            }
            ui.label(pretty(&self.cfg.rom));
        });
        if !self.rom_error.is_empty() {
            ui.colored_label(RED, &self.rom_error);
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut lang = LANG.load(Relaxed);
            if cycle(ui, t("IDIOMA DEL LAUNCHER", "LAUNCHER LANGUAGE"), &mut lang, &LANGS, 340.) {
                LANG.store(lang, Relaxed);
                self.cfg.lang = lang;
            }
            ui.add_space(8.);
            self.pickers(ui);
            ui.add_space(8.);
            ui.label(RichText::new(t("EMULADOR INTEGRADO (NÚCLEO LIBRETRO)", "BUILT-IN EMULATOR (LIBRETRO CORE)")).color(DIM));
            ui.horizontal(|ui| {
                if btn(ui, egui::Button::new(t("ELEGIR...", "CHOOSE..."))).clicked()
                    && let Some(p) = rfd::FileDialog::new().add_filter(t("Núcleo libretro", "Libretro core"), &["so"]).pick_file()
                {
                    self.cfg.core = p;
                }
                ui.label(self.cfg.core.display().to_string());
            });
            if !self.cfg.core.is_file() {
                ui.colored_label(RED, t("No se encuentra el núcleo", "Core not found"));
            }
            ui.add_space(8.);
            ui.label(RichText::new(t("EMULADOR EXTERNO (OPCIONAL: SI LO RELLENAS SE USA EN LUGAR DEL INTEGRADO)", "EXTERNAL EMULATOR (OPTIONAL: IF SET IT REPLACES THE BUILT-IN ONE)")).color(DIM));
            ui.add(
                egui::TextEdit::singleline(&mut self.cfg.external)
                    .hint_text(t("ej: snes9x-gtk", "e.g. snes9x-gtk"))
                    .font(FontId::proportional(24.))
                    .desired_width(480.),
            );
            if !self.cfg.external.trim().is_empty() && find_program(&self.cfg.external).is_none() {
                ui.colored_label(RED, t("No se encuentra ese emulador", "Emulator not found"));
            }
            ui.add_space(16.);
            ui.label(RichText::new(t("CONTROLES EN EL JUEGO", "IN-GAME CONTROLS")).color(YELLOW));
            ui.label(t("Pausa: HOME, SELECT+START o ESC.", "Pause: HOME, SELECT+START or ESC."));
            ui.label(t("Teclado: flechas, X=A, Z=B, S=X, A=Y, Q=L, W=R, ENTER=START, RETROCESO=SELECT.", "Keyboard: arrows, X=A, Z=B, S=X, A=Y, Q=L, W=R, ENTER=START, BACKSPACE=SELECT."));
            ui.label(t("F11: pantalla completa.", "F11: fullscreen."));
        });
    }

    fn game(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let was_paused = self.paused;
        if !self.paused {
            let keys = ui.input(|i| KEYMAP.iter().filter(|(k, _)| i.key_down(*k)).fold(0, |a, (_, b)| a | 1 << b));
            emu::KEYS.store(keys, Relaxed);
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                self.set_paused(true);
            }
        }
        let Some(e) = &self.emu else { return };
        let aspect = e.aspect;
        if let Some(img) = e.frame.lock().unwrap().take() {
            match &mut self.tex {
                Some(t) => t.set(img, TextureOptions::NEAREST),
                None => self.tex = Some(ctx.load_texture("emu", img, TextureOptions::NEAREST)),
            }
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::BLACK)).show(ui, |ui| {
            let full = ui.max_rect();
            if let Some(t) = &self.tex {
                let h = full.height().min(full.width() / aspect);
                let rect = Rect::from_center_size(full.center(), vec2(h * aspect, h));
                let uv = Rect::from_min_max(pos2(0., 0.), pos2(1., 1.));
                ui.painter().image(t.id(), rect, uv, Color32::WHITE);
            }
            if !self.paused && ui.input(|i| i.time) - self.play_t < 5. {
                ui.painter().text(
                    full.center_bottom() - vec2(0., 24.),
                    egui::Align2::CENTER_BOTTOM,
                    t("PAUSA: HOME, SELECT+START o ESC", "PAUSE: HOME, SELECT+START or ESC"),
                    FontId::proportional(16.),
                    DIM,
                );
            }
        });
        if !self.paused {
            return;
        }
        let mut act = 0;
        let modal = egui::Modal::new(egui::Id::new("pause")).show(&ctx, |ui| {
            ui.set_width(380.);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(t("PAUSA", "PAUSED")).heading().color(YELLOW));
                ui.add_space(8.);
                for (i, t) in [t("CONTINUAR", "RESUME"), t("REINICIAR", "RESET"), t("VOLVER A LA BIBLIOTECA", "BACK TO LIBRARY")].into_iter().enumerate() {
                    let r = big(ui, t);
                    autofocus(&mut self.focus_first, &r);
                    if r.clicked() {
                        act = i + 1;
                    }
                }
            });
        });
        // B / Esc / clic fuera = continuar (salvo en el mismo frame en que se pausó con Esc).
        if act == 0 && was_paused && modal.should_close() {
            act = 1;
        }
        match act {
            1 => self.set_paused(false),
            2 => {
                if let Some(e) = &self.emu {
                    e.reset();
                }
                self.set_paused(false);
            }
            3 => self.stop_game(),
            _ => {}
        }
    }

    fn setup_modal(&mut self, ctx: &egui::Context) {
        if self.cfg.dest.is_some() && self.cfg.rom.is_some() {
            return;
        }
        egui::Modal::new(egui::Id::new("setup")).show(ctx, |ui| {
            ui.set_width(560.);
            ui.label(RichText::new(t("¡BIENVENIDO!", "WELCOME!")).heading().color(YELLOW));
            ui.label(t("Antes de empezar elige dónde guardar los hacks y tu ROM original.", "Before starting, choose where to save hacks and your original ROM."));
            ui.add_space(8.);
            self.pickers(ui);
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if let Some(d) = self.pad_dir.take() {
            // Sin nada enfocado una dirección no hace nada: el primer toque enfoca el primer widget.
            let d = if ctx.memory(|m| m.focused().is_none()) { egui::FocusDirection::Next } else { d };
            ctx.memory_mut(|m| m.move_focus(d));
        }
        if ui.input(|i| i.key_pressed(Key::F11)) {
            let fs = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
        }
        if self.emu.is_some() {
            return self.game(ui);
        }
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            if self.details.is_some() {
                self.close_details();
            } else {
                self.osk = false;
            }
        }
        // left margin = item_spacing.x + the 32px gap after the logo, so both sides match
        let frame = egui::Frame::side_top_panel(&ui.style()).inner_margin(egui::Margin { left: 44, right: 8, top: 12, bottom: 12 });
        egui::Panel::top("tabs").frame(frame).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(egui::include_image!("../assets/logo.png")).fit_to_exact_size(egui::vec2(168., 64.)));
                ui.add_space(32.);
                chip(ui, "LB");
                for (t, l) in [(Tab::Library, t("BIBLIOTECA", "LIBRARY")), (Tab::Search, t("BUSCAR", "SEARCH")), (Tab::Settings, t("AJUSTES", "SETTINGS"))] {
                    if ui.add(egui::Button::selectable(self.tab == t, l)).clicked() {
                        self.go(t);
                    }
                }
                chip(ui, "RB");
            });
        });
        egui::Panel::bottom("hints").show(ui, |ui| {
            ui.add_space(6.);
            ui.horizontal(|ui| {
                hint(ui, "A", GREEN, t("ACEPTAR", "SELECT"));
                hint(ui, "B", RED, t("ATRÁS", "BACK"));
                chip(ui, "LB");
                chip(ui, "RB");
                ui.label(t("PESTAÑAS", "TABS"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&self.status).color(DIM));
                });
            });
            ui.add_space(6.);
        });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(16)).show(ui, |ui| {
            if let Some(h) = self.details.clone() {
                self.details_page(ui, h);
            } else {
                match self.tab {
                    Tab::Library => self.library(ui),
                    Tab::Search => self.search_tab(ui),
                    Tab::Settings => self.settings(ui),
                }
            }
        });
        self.setup_modal(&ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, input: &mut egui::RawInput) {
        while let Ok(m) = self.rx.try_recv() {
            self.handle(m, input);
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, &self.cfg);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("SMW Launcher")
            .with_app_id("smw-launcher")
            .with_inner_size([1280., 800.])
            .with_min_inner_size([800., 600.]),
        ..Default::default()
    };
    eframe::run_native("smw-launcher", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
