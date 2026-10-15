#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod free_translate;
mod layout;
mod net;
mod screen_clip;
mod secret_store;
mod volcengine;
mod windows_ocr;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use eframe::egui::{self, Color32, RichText, Stroke};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const CONFIG_FILE: &str = "config.json";
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const DEFAULT_FONT_SIZE: u8 = 13;
const FONT_SIZE_RANGE: std::ops::RangeInclusive<u8> = 12..=22;
const TARGET_LANGUAGES: [&str; 9] = [
    "简体中文",
    "繁体中文",
    "英语",
    "日语",
    "韩语",
    "法语",
    "西班牙语",
    "俄语",
    "德语",
];

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TranslationMode {
    #[default]
    Tencent,
    Google,
    Volcengine,
    Vision,
}

impl TranslationMode {
    fn label(self) -> &'static str {
        match self {
            Self::Tencent => "腾讯交互翻译（免 Key）",
            Self::Google => "Google 翻译（免 Key，可能限流）",
            Self::Volcengine => "火山机器翻译（AK/SK）",
            Self::Vision => "自定义视觉模型",
        }
    }

    fn short_label(self) -> &'static str {
        match self {
            Self::Tencent => "腾讯翻译",
            Self::Google => "Google 翻译",
            Self::Volcengine => "火山翻译",
            Self::Vision => "视觉模型",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ThemeMode {
    Day,
    #[default]
    Night,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    mode: TranslationMode,
    theme: ThemeMode,
    font_size: u8,
    ocr_language: String,
    api_url: String,
    api_key: String,
    model: String,
    target_language: String,
    shortcut: String,
    proxy_mode: net::ProxyMode,
    proxy_url: String,
    volcengine_access_key: String,
    volcengine_secret_key: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: TranslationMode::Tencent,
            theme: ThemeMode::Night,
            font_size: DEFAULT_FONT_SIZE,
            ocr_language: "en-US".into(),
            api_url: "https://api.openai.com/v1/chat/completions".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            target_language: "简体中文".into(),
            shortcut: "Ctrl+Shift+T".into(),
            proxy_mode: net::ProxyMode::System,
            proxy_url: "http://127.0.0.1:7890".into(),
            volcengine_access_key: String::new(),
            volcengine_secret_key: String::new(),
        }
    }
}

impl AppConfig {
    fn proxy(&self) -> net::ProxySetting {
        net::ProxySetting {
            mode: self.proxy_mode,
            url: self.proxy_url.trim().to_owned(),
        }
    }

    fn secrets_mut(&mut self) -> [&mut String; 3] {
        [
            &mut self.api_key,
            &mut self.volcengine_access_key,
            &mut self.volcengine_secret_key,
        ]
    }

    /// A copy for writing to disk, with every non-empty secret encrypted.
    fn protected(&self) -> Result<Self, String> {
        let mut stored = self.clone();
        for secret in stored.secrets_mut() {
            if !secret.is_empty() {
                *secret = secret_store::protect(secret)?;
            }
        }
        Ok(stored)
    }

    /// Decrypts secrets read from disk. Returns whether plaintext secrets saved by an older
    /// version were found, and whether any encrypted secret could not be decrypted.
    fn reveal_secrets(&mut self) -> (bool, bool) {
        let (mut plaintext, mut undecryptable) = (false, false);
        for secret in self.secrets_mut() {
            if secret_store::is_protected(secret) {
                match secret_store::unprotect(secret) {
                    Ok(plain) => *secret = plain,
                    Err(_) => {
                        secret.clear();
                        undecryptable = true;
                    }
                }
            } else if !secret.is_empty() {
                plaintext = true;
            }
        }
        (plaintext, undecryptable)
    }
}

const POPUP_WIDTH: f32 = 380.0;
const POPUP_MAX_HEIGHT: f32 = 420.0;
const POPUP_MARGIN: i8 = 14;

/// The floating result window shown next to a hotkey screen capture.
struct ResultPopup {
    /// Where the selection ended and the work area of that monitor, in physical pixels.
    anchor: [i32; 2],
    work_area: [i32; 4],
    opened_at: Instant,
    pinned: bool,
    show_source: bool,
    copied: bool,
    /// Physical rectangle last requested for the window, to tell clicks inside from outside.
    placed: Option<egui::Rect>,
    width: f32,
    height: f32,
    /// Image view (translation painted over the capture) or text view; decided per result.
    image_view: Option<bool>,
    /// The capture uploaded for the image view, keyed by the layout it came from.
    texture: Option<(usize, egui::TextureHandle)>,
    escape_was_down: bool,
    click_was_down: bool,
}

impl ResultPopup {
    fn at_cursor() -> Self {
        let anchor = screen_clip::cursor_position();
        Self {
            anchor,
            work_area: screen_clip::work_area_at(anchor),
            opened_at: Instant::now(),
            pinned: false,
            show_source: false,
            copied: false,
            placed: None,
            width: POPUP_WIDTH,
            height: 120.0,
            image_view: None,
            texture: None,
            escape_was_down: screen_clip::escape_down(),
            click_was_down: screen_clip::mouse_button_down(),
        }
    }

    /// Top-left corner for a window of `size` physical pixels: below and right of the anchor,
    /// flipped to the other side when it would leave the monitor's work area.
    fn place(&self, size: egui::Vec2) -> egui::Pos2 {
        const GAP: f32 = 16.0;
        let [left, top, right, bottom] = self.work_area.map(|value| value as f32);
        let [x, y] = self.anchor.map(|value| value as f32);
        let mut corner = egui::pos2(x + GAP, y + GAP);
        if corner.x + size.x > right {
            corner.x = x - GAP - size.x;
        }
        if corner.y + size.y > bottom {
            corner.y = y - GAP - size.y;
        }
        egui::pos2(
            corner.x.clamp(left, (right - size.x).max(left)),
            corner.y.clamp(top, (bottom - size.y).max(top)),
        )
    }

    /// Esc, or a click outside the window once the snip's own mouse release has passed, closes
    /// an unpinned popup. Polled globally because the popup does not take keyboard focus.
    fn dismissed(&mut self) -> bool {
        let escape = screen_clip::escape_down();
        let click = screen_clip::mouse_button_down();
        let escape_pressed = escape && !self.escape_was_down;
        let click_pressed = click && !self.click_was_down;
        self.escape_was_down = escape;
        self.click_was_down = click;
        if self.pinned {
            return false;
        }
        if escape_pressed {
            return true;
        }
        if click_pressed && self.opened_at.elapsed() > Duration::from_millis(400) {
            let [x, y] = screen_clip::cursor_position();
            let cursor = egui::pos2(x as f32, y as f32);
            return !self.placed.is_some_and(|rect| rect.contains(cursor));
        }
        false
    }
}

#[derive(Default, Clone)]
struct TranslationResult {
    source_text: String,
    translated_text: String,
    /// Block positions and translations for painting the result over the screenshot.
    layout: Option<Arc<layout::ImageLayout>>,
}

enum RequestResult {
    Source(String),
    Success(TranslationResult),
    Error(String),
}

struct OcrTranslateApp {
    config: AppConfig,
    config_path: PathBuf,
    result: TranslationResult,
    selected_file: Option<PathBuf>,
    temporary_capture: Option<PathBuf>,
    request_rx: Option<Receiver<RequestResult>>,
    capture_rx: Option<Receiver<Result<PathBuf, String>>>,
    hotkey: screen_clip::HotkeyListener,
    active_hotkey: Option<screen_clip::Hotkey>,
    shortcut_draft: screen_clip::Hotkey,
    hotkey_capture_active: bool,
    paste_key_down: bool,
    popup: Option<ResultPopup>,
    volcengine_usage: Option<Result<volcengine::Usage, String>>,
    usage_rx: Option<Receiver<Result<volcengine::Usage, String>>>,
    status: String,
    settings_open: bool,
    show_api_key: bool,
    show_volcengine_keys: bool,
    ocr_languages: Vec<String>,
}

