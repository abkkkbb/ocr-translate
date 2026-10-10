use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use eframe::egui;
use windows::{
    core::w,
    Win32::{
        System::DataExchange::GetClipboardSequenceNumber,
        UI::{
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, RegisterHotKey, SendInput, UnregisterHotKey, INPUT, INPUT_0,
                INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, MOD_CONTROL,
                MOD_NOREPEAT, MOD_SHIFT, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_S, VK_SHIFT, VK_T,
            },
            WindowsAndMessaging::{
                FindWindowW, GetMessageW, SetForegroundWindow, ShowWindowAsync, MSG, SW_RESTORE,
                WM_HOTKEY,
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
    while started.elapsed() < Duration::from_secs(60) {
        // Escape closes the Windows selector without modifying the clipboard.
        if unsafe { GetAsyncKeyState(VK_ESCAPE.0 as i32) } & 1 != 0 {
            return Err("已取消框选截图。".into());
        }
        let current = unsafe { GetClipboardSequenceNumber() };
        if current != sequence {
            sequence = current;
            // The screenshot app can briefly hold the clipboard open after writing it.
            for _ in 0..10 {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(data) = clipboard.get_image() {
                        let image = image::RgbaImage::from_raw(
                            data.width as u32,
                            data.height as u32,
                            data.bytes.into_owned(),
                        )
                        .ok_or("截图像素数据无效。")?;
                        let stamp = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_err(|error| error.to_string())?
                            .as_nanos();
                        let path = std::env::temp_dir()
                            .join(format!("ocr-translate-{}-{stamp}.png", std::process::id()));
                        image
                            .save(&path)
                            .map_err(|error| format!("保存临时截图失败：{error}"))?;
                        return Ok(path);
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("等待框选截图超时。若已取消，请重试快捷键。".into())
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
