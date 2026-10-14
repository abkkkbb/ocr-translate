use std::{
    fmt, fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use windows::{
    core::{w, PWSTR},
    Win32::{
        Foundation::{CloseHandle, LPARAM, WPARAM},
        System::{
            DataExchange::GetClipboardSequenceNumber,
            Threading::{
                GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS,
                INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
                MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE,
                VK_F1, VK_F12, VK_LWIN, VK_MENU, VK_S, VK_SHIFT, VK_T, VK_V,
            },
            WindowsAndMessaging::{
                FindWindowW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
                PeekMessageW, PostThreadMessageW, SetForegroundWindow, ShowWindowAsync, MSG,
                PM_NOREMOVE, SW_RESTORE, WM_APP, WM_HOTKEY,
            },
        },
    },
};

const HOTKEY_ID: i32 = 1;
const WM_SET_HOTKEY: u32 = WM_APP + 1;
const STALE_CAPTURE_AGE: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: u16,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: true,
            key: VK_T.0,
        }
    }
}

impl Hotkey {
    /// Letters, digits and F1-F12. Virtual-key codes for letters and digits match ASCII.
    pub fn key_choices() -> impl Iterator<Item = u16> {
        (b'A'..=b'Z')
            .chain(b'0'..=b'9')
            .map(u16::from)
            .chain(VK_F1.0..=VK_F12.0)
    }

    pub fn key_name(key: u16) -> String {
        if (VK_F1.0..=VK_F12.0).contains(&key) {
            format!("F{}", key - VK_F1.0 + 1)
        } else {
            char::from_u32(key.into())
                .map(String::from)
                .unwrap_or_default()
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let mut hotkey = Self {
            ctrl: false,
            alt: false,
            shift: false,
            key: 0,
        };
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => hotkey.ctrl = true,
                "alt" => hotkey.alt = true,
                "shift" => hotkey.shift = true,
                _ if hotkey.key != 0 => return None,
                _ => {
                    hotkey.key = Self::key_choices()
                        .find(|key| Self::key_name(*key).eq_ignore_ascii_case(part))?
                }
            }
        }
        hotkey.validate().ok().map(|_| hotkey)
    }

    /// Ctrl or Alt is required so the shortcut never swallows ordinary typing.
    pub fn validate(&self) -> Result<(), String> {
        if self.key == 0 {
            Err("请选择快捷键按键。".into())
        } else if !self.ctrl && !self.alt {
            Err("快捷键需要包含 Ctrl 或 Alt。".into())
        } else {
            Ok(())
        }
    }

    fn modifiers(&self) -> HOT_KEY_MODIFIERS {
        [
            (self.ctrl, MOD_CONTROL),
            (self.alt, MOD_ALT),
            (self.shift, MOD_SHIFT),
        ]
        .into_iter()
        .filter(|(held, _)| *held)
        .fold(MOD_NOREPEAT, |all, (_, modifier)| all | modifier)
    }

    fn to_message(self) -> (WPARAM, LPARAM) {
        let flags = self.ctrl as isize | (self.alt as isize) << 1 | (self.shift as isize) << 2;
        (WPARAM(self.key.into()), LPARAM(flags))
    }

    fn from_message(wparam: WPARAM, lparam: LPARAM) -> Self {
        Self {
            ctrl: lparam.0 & 1 != 0,
            alt: lparam.0 & 2 != 0,
            shift: lparam.0 & 4 != 0,
            key: wparam.0 as u16,
        }
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (held, name) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
        ] {
            if held {
                formatter.write_str(name)?;
            }
        }
        formatter.write_str(&Self::key_name(self.key))
    }
}

pub enum HotkeyEvent {
    Ready(Hotkey),
    Busy,
    Started,
    Captured(Result<PathBuf, String>),
    /// Registration failed; `active` is the shortcut still in effect, if any.
    Failed {
        message: String,
        active: Option<Hotkey>,
    },
}

pub struct HotkeyListener {
    pub events: Receiver<HotkeyEvent>,
    busy: Arc<AtomicBool>,
    thread_id: u32,
}