impl OcrTranslateApp {
    fn new(ctx: &egui::Context) -> Self {
        let config_path = config_path();
        let config_existed = config_path.exists();
        let mut config = load_config(&config_path);
        let (plaintext_secrets, undecryptable_secrets) = config.reveal_secrets();
        config.font_size = config
            .font_size
            .clamp(*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end());
        let shortcut = screen_clip::Hotkey::parse(&config.shortcut).unwrap_or_default();
        config.shortcut = shortcut.to_string();
        apply_appearance(ctx, &config);
        net::set_proxy(config.proxy());
        thread::spawn(screen_clip::remove_stale_captures);
        let ocr_languages = windows_ocr::available_languages();
        if config.ocr_language != "auto"
            && !ocr_languages.iter().any(|tag| tag == &config.ocr_language)
        {
            config.ocr_language = "auto".into();
        }
        let mut app = Self {
            config,
            config_path,
            result: TranslationResult::default(),
            selected_file: None,
            temporary_capture: None,
            request_rx: None,
            capture_rx: None,
            hotkey: screen_clip::listen_for_hotkey(ctx.clone(), shortcut),
            active_hotkey: None,
            shortcut_draft: shortcut,
            hotkey_capture_active: false,
            paste_key_down: false,
            popup: None,
            volcengine_usage: None,
            usage_rx: None,
            status: format!("准备就绪。按 {shortcut} 框选屏幕。"),
            // First run lands on settings; afterwards go straight to translating.
            settings_open: !config_existed,
            show_api_key: false,
            show_volcengine_keys: false,
            ocr_languages,
        };
        if undecryptable_secrets {
            app.status =
                "已保存的密钥无法解密（可能来自其他电脑或 Windows 账户），请在设置中重新填写。"
                    .into();
        } else if plaintext_secrets {
            // Re-save right away so keys written in plaintext by older versions get encrypted.
            if let Err(error) = app.write_config() {
                app.status = error;
            }
        }
        app
    }

    fn busy(&self) -> bool {
        self.request_rx.is_some() || self.capture_rx.is_some() || self.hotkey_capture_active
    }

    fn shortcut_label(&self) -> String {
        self.active_hotkey
            .map_or_else(|| "全局快捷键".into(), |hotkey| hotkey.to_string())
    }

    fn write_config(&self) -> Result<(), String> {
        const FAILED: &str = "设置保存失败，请检查应用数据目录权限。";
        let json = serde_json::to_string_pretty(&self.config.protected()?)
            .map_err(|_| FAILED.to_string())?;
        if let Some(directory) = self.config_path.parent() {
            fs::create_dir_all(directory).map_err(|_| FAILED.to_string())?;
        }
        fs::write(&self.config_path, json).map_err(|_| FAILED.to_string())
    }

    fn save_config(&mut self) {
        if let Err(error) = self.shortcut_draft.validate() {
            self.status = error;
            return;
        }
        if self.config.proxy_mode == net::ProxyMode::Custom {
            if let Err(error) = net::validate_proxy_url(&self.config.proxy_url) {
                self.status = error;
                return;
            }
        }
        net::set_proxy(self.config.proxy());
        self.config.shortcut = self.shortcut_draft.to_string();
        self.status = match self.write_config() {
            Ok(()) => "设置已保存。".into(),
            Err(error) => error,
        };
        if self.active_hotkey != Some(self.shortcut_draft) {
            if let Err(error) = self.hotkey.change(self.shortcut_draft) {
                self.status = error;
            }
        }
    }

    fn choose_image(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp"])
            .pick_file();
        if let Some(path) = picked {
            if let Some(previous) = self.temporary_capture.take() {
                let _ = fs::remove_file(previous);
            }
            self.selected_file = Some(path.clone());
            self.start_translation(path, ctx);
        }
    }

