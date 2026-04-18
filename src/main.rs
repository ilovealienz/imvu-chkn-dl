#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

const FONT_BYTES: &[u8] = include_bytes!("../JetBrainsMono-Regular.ttf");
const FONT_INTER: &[u8] = include_bytes!("../InterVariable.ttf");
const ICON_PNG:   &[u8] = include_bytes!("../chkn-logo-32.png");

#[derive(Debug, Clone, Deserialize)]
struct ManifestEntry {
    name: String,
    url:  Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ProductInfo {
    name:          String,
    creator:       String,
    creator_cid:   u64,
    rating:        String,
    shop_url:      String,
    product_image: String,
    categories:         Vec<String>,
    parent_id:          Option<u64>,
    parent_name:        String,
    allows_derivation:  bool,
}

#[derive(Debug, Clone, PartialEq)]
enum DownloadMode { Chkn, AllExtracted, MediaOnly }

#[derive(Debug, Clone)]
struct Revision {
    number:   u32,
    manifest: Vec<ManifestEntry>,
}

impl Revision {
    fn media_count(&self) -> usize { self.manifest.iter().filter(|f| is_media(&f.name)).count() }
    fn mesh_count(&self)  -> usize { self.manifest.iter().filter(|f| is_mesh(&f.name)).count() }
}

#[derive(Debug, Clone, PartialEq)]
enum State {
    Idle, Scanning, Ready,
    Downloading { rev: u32, mode: DownloadMode },
    Done { path: PathBuf, label: String },
    Error(String),
}

#[derive(Clone)]
struct Settings {
    scan_batch:     usize,
    scan_start_rev: u32,
    scan_max_rev:   u32,
    dl_batch:       usize,
    show_advanced:  bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { scan_batch: 5, scan_start_rev: 1, scan_max_rev: 100, dl_batch: 4, show_advanced: false }
    }
}

#[derive(Clone)]
#[allow(dead_code)]
enum TexEntry { Loading, Loaded(egui::TextureHandle), Failed }
type TexCache  = Arc<Mutex<HashMap<String, TexEntry>>>;
type DimsCache = Arc<Mutex<HashMap<String, (u32, u32)>>>;

struct App {
    input:            String,
    pid:              Option<u64>,
    revisions:        Arc<Mutex<Vec<Revision>>>,
    state:            Arc<Mutex<State>>,
    log:              Arc<Mutex<String>>,
    progress:         Arc<Mutex<f32>>,
    save_dir:         Option<PathBuf>,
    settings:         Settings,
    textures:         TexCache,
    pending_tex:      Arc<Mutex<Vec<(String, u32, u32, u32, u32, Vec<u8>)>>>,
    dims_cache:       DimsCache,
    product_info:     Arc<Mutex<Option<ProductInfo>>>,
    single_dl:        Arc<Mutex<Option<SingleDlState>>>,
    rev_reversed:     bool,
}

#[derive(Debug, Clone)]
enum SingleDlState {
    Downloading(String),
    Done(PathBuf),
    Error(String),
}

