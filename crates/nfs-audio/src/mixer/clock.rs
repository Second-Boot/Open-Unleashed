//! The service clock, the pump and the bridge between the game thread and the device callback.

/// The most frames 0x598940 mixes at once (`cmp edi, 0x100`, 0x598979): a block, the unit every
/// voice's chain is pulled in (0x598760).
pub const BLOCK: usize = 256;

/// **0x58FBC0**'s count of the mix in service ticks: before each tick's frames the library
/// service runs (`call [0x6B49AC]` = 0x589520, set at 0x5657C2), and a tick is a hundredth of a
/// second of the mix rate — `[0x6AD7AC]` the ticks, `[0x6AD7B0]` the frames counted so far,
/// `rate · tick / 100` (`imul`, the magic 0x51EB851F, `sar 5`, toward zero) less those, rounded
/// down to 16 (`and edx, 0xFFFFFF0`); both reset after tick 30000 (`cmp esi, 0x7530 ; jle`).
/// `[0x6AD7A8]` is what is left of the tick (0x58FC5F). At 32000 Hz a tick is 320 frames; at
/// 22050 it is 208, 224, 224, … (220.5 on average); at 16000, 160.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServiceClock {
    pub tick: u32,
    pub counted: u32,
    pub left: u32,
}

impl ServiceClock {
    /// The next tick's frames (0x58FBF6…0x58FC47).
    pub fn next_tick(&mut self, mix_rate: u32) -> u32 {
        self.tick += 1;
        let total = (mix_rate as i64 * self.tick as i64 / 100) as u32;
        let frames = total.wrapping_sub(self.counted) & 0x0FFF_FFF0;
        self.counted = self.counted.wrapping_add(frames);
        if self.tick > 30000 {
            self.tick = 0;
            self.counted = 0;
        }
        frames
    }
}

/// The library's mixing rate per `AUDIOQUALITY` `[0x657C98]`: `0x5B4AE4[q]` = 32000 / 22050 /
/// 16000 (0x4A67AE), taken as the smaller of it and the caps word `[0x6B48C2]` — the library's
/// constant 48000 (0x58A555), so always the table's — into config `+0x1A` (0x4A67D0 / 0x4A67D7),
/// which 0x5656D0 copies to `[0x6B48DA]`; 0x58A703…0x58A72A clamps it to 11025…48000. At quality 3
/// the configuration is skipped and the library's 22050 stays (0x58A5C2).
///
/// The DirectSound driver opens its **primary** buffer at that rate (`SetFormat`, vtable `+0x38`:
/// PCM, 2 channels, `[0x6B48DA]` a second, 16 bits, 0x58ED4B…0x58ED88) and its mixing buffer
/// (`CreateSoundBuffer`, flags 0x18000) the same (0x58EDBC…0x58EE6B), and every software voice
/// steps through its sample at `sample rate / [0x6B48DA]` (0x596B60). So the bank samples are
/// resampled to this rate, and the card (or the system mixer) plays that.
pub fn mix_rate(audio_quality: u8) -> u32 {
    match audio_quality {
        0 => 32000,
        1 => 22050,
        2 => 16000,
        _ => 22050,
    }
}

/// The driver thread's `Sleep` between two passes, `[0x6B48E1]` (0x58F8AD…0x58F8B3, `Sleep` the
/// import at 0x5B220C): config `+0x21` from `0x5B4B14[q]` = 8 / 20 / 50 ms (0x4A67B5, stored at
/// 0x4A67C4 — `[esp+0x2D]` under the pushed `edi`; copied by 0x5656D0 from `+0x14` to 0x6B48D4),
/// clamped to 200 (0x58A855…0x58A86B); the library's default 10 (0x58A570) at quality 3, where the
/// configuration is skipped.
pub fn pump_sleep_ms(audio_quality: u8) -> u32 {
    match audio_quality {
        0 => 8,
        1 => 20,
        2 => 50,
        _ => 10,
    }
}

/// The DirectSound mixing buffer, in frames `[0x6AD7B4]`: `rate · 400 / 1000` (`lea ·5 ; lea ·5 ;
/// shl 4`, the magic 0x10624DD3 `sar 6`) rounded down to 16 (`and 0xFFFF0`, 0x58EDD5…0x58EE19) —
/// 0.4 s of the mix rate.
pub fn ring_frames(mix_rate: u32) -> u32 {
    (mix_rate * 400 / 1000) & 0xF_FFF0
}

