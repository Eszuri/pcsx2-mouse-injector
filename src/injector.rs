use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Media::timeBeginPeriod;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect,
    GetWindowThreadProcessId, SetCursorPos, SM_CXSCREEN, SM_CYSCREEN,
};

use crate::config::InjectionMode;
use crate::games::{get_available_games, GameDriver};
use crate::memory::Pcsx2Process;
use crate::raw_input::{start_raw_input_listener, RawMouseAccumulator};

pub struct SharedState {
    pub mode: AtomicU32,
    pub sensitivity: AtomicU32,
    pub invert_pitch: AtomicBool,

    pub pcsx2_connected: AtomicBool,
    pub pcsx2_pid: AtomicU32,
    pub pcsx2_name: RwLock<String>,
    pub pcsx2_focused: AtomicBool,
    pub pcsx2_paused: AtomicBool,
    pub active_game_name: RwLock<Option<String>>,
    pub is_injecting: AtomicBool,
}

impl SharedState {
    pub fn new(mode: InjectionMode, sensitivity: u32, invert_pitch: bool) -> Arc<Self> {
        let mode_val = match mode {
            InjectionMode::Off => 0,
            InjectionMode::Auto => 1,
        };
        Arc::new(Self {
            mode: AtomicU32::new(mode_val),
            sensitivity: AtomicU32::new(sensitivity),
            invert_pitch: AtomicBool::new(invert_pitch),

            pcsx2_connected: AtomicBool::new(false),
            pcsx2_pid: AtomicU32::new(0),
            pcsx2_name: RwLock::new(String::new()),
            pcsx2_focused: AtomicBool::new(false),
            pcsx2_paused: AtomicBool::new(false),
            active_game_name: RwLock::new(None),
            is_injecting: AtomicBool::new(false),
        })
    }

    pub fn get_mode(&self) -> InjectionMode {
        match self.mode.load(Ordering::Relaxed) {
            1 => InjectionMode::Auto,
            _ => InjectionMode::Off,
        }
    }

    pub fn set_mode(&self, mode: InjectionMode) {
        let val = match mode {
            InjectionMode::Off => 0,
            InjectionMode::Auto => 1,
        };
        self.mode.store(val, Ordering::Relaxed);
    }
}

