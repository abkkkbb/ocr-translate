//! Private inherited pipes carry UI commands and results; no sockets or credential files.
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{screen_clip, AppConfig, RequestResult, TranslationResult};

const MAX_FRAME_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum View {
    Main,
    Settings,
    Popup,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub config: Option<AppConfig>,
    pub result: TranslationResult,
    pub selected_file: Option<PathBuf>,
    pub temporary_capture: Option<PathBuf>,
    pub status: String,
    pub anchor: Option<[i32; 2]>,
    pub task_id: Option<u64>,
}

#[derive(Serialize, Deserialize)]
pub enum Command {
    Open(View),
    Quit,
    Config(Box<AppConfig>),
    Capture(Box<AppConfig>),
    Image {
        config: Box<AppConfig>,
        path: PathBuf,
        temporary: bool,
    },
    Text {
        config: Box<AppConfig>,
        source: String,
    },
    Suspend(Box<Session>),
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    Init {
        session: Box<Session>,
        view: View,
        lightweight: bool,
        tray_available: bool,
        active_hotkey: Option<screen_clip::Hotkey>,
        busy: bool,
    },
    Show(View),
    Lightweight(bool),
    Hotkey(screen_clip::HotkeyEvent),
    Accepted {
        task_id: u64,
        path: PathBuf,
        popup: bool,
        anchor: [i32; 2],
    },
    TaskStarted(u64),
    Request {
        task_id: u64,
        result: RequestResult,
    },
    Quit,
}

pub fn read_frame<T: DeserializeOwned>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut bytes = Vec::new();
    let count = reader
        .by_ref()
        .take(MAX_FRAME_BYTES + 1)
        .read_until(b'\n', &mut bytes)?;
    if count == 0 {
        return Ok(None);
    }
    if count as u64 > MAX_FRAME_BYTES || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid IPC frame length",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(io::Error::other)
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

pub struct Client {
    pub events: Receiver<Event>,
    commands: Sender<Command>,
}

impl Client {
    pub fn connect(ctx: eframe::egui::Context) -> Self {
        let (event_tx, events) = mpsc::channel();
        thread::spawn(move || {
            let mut input = BufReader::new(io::stdin());
            while let Ok(Some(event)) = read_frame(&mut input) {
                if event_tx.send(event).is_err() {
                    return;
                }
                crate::tray::wake(&ctx);
            }
            let _ = event_tx.send(Event::Quit);
            crate::tray::wake(&ctx);
        });
        let (commands, output) = mpsc::channel();
        thread::spawn(move || {
            let mut stdout = io::stdout().lock();
            for command in output {
                if write_frame(&mut stdout, &command).is_err() {
                    break;
                }
            }
        });
        Self { events, commands }
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "后台连接已断开，请重新启动应用。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_keeps_newlines_inside_text_and_rejects_partial_frames() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &Command::Text {
                config: Box::default(),
                source: "a\nb".into(),
            },
        )
        .unwrap();
        let mut reader = bytes.as_slice();
        assert!(
            matches!(read_frame::<Command>(&mut reader).unwrap(), Some(Command::Text { source, .. }) if source == "a\nb")
        );
        assert!(read_frame::<Command>(&mut reader).unwrap().is_none());
        assert!(read_frame::<Command>(&mut b"{\"Quit\":".as_slice()).is_err());
    }

    #[test]
    fn layout_ipc_does_not_copy_pixels() {
        let page = crate::layout::ImageLayout {
            width: 2,
            height: 1,
            rgba: vec![255; 8],
            blocks: Vec::new(),
            dispersed: false,
        };
        let bytes = serde_json::to_vec(&page).unwrap();
        let decoded: crate::layout::ImageLayout = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded.width, 2);
        assert!(decoded.rgba.is_empty());
    }
}