    fn start_screen_capture(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.status = "请在屏幕上拖动鼠标，框选需要翻译的区域。按 Esc 取消。".into();
        let (tx, rx) = mpsc::channel();
        self.capture_rx = Some(rx);
        self.hotkey.set_busy(true);
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(250));
            let _ = tx.send(screen_clip::capture_region(None));
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }

    fn poll_screen_capture(&mut self, ctx: &egui::Context) {
        let received = self.capture_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(Ok(path))) => {
                self.capture_rx = None;
                self.accept_capture(path, true, ctx);
            }
            Some(Ok(Err(error))) => {
                self.capture_rx = None;
                self.status = error;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.capture_rx = None;
                self.status = "截图任务意外结束。".into();
            }
            _ => {
                if self.capture_rx.is_some() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }
    }

    fn poll_hotkey(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.hotkey.events.try_recv() {
            match event {
                screen_clip::HotkeyEvent::Ready(hotkey) => {
                    let previous = self.active_hotkey.replace(hotkey);
                    if previous.is_some_and(|previous| previous != hotkey) {
                        self.status = format!("全局快捷键已更新为 {hotkey}。");
                    }
                }
                screen_clip::HotkeyEvent::Busy => {
                    self.status = "上一项截图或翻译还在进行，请完成后再按快捷键。".into();
                    if self.popup.is_none() {
                        self.popup = Some(ResultPopup::at_cursor());
                    }
                }
                screen_clip::HotkeyEvent::Started => {
                    self.hotkey_capture_active = true;
                    self.popup = None;
                    self.status = "请在屏幕上拖动鼠标，框选需要翻译的区域。按 Esc 取消。".into();
                }
                screen_clip::HotkeyEvent::Captured(Ok(path)) => {
                    self.hotkey_capture_active = false;
                    // The selection ends where the mouse was released, so open the popup there.
                    self.popup = Some(ResultPopup::at_cursor());
                    self.accept_capture(path, false, ctx);
                }
                screen_clip::HotkeyEvent::Captured(Err(error)) => {
                    self.hotkey_capture_active = false;
                    if error != screen_clip::CANCELLED {
                        self.popup = Some(ResultPopup::at_cursor());
                    }
                    self.status = error;
                }
                screen_clip::HotkeyEvent::Failed { message, active } => {
                    self.status = message;
                    self.active_hotkey = active;
                    // The listener fell back to the previous shortcut; keep the saved setting
                    // in step so the next launch does not retry the rejected one.
                    if let Some(active) = active {
                        if self.config.shortcut != active.to_string() {
                            self.config.shortcut = active.to_string();
                            self.shortcut_draft = active;
                            let _ = self.write_config();
                        }
                    }
                }
            }
        }
    }

    /// Hotkey captures leave the main window alone and show their result in the popup.
    fn accept_capture(&mut self, path: PathBuf, reveal_main_window: bool, ctx: &egui::Context) {
        if let Some(previous) = self.temporary_capture.replace(path.clone()) {
            let _ = fs::remove_file(previous);
        }
        self.selected_file = Some(path.clone());
        self.settings_open = false;
        if reveal_main_window {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.start_translation(path, ctx);
    }

    fn paste_image(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        match screen_clip::paste_clipboard_image() {
            Ok(Some(path)) => self.accept_capture(path, true, ctx),
            Ok(None) => {}
            Err(error) => self.status = error,
        }
    }

    /// Returns false and opens the settings page when the selected channel lacks credentials.
    fn ensure_credentials(&mut self) -> bool {
        let config = &self.config;
        let missing = match config.mode {
            TranslationMode::Vision if config.api_key.trim().is_empty() => {
                Some("请先填写 API Key。")
            }
            TranslationMode::Vision
                if config.api_url.trim().is_empty() || config.model.trim().is_empty() =>
            {
                Some("请先填写 API URL 和模型名称。")
            }
            TranslationMode::Volcengine => {
                let (access_key, secret_key) = volcengine_credentials(config);
                (access_key.trim().is_empty() || secret_key.trim().is_empty())
                    .then_some("请先在设置中填写火山引擎 Access Key 和 Secret Key。")
            }
            _ => None,
        };
        if let Some(message) = missing {
            self.status = message.into();
            self.settings_open = true;
        }
        missing.is_none()
    }

    fn start_translation(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !self.ensure_credentials() {
            return;
        }

        let config = self.config.clone();
        let (tx, rx) = mpsc::channel();
        self.request_rx = Some(rx);
        self.result = TranslationResult::default();
        self.status = "正在识别并翻译…".into();
        ctx.request_repaint();

        thread::spawn(move || {
            let result = (|| {
                if config.mode == TranslationMode::Vision {
                    return translate_image(&config, &path);
                }
                validate_image_size(&path)?;
                let lines = windows_ocr::recognize_lines(&path, &config.ocr_language)?;
                let mut page = layout::ImageLayout::build(&path, lines)?;
                let source = page.source_text();
                let _ = tx.send(RequestResult::Source(source.clone()));
                // Each block is translated on its own so the result maps back onto the image.
                let indexes = page.translatable();
                let texts: Vec<String> = indexes
                    .iter()
                    .map(|&index| page.blocks[index].source.clone())
                    .collect();
                for (index, translated) in indexes.into_iter().zip(translate_many(&config, &texts)?)
                {
                    page.blocks[index].translated = Some(translated);
                }
                Ok(TranslationResult {
                    source_text: source,
                    translated_text: page.translated_text(),
                    layout: Some(Arc::new(page)),
                })
            })()
            .map(RequestResult::Success)
            .unwrap_or_else(RequestResult::Error);
            let _ = tx.send(result);
        });
    }

    fn start_text_translation(&mut self, ctx: &egui::Context) {
        let source = self.result.source_text.trim().to_owned();
        if source.is_empty() {
            self.status = "请先在原文框输入或粘贴文字。".into();
            return;
        }
        if !self.ensure_credentials() {
            return;
        }

        let config = self.config.clone();
        let (tx, rx) = mpsc::channel();
        self.request_rx = Some(rx);
        self.result.translated_text.clear();
        self.status = "正在翻译…".into();
        ctx.request_repaint();

        thread::spawn(move || {
            let result = (|| {
                let translated = translate_text(&config, &source)?;
                Ok(TranslationResult {
                    source_text: source,
                    translated_text: translated,
                    layout: None,
                })
            })()
            .map(RequestResult::Success)
            .unwrap_or_else(RequestResult::Error);
            let _ = tx.send(result);
        });
    }

    fn poll_request(&mut self, ctx: &egui::Context) {
        let received = self.request_rx.as_ref().map(|rx| rx.try_recv());
        if let Some(message) = received {
            match message {
                Ok(RequestResult::Source(source)) => {
                    self.result.source_text = source;
                    self.status = "识别完成，正在翻译…".into();
                }
                Ok(RequestResult::Success(result)) => {
                    self.request_rx = None;
                    self.result = result;
                    self.status = "翻译完成。".into();
                }
                Ok(RequestResult::Error(error)) => {
                    self.request_rx = None;
                    self.status = error;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.request_rx = None;
                    self.status = "翻译任务意外结束。".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            ctx.request_repaint();
        } else if self.request_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        let palette = Palette::of(self.config.theme);
        let width = ui.available_width().min(680.0);
        ui.horizontal(|ui| {
            ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.add_space(4.0);
                self.draw_appearance_card(ui, &palette);
                ui.add_space(14.0);
                self.draw_channel_card(ui, &palette);
                ui.add_space(14.0);
                self.draw_shortcut_card(ui, &palette);
                ui.add_space(14.0);
                self.draw_proxy_card(ui, &palette);
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    if ui.add(primary_button(&palette, "保存设置")).clicked() {
                        self.save_config();
                    }
                    ui.add_space(8.0);
                    hint(
                        ui,
                        &palette,
                        "配置保存在当前用户的应用数据目录，密钥已加密。",
                    );
                });
                ui.add_space(12.0);
            });
        });
    }

    fn draw_appearance_card(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        card(ui, palette, |ui| {
            section_title(
                ui,
                palette,
                "外观",
                "主题与字号会立即预览，保存后下次启动仍然生效。",
            );
            egui::Grid::new("appearance")
                .num_columns(2)
                .spacing([24.0, 14.0])
                .show(ui, |ui| {
                    field_label(ui, palette, "主题");
                    if segmented(
                        ui,
                        palette,
                        &mut self.config.theme,
                        &[(ThemeMode::Day, "日间"), (ThemeMode::Night, "夜间")],
                    ) {
                        apply_appearance(ui.ctx(), &self.config);
                    }
                    ui.end_row();

                    field_label(ui, palette, "字号");
                    if ui
                        .add(
                            egui::Slider::new(&mut self.config.font_size, FONT_SIZE_RANGE)
                                .suffix(" px"),
                        )
                        .changed()
                    {
                        apply_appearance(ui.ctx(), &self.config);
                    }
                    ui.end_row();
                });
        });
    }

    fn draw_channel_card(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        card(ui, palette, |ui| {
            section_title(ui, palette, "翻译", "选择翻译渠道和语言。");
            field_label(ui, palette, "翻译渠道");
            egui::ComboBox::from_id_salt("translation_mode")
                .width(ui.available_width())
                .selected_text(self.config.mode.label())
                .show_ui(ui, |ui| {
                    for mode in [
                        TranslationMode::Tencent,
                        TranslationMode::Google,
                        TranslationMode::Volcengine,
                        TranslationMode::Vision,
                    ] {
                        ui.selectable_value(&mut self.config.mode, mode, mode.label());
                    }
                });
            match self.config.mode {
                TranslationMode::Tencent | TranslationMode::Google => {
                    hint(
                        ui,
                        palette,
                        "免 Key 渠道使用第三方网页接口，可用性会随服务方变化。",
                    );
                }
                TranslationMode::Volcengine => {
                    let (access_key, secret_key) = volcengine_credentials(&self.config);
                    let configured = !access_key.trim().is_empty() && !secret_key.trim().is_empty();
                    if configured {
                        ui.add_space(8.0);
                        self.draw_volcengine_usage(ui, palette);
                    } else {
                        hint(ui, palette, "火山凭证尚未配置，填写后点击“保存设置”。");
                    }
                }
                TranslationMode::Vision => {}
            }
            ui.add_space(14.0);

            if self.config.mode == TranslationMode::Vision {
                field_label(ui, palette, "API URL");
                ui.add(text_field(&mut self.config.api_url).desired_width(f32::INFINITY));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    field_label(ui, palette, "API Key");
                    ui.checkbox(&mut self.show_api_key, "显示");
                });
                ui.add(
                    text_field(&mut self.config.api_key)
                        .desired_width(f32::INFINITY)
                        .password(!self.show_api_key),
                );
                ui.add_space(10.0);
                field_label(ui, palette, "模型");
                ui.add(text_field(&mut self.config.model).desired_width(f32::INFINITY));
                ui.add_space(10.0);
            } else {
                if self.config.mode == TranslationMode::Volcengine {
                    ui.horizontal(|ui| {
                        field_label(ui, palette, "Access Key (AK)");
                        ui.checkbox(&mut self.show_volcengine_keys, "显示密钥");
                    });
                    ui.add(
                        text_field(&mut self.config.volcengine_access_key)
                            .desired_width(f32::INFINITY)
                            .password(!self.show_volcengine_keys),
                    );
                    ui.add_space(10.0);
                    field_label(ui, palette, "Secret Key (SK)");
                    ui.add(
                        text_field(&mut self.config.volcengine_secret_key)
                            .desired_width(f32::INFINITY)
                            .password(!self.show_volcengine_keys),
                    );
                    hint(
                        ui,
                        palette,
                        "开发环境也兼容 VOLCENGINE_ACCESS_KEY / VOLCENGINE_SECRET_KEY 环境变量。",
                    );
                    ui.add_space(10.0);
                }

                field_label(ui, palette, "原文识别语言");
                egui::ComboBox::from_id_salt("ocr_language")
                    .width(ui.available_width())
                    .selected_text(if self.config.ocr_language == "auto" {
                        "自动（系统语言）"
                    } else {
                        &self.config.ocr_language
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.config.ocr_language,
                            "auto".into(),
                            "自动（系统语言）",
                        );
                        for language in &self.ocr_languages {
                            ui.selectable_value(
                                &mut self.config.ocr_language,
                                language.clone(),
                                language,
                            );
                        }
                    });
                if self.ocr_languages.is_empty() {
                    hint(
                        ui,
                        palette,
                        "未检测到 Windows OCR 语言包，请在系统语言设置中安装。",
                    );
                }
                ui.add_space(10.0);
            }

            field_label(ui, palette, "目标语言");
            if self.config.mode == TranslationMode::Vision {
                ui.add(text_field(&mut self.config.target_language).desired_width(f32::INFINITY));
            } else {
                egui::ComboBox::from_id_salt("target_language")
                    .width(ui.available_width())
                    .selected_text(&self.config.target_language)
                    .show_ui(ui, |ui| {
                        for language in TARGET_LANGUAGES {
                            ui.selectable_value(
                                &mut self.config.target_language,
                                language.into(),
                                language,
                            );
                        }
                    });
            }
        });
    }

    fn refresh_usage(&mut self, ctx: &egui::Context) {
        if self.usage_rx.is_some() {
            return;
        }
        let (access_key, secret_key) = volcengine_credentials(&self.config);
        let (tx, rx) = mpsc::channel();
        self.usage_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let _ = tx.send(volcengine::query_usage(&access_key, &secret_key));
            ctx.request_repaint();
        });
    }

    fn poll_usage(&mut self, ctx: &egui::Context) {
        let received = self.usage_rx.as_ref().map(|rx| rx.try_recv());
        match received {
            Some(Ok(result)) => {
                self.usage_rx = None;
                self.volcengine_usage = Some(result);
            }
            Some(Err(mpsc::TryRecvError::Disconnected)) => {
                self.usage_rx = None;
                self.volcengine_usage = Some(Err("费用查询意外结束。".into()));
            }
            Some(Err(mpsc::TryRecvError::Empty)) => {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
            None => {}
        }
    }

    /// Free-tier usage this month and the account balance, queried once when first shown.
    fn draw_volcengine_usage(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        if self.volcengine_usage.is_none() && self.usage_rx.is_none() {
            self.refresh_usage(ui.ctx());
        }
        egui::Frame::new()
            .fill(palette.input)
            .corner_radius(8u8)
            .inner_margin(12i8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("本月免费额度").strong().color(palette.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.usage_rx.is_some() {
                            ui.add(egui::Spinner::new().size(12.0).color(palette.accent));
                        } else if ui
                            .add(
                                egui::Button::new(RichText::new("刷新").color(palette.accent))
                                    .frame(false),
                            )
                            .clicked()
                        {
                            self.refresh_usage(ui.ctx());
                        }
                    });
                });
                match &self.volcengine_usage {
                    None => hint(ui, palette, "正在查询…"),
                    Some(Err(error)) => {
                        ui.label(RichText::new(error).small().color(palette.danger));
                        hint(ui, palette, "查询用量需要该 AK 拥有费用中心的读取权限。");
                    }
                    Some(Ok(usage)) => {
                        let total = volcengine::FREE_CHARS_PER_MONTH;
                        let fraction = (usage.used_chars / total).clamp(0.0, 1.0) as f32;
                        let remaining = (total - usage.used_chars).max(0.0);
                        ui.label(
                            RichText::new(format!(
                                "已用 {} / {} 字符，剩余 {}",
                                format_chars(usage.used_chars),
                                format_chars(total),
                                format_chars(remaining)
                            ))
                            .color(palette.text),
                        );
                        let color = if fraction >= 0.9 {
                            palette.danger
                        } else {
                            palette.accent
                        };
                        // Drawn by hand: the built-in bar's track matches this panel's fill.
                        let (track, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 6.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().rect_filled(track, 3.0, palette.border);
                        let mut filled = track;
                        filled.set_width((track.width() * fraction).max(6.0));
                        ui.painter().rect_filled(filled, 3.0, color);
                        if usage.used_chars > total {
                            ui.label(
                                RichText::new("免费额度已用完，超出部分按量从余额扣费。")
                                    .small()
                                    .color(palette.danger),
                            );
                        }
                        let currency = if usage.currency == "CNY" {
                            "元"
                        } else {
                            usage.currency.as_str()
                        };
                        ui.label(
                            RichText::new(format!("账户可用余额：{} {currency}", usage.balance))
                                .color(palette.text),
                        );
                        hint(
                            ui,
                            palette,
                            "数据来自火山账单，可能延迟数小时；按整个账号统计。",
                        );
                    }
                }
            });
    }

    fn draw_proxy_card(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        card(ui, palette, |ui| {
            section_title(ui, palette, "网络代理", "翻译请求使用的代理，保存后生效。");
            segmented(
                ui,
                palette,
                &mut self.config.proxy_mode,
                &[
                    (net::ProxyMode::System, "跟随系统"),
                    (net::ProxyMode::None, "不使用"),
                    (net::ProxyMode::Custom, "自定义"),
                ],
            );
            ui.add_space(4.0);
            match self.config.proxy_mode {
                net::ProxyMode::System => hint(
                    ui,
                    palette,
                    "使用 Windows 设置中的代理；Clash 等工具开启“系统代理”后会自动生效。",
                ),
                net::ProxyMode::None => hint(ui, palette, "所有翻译请求直接连接，不经过代理。"),
                net::ProxyMode::Custom => {
                    ui.add_space(6.0);
                    field_label(ui, palette, "代理地址");
                    ui.add(text_field(&mut self.config.proxy_url).desired_width(f32::INFINITY));
                    match net::validate_proxy_url(&self.config.proxy_url) {
                        Ok(()) => hint(
                            ui,
                            palette,
                            "支持 http:// 和 socks5://，例如 http://127.0.0.1:7890。",
                        ),
                        Err(error) => {
                            ui.label(RichText::new(error).small().color(palette.danger));
                        }
                    }
                }
            }
        });
    }

    fn draw_shortcut_card(&mut self, ui: &mut egui::Ui, palette: &Palette) {
        card(ui, palette, |ui| {
            section_title(
                ui,
                palette,
                "全局快捷键",
                "在任何窗口按下即可框选屏幕并翻译。",
            );
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.shortcut_draft.ctrl, "Ctrl");
                ui.checkbox(&mut self.shortcut_draft.alt, "Alt");
                ui.checkbox(&mut self.shortcut_draft.shift, "Shift");
                ui.add_space(4.0);
                egui::ComboBox::from_id_salt("shortcut_key")
                    .width(72.0)
                    .selected_text(screen_clip::Hotkey::key_name(self.shortcut_draft.key))
                    .show_ui(ui, |ui| {
                        for key in screen_clip::Hotkey::key_choices() {
                            ui.selectable_value(
                                &mut self.shortcut_draft.key,
                                key,
                                screen_clip::Hotkey::key_name(key),
                            );
                        }
                    });
            });
            ui.add_space(4.0);
            match (self.shortcut_draft.validate(), self.active_hotkey) {
                (Err(error), _) => hint(ui, palette, &error),
                (Ok(()), Some(active)) if active != self.shortcut_draft => hint(
                    ui,
                    palette,
                    &format!(
                        "当前生效：{active}。点击“保存设置”后改为 {}。",
                        self.shortcut_draft
                    ),
                ),
                (Ok(()), Some(active)) => hint(ui, palette, &format!("当前生效：{active}")),
                (Ok(()), None) => hint(
                    ui,
                    palette,
                    "当前没有生效的全局快捷键，点击“保存设置”重试注册。",
                ),
            }
        });
    }

    fn draw_result(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let palette = Palette::of(self.config.theme);
        let idle = !self.busy();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(idle, primary_button(&palette, "框选屏幕"))
                .on_hover_text(format!("快捷键：{}", self.shortcut_label()))
                .clicked()
            {
                self.start_screen_capture(ctx);
            }
            if ui.add_enabled(idle, secondary_button("选择图片")).clicked() {
                self.choose_image(ctx);
            }
            if ui
                .add_enabled(idle, secondary_button("翻译原文"))
                .on_hover_text("翻译原文框中输入或修改过的文字")
                .clicked()
            {
                self.start_text_translation(ctx);
            }
            if ui
                .add_enabled(
                    idle && self.selected_file.is_some(),
                    secondary_button("重新翻译"),
                )
                .on_hover_text("对当前图片重新识别并翻译")
                .clicked()
            {
                if let Some(path) = self.selected_file.clone() {
                    self.start_translation(path, ctx);
                }
            }
            if let Some(path) = &self.selected_file {
                let name = if self.temporary_capture.as_ref() == Some(path) {
                    "截图"
                } else {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("已选择图片")
                };
                ui.add_space(6.0);
                ui.label(RichText::new(format!("当前图片：{name}")).color(palette.muted));
            }
        });
        ui.add_space(14.0);

        let paste_released = ui.input(|input| {
            input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::V,
                        pressed: false,
                        modifiers,
                        ..
                    } if modifiers.ctrl
                )
            })
        });
        let paste_keys_down = screen_clip::paste_keys_down();
        let source_hint = format!(
            "输入或粘贴文字，Ctrl+V 可直接粘贴图片\n也可以按 {} 框选屏幕",
            self.shortcut_label()
        );
        let mut source_focused = false;
        let mut clear_source = false;
        let mut copy_translation = false;
        ui.columns(2, |columns| {
            (source_focused, clear_source) = text_card(
                &mut columns[0],
                &palette,
                "原文",
                &mut self.result.source_text,
                &source_hint,
                "清空",
            );
            (_, copy_translation) = text_card(
                &mut columns[1],
                &palette,
                "译文",
                &mut self.result.translated_text,
                "翻译结果会显示在这里",
                "复制",
            );
        });
        if clear_source {
            self.result.source_text.clear();
        }
        if copy_translation {
            ctx.copy_text(self.result.translated_text.clone());
            self.status = "译文已复制到剪贴板。".into();
        }
        if source_focused && ((paste_keys_down || paste_released) && !self.paste_key_down) {
            self.paste_image(ctx);
        }
        self.paste_key_down = paste_keys_down;
    }

    fn show_popup(&mut self, ctx: &egui::Context) {
        let Some(mut popup) = self.popup.take() else {
            return;
        };
        if popup.dismissed() {
            // Not showing the viewport this frame closes its window.
            ctx.request_repaint();
            return;
        }
        let palette = Palette::of(self.config.theme);
        let root_scale = ctx.pixels_per_point();
        let page = self.result.layout.clone().filter(|_| !self.busy());
        if let Some(page) = &page {
            // Scattered text such as cards reads better in place; prose reads better as text.
            popup.image_view.get_or_insert(page.dispersed);
            let key = Arc::as_ptr(page) as usize;
            if popup
                .texture
                .as_ref()
                .is_none_or(|(loaded, _)| *loaded != key)
            {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [page.width as usize, page.height as usize],
                    &page.rgba,
                );
                let texture = ctx.load_texture("capture", image, egui::TextureOptions::LINEAR);
                popup.texture = Some((key, texture));
            }
        }
        let image_view = page.is_some() && popup.image_view == Some(true);
        // Show the capture at its real size, shrunk to fit two thirds of the monitor.
        let [left, top, right, bottom] = popup.work_area.map(|value| value as f32 / root_scale);
        let image_size = page.as_ref().map(|page| {
            let natural = egui::vec2(page.width as f32, page.height as f32) / root_scale;
            let limit = egui::vec2((right - left) * 0.66, (bottom - top) * 0.75 - 80.0);
            natural * (limit.x / natural.x).min(limit.y / natural.y).min(1.0)
        });
        popup.width = match image_size {
            Some(size) if image_view => (size.x + 2.0 * f32::from(POPUP_MARGIN)).max(POPUP_WIDTH),
            _ => POPUP_WIDTH,
        };
        let max_height = if image_view {
            (bottom - top) * 0.9
        } else {
            POPUP_MAX_HEIGHT
        };
        let width = popup.width;
        let initial = popup.place(egui::vec2(width, popup.height) * root_scale);
        let builder = egui::ViewportBuilder::default()
            .with_title("OCR Translate 译文")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_inner_size([width, popup.height])
            .with_position([initial.x / root_scale, initial.y / root_scale]);
        let mut close = false;
        let mut open_main_window = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("result_popup"),
            builder,
            |ctx, _| {
                let content_height = egui::CentralPanel::default()
                    .frame(
                        egui::Frame::new()
                            .fill(palette.surface)
                            .stroke(Stroke::new(1.0_f32, palette.border))
                            .inner_margin(POPUP_MARGIN),
                    )
                    .show(ctx, |ui| {
                        ui.scope(|ui| {
                            self.draw_popup(
                                ui,
                                &palette,
                                &mut popup,
                                image_size.filter(|_| image_view),
                                &mut close,
                                &mut open_main_window,
                            )
                        })
                        .response
                        .rect
                        .height()
                    })
                    .inner;

                // Fit the window to its content and keep it inside the monitor's work area.
                let height =
                    (content_height + 2.0 * f32::from(POPUP_MARGIN)).clamp(80.0, max_height);
                let scale = ctx.pixels_per_point();
                let size = egui::vec2(width, height) * scale;
                let corner = popup.place(size);
                let rect = egui::Rect::from_min_size(corner, size);
                if popup.placed != Some(rect) {
                    popup.placed = Some(rect);
                    popup.height = height;
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        width, height,
                    )));
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        corner.x / scale,
                        corner.y / scale,
                    )));
                }
                if ctx.input(|input| input.viewport().close_requested()) {
                    close = true;
                }
            },
        );
        if open_main_window {
            self.settings_open = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            close = true;
        }
        if !close {
            self.popup = Some(popup);
            // Keep polling Esc and outside clicks while the popup is open.
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }

    fn draw_popup(
        &self,
        ui: &mut egui::Ui,
        palette: &Palette,
        popup: &mut ResultPopup,
        image_size: Option<egui::Vec2>,
        close: &mut bool,
        open_main_window: &mut bool,
    ) {
        let translated = self.result.translated_text.trim();
        let source = self.result.source_text.trim();
        let action = |ui: &mut egui::Ui, text: &str, color: Color32| {
            ui.add(egui::Button::new(RichText::new(text).small().color(color)).frame(false))
                .clicked()
        };
        ui.horizontal(|ui| {
            if self.busy() {
                ui.add(egui::Spinner::new().size(12.0).color(palette.accent));
                ui.label(RichText::new(&self.status).small().color(palette.muted));
            } else if self.result.layout.is_some() && popup.texture.is_some() {
                let mut image_view = popup.image_view == Some(true);
                if segmented(
                    ui,
                    palette,
                    &mut image_view,
                    &[(true, "图片"), (false, "文字")],
                ) {
                    popup.image_view = Some(image_view);
                }
            } else {
                ui.label(RichText::new("译文").small().strong().color(palette.muted));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if action(ui, "关闭", palette.muted) {
                    *close = true;
                }
                if !translated.is_empty() {
                    let label = if popup.copied { "已复制" } else { "复制" };
                    if action(ui, label, palette.accent) {
                        ui.ctx().copy_text(translated.to_owned());
                        popup.copied = true;
                    }
                }
                let (label, color) = if popup.pinned {
                    ("已固定", palette.accent)
                } else {
                    ("固定", palette.muted)
                };
                if action(ui, label, color) {
                    popup.pinned = !popup.pinned;
                }
                if action(ui, "主窗口", palette.muted) {
                    *open_main_window = true;
                }
            });
        });
        ui.add_space(4.0);

        if let (Some(size), Some(page), Some((_, texture))) =
            (image_size, &self.result.layout, &popup.texture)
        {
            paint_translated_capture(ui, texture, page, size, popup.show_source);
            ui.add_space(2.0);
            let label = if popup.show_source {
                "显示译文"
            } else {
                "对比原图"
            };
            if action(ui, label, palette.accent) {
                popup.show_source = !popup.show_source;
            }
            return;
        }

        egui::ScrollArea::vertical()
            .max_height(POPUP_MAX_HEIGHT - 110.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if !translated.is_empty() {
                    ui.add(
                        egui::Label::new(RichText::new(translated).color(palette.text))
                            .wrap()
                            .selectable(true),
                    );
                } else if self.busy() {
                    ui.label(RichText::new("正在识别并翻译…").color(palette.muted));
                } else {
                    ui.add(
                        egui::Label::new(RichText::new(&self.status).color(palette.danger)).wrap(),
                    );
                }
                if popup.show_source && !source.is_empty() {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add(
                        egui::Label::new(RichText::new(source).color(palette.muted))
                            .wrap()
                            .selectable(true),
                    );
                }
            });

        if !source.is_empty() {
            ui.add_space(2.0);
            let label = if popup.show_source {
                "收起原文"
            } else {
                "查看原文"
            };
            if action(ui, label, palette.accent) {
                popup.show_source = !popup.show_source;
            }
        }
    }

    fn draw_status_bar(&self, ui: &mut egui::Ui, palette: &Palette) {
        egui::Sides::new().shrink_left().show(
            ui,
            |ui| {
                if self.busy() {
                    ui.add(egui::Spinner::new().size(12.0).color(palette.accent));
                } else {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot.center(), 3.5, palette.accent);
                }
                ui.add(
                    egui::Label::new(RichText::new(&self.status).color(palette.text)).truncate(),
                );
            },
            |ui| {
                let shortcut = self
                    .active_hotkey
                    .map_or_else(|| "快捷键未注册".into(), |hotkey| hotkey.to_string());
                ui.label(
                    RichText::new(format!("{}  ·  {shortcut}", self.config.mode.short_label()))
                        .color(palette.muted),
                );
            },
        );
    }
}

