use crate::memory::Pcsx2Memory;
use super::GameDriver;

pub struct BlackGame;

const BL_CAM_X: u32 = 0x5A8FA8;
const BL_CAM_Y: u32 = 0x5A8FAC;
const SENSITIVITY_DIVISOR: f32 = 500.0;
const PITCH_RATIO_SCALE: f32 = 4.0 / 3.0;

fn check_serial(mem: &Pcsx2Memory, addr: u32) -> bool {
    let mut buf = [0u8; 12];
    if mem.read_bytes(addr, &mut buf) {
        // Check ASCII bytes directly
        if buf.starts_with(b"SLUS_213.76") || buf.starts_with(b"SLUS-21376") {
            return true;
        }
        if buf.starts_with(b"SLES_538.86") || buf.starts_with(b"SLES-53886") {
            return true;
        }
    }

    let w0 = mem.read_word_swapped(addr);
    let w1 = mem.read_word_swapped(addr + 4);
    let w2 = mem.read_word_swapped(addr + 8);

    // USA: SLUS_213.76;
    if w0 == 0x534C5553 && w1 == 0x5F323133 && (w2 == 0x2E37363B || (w2 & 0xFFFF_FF00) == 0x2E373600) {
        return true;
    }

    // PAL: SLES_538.86;
    if w0 == 0x534C4553 && w1 == 0x5F353338 && (w2 == 0x2E38363B || (w2 & 0xFFFF_FF00) == 0x2E383600) {
        return true;
    }

    false
}

impl GameDriver for BlackGame {
    fn name(&self) -> &'static str {
        "Black (SLUS-21376 / SLES-53886)"
    }

    fn detect(&self, mem: &Pcsx2Memory) -> bool {
        let addrs = [
            0x00012610,
            0x00089588,
            0x0008C010,
            0x0040ECCC,
            0x00013390,
            0x00093390,
        ];
        for &addr in &addrs {
            if check_serial(mem, addr) {
                return true;
            }
        }
        false
    }

    fn tickrate_ms(&self) -> u64 {
        1
    }

    fn inject(&self, mem: &Pcsx2Memory, dx: i32, dy: i32, sens: u32, invert_pitch: bool) {
        if dx == 0 && dy == 0 {
            return;
        }

        let look_sensitivity = (sens as f32) / SENSITIVITY_DIVISOR;

        let mut cam_x = mem.read_f32(BL_CAM_X);
        if cam_x.is_finite() {
            cam_x -= (dx as f32) * look_sensitivity;
            if cam_x.is_finite() {
                mem.write_f32(BL_CAM_X, cam_x);
            }
        }

        let mut cam_y = mem.read_f32(BL_CAM_Y);
        if cam_y.is_finite() {
            let eff_dy = if invert_pitch { -(dy as f32) } else { dy as f32 };
            cam_y += eff_dy * look_sensitivity * PITCH_RATIO_SCALE;
            cam_y = cam_y.clamp(-1.55, 1.55);
            if cam_y.is_finite() {
                mem.write_f32(BL_CAM_Y, cam_y);
            }
        }

        static LOG_INJECT_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if LOG_INJECT_COUNT.load(std::sync::atomic::Ordering::Relaxed) < 5 {
            LOG_INJECT_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::log(&format!("Black injected: dx={}, dy={}, camX={}, camY={}", dx, dy, cam_x, cam_y));
        }
    }
}
