use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Media::timeBeginPeriod;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetSystemMetrics, GetWindowRect, SetCursorPos, SM_CXSCREEN,
    SM_CYSCREEN,
};

use crate::config::InjectionMode;
use crate::games::{get_available_games, GameDriver};
use crate::memory::Pcsx2Process;
use crate::raw_input::{start_raw_input_listener, RawMouseAccumulator};

pub struct SharedState {
    // User Settings
    pub mode: AtomicU32,          // 0: Off, 1: On, 2: Auto
    pub sensitivity: AtomicU32,   // 1 .. 200 (20 = 100%)
    pub invert_pitch: AtomicBool, // true / false

    // Real-time Status
    pub pcsx2_connected: AtomicBool,
    pub pcsx2_pid: AtomicU32,
    pub pcsx2_name: RwLock<String>,
    pub pcsx2_focused: AtomicBool,
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
        let mut was_injecting = false;
        let mut lock_counter: u32 = 0;

        loop {
            // Process check / attachment
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
                    state.is_injecting.store(false, Ordering::Relaxed);
                    accumulator.reset();
                }

                std::thread::sleep(Duration::from_millis(50));
                continue;
            }

            let proc = pcsx2.as_mut().unwrap();

            // 3. Focus check
            let is_focused = proc.is_focused();
            state.pcsx2_focused.store(is_focused, Ordering::Relaxed);

            // 4. EEmem check (throttled to 1000ms to avoid aggressive continuous memory scanning)
            if proc.ram_base == 0 && last_eemem_scan.elapsed() >= Duration::from_millis(1000) {
                last_eemem_scan = Instant::now();
                if proc.find_eemem_base() {
                    crate::log(&format!("EEmem base located at 0x{:X}", proc.ram_base));
                }
            }

            // 5. Game detection (if RAM base is known)
            if proc.ram_base != 0 {
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

            // 6. Injection State Decision
            let mode = state.get_mode();
            let should_inject = match mode {
                InjectionMode::Off => false,
                InjectionMode::Auto => is_focused && active_game_idx.is_some(),
            };

            if should_inject {
                if !was_injecting {
                    was_injecting = true;
                    state.is_injecting.store(true, Ordering::Relaxed);
                    accumulator.reset();
                    crate::log("Injection ACTIVATED (should_inject = true)");

                    // Center cursor inside PCSX2 window (supports multi-monitor setups)
                    unsafe {
                        let fg = GetForegroundWindow();
                        let mut rect: RECT = std::mem::zeroed();
                        if !fg.is_null() && GetWindowRect(fg, &mut rect) != 0 {
                            lock_pos = POINT {
                                x: (rect.left + rect.right) / 2,
                                y: (rect.top + rect.bottom) / 2,
                            };
                        } else {
                            let screen_w = GetSystemMetrics(SM_CXSCREEN);
                            let screen_h = GetSystemMetrics(SM_CYSCREEN);
                            lock_pos = POINT {
                                x: screen_w / 2,
                                y: screen_h / 2,
                            };
                        }
                        SetCursorPos(lock_pos.x, lock_pos.y);
                    }
                }

                let game_idx = active_game_idx.unwrap();
                let game = &games[game_idx];
                let tickrate = game.tickrate_ms();

                // Poll raw mouse delta from RAWINPUT
                let (raw_dx, raw_dy) = accumulator.take();

                // Also poll cursor position delta as fail-proof companion
                let mut cur = POINT { x: 0, y: 0 };
                unsafe { GetCursorPos(&mut cur) };
                let cur_dx = cur.x - lock_pos.x;
                let cur_dy = cur.y - lock_pos.y;

                // If cursor moved or on periodic interval, reset to center
                if cur_dx != 0 || cur_dy != 0 || lock_counter.is_multiple_of(20) {
                    unsafe { SetCursorPos(lock_pos.x, lock_pos.y) };
                }
                lock_counter = lock_counter.wrapping_add(1);

                // Combine: prefer raw delta, fallback to cursor delta
                let dx = if raw_dx != 0 { raw_dx } else { cur_dx };
                let dy = if raw_dy != 0 { raw_dy } else { cur_dy };

                if dx != 0 || dy != 0 {
                    static LOG_DELTA_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                    if LOG_DELTA_COUNT.load(Ordering::Relaxed) < 10 {
                        LOG_DELTA_COUNT.fetch_add(1, Ordering::Relaxed);
                        crate::log(&format!("Input delta: dx={}, dy={} (raw: {},{}, cur: {},{})", dx, dy, raw_dx, raw_dy, cur_dx, cur_dy));
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
                    state.is_injecting.store(false, Ordering::Relaxed);
                    accumulator.reset();
                    crate::log("Injection DEACTIVATED (should_inject = false)");
                }

                std::thread::sleep(Duration::from_millis(15));
            }
        }
    })
}