impl eframe::App for OcrTranslateApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_hotkey(ctx);
        self.poll_screen_capture(ctx);
        self.poll_request(ctx);
        self.poll_usage(ctx);
        let palette = Palette::of(self.config.theme);

        egui::TopBottomPanel::top("top_bar")
            .frame(
                egui::Frame::new()
                    .fill(palette.surface)
                    .inner_margin(egui::Margin::symmetric(20, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("OCR Translate")
                            .heading()
                            .strong()
                            .color(palette.text),
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new("截图识别 · 即时翻译").color(palette.muted));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        segmented(
                            ui,
                            &palette,
                            &mut self.settings_open,
                            &[(false, "翻译"), (true, "设置")],
                        );
                    });
                });
            });

        egui::TopBottomPanel::bottom("status_bar")
            .frame(
                egui::Frame::new()
                    .fill(palette.surface)
                    .inner_margin(egui::Margin::symmetric(20, 8)),
            )
            .show(ctx, |ui| self.draw_status_bar(ui, &palette));

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(palette.background)
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                if self.settings_open {
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| self.draw_settings(ui));
                } else {
                    self.draw_result(ui, ctx);
                }
            });
        self.show_popup(ctx);
        self.hotkey.set_busy(self.busy());
    }
}

impl Drop for OcrTranslateApp {
    fn drop(&mut self) {
        if let Some(path) = self.temporary_capture.take() {
            let _ = fs::remove_file(path);
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ocr-translate")
        .join(CONFIG_FILE)
}

fn load_config(path: &Path) -> AppConfig {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn volcengine_credentials(config: &AppConfig) -> (String, String) {
    let access_key = if config.volcengine_access_key.trim().is_empty() {
        std::env::var("VOLCENGINE_ACCESS_KEY").unwrap_or_default()
    } else {
        config.volcengine_access_key.clone()
    };
    let secret_key = if config.volcengine_secret_key.trim().is_empty() {
        std::env::var("VOLCENGINE_SECRET_KEY").unwrap_or_default()
    } else {
        config.volcengine_secret_key.clone()
    };
    (access_key, secret_key)
}

/// Translates each text separately, returning one translation per input in order.
fn translate_many(config: &AppConfig, texts: &[String]) -> Result<Vec<String>, String> {
    match config.mode {
        TranslationMode::Volcengine => {
            let target = free_translate::target_code(&config.target_language, config.mode)?;
            let (access_key, secret_key) = volcengine_credentials(config);
            volcengine::translate_list(texts, target, &access_key, &secret_key)
        }
        TranslationMode::Vision => texts
            .iter()
            .map(|text| translate_text(config, text))
            .collect(),
        TranslationMode::Tencent | TranslationMode::Google => {
            // Send a few lines per request, keeping URL-based providers under their size limit.
            let mut translations = Vec::with_capacity(texts.len());
            let mut start = 0;
            while start < texts.len() {
                let mut end = start + 1;
                let mut length = texts[start].len();
                while end < texts.len() && length + texts[end].len() < 1500 {
                    length += texts[end].len() + 1;
                    end += 1;
                }
                translations.extend(translate_lines(config, &texts[start..end])?);
                start = end;
            }
            Ok(translations)
        }
    }
}

/// Joins texts with line breaks in one request. If the provider merges or splits lines, falls
/// back to translating each text on its own, a few at a time.
fn translate_lines(config: &AppConfig, texts: &[String]) -> Result<Vec<String>, String> {
    if texts.len() > 1 {
        let joined =
            free_translate::translate(&texts.join("\n"), &config.target_language, config.mode)?;
        let lines: Vec<String> = joined
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if lines.len() == texts.len() {
            return Ok(lines);
        }
    }
    let mut translations = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(6) {
        let results: Vec<Result<String, String>> = thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|text| scope.spawn(|| translate_text(config, text)))
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("翻译任务意外结束。".into()))
                })
                .collect()
        });
        for result in results {
            translations.push(result?);
        }
    }
    Ok(translations)
}