impl Default for App {
    fn default() -> Self {
        Self {
            input: String::new(), pid: None,
            revisions:    Arc::new(Mutex::new(Vec::new())),
            state:        Arc::new(Mutex::new(State::Idle)),
            log:          Arc::new(Mutex::new(String::new())),
            progress:     Arc::new(Mutex::new(0.0)),
            save_dir:     None,
            settings:     Settings::default(),
            textures:     Arc::new(Mutex::new(HashMap::new())),
            pending_tex:  Arc::new(Mutex::new(Vec::new())),
            dims_cache:   Arc::new(Mutex::new(HashMap::<String,(u32,u32)>::new())),
            product_info: Arc::new(Mutex::new(None)),
            single_dl:    Arc::new(Mutex::new(None)),
            rev_reversed: true,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        {
            let mut pending = self.pending_tex.lock().unwrap();
            if !pending.is_empty() {
                let mut cache = self.textures.lock().unwrap();
                for (key, orig_w, orig_h, dw, dh, rgba) in pending.drain(..) {
                    let img = egui::ColorImage::from_rgba_unmultiplied([dw as usize, dh as usize], &rgba);
                    let handle = ctx.load_texture(&key, img, egui::TextureOptions::LINEAR);
                    cache.insert(key.clone(), TexEntry::Loaded(handle));
                    self.dims_cache.lock().unwrap().insert(key, (orig_w, orig_h));
                }
                ctx.request_repaint();
            }
        }

        let mut vis = egui::Visuals::dark();
        vis.panel_fill               = egui::Color32::from_rgb(20, 12, 18);
        vis.window_fill              = egui::Color32::from_rgb(28, 16, 24);
        vis.widgets.inactive.bg_fill = egui::Color32::from_rgb(40, 24, 35);
        vis.widgets.hovered.bg_fill  = egui::Color32::from_rgb(58, 30, 48);
        vis.widgets.active.bg_fill   = egui::Color32::from_rgb(200, 75, 135);
        ctx.set_visuals(vis);

        let state = self.state.lock().unwrap().clone();
        let pink  = egui::Color32::from_rgb(255, 130, 190);
        let muted = egui::Color32::from_rgb(130, 100, 118);
        let green = egui::Color32::from_rgb(80, 210, 120);
        let red   = egui::Color32::from_rgb(220, 80, 80);
        let amber = egui::Color32::from_rgb(220, 170, 60);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("chkn").size(28.0).color(pink).strong());
                        ui.label(egui::RichText::new("downloader").size(28.0)
                            .color(egui::Color32::from_rgb(180, 100, 145)));
                    });
                    ui.label(egui::RichText::new("made by Silver Spooner")
                        .size(10.0).color(egui::Color32::from_rgb(90, 55, 75)).italics());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("paste a product URL or ID")
                        .size(11.0).color(egui::Color32::from_rgb(90, 55, 75)).italics());
                });
            });
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(10.0);

            // Input
            ui.horizontal(|ui| {
                let scanning = state == State::Scanning;
                let w = ui.available_width() - 80.0;
                ui.add(egui::TextEdit::singleline(&mut self.input)
                    .desired_width(w)
                    .hint_text("44576114  or  https://www.imvu.com/shop/product.php?products_id=44576114")
                );
                let b = egui::Button::new(
                    egui::RichText::new(if scanning { "..." } else { "Scan" })
                        .color(egui::Color32::WHITE).size(14.0)
                )
                .fill(egui::Color32::from_rgb(175, 55, 105))
                .rounding(egui::Rounding::same(6.0));
                if ui.add_enabled(!scanning, b).clicked()
                    || (ui.input(|i| i.key_pressed(egui::Key::Enter)) && !scanning)
                { self.start_scan(ctx); }
            });

            ui.add_space(8.0);

            // Save dir + advanced toggle
            ui.horizontal(|ui| {
                let dir_txt = self.save_dir.as_ref()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "Downloads folder".into());
                ui.label(egui::RichText::new(format!("Save to: {}", dir_txt))
                    .size(11.0).color(muted).italics());
                if ui.small_button("Choose...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() { self.save_dir = Some(p); }
                }
                if self.save_dir.is_some() && ui.small_button("X").clicked() { self.save_dir = None; }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let lbl = if self.settings.show_advanced { "^ Advanced" } else { "v Advanced" };
                    if ui.small_button(lbl).clicked() { self.settings.show_advanced = !self.settings.show_advanced; }
                });
            });

            if self.settings.show_advanced {
                ui.add_space(8.0);
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(28, 16, 24))
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::symmetric(14.0, 10.0))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Advanced Settings").size(12.0).color(amber));
                        ui.add_space(8.0);
                        egui::Grid::new("adv").num_columns(2).spacing([20.0, 6.0]).show(ui, |ui| {
                            ui.label(egui::RichText::new("Scan batch size").size(12.0).color(muted));
                            ui.add(egui::Slider::new(&mut self.settings.scan_batch, 1..=20).suffix(" parallel"));
                            ui.end_row();
                            ui.label(egui::RichText::new("Start from revision").size(12.0).color(muted));
                            ui.add(egui::Slider::new(&mut self.settings.scan_start_rev, 1..=50));
                            ui.end_row();
                            ui.label(egui::RichText::new("Scan down from rev").size(12.0).color(muted));
                            ui.horizontal(|ui| {
                                ui.add(egui::Slider::new(&mut self.settings.scan_max_rev, 10..=500));
                                ui.label(egui::RichText::new("(fallback if going up finds nothing)")
                                    .size(10.0).color(muted).italics());
                            });
                            ui.end_row();
                            ui.label(egui::RichText::new("Download batch size").size(12.0).color(muted));
                            ui.add(egui::Slider::new(&mut self.settings.dl_batch, 1..=16).suffix(" parallel"));
                            ui.end_row();
                        });
                    });
            }

            ui.add_space(8.0);

            // State feedback
            match &state {
                State::Idle | State::Ready => {}
                State::Scanning => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(self.log.lock().unwrap().clone()).size(12.0).color(muted));
                    });
                    ctx.request_repaint();
                }
                State::Error(msg) => {
                    ui.label(egui::RichText::new(format!("  {}", msg)).size(13.0).color(red));
                }
                State::Done { path, label } => {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("Done: {}", label)).size(13.0).color(green));
                        let open_btn = egui::Button::new(
                            egui::RichText::new("Open folder").size(12.0).color(egui::Color32::WHITE)
                        )
                        .fill(egui::Color32::from_rgb(40, 24, 35))
                        .rounding(egui::Rounding::same(5.0));
                        if ui.add(open_btn).clicked() {
                            let folder = if path.is_dir() { path.clone() } else {
                                path.parent().unwrap_or(path).to_path_buf()
                            };
                            let _ = open::that(&folder);
                        }
                    });
                }
                State::Downloading { .. } => {
                    let p = *self.progress.lock().unwrap();
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(egui::RichText::new(self.log.lock().unwrap().clone()).size(12.0).color(muted));
                    });
                    ui.add(egui::ProgressBar::new(p).show_percentage());
                    ctx.request_repaint();
                }
            }

            // Single file download status
            {
                let sdl = self.single_dl.lock().unwrap().clone();
                match sdl {
                    Some(SingleDlState::Downloading(name)) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label(egui::RichText::new(format!("Saving {}...", name)).size(11.0).color(muted));
                        });
                        ctx.request_repaint();
                    }
                    Some(SingleDlState::Done(p)) => {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("Saved: {}", p.file_name().unwrap_or_default().to_string_lossy())).size(11.0).color(green));
                            if ui.small_button("Open folder").clicked() {
                                let folder = p.parent().unwrap_or(&p).to_path_buf();
                                let _ = open::that(&folder);
                            }
                        });
                    }
                    Some(SingleDlState::Error(e)) => {
                        ui.label(egui::RichText::new(format!("Save failed: {}", e)).size(11.0).color(red));
                    }
                    None => {}
                }
            }

            // Product info
            {
                let info = self.product_info.lock().unwrap().clone();
                if let Some(ref pi) = info {
                    // Kick off thumbnail fetch
                    let thumb_key = format!("product_thumb_{}", pi.shop_url);
                    let thumb_state = self.textures.lock().unwrap().get(&thumb_key).cloned();
                    if thumb_state.is_none() && !pi.product_image.is_empty() {
                        self.textures.lock().unwrap().insert(thumb_key.clone(), TexEntry::Loading);
                        let url2 = pi.product_image.clone();
                        let key2 = thumb_key.clone();
                        let pending = Arc::clone(&self.pending_tex);
                        let ctx2 = ctx.clone();
                        thread::spawn(move || {
                            if let Ok(resp) = reqwest::blocking::get(&url2) {
                                if let Ok(bytes) = resp.bytes() {
                                    if let Ok(img) = image::load_from_memory(&bytes) {
                                        let rgba = img.to_rgba8();
                                        let (w, h) = rgba.dimensions();
                                        pending.lock().unwrap().push((key2, w, h, w, h, rgba.into_raw()));
                                        ctx2.request_repaint();
                                    }
                                }
                            }
                        });
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        // Thumbnail
                        match self.textures.lock().unwrap().get(&thumb_key).cloned() {
                            Some(TexEntry::Loaded(handle)) => {
                                ui.add(egui::Image::new(&handle)
                                    .fit_to_exact_size(egui::vec2(100.0, 80.0))
                                    .rounding(egui::Rounding::same(4.0)));
                            }
                            Some(TexEntry::Loading) | None => {
                                let (r, _) = ui.allocate_exact_size(egui::vec2(100.0, 80.0), egui::Sense::hover());
                                ui.painter().rect_filled(r, 4.0, egui::Color32::from_rgb(26, 15, 22));
                                ctx.request_repaint();
                            }
                            Some(TexEntry::Failed) => {
                                ui.allocate_exact_size(egui::vec2(100.0, 80.0), egui::Sense::hover());
                            }
                        }

                        ui.add_space(10.0);

                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&pi.name).size(18.0).color(pink).strong());
                                if !pi.rating.is_empty() {
                                    ui.label(egui::RichText::new(format!("[{}]", pi.rating))
                                        .size(11.0).color(muted));
                                }
                            });
                            if !pi.creator.is_empty() {
                                ui.label(egui::RichText::new(
                                    format!("by {}  (CID {})", pi.creator, pi.creator_cid))
                                    .size(12.0).color(muted));
                            }
                            if !pi.categories.is_empty() {
                                ui.label(egui::RichText::new(pi.categories.join("  >  "))
                                    .size(10.0).color(egui::Color32::from_rgb(100, 75, 90)));
                            }
                            // Parent — click to scan it
                            if let Some(ppid) = pi.parent_id {
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("derives from:").size(10.0).color(muted));
                                    let display = if pi.parent_name.is_empty() {
                                        ppid.to_string()
                                    } else {
                                        pi.parent_name.clone()
                                    };
                                    let btn = egui::Button::new(
                                        egui::RichText::new(&display).size(10.0)
                                            .color(egui::Color32::from_rgb(180, 130, 220))
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .frame(false);
                                    if ui.add(btn).on_hover_text(format!("scan product {}", ppid)).clicked() {
                                        self.input = ppid.to_string();
                                        self.start_scan(ctx);
                                    }
                                });
                            }
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                let link_color = egui::Color32::from_rgb(130, 100, 180);
                                ui.hyperlink_to(
                                    egui::RichText::new("product page").size(11.0).color(link_color),
                                    &pi.shop_url,
                                );
                                ui.label(egui::RichText::new("|").size(11.0).color(egui::Color32::from_rgb(60, 35, 55)));
                                let tree_url = format!("https://www.imvu.com/shop/derivation_tree.php?products_id={}",
                                    pi.shop_url.split("products_id=").nth(1).unwrap_or(""));
                                ui.hyperlink_to(
                                    egui::RichText::new("derivation tree").size(11.0).color(link_color),
                                    &tree_url,
                                );
                                if pi.allows_derivation {
                                    ui.label(egui::RichText::new("|").size(11.0).color(egui::Color32::from_rgb(60, 35, 55)));
                                    let derived_url = format!("https://www.imvu.com/shop/web_search.php?derived_from={}",
                                        pi.shop_url.split("products_id=").nth(1).unwrap_or(""));
                                    ui.hyperlink_to(
                                        egui::RichText::new("derived products").size(11.0).color(link_color),
                                        &derived_url,
                                    );
                                }
                                if pi.creator_cid > 0 {
                                    ui.label(egui::RichText::new("|").size(11.0).color(egui::Color32::from_rgb(60, 35, 55)));
                                    let shop_url = format!("https://www.imvu.com/shop/web_search.php?manufacturers_id={}",
                                        pi.creator_cid);
                                    ui.hyperlink_to(
                                        egui::RichText::new("creator's shop").size(11.0).color(link_color),
                                        &shop_url,
                                    );
                                }
                            });
                        });
                    });
                }
            }

            // Revision list
            let revisions = self.revisions.lock().unwrap().clone();
            if !revisions.is_empty() {
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                let pid      = self.pid.unwrap_or(0);
                let busy     = matches!(state, State::Downloading { .. } | State::Scanning);
                let dl_batch = self.settings.dl_batch;
                let save_dir = self.save_dir.clone().unwrap_or_else(default_download_dir);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{} revision(s)", revisions.len()))
                        .size(11.0).color(muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let order_lbl = if self.rev_reversed { "newest first" } else { "oldest first" };
                        if ui.small_button(order_lbl).clicked() {
                            self.rev_reversed = !self.rev_reversed;
                        }
                    });
                });
                ui.add_space(4.0);
                let mut revisions_sorted = revisions.clone();
                if self.rev_reversed {
                    revisions_sorted.sort_by(|a, b| b.number.cmp(&a.number));
                } else {
                    revisions_sorted.sort_by(|a, b| a.number.cmp(&b.number));
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.set_max_width(ui.available_width());
                    for rev in &revisions_sorted {
                        self.draw_rev_row(ui, pid, rev, busy, dl_batch, &save_dir, pink, muted, ctx, &self.dims_cache.clone());
                        ui.add_space(6.0);
                    }
                });
            }
        });
    }
}