impl HotkeyListener {
    /// Presses arriving while the app is busy are reported as `Busy` instead of starting a capture.
    pub fn set_busy(&self, busy: bool) {
        self.busy.store(busy, Ordering::Release);
    }

    pub fn change(&self, hotkey: Hotkey) -> Result<(), String> {
        let (wparam, lparam) = hotkey.to_message();
        unsafe { PostThreadMessageW(self.thread_id, WM_SET_HOTKEY, wparam, lparam) }
            .map_err(|error| format!("无法更新全局快捷键：{error}"))
    }
}

fn register(hotkey: Hotkey) -> windows::core::Result<()> {
    unsafe { RegisterHotKey(None, HOTKEY_ID, hotkey.modifiers(), hotkey.key.into()) }
}

/// RegisterHotKey delivers WM_HOTKEY to the registering thread's message queue.
pub fn listen_for_hotkey(ctx: egui::Context, hotkey: Hotkey) -> HotkeyListener {
    let (tx, rx) = mpsc::channel();
    let (thread_id_tx, thread_id_rx) = mpsc::sync_channel(1);
    let busy = Arc::new(AtomicBool::new(false));
    let thread_busy = busy.clone();
    thread::spawn(move || {
        let mut message = MSG::default();
        // Create this thread's message queue before the UI thread can post to it.
        let _ = unsafe { PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE) };
        let _ = thread_id_tx.send(unsafe { GetCurrentThreadId() });
        let notify = |event: HotkeyEvent| {
            let sent = tx.send(event).is_ok();
            ctx.request_repaint();
            sent
        };

        let mut active = None;
        let sent = match register(hotkey) {
            Ok(()) => {
                active = Some(hotkey);
                notify(HotkeyEvent::Ready(hotkey))
            }
            Err(error) => notify(HotkeyEvent::Failed {
                message: format!("{hotkey} 注册失败（可能已被其他程序占用）：{error}"),
                active: None,
            }),
        };
        if !sent {
            return;
        }

        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            match message.message {
                WM_HOTKEY if message.wParam.0 == HOTKEY_ID as usize => {
                    // A press queued during a capture or translation must not replace that work.
                    if thread_busy.swap(true, Ordering::AcqRel) {
                        if !notify(HotkeyEvent::Busy) {
                            break;
                        }
                        continue;
                    }
                    if !notify(HotkeyEvent::Started) {
                        break;
                    }
                    let result = capture_region(active);
                    if !notify(HotkeyEvent::Captured(result)) {
                        break;
                    }
                }
                WM_SET_HOTKEY => {
                    let requested = Hotkey::from_message(message.wParam, message.lParam);
                    if active != Some(requested) {
                        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
                        if let Err(error) = register(requested) {
                            active = active.filter(|previous| register(*previous).is_ok());
                            let fallback = match active {
                                Some(previous) => format!("仍使用 {previous}"),
                                None => "当前没有可用的全局快捷键".into(),
                            };
                            let failed = HotkeyEvent::Failed {
                                message: format!(
                                    "{requested} 注册失败（可能已被其他程序占用），{fallback}：{error}"
                                ),
                                active,
                            };
                            if !notify(failed) {
                                break;
                            }
                            continue;
                        }
                        active = Some(requested);
                    }
                    if !notify(HotkeyEvent::Ready(requested)) {
                        break;
                    }
                }
                _ => {}
            }
        }
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
    });
    HotkeyListener {
        events: rx,
        busy,
        thread_id: thread_id_rx.recv().unwrap_or_default(),
    }
}

/// `hotkey` is the shortcut that triggered the capture, whose keys must be released first.
pub fn capture_region(hotkey: Option<Hotkey>) -> Result<PathBuf, String> {
    let result = capture_region_inner(hotkey);
    restore_main_window();
    result
}

pub fn paste_keys_down() -> bool {
    unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 && GetAsyncKeyState(VK_V.0 as i32) < 0 }
}

