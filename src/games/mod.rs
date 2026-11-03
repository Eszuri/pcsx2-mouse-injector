use crate::memory::Pcsx2Memory;

pub trait GameDriver: Send + Sync {
    fn name(&self) -> &'static str;
    fn detect(&self, mem: &Pcsx2Memory) -> bool;
    fn tickrate_ms(&self) -> u64;
    fn inject(&self, mem: &Pcsx2Memory, dx: i32, dy: i32, sens: u32, invert_pitch: bool);
}

pub mod black;

pub fn get_available_games() -> Vec<Box<dyn GameDriver>> {
    vec![
        Box::new(black::BlackGame),
    ]
}