impl App {
    fn draw_rev_row(&self, ui: &mut egui::Ui, pid: u64, rev: &Revision,
                    busy: bool, dl_batch: usize, save_dir: &PathBuf,
                    pink: egui::Color32, muted: egui::Color32, ctx: &egui::Context,
                    dims_cache: &DimsCache) {
        let media: Vec<&ManifestEntry> = rev.manifest.iter().filter(|f| is_media(&f.name)).collect();
        let other: Vec<&ManifestEntry> = rev.manifest.iter().filter(|f| !is_media(&f.name)).collect();

        let available_w = ui.available_width();
        egui::Frame::none()
            .fill(egui::Color32::from_rgb(30, 18, 26))
            .rounding(egui::Rounding::same(6.0))
            .inner_margin(egui::Margin::symmetric(12.0, 10.0))
            .show(ui, |ui| {
                ui.set_max_width(available_w - 24.0);
                ui.horizontal(|ui| {
                    egui::Frame::none()
                        .fill(egui::Color32::from_rgb(45, 25, 38))
                        .rounding(egui::Rounding::same(4.0))
                        .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(format!("Rev {}", rev.number))
                                .size(13.0).color(pink).strong());
                        });
                    ui.label(egui::RichText::new(format!(
                        "{}  files  *  {}  media  *  {}  mesh/anim",
                        rev.manifest.len(), rev.media_count(), rev.mesh_count()
                    )).size(11.0).color(muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (label, mode) in [
                            ("media only",    DownloadMode::MediaOnly),
                            ("all extracted", DownloadMode::AllExtracted),
                            (".chkn",         DownloadMode::Chkn),
                        ] {
                            let b = egui::Button::new(egui::RichText::new(label).size(12.0).color(egui::Color32::WHITE))
                                .fill(egui::Color32::from_rgb(42, 24, 36))
                                .rounding(egui::Rounding::same(5.0));
                            if ui.add_enabled(!busy, b).clicked() {
                                self.start_download(pid, rev.clone(), mode, dl_batch, save_dir.clone(), ctx);
                            }
                        }
                    });
                });

