#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod free_translate;
mod volcengine;
mod windows_ocr;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use eframe::egui;
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
};

const CONFIG_FILE: &str = "config.json";
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
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
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
struct AppConfig {
    mode: TranslationMode,
    ocr_language: String,
    api_url: String,
    api_key: String,
    model: String,
    target_language: String,
    shortcut: String,
    volcengine_access_key: String,
    volcengine_secret_key: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            mode: TranslationMode::Tencent,
            ocr_language: "en-US".into(),
            api_url: "https://api.openai.com/v1/chat/completions".into(),
            api_key: String::new(),
            model: "gpt-4o-mini".into(),
            target_language: "简体中文".into(),
            shortcut: "Ctrl+Shift+T".into(),
            volcengine_access_key: String::new(),
            volcengine_secret_key: String::new(),
        }
    }
}

#[derive(Default, Clone)]
struct TranslationResult {
    source_text: String,
    translated_text: String,
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
    request_rx: Option<Receiver<RequestResult>>,
    status: String,
    settings_open: bool,
    show_api_key: bool,
    show_volcengine_keys: bool,
    ocr_languages: Vec<String>,
}

impl OcrTranslateApp {
    fn new() -> Self {
        let config_path = config_path();
        let mut config = load_config(&config_path);
        let ocr_languages = windows_ocr::available_languages();
        if config.ocr_language != "auto"
            && !ocr_languages.iter().any(|tag| tag == &config.ocr_language)
        {
            config.ocr_language = "auto".into();
        }
        Self {
            config,
            config_path,
            result: TranslationResult::default(),
            selected_file: None,
            request_rx: None,
            status: "准备就绪。选择一张截图开始翻译。".into(),
            settings_open: true,
            show_api_key: false,
            show_volcengine_keys: false,
            ocr_languages,
        }
    }

    fn save_config(&mut self) {
        match serde_json::to_string_pretty(&self.config)
            .ok()
            .and_then(|json| {
                fs::create_dir_all(self.config_path.parent()?)
                    .ok()
                    .map(|_| json)
            })
            .and_then(|json| fs::write(&self.config_path, json).ok())
        {
            Some(_) => self.status = "设置已保存。".into(),
            None => self.status = "设置保存失败，请检查应用数据目录权限。".into(),
        }
    }

    fn choose_image(&mut self, ctx: &egui::Context) {
        let picked = rfd::FileDialog::new()
            .add_filter("图片", &["png", "jpg", "jpeg", "webp", "bmp"])
            .pick_file();
        if let Some(path) = picked {
            self.selected_file = Some(path.clone());
            self.start_translation(path, ctx);
        }
    }

