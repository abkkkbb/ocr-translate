use std::{
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};

use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0},
        System::Threading::{CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE},
        UI::WindowsAndMessaging::{
            AllowSetForegroundWindow, FindWindowW, GetWindowThreadProcessId,
        },
    },
};

static ACTIVATION_REQUESTED: AtomicBool = AtomicBool::new(false);

pub struct SingleInstance {
    _mutex: OwnedHandle,
    event: Arc<OwnedHandle>,
    stopping: Arc<AtomicBool>,
    listener: Option<JoinHandle<()>>,
}

fn handle(owned: &OwnedHandle) -> HANDLE {
    HANDLE(owned.as_raw_handle())
}

impl SingleInstance {
    pub fn acquire() -> windows::core::Result<Option<Self>> {
        Self::acquire_named(r"Local\OCRTranslate.SingleInstance.v1")
    }

    fn acquire_named(name: &str) -> windows::core::Result<Option<Self>> {
        let event_name: Vec<u16> = format!("{name}.Activate\0").encode_utf16().collect();
        let mutex_name: Vec<u16> = format!("{name}.Mutex\0").encode_utf16().collect();
        // Create the event first so a second launch during startup cannot lose its request.
        let event = unsafe { CreateEventW(None, false, false, PCWSTR(event_name.as_ptr())) }?;
        let event = Arc::new(unsafe { OwnedHandle::from_raw_handle(event.0) });
        let mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) }?;
        let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let mutex = unsafe { OwnedHandle::from_raw_handle(mutex.0) };
        if already_running {
            // The foreground launch may grant the existing process permission to focus itself.
            if let Ok(window) = unsafe { FindWindowW(None, w!("OCR Translate")) } {
                let mut process_id = 0;
                unsafe {
                    GetWindowThreadProcessId(window, Some(&mut process_id));
                    let _ = AllowSetForegroundWindow(process_id);
                }
            }
            unsafe { SetEvent(handle(&event)) }?;
            return Ok(None);
        }
        Ok(Some(Self {
            _mutex: mutex,
            event,
            stopping: Arc::new(AtomicBool::new(false)),
            listener: None,
        }))
    }

    pub fn listen(&mut self, wake: impl Fn() + Send + 'static) {
        let event = self.event.clone();
        let stopping = self.stopping.clone();
        self.listener = Some(thread::spawn(move || {
            while unsafe { WaitForSingleObject(handle(&event), INFINITE) } == WAIT_OBJECT_0 {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                ACTIVATION_REQUESTED.store(true, Ordering::Release);
                wake();
            }
        }));
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = unsafe { SetEvent(handle(&self.event)) };
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
    }
}

pub fn take_activation_request() -> bool {
    ACTIVATION_REQUESTED.swap(false, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_launch_signals_even_before_listener_starts_and_exit_releases_instance() {
        let name = format!(r"Local\OCRTranslate.Test.{}.startup", std::process::id());
        let first = SingleInstance::acquire_named(&name).unwrap().unwrap();
        assert!(SingleInstance::acquire_named(&name).unwrap().is_none());
        assert_eq!(
            unsafe { WaitForSingleObject(handle(&first.event), 0) },
            WAIT_OBJECT_0
        );
        drop(first);
        assert!(SingleInstance::acquire_named(&name).unwrap().is_some());
    }

    #[test]
    fn simultaneous_launches_elect_exactly_one_primary() {
        let name = format!(r"Local\OCRTranslate.Test.{}.race", std::process::id());
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let name = name.clone();
                thread::spawn(move || {
                    barrier.wait();
                    SingleInstance::acquire_named(&name).unwrap()
                })
            })
            .collect();
        let instances: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(
            instances
                .iter()
                .filter(|instance| instance.is_some())
                .count(),
            1
        );
        drop(instances);
        assert!(SingleInstance::acquire_named(&name).unwrap().is_some());
    }
}