                if !media.is_empty() {
                    ui.add_space(8.0);
                    ui.horizontal_wrapped(|ui| {
                        for entry in &media {
                            let file_url = entry.url.as_deref().unwrap_or(&entry.name);
                            let key = format!("{}/{}/{}", pid, rev.number, file_url);
                            let fetch_url = format!(
                                "https://userimages-akm.imvu.com/productdata/{}/{}/{}",
                                pid, rev.number, file_url
                            );
                            let tex_state = self.textures.lock().unwrap().get(&key).cloned();

                            let thumb_response = match tex_state {
                                None => {
                                    self.textures.lock().unwrap().insert(key.clone(), TexEntry::Loading);
                                    let key2    = key.clone();
                                    let pending = Arc::clone(&self.pending_tex);
                                    let ctx2    = ctx.clone();
                                    let url2    = fetch_url.clone();
                                    thread::spawn(move || {
                                        if let Ok(resp) = reqwest::blocking::get(&url2) {
                                            if let Ok(bytes) = resp.bytes() {
                                                if let Ok(img) = image::load_from_memory(&bytes) {
                                                    let rgba = img.to_rgba8();
                                                    let (orig_w, orig_h) = rgba.dimensions();
                                                    let (dw, dh, pixels) = if orig_w > 256 || orig_h > 256 {
                                                        let s = image::imageops::resize(&rgba, 128, 128,
                                                            image::imageops::FilterType::Triangle);
                                                        (128u32, 128u32, s.into_raw())
                                                    } else { (orig_w, orig_h, rgba.into_raw()) };
                                                    pending.lock().unwrap().push((key2, orig_w, orig_h, dw, dh, pixels));
                                                    ctx2.request_repaint();
                                                }
                                            }
                                        }
                                    });
                                    placeholder(ui, "...")
                                }
                                Some(TexEntry::Loading) => {
                                    ctx.request_repaint();
                                    placeholder(ui, "...")
                                }
                                Some(TexEntry::Loaded(handle)) => {
                                    let img = egui::Image::new(&handle)
                                        .fit_to_exact_size(egui::vec2(64.0, 64.0))
                                        .rounding(egui::Rounding::same(4.0));
                                    let hover_txt = if let Some(&(w, h)) = dims_cache.lock().unwrap().get(&key) {
                                        format!("{} - {}x{}", entry.name, w, h)
                                    } else {
                                        entry.name.clone()
                                    };
                                    Some(ui.add(img).on_hover_text(hover_txt))
                                }
                                Some(TexEntry::Failed) => placeholder(ui, "X"),
                            };

                            if let Some(resp) = thumb_response {
                                resp.context_menu(|ui| {
                                    ui.label(egui::RichText::new(&entry.name).size(11.0).color(muted));
                                    ui.separator();
                                    if ui.button("Save this file").clicked() {
                                        let name = entry.name.clone();
                                        let url  = fetch_url.clone();
                                        let dir  = self.save_dir.clone().unwrap_or_else(default_download_dir);
                                        let sdl  = Arc::clone(&self.single_dl);
                                        let ctx2 = ctx.clone();
                                        *sdl.lock().unwrap() = Some(SingleDlState::Downloading(name.clone()));
                                        thread::spawn(move || {
                                            match reqwest::blocking::get(&url) {
                                                Ok(resp) => {
                                                    let mime = resp.headers()
                                                        .get("content-type")
                                                        .and_then(|v| v.to_str().ok())
                                                        .unwrap_or("")
                                                        .split(';').next().unwrap_or("").trim().to_string();
                                                    match resp.bytes() {
                                                        Ok(bytes) => {
                                                            let out_name = fix_extension_mime(&name, &mime, &bytes);
                                                            let out_path = dir.join(&out_name);
                                                            match std::fs::write(&out_path, &bytes) {
                                                                Ok(_)  => *sdl.lock().unwrap() = Some(SingleDlState::Done(out_path)),
                                                                Err(e) => *sdl.lock().unwrap() = Some(SingleDlState::Error(e.to_string())),
                                                            }
                                                        }
                                                        Err(e) => *sdl.lock().unwrap() = Some(SingleDlState::Error(e.to_string())),
                                                    }
                                                }
                                                Err(e) => *sdl.lock().unwrap() = Some(SingleDlState::Error(e.to_string())),
                                            }
                                            ctx2.request_repaint();
                                        });
                                        ui.close_menu();
                                    }
                                });
                            }
                        }
                    });
                }

