//! Race/game state reconstructed from the original (see `RE/ARCHITECTURE.md`).
//! Currently: start-grid extraction and scene assembly for tracks and cars.
//! Race logic (checkpoints, laps, positions) follows M5.
pub mod ai;
pub mod career;
pub mod race;
pub mod racecar;
pub mod records;
pub mod skill;
pub mod sky;
pub mod start;
pub mod text;
pub mod world;
