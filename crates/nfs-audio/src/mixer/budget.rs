//! The voice budget and the steal rule of the allocation 0x5898F0.

/// The sound library's software voice count per `AUDIOQUALITY` `[0x657C98]`: `0x5B4B08[q]` = 48 /
/// 24 / 12, a byte of the configuration 0x4A66B0 hands to 0x5656D0 (0x4A66F3, 0x4A6719), copied to
/// `[0x6B48DD]` (0x5656E6) and capped at the library's 128 (`[0x6B48C6]` = 0x80, 0x58A545; the cap
/// 0x58A731…0x58A742). At quality 3 the configuration is skipped (0x4A66E8) and the library's
/// default 24 stays (0x58A5B5) — the sound is closed at that setting anyway (0x4A6854 / 0x4A6A09).
///
/// The hardware-3D voices `[0x6B48DF]` (`min(device's, 0x5B4AF0[q])`, 0x4A6755…0x4A67A1) are never
/// configured: that branch needs `[0x657C9C]`, which both start-ups clear just before the call
/// (0x4A684C, 0x4A6A61), so the device's own voice count does not enter the budget.
/// `[0x6B49A4]` = software + hardware (0x58A786…0x58A7A8) is the voice table's length (0x565792).
pub fn voice_budget(audio_quality: u8) -> usize {
    match audio_quality {
        0 => 48,
        1 => 24,
        2 => 12,
        _ => 24,
    }
}

/// The priority a stream voice is started with — `push 0x65` at 0x5613B2 in the stream voice
/// start 0x5612A0 (the stream player 0x566FE3 / 0x567304: music, the movies' sound). It is not
/// below [`STEAL_BELOW`], so a stream is never taken.
pub const STREAM_PRIORITY: u8 = 0x65;

/// The allocator's second pass takes only voices whose `voice+0x1A` is below this (0x589A38
/// `cmp cl, 0x65 ; jae`, unsigned): a priority of 0x65 or more is never stolen.
pub const STEAL_BELOW: u8 = 0x65;

/// **0x5898F0(n, priority, &handle, lo, hi)**, the voice allocator, for a start that needs `need`
/// voices (the patch's channel count, code 0x82 → layer `+3`, 0x5904CD) out of `budget`, with the
/// live voices given as `(priority, channels, stamp)` — `stamp` is [`Voice::stamp`], the service
/// tick `[0x6B49A8]` the voice started at ([`Shared::service_tick`]):
///
/// * pass 1 (0x589943…0x5899CA) takes free voices (`voice+0xF == 0`);
/// * pass 2 (0x5899EF…0x589AA8, read to the instruction 2026-09-27) takes, one at a time, the voice
///   with the **lowest** priority below [`STEAL_BELOW`] (0x589a3b…0x589a48: `jge` skips a
///   candidate whose priority is not strictly lower than the best so far, so on equal priority
///   the tie goes to the **smallest** stamp (0x589a51…0x589a58, `jae` — unsigned, so the oldest);
///   on an exact tie (equal priority *and* stamp) neither replaces the earlier one, so the
///   **lowest scanned voice-table slot** wins, whatever its stamp. The request's own priority is
///   **not** compared: any start takes a stealable voice;
/// * fewer than `need` found → −9 (0x589BE9) and nothing is stopped (the victims are only
///   stopped, 0x565D60 on their handle, after the count matched, 0x589B26).
///
/// Returns the indices of the voices to stop, or `None` for the refusal. A stolen stereo voice
/// frees both its channels (0x565D60 stops the whole handle). `sort_by_key` is stable, so `live`'s
/// own order (push order into [`Shared::voices`], not necessarily the original's slot order) is
/// this port's stand-in for "lowest slot" on an exact tie. *(Until 2026-09-27 `stamp` was the
/// port's own start-order counter (`VoiceId`), not the service tick — so two voices started in
/// different ticks but close together could tie here when the original's wouldn't, and vice
/// versa; withdrawn — [`Shared::service_tick`] now stamps a start with `[0x6B49A8]` exactly as
/// 0x589B3F…0x589B54 does. The remaining gap — an exact tie's winner depending on the original's
/// free-list history, which decides which physical slot a voice starts in — is unmodeled and,
/// short of tracking the 128-slot table itself, unmodelable from the port's `Vec<Voice>`.)*
pub fn pick_victims(live: &[(u8, u16, u32)], budget: usize, need: usize) -> Option<Vec<usize>> {
    let (mut order, mut victims) = (Vec::new(), Vec::new());
    pick_victims_into(live, budget, need, &mut order, &mut victims).then_some(victims)
}

