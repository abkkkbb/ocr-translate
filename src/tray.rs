use std::sync::{mpsc, OnceLock};

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{IsIconic, IsWindowVisible, PostMessageW, WM_PAINT},
};

static MAIN_WINDOW: OnceLock<usize> = OnceLock::new();

pub enum Action {
    Open,
    Settings,
    Capture,
    Exit,
    Lightweight,
}

pub struct AppTray {
    _icon: TrayIcon,
    pub actions: mpsc::Receiver<Action>,
    lightweight: CheckMenuItem,
}

pub fn main_window_visible() -> bool {
    MAIN_WINDOW.get().is_none_or(|&window| {
        let window = HWND(window as *mut _);
        unsafe { IsWindowVisible(window).as_bool() && !IsIconic(window).as_bool() }
    })
}

/// Hidden root windows must still process hotkey and tray events.
pub fn wake(ctx: &egui::Context) {
    ctx.request_repaint();
    if let Some(&window) = MAIN_WINDOW.get() {
        let window = HWND(window as *mut _);
        unsafe {
            if !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
                let _ = PostMessageW(Some(window), WM_PAINT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl AppTray {
    pub fn bind_window(cc: &eframe::CreationContext<'_>) {
        if let Ok(handle) = cc.window_handle() {
            if let RawWindowHandle::Win32(handle) = handle.as_raw() {
                let _ = MAIN_WINDOW.set(handle.hwnd.get() as usize);
            }
        }
    }

    pub fn set_lightweight(&self, enabled: bool) {
        self.lightweight.set_checked(enabled);
    }

    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Result<Self, String> {
        let menu = Menu::new();
        let open = MenuItem::with_id("open", "打开主窗口", true, None);
        let capture = MenuItem::with_id("capture", "框选翻译", true, None);
        let settings = MenuItem::with_id("settings", "设置", true, None);
        let exit = MenuItem::with_id("exit", "退出", true, None);
        let lightweight = CheckMenuItem::with_id("lightweight", "轻量模式", true, false, None);
        menu.append_items(&[
            &open,
            &capture,
            &settings,
            &PredefinedMenuItem::separator(),
            &lightweight,
            &PredefinedMenuItem::separator(),
            &exit,
        ])
        .map_err(|error| error.to_string())?;

        let image = image::load_from_memory(include_bytes!("../assets/icon_64.png"))
            .map_err(|error| error.to_string())?
            .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = tray_icon::Icon::from_rgba(image.into_raw(), width, height)
            .map_err(|error| error.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_id("ocr-translate")
            .with_tooltip("OCR Translate")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| error.to_string())?;

        let (sender, actions) = mpsc::channel();
        let menu_sender = sender.clone();
        let wake = std::sync::Arc::new(wake);
        let menu_wake = wake.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = match event.id.as_ref() {
                "open" => Action::Open,
                "capture" => Action::Capture,
                "settings" => Action::Settings,
                "exit" => Action::Exit,
                "lightweight" => Action::Lightweight,
                _ => return,
            };
            let _ = menu_sender.send(action);
            menu_wake();
        }));
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                let _ = sender.send(Action::Open);
                wake();
            }
        }));
        Ok(Self {
            _icon: icon,
            actions,
            lightweight,
        })
    }
}