pub fn paste_clipboard_image() -> Result<Option<PathBuf>, String> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|error| format!("无法读取剪贴板：{error}"))?;
    // WPS and Office put both text and a rendered picture on the clipboard when copying text.
    // Prefer the text so the source pane's normal paste is not overwritten by OCR.
    if clipboard
        .get_text()
        .is_ok_and(|text| !text.trim().is_empty())
    {
        return Ok(None);
    }
    for _ in 0..3 {
        match clipboard.get_image() {
            Ok(data) => return save_image(data).map(Some),
            Err(arboard::Error::ContentNotAvailable) => return Ok(None),
            Err(arboard::Error::ClipboardOccupied) => thread::sleep(Duration::from_millis(30)),
            Err(error) => return Err(format!("读取剪贴板图片失败：{error}")),
        }
    }
    Err("剪贴板正被占用，请重试粘贴。".into())
}

/// Captures are deleted on normal exit; this removes ones left behind by a killed process.
pub fn remove_stale_captures() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_capture_file_name) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_CAPTURE_AGE);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Matches only names produced by `save_image`: `ocr-translate-<pid>-<nanos>.png`.
fn is_capture_file_name(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix("ocr-translate-")
        .and_then(|rest| rest.strip_suffix(".png"))
    else {
        return false;
    };
    let parts: Vec<&str> = rest.split('-').collect();
    parts.len() == 2
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn restore_main_window() {
    if let Ok(window) = unsafe { FindWindowW(None, w!("OCR Translate")) } {
        unsafe {
            let _ = ShowWindowAsync(window, SW_RESTORE);
            let _ = SetForegroundWindow(window);
        }
    }
}

/// Text or a bitmap copied before the snip, put back once the screenshot has been read.
enum ClipboardBackup {
    Text(String),
    Image(arboard::ImageData<'static>),
}

impl ClipboardBackup {
    fn take() -> Option<Self> {
        let mut clipboard = arboard::Clipboard::new().ok()?;
        match clipboard.get_text() {
            Ok(text) if !text.is_empty() => Some(Self::Text(text)),
            _ => clipboard.get_image().ok().map(Self::Image),
        }
    }

    fn restore(self) {
        let Ok(mut clipboard) = arboard::Clipboard::new() else {
            return;
        };
        let _ = match self {
            Self::Text(text) => clipboard.set_text(text),
            Self::Image(image) => clipboard.set_image(image),
        };
    }
}

fn capture_region_inner(hotkey: Option<Hotkey>) -> Result<PathBuf, String> {
    // Let the global shortcut keys go before invoking the system snip shortcut.
    if let Some(hotkey) = hotkey {
        let keys = [VK_CONTROL, VK_SHIFT, VK_MENU, VIRTUAL_KEY(hotkey.key)];
        let release_deadline = Instant::now() + Duration::from_secs(3);
        while keys
            .iter()
            .any(|key| unsafe { GetAsyncKeyState(key.0 as i32) } < 0)
        {
            if Instant::now() >= release_deadline {
                return Err(format!("请先松开 {hotkey}，再重试截图。"));
            }
            thread::sleep(Duration::from_millis(30));
        }
    }
    let backup = ClipboardBackup::take();
    let previous_sequence = unsafe { GetClipboardSequenceNumber() };
    open_windows_snip()?;

    let started = Instant::now();
    let mut sequence = previous_sequence;
    // Only a fresh Escape press counts; one already held before the snip started is ignored.
    let mut escape_was_down = escape_down();
    let mut overlay_seen = false;
    let mut overlay_left_at: Option<Instant> = None;
    while started.elapsed() < Duration::from_secs(60) {
        let escape_is_down = escape_down();
        if escape_is_down && !escape_was_down {
            return Err("已取消框选截图。".into());
        }
        escape_was_down = escape_is_down;

        // The selector is a foreground Snipping Tool window. Once it has been seen and then
        // loses the foreground without a new clipboard image, the user cancelled it.
        if foreground_is_snipping_tool() {
            overlay_seen = true;
            overlay_left_at = None;
        } else if overlay_seen {
            let left_at = *overlay_left_at.get_or_insert_with(Instant::now);
            if left_at.elapsed() > Duration::from_millis(1500) {
                return Err("已取消框选截图。".into());
            }
        }

        let current = unsafe { GetClipboardSequenceNumber() };
        if current != sequence {
            sequence = current;
            // The screenshot app can briefly hold the clipboard open after writing it.
            for _ in 0..10 {
                let image =
                    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_image());
                if let Ok(data) = image {
                    let saved = save_image(data);
                    if let Some(backup) = backup {
                        backup.restore();
                    }
                    return saved;
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("等待框选截图超时。若已取消，请重试快捷键。".into())
}

fn escape_down() -> bool {
    unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) < 0 }
}

fn foreground_is_snipping_tool() -> bool {
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut process_id)) };
    if process_id == 0 {
        return false;
    }
    let Ok(process) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
    else {
        return false;
    };
    let mut buffer = [0u16; 260];
    let mut length = buffer.len() as u32;
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    if queried.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]).to_ascii_lowercase();
    // Windows 11 uses SnippingTool.exe; older builds use ScreenClippingHost.exe.
    ["\\snippingtool.exe", "\\screenclippinghost.exe"]
        .iter()
        .any(|name| path.ends_with(name))
}