/// [`pick_victims`] into the caller's buffers (the callback's allocator allocates nothing): `true`
/// for a start that is served, with the victims in `victims`. The stable sort of the original
/// order is the unstable one with the index as the last key — the same total order, no scratch
/// allocation.
pub fn pick_victims_into(live: &[(u8, u16, u32)], budget: usize, need: usize, order: &mut Vec<usize>, victims: &mut Vec<usize>) -> bool {
    victims.clear();
    let used: usize = live.iter().map(|&(_, ch, _)| ch.max(1) as usize).sum();
    let mut free = budget.saturating_sub(used);
    if free >= need {
        return true;
    }
    order.clear();
    order.extend((0..live.len()).filter(|&i| live[i].0 < STEAL_BELOW));
    order.sort_unstable_by_key(|&i| (live[i].0, live[i].2, i));
    for &i in order.iter() {
        victims.push(i);
        free += live[i].1.max(1) as usize;
        if free >= need {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod budget_tests {
    use super::{pick_victims, voice_budget, STREAM_PRIORITY};

    #[test]
    fn the_budget_is_the_quality_tables() {
        // 0x5B4B08: 48 / 24 / 12; quality 3 keeps the library's 24 (0x58A5B5)
        assert_eq!([0, 1, 2, 3].map(voice_budget), [48, 24, 12, 24]);
    }

    #[test]
    fn a_free_voice_is_taken_first() {
        let live = vec![(100, 1, 0); 23];
        assert_eq!(pick_victims(&live, 24, 1), Some(vec![]));
    }

    #[test]
    fn a_full_budget_steals_the_lowest_priority_then_the_oldest() {
        // 24 voices: engine layers at 100, a collision at 26 started late, one at 26 earlier
        let mut live: Vec<(u8, u16, u32)> = (0..22).map(|i| (100, 1, i)).collect();
        live.push((26, 1, 30));
        live.push((26, 1, 25));
        // the lowest priority goes first, of the two the older (stamp 25, index 23)
        assert_eq!(pick_victims(&live, 24, 1), Some(vec![23]));
        // two needed: both 26s, oldest first
        assert_eq!(pick_victims(&live, 24, 2), Some(vec![23, 22]));
        // the request's own priority is not compared: any start takes a voice, and with only
        // 100s left the oldest of them
        let live: Vec<(u8, u16, u32)> = (0..24).map(|i| (100, 1, 50 - i)).collect();
        assert_eq!(pick_victims(&live, 24, 1), Some(vec![23]));
    }

    #[test]
    fn an_exact_tie_of_priority_and_stamp_steals_the_lowest_scanned_slot() {
        // 0x589a51…0x589a58: on equal priority *and* equal stamp, `jae` skips the later
        // candidate, so the earlier-scanned slot (the port's earlier-pushed voice, its stand-in
        // for "lowest slot" — `pick_victims`'s doc) keeps the pick, in whatever order the ties
        // are given
        let live: Vec<(u8, u16, u32)> = vec![(26, 1, 5), (26, 1, 5), (26, 1, 5)];
        assert_eq!(pick_victims(&live, 3, 1), Some(vec![0]));
    }

    #[test]
    fn streams_and_priorities_from_0x65_are_never_stolen() {
        // a stereo stream (0x65) and 22 voices at 0x65 or above: nothing can be taken
        let mut live: Vec<(u8, u16, u32)> = vec![(STREAM_PRIORITY, 2, 0)];
        live.extend((1..23).map(|i| (if i % 2 == 0 { 0x65 } else { 0xC8 }, 1, i)));
        assert_eq!(pick_victims(&live, 24, 1), None);
        // one stealable voice among them is the one taken
        live.push((100, 1, 99));
        assert_eq!(pick_victims(&live, 26, 2), Some(vec![23]));
        // with no free voice beside it, one stealable mono voice cannot make room for a stereo start
        assert_eq!(pick_victims(&live, 25, 2), None);
    }

    #[test]
    fn a_refusal_stops_nothing_and_a_stereo_victim_frees_two() {
        // 12 voices (quality 2), ten mono at 100 and one stereo at 90: a stereo start needs 2
        let mut live: Vec<(u8, u16, u32)> = (0..10).map(|i| (100, 1, i)).collect();
        live.push((90, 2, 20));
        assert_eq!(pick_victims(&live, 12, 2), Some(vec![10]));
        // a stereo start into a budget of one voice: refused, not one voice given up
        assert_eq!(pick_victims(&[(0, 1, 0)], 1, 2), None);
    }
}