pub fn spawn_injector_thread(state: Arc<SharedState>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        unsafe { timeBeginPeriod(1) };

        let accumulator = RawMouseAccumulator::new();
        start_raw_input_listener(accumulator.clone());

        let games: Vec<Box<dyn GameDriver>> = get_available_games();
        let mut pcsx2: Option<Pcsx2Process> = None;
        let mut last_scan = Instant::now() - Duration::from_secs(5);
        let mut last_eemem_scan = Instant::now() - Duration::from_secs(5);
        let mut active_game_idx: Option<usize> = None;

        let mut lock_pos = POINT { x: 0, y: 0 };
        let mut last_cur = POINT { x: 0, y: 0 };
        let mut suppress_cursor_frames: u32 = 0;
        let mut was_injecting = false;
        let mut lock_counter: u32 = 0;

        loop {
            let mut proc_alive = false;
            if let Some(ref mut proc) = pcsx2 {
                if proc.is_still_alive() {
                    proc_alive = true;
                } else {
                    pcsx2 = None;
                }
            }

            if !proc_alive {
                state.pcsx2_connected.store(false, Ordering::Relaxed);
                state.pcsx2_pid.store(0, Ordering::Relaxed);
                *state.pcsx2_name.write().unwrap() = String::new();
                state.pcsx2_focused.store(false, Ordering::Relaxed);
                state.pcsx2_paused.store(false, Ordering::Relaxed);
                *state.active_game_name.write().unwrap() = None;
                active_game_idx = None;

                if last_scan.elapsed() >= Duration::from_millis(600) {
                    last_scan = Instant::now();
                    if let Some(new_proc) = Pcsx2Process::find_and_attach() {
                        crate::log(&format!("PCSX2 attached: {} (PID: {})", new_proc.name, new_proc.pid));
                        state.pcsx2_connected.store(true, Ordering::Relaxed);
                        state.pcsx2_pid.store(new_proc.pid, Ordering::Relaxed);
                        *state.pcsx2_name.write().unwrap() = new_proc.name.clone();
                        pcsx2 = Some(new_proc);
                    }
                }

                if was_injecting {
                    was_injecting = false;
                    unsafe { ClipCursor(std::ptr::null()) };
                    state.is_injecting.store(false, Ordering::Relaxed);
                    accumulator.reset();
                }

                std::thread::sleep(Duration::from_millis(50));
                continue;
            }

            let proc = pcsx2.as_mut().unwrap();

            unsafe {
                let fg = GetForegroundWindow();
                if !fg.is_null() {
                    let mut fg_pid = 0u32;
                    GetWindowThreadProcessId(fg, &mut fg_pid);
                    let my_pid = windows_sys::Win32::System::Threading::GetCurrentProcessId();
                    if fg_pid != 0
                        && fg_pid != proc.pid
                        && fg_pid != my_pid
                        && let Some(fg_name) = crate::memory::get_process_name(fg_pid)
                        && crate::memory::is_pcsx2_exe_name(&fg_name)
                        && let Some(new_proc) = Pcsx2Process::attach_pid(fg_pid, fg_name)
                    {
                        crate::log(&format!(
                            "Switched attachment to active PCSX2 window: {} (PID: {})",
                            new_proc.name, new_proc.pid
                        ));
                        state.pcsx2_pid.store(new_proc.pid, Ordering::Relaxed);
                        *state.pcsx2_name.write().unwrap() = new_proc.name.clone();
                        *proc = new_proc;
                        active_game_idx = None;
                        *state.active_game_name.write().unwrap() = None;
                        last_eemem_scan = Instant::now() - Duration::from_secs(10);
                    }
                }
            }

            let is_focused = proc.is_focused();
            let is_paused = proc.update_pause_state();
            state.pcsx2_focused.store(is_focused, Ordering::Relaxed);
            state.pcsx2_paused.store(is_paused, Ordering::Relaxed);

            let emu_running = proc.is_emulation_running();
            if !emu_running {
                proc.ram_base = 0;
                if active_game_idx.is_some() {
                    crate::log("PCSX2 returned to game list / emulation stopped");
                    active_game_idx = None;
                    *state.active_game_name.write().unwrap() = None;
                }
            } else if proc.ram_base != 0 && !proc.is_eemem_valid() {
                proc.ram_base = 0;
                if active_game_idx.is_some() {
                    crate::log("EEmem unmapped / invalidated");
                    active_game_idx = None;
                    *state.active_game_name.write().unwrap() = None;
                }
            }

            if emu_running && proc.ram_base == 0 && last_eemem_scan.elapsed() >= Duration::from_millis(500) {
                last_eemem_scan = Instant::now();
                if proc.find_eemem_base() {
                    crate::log(&format!("EEmem base located at 0x{:X}", proc.ram_base));
                }
            }

            if emu_running && proc.ram_base != 0 {
                let mem = proc.memory();
                if let Some(idx) = active_game_idx
                    && !games[idx].detect(&mem)
                {
                    crate::log("Game unloaded / changed");
                    active_game_idx = None;
                    *state.active_game_name.write().unwrap() = None;
                }

                if active_game_idx.is_none() {
                    for (i, game) in games.iter().enumerate() {
                        if game.detect(&mem) {
                            crate::log(&format!("Game detected: {}", game.name()));
                            active_game_idx = Some(i);
                            *state.active_game_name.write().unwrap() = Some(game.name().to_string());
                            break;
                        }
                    }
                }
            } else {
                active_game_idx = None;
                *state.active_game_name.write().unwrap() = None;
            }

            let mode = state.get_mode();
            let should_inject = match mode {
                InjectionMode::Off => false,
                InjectionMode::Auto => is_focused && !is_paused && active_game_idx.is_some(),
            };

            if should_inject {
                if !was_injecting {
                    was_injecting = true;
                    state.is_injecting.store(true, Ordering::Relaxed);
                    accumulator.reset();
                    crate::raw_input::ensure_raw_input_registered();
                    crate::log("Injection ACTIVATED (PCSX2 in foreground and emulation active)");

                    unsafe {
                        let fg = GetForegroundWindow();
                        let mut rect: RECT = std::mem::zeroed();
                        if !fg.is_null() && GetWindowRect(fg, &mut rect) != 0 {
                            lock_pos = POINT {
                                x: (rect.left + rect.right) / 2,
                                y: (rect.top + rect.bottom) / 2,
                            };
                            ClipCursor(&rect);
                        } else {
                            let screen_w = GetSystemMetrics(SM_CXSCREEN);
                            let screen_h = GetSystemMetrics(SM_CYSCREEN);
                            lock_pos = POINT {
                                x: screen_w / 2,
                                y: screen_h / 2,
                            };
                        }
                        SetCursorPos(lock_pos.x, lock_pos.y);
                        last_cur = lock_pos;
                        suppress_cursor_frames = 5;
                    }
                }

                let game_idx = active_game_idx.unwrap();
                let game = &games[game_idx];
                let tickrate = game.tickrate_ms();

                // 1. Hardware Raw Input
                let (raw_dx, raw_dy) = accumulator.take();

                // 2. Cursor position tracking (frame-to-frame delta fallback)
                let mut cur = POINT { x: 0, y: 0 };
                let cur_ok = unsafe { GetCursorPos(&mut cur) != 0 };

                let (cur_dx, cur_dy) = if cur_ok && suppress_cursor_frames == 0 {
                    let d_x = cur.x - last_cur.x;
                    let d_y = cur.y - last_cur.y;
                    // Glitch / click-warp rejection: at 1ms tickrate, no human hand can move > 35 px/ms.
                    // Clicking or window message handling can create momentary > 40px leaps.
                    if d_x.abs() > 35 || d_y.abs() > 35 {
                        (0, 0)
                    } else {
                        (d_x, d_y)
                    }
                } else {
                    (0, 0)
                };

                if suppress_cursor_frames > 0 {
                    suppress_cursor_frames -= 1;
                }
                if cur_ok {
                    last_cur = cur;
                }

                // 3. Recenter cursor if it drifts away from center (threshold: 80 pixels)
                // or periodically every 50 ticks to keep cursor near center of PCSX2 window
                let drift_x = (cur.x - lock_pos.x).abs();
                let drift_y = (cur.y - lock_pos.y).abs();
                if drift_x > 80 || drift_y > 80 || lock_counter % 50 == 0 {
                    unsafe {
                        SetCursorPos(lock_pos.x, lock_pos.y);
                    }
                    last_cur = lock_pos;
                    suppress_cursor_frames = 2; // suppress next 2 frames while SetCursorPos applies
                }
                lock_counter = lock_counter.wrapping_add(1);

                // Periodic Raw Input registration refresh every 500 ticks (~500ms)
                if lock_counter % 500 == 0 {
                    crate::raw_input::ensure_raw_input_registered();
                }

                // 4. Select delta: prefer Raw Input if non-zero; otherwise fall back to safe frame delta
                let (dx, dy) = if raw_dx != 0 || raw_dy != 0 {
                    (raw_dx.clamp(-500, 500), raw_dy.clamp(-350, 350))
                } else {
                    (cur_dx.clamp(-35, 35), cur_dy.clamp(-35, 35))
                };

                if dx != 0 || dy != 0 {
                    static LAST_INPUT_LOG: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let last = LAST_INPUT_LOG.load(Ordering::Relaxed);
                    let is_flick = dx.abs() > 35 || dy.abs() > 35;

                    if is_flick || now_ms.saturating_sub(last) >= 200 {
                        LAST_INPUT_LOG.store(now_ms, Ordering::Relaxed);
                        let src = if raw_dx != 0 || raw_dy != 0 { "raw" } else { "cursor_fallback" };
                        crate::log(&format!(
                            "[INPUT:{}] raw=({},{}) cur=({},{}) -> applied=({},{})",
                            src, raw_dx, raw_dy, cur_dx, cur_dy, dx, dy
                        ));
                    }
                }

                let sens = state.sensitivity.load(Ordering::Relaxed);
                let invert = state.invert_pitch.load(Ordering::Relaxed);

                let mem = proc.memory();
                game.inject(&mem, dx, dy, sens, invert);

                std::thread::sleep(Duration::from_millis(tickrate.max(1)));
            } else {
                if was_injecting {
                    was_injecting = false;
                    unsafe { ClipCursor(std::ptr::null()) };
                    state.is_injecting.store(false, Ordering::Relaxed);
                    accumulator.reset();
                    let reason = if is_paused {
                        "PCSX2 emulator paused"
                    } else if !is_focused {
                        "PCSX2 lost focus"
                    } else if active_game_idx.is_none() {
                        "No supported game loaded"
                    } else {
                        "Injection mode disabled"
                    };
                    crate::log(&format!("Injection DEACTIVATED ({})", reason));
                }

                std::thread::sleep(Duration::from_millis(15));
            }
        }
    })
}