fn save_image(data: arboard::ImageData<'_>) -> Result<PathBuf, String> {
    let image = image::RgbaImage::from_raw(
        data.width as u32,
        data.height as u32,
        data.bytes.into_owned(),
    )
    .ok_or("剪贴板图片像素数据无效。")?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("ocr-translate-{}-{stamp}.png", std::process::id()));
    image
        .save(&path)
        .map_err(|error| format!("保存临时图片失败：{error}"))?;
    Ok(path)
}

fn key_event(key: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }
}

fn open_windows_snip() -> Result<(), String> {
    let inputs = [
        key_event(VK_LWIN, false),
        key_event(VK_SHIFT, false),
        key_event(VK_S, false),
        key_event(VK_S, true),
        key_event(VK_SHIFT, true),
        key_event(VK_LWIN, true),
    ];
    let inserted = unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    if inserted != inputs.len() as u32 {
        let releases = [
            key_event(VK_S, true),
            key_event(VK_SHIFT, true),
            key_event(VK_LWIN, true),
        ];
        unsafe { SendInput(&releases, std::mem::size_of::<INPUT>() as i32) };
        return Err("无法启动 Windows 框选截图；请确认系统支持 Win+Shift+S。".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_round_trips_through_text() {
        for text in ["Ctrl+Shift+T", "Alt+F2", "Ctrl+Alt+Shift+7"] {
            assert_eq!(Hotkey::parse(text).unwrap().to_string(), text);
        }
        assert_eq!(Hotkey::parse("shift + ctrl + t"), Some(Hotkey::default()));
    }

    #[test]
    fn rejects_hotkeys_that_would_capture_typing() {
        assert!(Hotkey::parse("Shift+T").is_none());
        assert!(Hotkey::parse("T").is_none());
        assert!(Hotkey::parse("Ctrl+T+Y").is_none());
        assert!(Hotkey::parse("Ctrl+Enter").is_none());
    }

    #[test]
    fn hotkey_survives_thread_message_encoding() {
        let hotkey = Hotkey::parse("Ctrl+Alt+F12").unwrap();
        let (wparam, lparam) = hotkey.to_message();
        assert_eq!(Hotkey::from_message(wparam, lparam), hotkey);
    }

    #[test]
    fn only_capture_files_are_cleaned_up() {
        assert!(is_capture_file_name(
            "ocr-translate-24288-1791996083384609400.png"
        ));
        assert!(!is_capture_file_name("ocr-translate-chinese-test.png"));
        assert!(!is_capture_file_name("ocr-translate-1-2-3.png"));
        assert!(!is_capture_file_name("ocr-translate-24288-17.jpg"));
        assert!(!is_capture_file_name("other-24288-17.png"));
    }
}
