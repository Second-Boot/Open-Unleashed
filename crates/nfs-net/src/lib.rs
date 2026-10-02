//! The game's own network protocol — `NfsNet` (`RE/notes/network.md` §5).
//!
//! The original's transport is raw WinSock, speaking IP on UDP **9442** or IPX on socket 1106, under EA's
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
pub mod outbox;
pub mod reliable;
pub mod room;
pub mod session;
pub mod slots;
pub mod socket;
pub mod state;
pub mod tcp;
pub mod wire;

/// The UDP port of the IP transport — `0x24E2` = 9442, the word after the application GUID at
/// **0x5B4990**; the connect path pushes it as the default port of a typed address (0x48E4CD:
/// `mov eax,[0x5B4990]; push eax; push edi; call 0x48AC90`, which lets a fifth number of the string
/// `a.b.c.d.port` override it) and a real two-instance capture shows the host on UDP 9442
/// (`RE/notes/REFERENCE-2026-10-02-SRVE.md`). Until 2026-10-02 this said 1106.
pub const PORT: u16 = 0x24E2;

/// `0x452` = 1106 — the **IPX** socket (0x560B44, 0x560BE1, 0x560E1C, 0x560E83 are the IPX
/// paths); the IP transport does not use it.
pub const IPX_PORT: u16 = 0x452;