                if !other.is_empty() {
                    ui.add_space(4.0);
                    let type_order: &[&str] = &["xsf","xaf","xrf","xmf","xcf","xof","xml"];
                    let mut grouped: Vec<&ManifestEntry> = Vec::new();
                    for ext in type_order {
                        for e in &other { if e.name.to_lowercase().ends_with(ext) { grouped.push(e); } }
                    }
                    for e in &other {
                        let ext = e.name.rsplit('.').next().unwrap_or("").to_lowercase();
                        if !type_order.contains(&ext.as_str()) { grouped.push(e); }
                    }
                    let header_id = ui.id().with(("files", rev.number));
                    egui::collapsing_header::CollapsingState::load_with_default_open(
                        ui.ctx(), header_id, false
                    )
                    .show_header(ui, |ui| {
                        ui.label(egui::RichText::new(
                            format!("{} other files", other.len())
                        ).size(11.0).color(muted));
                    })
                    .body(|ui| {
                        ui.add_space(4.0);
                        for entry in &grouped {
                            let ext = entry.name.rsplit('.').next().unwrap_or("").to_lowercase();
                            let color = match ext.as_str() {
                                "xsf"         => egui::Color32::from_rgb(220, 140, 60),
                                "xaf"         => egui::Color32::from_rgb(200, 110, 50),
                                "xrf"         => egui::Color32::from_rgb(100, 160, 220),
                                "xmf"         => egui::Color32::from_rgb(80,  130, 200),
                                "xcf" | "xof" => egui::Color32::from_rgb(140, 100, 200),
                                "xml"         => egui::Color32::from_rgb(100, 190, 130),
                                _             => egui::Color32::from_rgb(110, 90, 100),
                            };
                            let decoded = percent_decode(&entry.name);
                            let resp = egui::Frame::none()
                                .fill(egui::Color32::from_rgb(26, 15, 22))
                                .rounding(egui::Rounding::same(3.0))
                                .inner_margin(egui::Margin::symmetric(5.0, 2.0))
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(&decoded).size(10.0).color(color));
                                });
                            if decoded != entry.name {
                                resp.response.on_hover_text(&entry.name);
                            }
                            ui.add_space(2.0);
                        }
                    });
                }
            });
    }

    fn start_scan(&mut self, ctx: &egui::Context) {
        let pid = match extract_pid(self.input.trim()) {
            Some(p) => p,
            None => {
                *self.state.lock().unwrap() = State::Error("couldn't find a product ID".into());
                return;
            }
        };
        self.pid = Some(pid);
        *self.state.lock().unwrap() = State::Scanning;
        *self.revisions.lock().unwrap() = Vec::new();
        self.textures.lock().unwrap().clear();
        self.pending_tex.lock().unwrap().clear();
        *self.single_dl.lock().unwrap() = None;
        *self.product_info.lock().unwrap() = None;
        *self.log.lock().unwrap() = format!("scanning pid {}...", pid);

        let state        = Arc::clone(&self.state);
        let revs         = Arc::clone(&self.revisions);
        let log          = Arc::clone(&self.log);
        let ctx          = ctx.clone();
        let batch        = self.settings.scan_batch;
        let start_rev    = self.settings.scan_start_rev;
        let max_rev      = self.settings.scan_max_rev;
        let product_info = Arc::clone(&self.product_info);

        thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build().unwrap();

            {
                let pi   = Arc::clone(&product_info);
                let ctx2 = ctx.clone();
                let c    = client.clone();
                thread::spawn(move || {
                    let url = format!("https://api.imvu.com/product/product-{}", pid);
                    if let Ok(resp) = c.get(&url).send() {
                        if let Ok(json) = resp.json::<serde_json::Value>() {
                            let key = format!("https://api.imvu.com/product/product-{}", pid);
                            let entry = &json["denormalized"][&key];
                            if !entry.is_null() {
                                let data = &entry["data"];
                                let categories: Vec<String> = data["categories"]
                                    .as_array()
                                    .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                    .unwrap_or_default();
                                let parent_id = entry["relations"]["parent"]
                                    .as_str()
                                    .and_then(|s| s.split("product-").last())
                                    .and_then(|s| s.parse::<u64>().ok());
                                // fetch parent name if there is one
                                let parent_name = if let Some(ppid) = parent_id {
                                    let purl = format!("https://api.imvu.com/product/product-{}", ppid);
                                    let pkey = format!("https://api.imvu.com/product/product-{}", ppid);
                                    c.get(&purl).send()
                                        .ok()
                                        .and_then(|r| r.json::<serde_json::Value>().ok())
                                        .and_then(|j| j["denormalized"][&pkey]["data"]["product_name"]
                                            .as_str().map(String::from))
                                        .unwrap_or_default()
                                } else { String::new() };
                                let allows_derivation = data["allows_derivation"]
                                    .as_u64().unwrap_or(0) != 0;
                                let info = ProductInfo {
                                    name:               data["product_name"].as_str().unwrap_or("Unknown").to_string(),
                                    creator:            data["creator_name"].as_str().unwrap_or("").to_string(),
                                    creator_cid:        data["creator_cid"].as_u64().unwrap_or(0),
                                    rating:             data["rating"].as_str().unwrap_or("").to_string(),
                                    shop_url:           format!("https://www.imvu.com/shop/product.php?products_id={}", pid),
                                    product_image:      data["product_image"].as_str().unwrap_or("").to_string(),
                                    categories,
                                    parent_id,
                                    parent_name,
                                    allows_derivation,
                                };
                                *pi.lock().unwrap() = Some(info);
                                ctx2.request_repaint();
                            }
                        }
                    }
                });
            }

            let mut found = scan_upward(pid, &client, start_rev, batch, |n| {
                *log.lock().unwrap() = format!("scanning up... rev {}", n);
                ctx.request_repaint();
            });

            if found.is_empty() {
                *log.lock().unwrap() = format!("nothing going up, scanning down from {}...", max_rev);
                ctx.request_repaint();
                found = scan_downward(pid, &client, max_rev, batch, |n| {
                    *log.lock().unwrap() = format!("scanning down... rev {}", n);
                    ctx.request_repaint();
                });
            }

            if found.is_empty() {
                *state.lock().unwrap() = State::Error("no revisions found".into());
                ctx.request_repaint();
                return;
            }

            *log.lock().unwrap() = format!("fetching {} manifest(s)...", found.len());
            ctx.request_repaint();

            let mut result = Vec::new();
            for rev in &found {
                let url = format!(
                    "https://userimages-akm.imvu.com/productdata/{}/{}/_contents.json", pid, rev);
                if let Ok(manifest) = client.get(&url).send()
                    .and_then(|r| r.json::<Vec<ManifestEntry>>()) {
                    result.push(Revision { number: *rev, manifest });
                }
            }

            *revs.lock().unwrap()  = result;
            *state.lock().unwrap() = State::Ready;
            *log.lock().unwrap()   = String::new();
            ctx.request_repaint();
        });
    }

    fn start_download(&self, pid: u64, rev: Revision, mode: DownloadMode,
                      dl_batch: usize, save_dir: PathBuf, ctx: &egui::Context) {
        let state    = Arc::clone(&self.state);
        let log      = Arc::clone(&self.log);
        let progress = Arc::clone(&self.progress);
        let ctx      = ctx.clone();
        let rev_num  = rev.number;
        let dl_mode  = mode.clone();

        *state.lock().unwrap()    = State::Downloading { rev: rev_num, mode: dl_mode.clone() };
        *progress.lock().unwrap() = 0.0;
        *log.lock().unwrap()      = format!("downloading rev {}...", rev_num);

        thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build().unwrap();

            let files: Vec<&ManifestEntry> = match dl_mode {
                DownloadMode::MediaOnly => rev.manifest.iter().filter(|f| is_media(&f.name)).collect(),
                _                      => rev.manifest.iter().collect(),
            };
            let total   = files.len();
            let fix_ext = dl_mode != DownloadMode::Chkn;
            let is_chkn = dl_mode == DownloadMode::Chkn;

            let (out_path, out_folder) = if is_chkn {
                let p = save_dir.join(format!("{}_rev{}.chkn", pid, rev_num));
                (p, None)
            } else {
                let suf = if dl_mode == DownloadMode::MediaOnly { "_media" } else { "_extracted" };
                let folder = save_dir.join(format!("{}_rev{}{}", pid, rev_num, suf));
                if let Err(e) = std::fs::create_dir_all(&folder) {
                    *state.lock().unwrap() = State::Error(format!("mkdir: {}", e));
                    ctx.request_repaint();
                    return;
                }
                (folder.clone(), Some(folder))
            };

            let mut zip_writer = if is_chkn {
                Some(zip::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new())))
            } else { None };
            let opts = zip::write::FileOptions::<()>::default()
                .compression_method(zip::CompressionMethod::Deflated);

            use std::sync::mpsc;
            let mut done = 0usize;

            for chunk in files.chunks(dl_batch) {
                let (tx, rx) = mpsc::channel::<(String, String, Vec<u8>)>();
                for entry in chunk {
                    let file_url = entry.url.as_deref().unwrap_or(&entry.name);
                    let url = format!("https://userimages-akm.imvu.com/productdata/{}/{}/{}",
                        pid, rev_num, file_url);
                    let tx = tx.clone(); let name = entry.name.clone(); let client = client.clone();
                    thread::spawn(move || {
                        if let Ok(resp) = client.get(&url).send() {
                            let mime = resp.headers().get("content-type")
                                .and_then(|v| v.to_str().ok()).unwrap_or("")
                                .split(';').next().unwrap_or("").trim().to_string();
                            if let Ok(bytes) = resp.bytes() {
                                let _ = tx.send((name, mime, bytes.to_vec()));
                            }
                        }
                    });
                }
                drop(tx);
                for (name, mime, bytes) in rx {
                    let out_name = if fix_ext { fix_extension_mime(&name, &mime, &bytes) } else { name };
                    if let Some(ref mut zw) = zip_writer {
                        if zw.start_file(&out_name, opts).is_ok() { let _ = zw.write_all(&bytes); }
                    } else if let Some(ref folder) = out_folder {
                        let _ = std::fs::write(folder.join(&out_name), &bytes);
                    }
                    done += 1;
                    *progress.lock().unwrap() = done as f32 / total as f32;
                    *log.lock().unwrap() = format!("downloaded {} / {}...", done, total);
                    ctx.request_repaint();
                }
            }

            if let Some(zw) = zip_writer {
                match zw.finish() {
                    Ok(cursor) => {
                        match std::fs::write(&out_path, cursor.into_inner()) {
                            Ok(_)  => *state.lock().unwrap() = State::Done {
                                path: out_path.clone(),
                                label: out_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                            },
                            Err(e) => *state.lock().unwrap() = State::Error(format!("save: {}", e)),
                        }
                    }
                    Err(e) => *state.lock().unwrap() = State::Error(format!("zip: {}", e)),
                }
            } else {
                *state.lock().unwrap() = State::Done {
                    label: out_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                    path: out_path,
                };
            }
            ctx.request_repaint();
        });
    }
}