    fn start_translation(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.config.mode == TranslationMode::Vision {
            if self.config.api_key.trim().is_empty() {
                self.status = "请先填写 API Key。".into();
                self.settings_open = true;
                return;
            }
            if self.config.api_url.trim().is_empty() || self.config.model.trim().is_empty() {
                self.status = "请先填写 API URL 和模型名称。".into();
                self.settings_open = true;
                return;
            }
        } else if self.config.mode == TranslationMode::Volcengine {
            let (access_key, secret_key) = volcengine_credentials(&self.config);
            if access_key.trim().is_empty() || secret_key.trim().is_empty() {
                self.status = "请先在设置中填写火山引擎 Access Key 和 Secret Key。".into();
                self.settings_open = true;
                return;
            }
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
                let source = windows_ocr::recognize(&path, &config.ocr_language)?;
                let _ = tx.send(RequestResult::Source(source.clone()));
                let translated = if config.mode == TranslationMode::Volcengine {
                    let target = free_translate::target_code(&config.target_language, config.mode)?;
                    let (access_key, secret_key) = volcengine_credentials(&config);
                    volcengine::translate(&source, target, &access_key, &secret_key)?
                } else {
                    free_translate::translate(&source, &config.target_language, config.mode)?
                };
                Ok(TranslationResult {
                    source_text: source,
                    translated_text: translated,
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
        if self.config.mode == TranslationMode::Vision {
            if self.config.api_key.trim().is_empty() {
                self.status = "请先填写 API Key。".into();
                self.settings_open = true;
                return;
            }
            if self.config.api_url.trim().is_empty() || self.config.model.trim().is_empty() {
                self.status = "请先填写 API URL 和模型名称。".into();
                self.settings_open = true;
                return;
            }
        } else if self.config.mode == TranslationMode::Volcengine {
            let (access_key, secret_key) = volcengine_credentials(&self.config);
            if access_key.trim().is_empty() || secret_key.trim().is_empty() {
                self.status = "请先在设置中填写火山引擎 Access Key 和 Secret Key。".into();
                self.settings_open = true;
                return;
            }
        }

        let config = self.config.clone();
        let (tx, rx) = mpsc::channel();
        self.request_rx = Some(rx);
        self.result.translated_text.clear();
        self.status = "正在翻译…".into();
        ctx.request_repaint();

        thread::spawn(move || {
            let result = (|| {
                let translated = match config.mode {
                    TranslationMode::Vision => translate_text_with_vision(&config, &source)?,
                    TranslationMode::Volcengine => {
                        let target =
                            free_translate::target_code(&config.target_language, config.mode)?;
                        let (access_key, secret_key) = volcengine_credentials(&config);
                        volcengine::translate(&source, target, &access_key, &secret_key)?
                    }
                    TranslationMode::Tencent | TranslationMode::Google => {
                        free_translate::translate(&source, &config.target_language, config.mode)?
                    }
                };
                Ok(TranslationResult {
                    source_text: source,
                    translated_text: translated,
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
        ui.heading("翻译设置");
        ui.add_space(8.0);
        ui.label("翻译渠道");
        egui::ComboBox::from_id_salt("translation_mode")
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
        ui.add_space(12.0);

        if self.config.mode == TranslationMode::Vision {
            ui.label("API URL");
            ui.add(
                egui::TextEdit::singleline(&mut self.config.api_url).desired_width(f32::INFINITY),
            );
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                ui.label("API Key");
                ui.checkbox(&mut self.show_api_key, "显示");
            });
            let mut key_edit =
                egui::TextEdit::singleline(&mut self.config.api_key).desired_width(f32::INFINITY);
            if !self.show_api_key {
                key_edit = key_edit.password(true);
            }
            ui.add(key_edit);
            ui.add_space(8.0);

            ui.label("模型");
            ui.add(egui::TextEdit::singleline(&mut self.config.model).desired_width(f32::INFINITY));
            ui.add_space(8.0);
        } else {
            if self.config.mode == TranslationMode::Volcengine {
                let (access_key, secret_key) = volcengine_credentials(&self.config);
                let configured = !access_key.trim().is_empty() && !secret_key.trim().is_empty();
                ui.label(if configured {
                    "火山凭证：已配置"
                } else {
                    "火山凭证：尚未配置"
                });
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label("Access Key (AK)");
                    ui.checkbox(&mut self.show_volcengine_keys, "显示密钥");
                });
                let access_edit =
                    egui::TextEdit::singleline(&mut self.config.volcengine_access_key)
                        .desired_width(f32::INFINITY)
                        .password(!self.show_volcengine_keys);
                ui.add(access_edit);
                ui.add_space(8.0);

                ui.label("Secret Key (SK)");
                let secret_edit =
                    egui::TextEdit::singleline(&mut self.config.volcengine_secret_key)
                        .desired_width(f32::INFINITY)
                        .password(!self.show_volcengine_keys);
                ui.add(secret_edit);
                ui.small("填写后点击“保存设置”。开发环境也兼容 VOLCENGINE_ACCESS_KEY / VOLCENGINE_SECRET_KEY 环境变量。");
                ui.add_space(8.0);
            }
            ui.label("原文识别语言");
            egui::ComboBox::from_id_salt("ocr_language")
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
                ui.small("未检测到 Windows OCR 语言包，请在系统语言设置中安装。");
            }
            ui.add_space(8.0);
        }

        ui.label("目标语言");
        if self.config.mode == TranslationMode::Vision {
            ui.add(
                egui::TextEdit::singleline(&mut self.config.target_language)
                    .desired_width(f32::INFINITY),
            );
        } else {
            egui::ComboBox::from_id_salt("target_language")
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
            if matches!(
                self.config.mode,
                TranslationMode::Tencent | TranslationMode::Google
            ) {
                ui.small("免 Key 渠道使用第三方网页接口，可用性会随服务方变化。");
            }
        }
        ui.add_space(8.0);

        ui.label("截图快捷键（即将接入全局监听）");
        ui.add_enabled(
            false,
            egui::TextEdit::singleline(&mut self.config.shortcut).desired_width(f32::INFINITY),
        );
        ui.add_space(14.0);

        if ui.button("保存设置").clicked() {
            self.save_config();
        }
        ui.add_space(8.0);
        ui.small("配置保存在当前用户的应用数据目录中。");
    }

    fn draw_result(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("截图翻译");
        ui.add_space(4.0);
        ui.label("选择图片识别，或直接输入原文后翻译。");
        ui.add_space(12.0);

        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.request_rx.is_none(),
                    egui::Button::new("选择截图 / 图片"),
                )
                .clicked()
            {
                self.choose_image(ctx);
            }
            if ui
                .add_enabled(self.request_rx.is_none(), egui::Button::new("翻译原文"))
                .clicked()
            {
                self.start_text_translation(ctx);
            }
            if ui
                .add_enabled(self.request_rx.is_none(), egui::Button::new("重新翻译"))
                .clicked()
            {
                if let Some(path) = self.selected_file.clone() {
                    self.start_translation(path, ctx);
                }
            }
            if let Some(path) = &self.selected_file {
                ui.label(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("已选择图片"),
                );
            }
        });
        ui.add_space(16.0);

        let available = ui.available_height();
        ui.columns(2, |columns| {
            columns[0].label("原文");
            columns[0].add_sized(
                [columns[0].available_width(), (available - 48.0).max(160.0)],
                egui::TextEdit::multiline(&mut self.result.source_text)
                    .hint_text("输入或识别出的原文会显示在这里")
                    .desired_rows(12),
            );
            columns[1].label("译文");
            columns[1].add_sized(
                [columns[1].available_width(), (available - 48.0).max(160.0)],
                egui::TextEdit::multiline(&mut self.result.translated_text)
                    .hint_text("翻译结果会显示在这里")
                    .desired_rows(12),
            );
        });
    }
}

impl eframe::App for OcrTranslateApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_request(ctx);
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("OCR Translate");
                ui.separator();
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.selectable_label(self.settings_open, "设置").clicked() {
                        self.settings_open = true;
                    }
                    if ui.selectable_label(!self.settings_open, "翻译").clicked() {
                        self.settings_open = false;
                    }
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.settings_open {
                self.draw_settings(ui);
            } else {
                self.draw_result(ui, ctx);
            }
        });
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
    let body = serde_json::json!({
        "model": config.model,
        "temperature": 0,
        "messages": [{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {"url": image_data}}
            ]
        }]
    });

    let client = reqwest::blocking::Client::builder()
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

fn translate_text_with_vision(config: &AppConfig, source: &str) -> Result<String, String> {
    let prompt = format!(
        "将下面的原文翻译成{}。只返回严格 JSON，不要 Markdown，不要额外说明，格式为 {{\"source_text\":\"原文\",\"translated_text\":\"译文\"}}。原文如下：\n{}",
        config.target_language, source
    );
    let body = serde_json::json!({
        "model": config.model,
        "temperature": 0,
        "messages": [{
            "role": "user",
            "content": prompt
        }]
    });
    let client = reqwest::blocking::Client::builder()
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
    Ok(parse_translation_response(&text)?.translated_text)
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
    })
}

fn compact_error(text: &str) -> String {
    text.replace(['\r', '\n'], " ").chars().take(300).collect()
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
            Ok(Box::new(OcrTranslateApp::new()))
        }),
    )
}