fn translate_text(config: &AppConfig, source: &str) -> Result<String, String> {
    match config.mode {
        TranslationMode::Vision => translate_text_with_vision(config, source),
        TranslationMode::Volcengine => {
            let target = free_translate::target_code(&config.target_language, config.mode)?;
            let (access_key, secret_key) = volcengine_credentials(config);
            volcengine::translate(source, target, &access_key, &secret_key)
        }
        TranslationMode::Tencent | TranslationMode::Google => {
            free_translate::translate(source, &config.target_language, config.mode)
        }
    }
}

fn translate_image(config: &AppConfig, path: &Path) -> Result<TranslationResult, String> {
    validate_image_size(path)?;
    let bytes = fs::read(path).map_err(|error| format!("读取图片失败：{error}"))?;
    let mime = match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/png",
    };
    let image_data = format!("data:{mime};base64,{}", STANDARD.encode(bytes));
    let prompt = format!(
        "识别图片中的所有可读文字，并翻译成{}。只返回严格 JSON，不要 Markdown，不要额外说明，格式为 {{\"source_text\":\"原文\",\"translated_text\":\"译文\"}}。保留原文换行。",
        config.target_language
    );
    chat_completion(
        config,
        serde_json::json!([
            {"type": "text", "text": prompt},
            {"type": "image_url", "image_url": {"url": image_data}}
        ]),
    )
}

