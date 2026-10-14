use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use windows::{
    core::{w, PWSTR},
    Win32::{
        Foundation::CloseHandle,
        System::{
            DataExchange::GetClipboardSequenceNumber,
            Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION,
            },
        },
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0,
                INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_CONTROL,
                MOD_NOREPEAT, MOD_SHIFT, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_S, VK_SHIFT, VK_T,
                VK_V,
            },
            WindowsAndMessaging::{
                FindWindowW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
                SetForegroundWindow, ShowWindowAsync, MSG, SW_RESTORE, WM_HOTKEY,
            },
        },
    },
};

const HOTKEY_ID: i32 = 1;

pub enum HotkeyEvent {
    Ready,
    Started,
    Captured(Result<PathBuf, String>),
    Failed(String),
}

/// RegisterHotKey delivers WM_HOTKEY to the registering thread's message queue.
pub fn listen_for_hotkey(ctx: egui::Context) -> Receiver<HotkeyEvent> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let modifiers = MOD_CONTROL | MOD_SHIFT | MOD_NOREPEAT;
        if let Err(error) = unsafe { RegisterHotKey(None, HOTKEY_ID, modifiers, VK_T.0 as u32) } {
            let _ = tx.send(HotkeyEvent::Failed(format!(
                "Ctrl+Shift+T 注册失败：{error}"
            )));
            ctx.request_repaint();
            return;
        }
        let _ = tx.send(HotkeyEvent::Ready);
        ctx.request_repaint();
        let mut message = MSG::default();
        while unsafe { GetMessageW(&mut message, None, 0, 0) }.as_bool() {
            if message.message == WM_HOTKEY && message.wParam.0 == HOTKEY_ID as usize {
                if tx.send(HotkeyEvent::Started).is_err() {
                    break;
                }
                ctx.request_repaint();
                let result = capture_region();
                if tx.send(HotkeyEvent::Captured(result)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        }
        let _ = unsafe { UnregisterHotKey(None, HOTKEY_ID) };
    });
    rx
}

pub fn capture_region() -> Result<PathBuf, String> {
    let result = capture_region_inner();
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

fn restore_main_window() {
    if let Ok(window) = unsafe { FindWindowW(None, w!("OCR Translate")) } {
        unsafe {
            let _ = ShowWindowAsync(window, SW_RESTORE);
            let _ = SetForegroundWindow(window);
        }
    }
}

fn capture_region_inner() -> Result<PathBuf, String> {
    // Let the global shortcut keys go before invoking the system snip shortcut.
    let release_deadline = Instant::now() + Duration::from_secs(3);
    while [VK_CONTROL, VK_SHIFT, VK_T]
        .iter()
        .any(|key| unsafe { GetAsyncKeyState(key.0 as i32) } < 0)
    {
        if Instant::now() >= release_deadline {
            return Err("请先松开 Ctrl+Shift+T，再重试截图。".into());
        }
        thread::sleep(Duration::from_millis(30));
    }
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
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(data) = clipboard.get_image() {
                        return save_image(data);
                    }
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

fn key_event(key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY, key_up: bool) -> INPUT {
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
