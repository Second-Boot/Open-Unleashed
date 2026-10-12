//! The blocking waits of a launch: the barrier, the release and the go.

use super::*;

impl NetPlay {
    /// **0x48D6A0** — the barrier every machine runs at the end of 0x4B4DA0: `0xBD(id)`, wait for
    /// every player's bit in `[net+0xE6]`, `0xBD(id + 8)`, wait for `[net+0xE7]`, then the same
    /// two again to bring both bytes back to zero. The players are `[net+0xE5]`'s, read afresh
    /// at every look (a player who leaves, `0xB1`, drops out of it, 0x48F51B). Because a player's
    /// `0xBD` follows its `0xBC`s on the same link, a machine that has passed the first wait has
    /// applied the host's whole stream. Answers `false` when it left without them.
    ///
    /// The waits redraw the loading screen through `0x4A4A70(−1)` in the original, which is a
    /// **no-op here**: that function returns at once unless `[0x655A28]` is set, and only its step
    /// 0 sets it — called at 0x4B6CCA, *after* 0x4B4DA0 (0x4B6CC4) has returned, and cleared again
    /// by step 10 (0x4B68CB). So the barrier shows nothing new on either side.
    ///
    /// PORT-DEVIATION D12 (PORT-DEVIATIONS.md) [0x48CE27…, 0x48CE6F, 0x48DA60(−2, −1)]: the original
    /// waits for ever while the session lives, leaving for "Abort" (ESC, 0x55FF60 == 0x1B) or a lost
    /// link; both are ported (`run.keys.esc()`, `self.lost`), and — **glue**, so a headless run cannot
    /// hang — the port also leaves after [`BARRIER_LIMIT`].
    pub fn launch_barrier(&mut self) -> bool {
        let Some(mut run) = self.barrier_begin() else { return true };
        loop {
            if let Some(passed) = self.barrier_poll(&mut run) {
                if passed {
                    tracing::info!(target: "netplay", "launch barrier passed (0xBD ×4)");
                }
                return passed;
            }
            let _ = self.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The barrier as a loop a caller can draw inside — 0x48CDF0's wait loops call the loading
    /// screen's 0x4A4A70(−1) every turn (0x48CE27, 0x48CF5F, 0x48D079, 0x48D200), which redraws the
    /// other players' rows while the screen is up. `None` with no player id (nothing to wait for).
    pub fn barrier_begin(&self) -> Option<BarrierRun> {
        // **TEST GLUE**: `NFS_NET_STALL_MS=<ms>` — a joiner that does not answer for that long before its
        // first barrier (so the host's wait can be shown to end on ESC)
        static STALLED: std::sync::Once = std::sync::Once::new();
        if !self.is_host()
            && let Some(ms) = std::env::var("NFS_NET_STALL_MS").ok().and_then(|v| v.parse::<u64>().ok())
        {
            STALLED.call_once(|| std::thread::sleep(Duration::from_millis(ms)));
        }
        (self.room.me >= 0).then(|| BarrierRun { phase: 0, sent: false, end: Instant::now() + BARRIER_LIMIT, keys: KeyRing::begin() })
    }

    /// One look of the barrier's wait: `Some(true)` when it has passed, `Some(false)` when it left
    /// without the other players (the link lost, or [`BARRIER_LIMIT`]), `None` while it waits.
    pub fn barrier_poll(&mut self, run: &mut BarrierRun) -> Option<bool> {
        // (value to send, wait for `bits` of the byte to equal `want`)
        const PHASES: [(i32, bool, bool); 4] = [(0, false, true), (8, true, true), (0, false, false), (8, true, false)];
        let (add, high, want_all) = *PHASES.get(run.phase)?;
        if !run.sent {
            let out = vec![(To::All, room::encode(0xBD, Some(self.room.me + add), None, None))];
            self.dispatch(out);
            run.sent = true;
        }
        let mask = u16::from(self.in_race);
        let bits = if high { self.barrier >> 8 } else { self.barrier & 0xFF } & mask;
        if bits == if want_all { mask } else { 0 } {
            run.phase += 1;
            run.sent = false;
            return (run.phase == PHASES.len()).then_some(true);
        }
        // 0x48CE6F / 0x48CFA7 / 0x48D0C1 / 0x48D248: `0x55FF60() == 0x1B` — like a dead link, the abort arm
        if run.keys.esc() {
            tracing::info!(target: "netplay", "launch barrier: ESC (0x48CE6F…) — the wait is abandoned");
            return Some(false);
        }
        if self.lost || Instant::now() > run.end {
            tracing::warn!(target: "netplay", lost = self.lost, link = %self.net.report(), "launch barrier: left without the other players");
            return Some(false);
        }
        None
    }

    /// **PLAYGAME's wait, 0x50CF78…0x50CFF1**: pump the session until the room's `0xB3` has set `+0xC4`
    /// with the connection up (`+0xBF == 0`, `+0xBE`, `+0xDC ≥ 0`, 0x50CFA9), and give up — `false` —
    /// when the link is gone (0x50CF86 `net+8`, 0x50CFB7…0x50CFCF). PORT-DEVIATION D12
    /// (PORT-DEVIATIONS.md) [0x50CFD7]: the original also leaves when 0x55FF60 answers ESC (0x50CFD7
    /// `cmp eax, 0x1B`, ported below with `KeyRing`) and otherwise waits for ever; the port also
    /// leaves after [`BARRIER_LIMIT`] — glue, so a headless run cannot hang.
    pub fn wait_released(&mut self) -> bool {
        let end = Instant::now() + BARRIER_LIMIT;
        let keys = KeyRing::begin();
        loop {
            if !self.established() || self.lost {
                return false;
            }
            if self.go_wait {
                return true;
            }
            // 0x50CFD7 `0x55FF60() == 0x1B`: ESC leaves the wait through the `(−2, −1)` abort arm
            if keys.esc() {
                tracing::info!(target: "netplay", "PLAYGAME: ESC (0x50CFD7) — the wait is abandoned");
                return false;
            }
            if Instant::now() > end {
                tracing::warn!(target: "netplay", "PLAYGAME: no 0xB3 came");
                return false;
            }
            let _ = self.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// **The race frame's wait, 0x4152E0** (`while (… +0xC4 != 0) 0x4B0FA0`): pump until the room's first
    /// `0xC5` has cleared `+0xC4`. The loop's condition is the connection up and `+0xC4` set, so it
    /// also ends when the link goes. [`BARRIER_LIMIT`] is the D12 glue (PORT-DEVIATIONS.md, 0x4152E0 has no limit).
    pub fn wait_go(&mut self) {
        let end = Instant::now() + BARRIER_LIMIT;
        while self.go_wait && self.established() && !self.lost {
            if Instant::now() > end {
                tracing::warn!(target: "netplay", "race start: no 0xC5 came");
                break;
            }
            let _ = self.poll_lobby();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// **0x48D3D0** — the launch's first step (0x4B4E28): the barrier, then every player's `+0x50 =
    /// +0x48 = +0x4C = −1` — the finish times of an earlier race, and the race indices.
    pub fn launch_begin(&mut self) -> bool {
        let passed = self.launch_barrier();
        for p in &mut self.room.players {
            p.time = -1;
            p.progress = -1;
        }
        self.launch_index.clear();
        // 0x4B4DB6 `memset(0x6573E8, 0, 0x3E24)` clears `[0x658608 + slot·0x580]` with the rest of the
        // option block (the port refills the other tables from the stream and keeps them as they are)
        self.launch.progress.clear();
        passed
    }
}