fn translate_text_with_vision(config: &AppConfig, source: &str) -> Result<String, String> {
    let prompt = format!(
        "将下面的原文翻译成{}。只返回严格 JSON，不要 Markdown，不要额外说明，格式为 {{\"source_text\":\"原文\",\"translated_text\":\"译文\"}}。原文如下：\n{}",
        config.target_language, source
    );
    Ok(chat_completion(config, Value::String(prompt))?.translated_text)
}

/// Sends one user message to the OpenAI-compatible endpoint and parses the JSON reply.
fn chat_completion(config: &AppConfig, content: Value) -> Result<TranslationResult, String> {
    let body = serde_json::json!({
        "model": config.model,
        "temperature": 0,
        "messages": [{
            "role": "user",
            "content": content
        }]
    });
    let client = net::client_builder()?
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|error| format!("创建网络客户端失败：{error}"))?;
    let response = client
        .post(&config.api_url)
        .bearer_auth(&config.api_key)
        .json(&body)
        .send()
        .map_err(|error| format!("请求失败：{error}"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|error| format!("读取响应失败：{error}"))?;
    if !status.is_success() {
        return Err(format!("接口返回 {}：{}", status, compact_error(&text)));
    }
    parse_translation_response(&text)
}

fn validate_image_size(path: &Path) -> Result<(), String> {
    let metadata = fs::metadata(path).map_err(|error| format!("读取图片信息失败：{error}"))?;
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err("图片超过 20 MB，请先裁剪或压缩后重试。".into());
    }
    Ok(())
}