fn placeholder(ui: &mut egui::Ui, glyph: &str) -> Option<egui::Response> {
    let (r, resp) = ui.allocate_exact_size(egui::vec2(64.0, 64.0), egui::Sense::click());
    ui.painter().rect_filled(r, 4.0, egui::Color32::from_rgb(28, 15, 22));
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph,
        egui::FontId::proportional(20.0), egui::Color32::from_rgb(70, 45, 60));
    Some(resp)
}

fn head_ok(client: &reqwest::blocking::Client, pid: u64, rev: u32) -> bool {
    let url = format!("https://userimages-akm.imvu.com/productdata/{}/{}/_contents.json", pid, rev);
    client.head(&url).send().map(|r| r.status().is_success()).unwrap_or(false)
}

fn scan_upward(pid: u64, client: &reqwest::blocking::Client, start: u32,
               batch: usize, mut on_progress: impl FnMut(u32)) -> Vec<u32> {
    use std::sync::mpsc;
    let mut found = Vec::new();
    let mut cur = start;
    loop {
        let (tx, rx) = mpsc::channel();
        for i in 0..batch {
            let rev = cur + i as u32;
            let tx = tx.clone(); let client = client.clone();
            thread::spawn(move || { let _ = tx.send((rev, head_ok(&client, pid, rev))); });
        }
        drop(tx);
        let mut results: Vec<(u32, bool)> = rx.into_iter().collect();
        results.sort_by_key(|r| r.0);
        let mut any = false;
        for (rev, ok) in results { if ok { found.push(rev); any = true; } }
        on_progress(cur + batch as u32 - 1);
        cur += batch as u32;
        if !any { break; }
    }
    found.sort(); found
}