/// **The driver thread's writes** (0x58F880 → 0x58F8E0 → 0x58F8F0): each pass takes the play
/// cursor (`GetCurrentPosition`, vtable `+0x10`, in frames, `and 0xFFFFF0`, 0x58F94D…0x58F985),
/// writes from the last write's end `[0x6AD78C]` up to `cursor + lead [0x6AD790]` rounded down to
/// 16 (0x58F9BE…0x58F9C7), wrapping at [`ring_frames`], and hands each of the (one or two) locked
/// parts to 0x58FBC0 (0x58FA40…0x58FADD), which mixes it in service ticks and blocks
/// ([`Shared::render`]); then it `Sleep`s [`pump_sleep_ms`]. So a write is the frames the cursor
/// moved in one pass, in multiples of 16 (at 22050 Hz and 20 ms: 432 or 448, 441 on average),
/// cut in two at the ring's end. APPROXIMATION: the cursor is taken to move exactly the sleep's
/// frames a pass (the sum of every pass's `sleep · rate / 1000`, `n · sleep · rate / 1000` after
/// `n` passes of one setting) — the thread's own work, the scheduler and the first pass's lead
/// fill are not modelled.
///
/// The cursor moves with time, so a pass after the sleep (or the rate) changed writes one pass of
/// the new setting, not the whole run again at it. *(Until 2026-10-09 the end was `passes ·` the
/// current sleep: the library's 10 ms during the start-up movies, then `AUDIOQUALITY` 1's 20 ms
/// from the front end's first sound — one pass wrote as many frames as had been played so far, the
/// whole intro again, mixed at once before anything the front end started: AUDIO-START, seconds of
/// silence in the menu after the movies.)*
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Pump {
    pub passes: u64,
    /// the frames written so far (the write end, unwrapped)
    pub end: u64,
    /// `Σ sleep · rate` over the passes so far: the cursor, in thousandths of a frame
    pub moved: u64,
}

impl Pump {
    /// The next pass's parts: up to the ring's end, then the rest from its start.
    pub fn next_write(&mut self, mix_rate: u32, sleep_ms: u32) -> [u32; 2] {
        self.passes += 1;
        self.moved += sleep_ms as u64 * mix_rate as u64;
        let end = (self.moved / 1000) & !15;
        let w = end.saturating_sub(self.end) as u32;
        let ring = ring_frames(mix_rate).max(16);
        let pos = (self.end % ring as u64) as u32;
        self.end = end.max(self.end);
        let first = w.min(ring - pos);
        [first, w - first]
    }
}

/// From the mix rate to the output device's: the port mixes at [`mix_rate`] and interpolates
/// linearly between two mixed frames per device frame. PORT-DEVIATION D27 (device rate): the
/// original hands the mix-rate buffer to DirectSound, and the conversion to the card's rate (if
/// any) is the card's or the system mixer's, not the game's.
#[derive(Default)]
pub(super) struct Bridge {
    pub(super) pending: std::collections::VecDeque<[f32; 2]>,
    /// the position between `pending[0]` and `pending[1]`, in mix frames
    pub(super) t: f64,
}

impl Bridge {
    /// The next device frame. `step` is the mix rate over the device rate; `left` the device
    /// frames still wanted in this callback, which `render` is offered (the stream's renders one
    /// driver pass whatever it is offered, [`Shared::pump`]).
    pub(super) fn next(&mut self, step: f64, left: usize, mut render: impl FnMut(usize, &mut std::collections::VecDeque<[f32; 2]>)) -> [f32; 2] {
        while self.pending.len() < 2 {
            let want = (left as f64 * step).ceil() as usize + 2;
            render(want, &mut self.pending);
        }
        let (a, b, t) = (self.pending[0], self.pending[1], self.t as f32);
        self.t += step;
        while self.t >= 1.0 && self.pending.len() > 1 {
            self.t -= 1.0;
            self.pending.pop_front();
        }
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
    }
}

/// The frames still queued `since` the callback that left `frames` for the device, as 0x58F5D0
/// answers them: `frames · 1000 / rate`, toward zero, never below 0.
pub fn latency_ms(frames: u64, since: std::time::Duration, rate: u32) -> i32 {
    let gone = (since.as_secs_f64() * rate as f64) as u64;
    (frames.saturating_sub(gone) * 1000 / rate.max(1) as u64) as i32
}

/// `played · 1000 / rate`, truncated (0x590D80 / 0x590DF0 are the 64-bit multiply and unsigned
/// divide).
pub fn played_ms(frames: u64, rate: u32) -> i32 {
    (frames * 1000 / rate.max(1) as u64) as i32
}

#[cfg(test)]
mod position_tests {
    #[test]
    fn the_latency_is_what_is_queued_and_not_yet_heard() {
        use std::time::Duration;
        // 1024 frames of buffer and 1024 of device delay at 48 kHz: 42 ms, toward zero
        assert_eq!(super::latency_ms(2048, Duration::ZERO, 48000), 42);
        // 10 ms later 480 frames have gone
        assert_eq!(super::latency_ms(2048, Duration::from_millis(10), 48000), 32);
        // never negative
        assert_eq!(super::latency_ms(2048, Duration::from_secs(1), 48000), 0);
    }

    #[test]
    fn the_position_is_whole_milliseconds() {
        assert_eq!(super::played_ms(22050, 22050), 1000);
        assert_eq!(super::played_ms(22049, 22050), 999);
        assert_eq!(super::played_ms(0, 22050), 0);
    }
}