fn parse_translation_response(text: &str) -> Result<TranslationResult, String> {
    let root: Value =
        serde_json::from_str(text).map_err(|error| format!("接口响应不是有效 JSON：{error}"))?;
    let content = root["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| "接口响应中没有找到模型文本。".to_string())?;
    let clean = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = serde_json::from_str(clean)
        .or_else(|_| {
            let start = clean.find('{').ok_or(())?;
            let end = clean.rfind('}').ok_or(())?;
            serde_json::from_str(&clean[start..=end]).map_err(|_| ())
        })
        .map_err(|_| "模型返回的内容无法解析为原文和译文。".to_string())?;
    let source = parsed["source_text"]
        .as_str()
        .or_else(|| parsed["original"].as_str())
        .unwrap_or("");
    let translated = parsed["translated_text"]
        .as_str()
        .or_else(|| parsed["translation"].as_str())
        .unwrap_or("");
    if source.is_empty() && translated.is_empty() {
        return Err("模型返回了空的原文和译文。".into());
    }
    Ok(TranslationResult {
        source_text: source.into(),
        translated_text: translated.into(),
        layout: None,
    })
}

fn compact_error(text: &str) -> String {
    text.replace(['\r', '\n'], " ").chars().take(300).collect()
}

/// Colours shared by the custom widgets and the egui visuals for one theme.
struct Palette {
    background: Color32,
    surface: Color32,
    input: Color32,
    hover: Color32,
    pressed: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    danger: Color32,
    accent: Color32,
    on_accent: Color32,
}

impl Palette {
    fn of(theme: ThemeMode) -> Self {
        match theme {
            ThemeMode::Day => Self {
                background: Color32::from_rgb(0xF3, 0xF4, 0xF7),
                surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
                input: Color32::from_rgb(0xF6, 0xF7, 0xF9),
                hover: Color32::from_rgb(0xEC, 0xEE, 0xF2),
                pressed: Color32::from_rgb(0xE2, 0xE5, 0xEB),
                border: Color32::from_rgb(0xE1, 0xE4, 0xEA),
                text: Color32::from_rgb(0x1F, 0x23, 0x29),
                muted: Color32::from_rgb(0x6B, 0x72, 0x80),
                danger: Color32::from_rgb(0xD9, 0x3F, 0x3F),
                accent: Color32::from_rgb(0x3B, 0x6E, 0xF5),
                on_accent: Color32::WHITE,
            },
            ThemeMode::Night => Self {
                background: Color32::from_rgb(0x14, 0x16, 0x1B),
                surface: Color32::from_rgb(0x1C, 0x1F, 0x26),
                input: Color32::from_rgb(0x23, 0x27, 0x2F),
                hover: Color32::from_rgb(0x2B, 0x30, 0x3A),
                pressed: Color32::from_rgb(0x33, 0x39, 0x45),
                border: Color32::from_rgb(0x2D, 0x32, 0x3C),
                text: Color32::from_rgb(0xE6, 0xE8, 0xEC),
                muted: Color32::from_rgb(0x8B, 0x93, 0xA1),
                danger: Color32::from_rgb(0xF2, 0x6D, 0x6D),
                accent: Color32::from_rgb(0x5B, 0x8C, 0xFF),
                on_accent: Color32::WHITE,
            },
        }
    }
}

fn apply_appearance(ctx: &egui::Context, config: &AppConfig) {
    let palette = Palette::of(config.theme);
    let mut visuals = match config.theme {
        ThemeMode::Day => egui::Visuals::light(),
        ThemeMode::Night => egui::Visuals::dark(),
    };
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.surface;
    visuals.window_stroke = Stroke::new(1.0_f32, palette.border);
    visuals.extreme_bg_color = palette.input;
    visuals.text_edit_bg_color = Some(palette.input);
    visuals.faint_bg_color = palette.input;
    visuals.hyperlink_color = palette.accent;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.45);
    visuals.selection.stroke = Stroke::new(1.0_f32, palette.accent);
    visuals.window_corner_radius = egui::CornerRadius::same(12);
    visuals.menu_corner_radius = egui::CornerRadius::same(8);
    let shadow = egui::Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(match config.theme {
            ThemeMode::Day => 30,
            ThemeMode::Night => 90,
        }),
    };
    visuals.window_shadow = shadow;
    visuals.popup_shadow = shadow;

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = palette.surface;
    widgets.noninteractive.weak_bg_fill = palette.surface;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, palette.text);
    widgets.noninteractive.corner_radius = egui::CornerRadius::same(8);
    for (state, fill, stroke) in [
        (&mut widgets.inactive, palette.input, palette.border),
        (
            &mut widgets.hovered,
            palette.hover,
            palette.accent.gamma_multiply(0.6),
        ),
        (&mut widgets.active, palette.pressed, palette.accent),
        (&mut widgets.open, palette.input, palette.border),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0_f32, stroke);
        state.fg_stroke = Stroke::new(1.0_f32, palette.text);
        state.corner_radius = egui::CornerRadius::same(8);
        state.expansion = 0.0;
    }
    ctx.set_visuals(visuals);

    let size = config
        .font_size
        .clamp(*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end()) as f32;
    let mut style = (*ctx.style()).clone();
    for (text_style, font_size) in [
        (egui::TextStyle::Body, size),
        (egui::TextStyle::Button, size),
        (egui::TextStyle::Monospace, size),
        (egui::TextStyle::Heading, size + 5.0),
        (egui::TextStyle::Small, (size - 2.0).max(10.0)),
    ] {
        if let Some(font) = style.text_styles.get_mut(&text_style) {
            font.size = font_size;
        }
    }
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(14.0, 6.0);
    style.spacing.interact_size.y = size + 16.0;
    style.spacing.slider_width = 260.0;
    style.visuals.slider_trailing_fill = true;
    ctx.set_style(style);
    ctx.request_repaint();
}

/// Draws the capture and, unless `original` is set, paints each translated block over its
/// source text in the sampled colours. Hovering a block shows its source text.
fn paint_translated_capture(
    ui: &mut egui::Ui,
    texture: &egui::TextureHandle,
    page: &layout::ImageLayout,
    size: egui::Vec2,
    original: bool,
) {
    const MIN_FONT: f32 = 7.0;
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(texture.id(), rect, uv, Color32::WHITE);
    let scale = size.x / page.width as f32;
    let mut hovered = None;
    for block in &page.blocks {
        let [x, y, width, height] = block.rect;
        let area = egui::Rect::from_min_size(
            rect.min + egui::vec2(x, y) * scale,
            egui::vec2(width, height) * scale,
        );
        if response
            .hover_pos()
            .is_some_and(|pointer| area.contains(pointer))
        {
            hovered = Some(block.source.clone());
        }
        let Some(text) = block.translated.as_ref().filter(|_| !original) else {
            continue;
        };
        let [r, g, b] = block.background;
        painter.rect_filled(area.expand(2.0), 2.0, Color32::from_rgb(r, g, b));
        let [r, g, b] = block.foreground;
        let color = Color32::from_rgb(r, g, b);
        // Start near the source font size and shrink until the text fits the source box.
        let mut font_size = (block.line_height * scale * 0.95).max(MIN_FONT);
        let galley = loop {
            let galley = painter.layout(
                text.clone(),
                egui::FontId::proportional(font_size),
                color,
                area.width().max(1.0),
            );
            if galley.size().y <= area.height() * 1.15 || font_size <= MIN_FONT {
                break galley;
            }
            font_size = (font_size * 0.9).max(MIN_FONT);
        };
        let top = area.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(area.left(), top), galley, color);
    }
    if let Some(source) = hovered {
        response.on_hover_text(source);
    }
}

/// `16207` → "1.6 万", `2000000` → "200 万", small numbers as they are.
fn format_chars(chars: f64) -> String {
    if chars >= 10_000.0 {
        let wan = chars / 10_000.0;
        if (wan - wan.round()).abs() < 0.05 {
            format!("{} 万", wan.round())
        } else {
            format!("{wan:.1} 万")
        }
    } else {
        format!("{}", chars.round())
    }
}