fn scan_downward(pid: u64, client: &reqwest::blocking::Client, max_rev: u32,
                 batch: usize, mut on_progress: impl FnMut(u32)) -> Vec<u32> {
    use std::sync::mpsc;
    let mut found = Vec::new();
    let mut cur = max_rev as i64;
    loop {
        if cur < 1 { break; }
        let (tx, rx) = mpsc::channel();
        for i in 0..batch {
            let rev = (cur as u32).saturating_sub(i as u32);
            if rev < 1 { break; }
            let tx = tx.clone(); let client = client.clone();
            thread::spawn(move || { let _ = tx.send((rev, head_ok(&client, pid, rev))); });
        }
        drop(tx);
        let mut results: Vec<(u32, bool)> = rx.into_iter().collect();
        results.sort_by_key(|r| r.0);
        let mut any = false;
        for (rev, ok) in results { if ok { found.push(rev); any = true; } }
        on_progress(cur as u32);
        cur -= batch as i64;
        if !any { break; }
    }
    found.sort(); found
}

fn extract_pid(s: &str) -> Option<u64> {
    if let Some(m) = s.split("products_id=").nth(1) {
        let d: String = m.chars().take_while(|c| c.is_ascii_digit()).collect();
        return d.parse().ok();
    }
    s.trim().parse().ok()
}

