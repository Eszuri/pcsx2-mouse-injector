#![windows_subsystem = "windows"]

mod app;
mod config;
mod games;
mod injector;
mod memory;
mod raw_input;

use app::MouseInjectorApp;
use config::AppConfig;
use injector::{spawn_injector_thread, SharedState};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{ClipCursor, FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE};

struct SingleInstanceGuard(HANDLE);

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            ClipCursor(std::ptr::null());
        }
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn check_single_instance() -> Result<SingleInstanceGuard, ()> {
    let mutex_name: Vec<u16> = "Local\\PCSX2_Mouse_Injector_SingleInstance_Mutex\0"
        .encode_utf16()
        .collect();

    let handle = unsafe { CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr()) };
    if handle.is_null() {
        return Ok(SingleInstanceGuard(std::ptr::null_mut()));
    }

    let err = unsafe { GetLastError() };
    if err == ERROR_ALREADY_EXISTS {
        log("Duplicate instance detected. Focusing existing window and terminating.");

        let title: Vec<u16> = "PCSX2 Mouse Injector\0".encode_utf16().collect();
        let hwnd = unsafe { FindWindowW(std::ptr::null(), title.as_ptr()) };
        if !hwnd.is_null() {
            unsafe {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
        }

        unsafe { CloseHandle(handle) };
        return Err(());
    }

    Ok(SingleInstanceGuard(handle))
}

fn get_app_dir() -> std::path::PathBuf {
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let p = std::path::PathBuf::from(local_app_data).join("PCSX2MouseInjector");
        let _ = std::fs::create_dir_all(&p);
        p
    } else if let Ok(mut exe) = std::env::current_exe() {
        exe.pop();
        exe
    } else {
        std::path::PathBuf::from(".")
    }
}

fn get_config_path() -> String {

    if let Ok(mut exe_dir) = std::env::current_exe() {
        exe_dir.set_file_name("mouseinjector.json");
        if exe_dir.exists() {
            return exe_dir.to_string_lossy().to_string();
        }
    }

    let cfg_path = get_app_dir().join("mouseinjector.json");
    cfg_path.to_string_lossy().to_string()
}

use std::fs::OpenOptions;
use std::io::Write;

pub fn log(msg: &str) {
    let log_path = get_app_dir().join("injector_run.log");

    if let Ok(meta) = std::fs::metadata(&log_path)
        && meta.len() > 512 * 1024
    {
        let old_log = get_app_dir().join("injector_run.old.log");
        let _ = std::fs::rename(&log_path, &old_log);
    }

    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = writeln!(f, "[{}] {}", chrono_or_now(), msg);
    }
}

fn chrono_or_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn main() -> eframe::Result<()> {
    std::panic::set_hook(Box::new(|info| {
        log(&format!("PANIC: {:?}", info));
    }));

    let _single_instance_guard = match check_single_instance() {
        Ok(guard) => guard,
        Err(()) => {

            return Ok(());
        }
    };

    log("main() started");

    let config_file = get_config_path();
    let cfg = AppConfig::load_or_default(&config_file);
    log(&format!("config loaded: {:?}", cfg));

    let shared_state = SharedState::new(cfg.mode, cfg.sensitivity, cfg.invert_pitch);
    spawn_injector_thread(shared_state.clone());
    log("spawn_injector_thread complete");

    let icon_data = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).ok();

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([540.0, 580.0])
        .with_min_inner_size([460.0, 480.0])
        .with_resizable(true)
        .with_title("PCSX2 Mouse Injector");

    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(icon);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    log("calling eframe::run_native");
    let res = eframe::run_native(
        "PCSX2 Mouse Injector",
        native_options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(eframe::egui::Visuals::dark());
            log("CreationContext initialized successfully in closure");
            Ok(Box::new(MouseInjectorApp::new(cc, shared_state, config_file)))
        }),
    );

    if let Err(ref e) = res {
        log(&format!("eframe::run_native error: {:?}", e));
    } else {
        log("eframe::run_native finished with Ok(())");
    }

    res
}