fn card<R>(
    ui: &mut egui::Ui,
    palette: &Palette,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(12u8)
        .inner_margin(18i8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui)
        })
}

fn section_title(ui: &mut egui::Ui, palette: &Palette, title: &str, subtitle: &str) {
    let size = ui.style().text_styles[&egui::TextStyle::Body].size + 3.0;
    ui.label(RichText::new(title).size(size).strong().color(palette.text));
    hint(ui, palette, subtitle);
    ui.add_space(10.0);
}

fn field_label(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).color(palette.muted));
}

fn hint(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(RichText::new(text).small().color(palette.muted));
}

fn primary_button(palette: &Palette, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text).strong().color(palette.on_accent))
        .fill(palette.accent)
        .stroke(Stroke::NONE)
}

fn text_field(text: &mut String) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::vec2(10.0, 7.0))
}

fn secondary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(text.to_owned())
}

/// A pill-shaped switch between a few options. Returns true when the value changed.
fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    palette: &Palette,
    value: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(palette.input)
        .corner_radius(9u8)
        .inner_margin(3i8)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                let mut ordered: Vec<&(T, &str)> = options.iter().collect();
                // In a right-to-left layout, add in reverse so the options still read left to right.
                if ui.layout().prefer_right_to_left() {
                    ordered.reverse();
                }
                for (option, label) in ordered {
                    let selected = *value == *option;
                    let text = RichText::new(*label).color(if selected {
                        palette.on_accent
                    } else {
                        palette.muted
                    });
                    let button = egui::Button::new(text)
                        .fill(if selected {
                            palette.accent
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(7u8)
                        .min_size(egui::vec2(64.0, 0.0));
                    if ui.add(button).clicked() && !selected {
                        *value = *option;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// A titled card with a borderless editor filling the rest of the column.
/// Returns whether the editor has focus and whether the header action was clicked.
fn text_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    title: &str,
    text: &mut String,
    hint_text: &str,
    action: &str,
) -> (bool, bool) {
    let mut action_clicked = false;
    let focused = card(ui, palette, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong().color(palette.text));
            let count = text.chars().filter(|c| !c.is_whitespace()).count();
            if count > 0 {
                ui.label(
                    RichText::new(format!("{count} 字"))
                        .small()
                        .color(palette.muted),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                action_clicked = ui
                    .add_enabled(
                        !text.is_empty(),
                        egui::Button::new(RichText::new(action).color(palette.accent)).frame(false),
                    )
                    .clicked();
            });
        });
        ui.add_space(4.0);
        let height = ui.available_height().max(140.0);
        egui::ScrollArea::vertical()
            .id_salt(title)
            .auto_shrink(false)
            .max_height(height)
            .show(ui, |ui| {
                ui.add_sized(
                    [ui.available_width(), height],
                    egui::TextEdit::multiline(text)
                        .frame(false)
                        .hint_text(hint_text)
                        .desired_width(f32::INFINITY),
                )
                .has_focus()
            })
            .inner
    })
    .inner;
    (focused, action_clicked)
}

fn install_chinese_font(ctx: &egui::Context) {
    let font_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let font_bytes = ["msyh.ttc", "Deng.ttf", "simhei.ttf"]
        .iter()
        .find_map(|name| fs::read(font_dir.join(name)).ok());

    if let Some(bytes) = font_bytes {
        let mut fonts = egui::FontDefinitions::default();
        let name = "Windows Chinese".to_owned();
        fonts
            .font_data
            .insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            if let Some(names) = fonts.families.get_mut(&family) {
                names.insert(1, name.clone());
            }
        }
        ctx.set_fonts(fonts);
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "OCR Translate",
        options,
        Box::new(|cc| {
            install_chinese_font(&cc.egui_ctx);
            Ok(Box::new(OcrTranslateApp::new(&cc.egui_ctx)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_translation() {
        let response = r#"{"choices":[{"message":{"content":"{\"source_text\":\"Hello\",\"translated_text\":\"你好\"}"}}]}"#;
        let result = parse_translation_response(response).unwrap();
        assert_eq!(result.source_text, "Hello");
        assert_eq!(result.translated_text, "你好");
    }

    #[test]
    fn parses_fenced_translation_with_common_aliases() {
        let response = r#"{"choices":[{"message":{"content":"```json\n{\"original\":\"Good morning\",\"translation\":\"早上好\"}\n```"}}]}"#;
        let result = parse_translation_response(response).unwrap();
        assert_eq!(result.source_text, "Good morning");
        assert_eq!(result.translated_text, "早上好");
    }

    #[test]
    fn rejects_unstructured_model_text() {
        let response = r#"{"choices":[{"message":{"content":"Just a translation"}}]}"#;
        assert!(parse_translation_response(response).is_err());
    }

    #[test]
    fn old_config_without_volcengine_keys_still_loads() {
        let config: AppConfig = serde_json::from_str(
            r#"{"mode":"tencent","ocr_language":"en-US","target_language":"简体中文"}"#,
        )
        .unwrap();
        assert!(config.volcengine_access_key.is_empty());
        assert!(config.volcengine_secret_key.is_empty());
        assert_eq!(config.theme, ThemeMode::Night);
        assert_eq!(config.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn appearance_settings_round_trip() {
        let config = AppConfig {
            theme: ThemeMode::Day,
            font_size: 18,
            ..Default::default()
        };
        let restored: AppConfig =
            serde_json::from_str(&serde_json::to_string(&config).unwrap()).unwrap();
        assert_eq!(restored.theme, ThemeMode::Day);
        assert_eq!(restored.font_size, 18);
    }

    #[test]
    fn saved_config_encrypts_secrets_and_reveals_them_on_load() {
        let config = AppConfig {
            api_key: "vision-key".into(),
            volcengine_access_key: "access-key".into(),
            volcengine_secret_key: "secret-key".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&config.protected().unwrap()).unwrap();
        assert!(!json.contains("vision-key") && !json.contains("secret-key"));

        let mut loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.reveal_secrets(), (false, false));
        assert_eq!(loaded.api_key, "vision-key");
        assert_eq!(loaded.volcengine_access_key, "access-key");
        assert_eq!(loaded.volcengine_secret_key, "secret-key");
    }

    #[test]
    fn plaintext_secrets_from_old_versions_are_flagged_for_migration() {
        let mut config: AppConfig =
            serde_json::from_str(r#"{"volcengine_secret_key":"old-plaintext"}"#).unwrap();
        assert_eq!(config.reveal_secrets(), (true, false));
        assert_eq!(config.volcengine_secret_key, "old-plaintext");
    }

    #[test]
    fn undecryptable_secrets_are_cleared() {
        let mut config: AppConfig = serde_json::from_str(r#"{"api_key":"dpapi:AAAA"}"#).unwrap();
        assert_eq!(config.reveal_secrets(), (false, true));
        assert!(config.api_key.is_empty());
    }

    #[test]
    fn formats_character_counts_in_wan() {
        assert_eq!(format_chars(16_207.0), "1.6 万");
        assert_eq!(format_chars(2_000_000.0), "200 万");
        assert_eq!(format_chars(1_983_793.0), "198.4 万");
        assert_eq!(format_chars(820.0), "820");
    }

    fn popup_at(anchor: [i32; 2]) -> ResultPopup {
        ResultPopup {
            anchor,
            work_area: [0, 0, 1920, 1040],
            opened_at: Instant::now(),
            pinned: false,
            show_source: false,
            copied: false,
            placed: None,
            width: POPUP_WIDTH,
            height: 120.0,
            image_view: None,
            texture: None,
            escape_was_down: false,
            click_was_down: false,
        }
    }

    #[test]
    fn popup_opens_below_right_of_the_selection_end() {
        let corner = popup_at([500, 300]).place(egui::vec2(400.0, 200.0));
        assert_eq!(corner, egui::pos2(516.0, 316.0));
    }

    #[test]
    fn popup_flips_to_stay_inside_the_work_area() {
        let corner = popup_at([1900, 1000]).place(egui::vec2(400.0, 200.0));
        assert_eq!(corner, egui::pos2(1484.0, 784.0));
    }
}
