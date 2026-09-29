//! The game's own network protocol — `NfsNet` (`RE/notes/network.md` §5).
//!
//! The original's transport is raw WinSock on port **1106**, speaking either IP or IPX, under EA's
//! Comm middleware. This crate is the layer *above* that: the messages `NfsNet` itself sends and
//! receives, which is where the game's behaviour lives. The middleware's own framing is here too, in
//! [`wire`] (the one tag byte a datagram carries) and [`comm`] (the control messages that own that
//! byte) — §7 of the note. What is still missing for byte compatibility is listed in §7.8.
//!
//! Everything in this crate is transcribed from the disassembly; nothing is invented.

pub mod comm;
pub mod data;
pub mod exchange;
pub mod frame;
pub mod launchcar;
pub mod msg;
pub mod options;
pub mod reliable;
pub mod room;
pub mod session;
pub mod slots;
pub mod socket;
pub mod state;
pub mod tcp;
pub mod wire;

/// The UDP port the original binds and sends on — `0x452`, pushed at 0x560B44, 0x560BE1, 0x560E1C
/// and 0x560E83.
pub const PORT: u16 = 1106;