const MEDIA_EXTS: &[&str] = &["png","jpg","jpeg","gif","bmp","dds","tga","webp","tiff","tif"];
const MESH_EXTS:  &[&str] = &["xsf","xaf","xrf","xmf","xcf","xof"];

fn is_media(name: &str) -> bool {
    MEDIA_EXTS.contains(&name.rsplit('.').next().unwrap_or("").to_lowercase().as_str())
}
fn is_mesh(name: &str) -> bool {
    MESH_EXTS.contains(&name.rsplit('.').next().unwrap_or("").to_lowercase().as_str())
}

fn fix_extension_mime(name: &str, mime: &str, bytes: &[u8]) -> String {
    let ext_from_mime = match mime {
        "image/png"                      => Some("png"),
        "image/jpeg" | "image/jpg"       => Some("jpg"),
        "image/gif"                      => Some("gif"),
        "image/bmp"                      => Some("bmp"),
        "image/webp"                     => Some("webp"),
        "image/tiff"                     => Some("tiff"),
        "image/x-tga" | "image/x-targa" => Some("tga"),
        _ => None,
    };
    let real_ext = ext_from_mime.or_else(|| {
        if is_media(name) { detect_ext(bytes) } else { None }
    });
    if let Some(real) = real_ext {
        let declared = name.rsplit('.').next().unwrap_or("").to_lowercase();
        if real != declared {
            let stem = name.rsplitn(2, '.').last().unwrap_or(name);
            return format!("{}.{}", stem, real);
        }
    }
    name.to_string()
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn detect_ext(b: &[u8]) -> Option<&'static str> {
    if b.len() < 4 { return None; }
    if b[0]==0x89 && b[1]==0x50 && b[2]==0x4e && b[3]==0x47 { return Some("png"); }
    if b[0]==0xff && b[1]==0xd8 && b[2]==0xff               { return Some("jpg"); }
    if b[0]==0x47 && b[1]==0x49 && b[2]==0x46               { return Some("gif"); }
    if b[0]==0x42 && b[1]==0x4d                              { return Some("bmp"); }
    if b[0]==0x44 && b[1]==0x44 && b[2]==0x53 && b[3]==0x20 { return Some("dds"); }
    if b.len() >= 12 && b[0]==0x52 && b[1]==0x49 && b[2]==0x46 && b[3]==0x46
        && b[8]==0x57 && b[9]==0x45 && b[10]==0x42 && b[11]==0x50 { return Some("webp"); }
    None
}

fn default_download_dir() -> PathBuf {
    dirs::download_dir().or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("."))
}

fn main() -> eframe::Result<()> {
    eframe::run_native(
        "CHKN Downloader",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("CHKN Downloader")
                .with_inner_size([820.0, 640.0])
                .with_resizable(true)
                .with_icon({
                    let img = image::load_from_memory(ICON_PNG).unwrap().to_rgba8();
                    let (w, h) = img.dimensions();
                    egui::IconData { rgba: img.into_raw(), width: w, height: h }
                }),
            ..Default::default()
        },
        Box::new(|cc| {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("inter".into(), egui::FontData::from_static(FONT_INTER));
            fonts.font_data.insert("jbmono".into(), egui::FontData::from_static(FONT_BYTES));
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "inter".into());
            fonts.families.entry(egui::FontFamily::Proportional).or_default().push("jbmono".into());
            fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "jbmono".into());
            cc.egui_ctx.set_fonts(fonts);
            Box::new(App::default())
        }),
    )
}