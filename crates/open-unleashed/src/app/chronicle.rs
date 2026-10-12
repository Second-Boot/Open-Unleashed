//! The Chronicle (the Porsche history screens): groups, years, slider, columns and media.

use crate::*;

impl App {
    /// The chronicle picture the picture screens draw: the file `[0x699CFC]` points at.
    ///
    /// `FEChroniclePictureControl`'s draw, **0x508310**, takes `0x507480([0x699CFC])` — the
    /// selected name of the list 0x5074A0 built (`chronicle_media`: the clicked timeline column's
    /// year and variant, `p` / `m` / `v` by screen) — and blits its tiles; the badge's update
    /// 0x4C16AE reads the same pointer. `[0x699CFC]` is zeroed by every rebuild of the list and set
    /// to a thumbnail's `data.number` by 0x5089C0 (`chronicle_sel`).
    ///
    /// *(This function used to take the newest `M…` file at or before the career's year — a guess
    /// at "the browsing position", and the wrong kind on `chroniclepicture`,
    /// whose files are the `p` packs: the original shows the clicked column's first photo, the
    /// port the ski-and-fur-hat page of a media file. Corrected 2026-09-30, AJ.)*
    pub(crate) fn chronicle_picture(&self) -> Option<String> {
        self.chronicle_media().get(self.fe.chronicle_sel).cloned()
    }

    /// The car logo a chronicle pack names, as `(pack, pair)` — the pack without its extension,
    /// the way [`LayoutScreen::set_car_logo`] wants it.
    ///
    /// `FECarLogo::update` with `othersource=Chron` (0x4C1677) loads the chronicle pack, takes
    /// **entry 1** — the `ci` icon — and reads the string at `+0x24`, which is the entry's `0x6F`
    /// attachment. It wants at least three characters (0x4C1763); the first two are the logo pair
    /// as ASCII digits (0x4C17F6 computes `2·(10·s[0] + s[1]) − 0x420`, twice the number they
    /// spell) and the rest is the logo pack's name (0x4C17F9). CONFIRMED: `MA601a.fsh` carries
    /// `04finallogos2`, and `FEData/Art/finallogos2.fsh` is on disk.
    pub(crate) fn chronicle_logo(&self, file: &str) -> Option<(String, i16)> {
        let dir = self.paths.fe_data().join("Chron");
        let path = GamePaths::find_file(&dir, file)?;
        let data = std::fs::read(&path).ok()?;
        let entries = nfs_assets::texture::decode_pack(&path, &data).ok()?;
        let text = entries.get(1)?.text.as_deref()?;
        if text.len() < 3 {
            return None;
        }
        let pair = text[..2].parse::<i16>().ok()?;
        Some((text[2..].to_string(), pair))
    }

    /// Fill the chronicle's thumbnail column.
    ///
    /// `FEChroniclePictureIconControl`'s activate, **0x5087A0**, is the whole binding:
    ///
    /// ```text
    /// 0x5087BD  n = data.number                       ; [this+0x140]
    /// 0x5087CF  if (n >= [0x699D00]) nothing is drawn ; past the media count
    /// 0x5087DA  rec = 0x507480(n)                     ; [0x699C84][n], a file name
    /// 0x5087EB  [this+0x148] = 0x4D7A10(rec, 0)       ; entry 0 of that pack
    /// 0x5087F9  [this+0x144] = 0x4D7A10(rec, 1)       ; entry 1
    /// ```
    ///
    /// and the draw 0x508810 picks `[this+0x144]` when 0x4C7C90 says the widget is **hovered**
    /// (§4.109) and `[this+0x148]` otherwise, returning at 0x508832 when that pointer is null. 0x4D7A10's
    /// second argument really is an entry index — its tail, 0x4D7B09…0x4D7B16, is
    /// `pack[+0x1C][+8][k]`.
    ///
    /// The list itself is built by **0x5074A0(kind, year, variant)**, which formats
    /// `"%s%c%c%02d%d%c.fsh"` (0x50757F) and keeps every name that exists:
    ///
    /// ```text
    /// 0x507508  kind 0 -> 'p', kind 2 -> 'v', anything else -> 'm'
    /// 0x50756A  the model-group letter, "CBADE"[[0x5DBA44]]
    /// 0x50754D  the year, mod 100
    /// 0x507567  a 1-based index, and the variant 'a' + `variant`
    /// 0x507627  stop at the first name that is not on disk, or at four (0x507639)
    /// ```
    ///
    /// so a screen shows **at most four** pictures of one year and variant, and 0x507649 adds the
    /// matching `"%s%c%c%02d%d%c.mad"` for kind 2 — the movie box's file. 0x5080F8 passes kind 0
    /// for `chroniclepicture` and 0x508137 kind 1 for `chroniclemedia`.
    ///
    /// **Where the year and the variant come from** (closed 2026-09-30): 0x5080E0
    /// reads them from `[this + col*4 + 0x160]` / `[+0x174]` of the timeline screen — the table
    /// 0x507B4F fills — at the column the pointer was on (`[this+0x154]`), so they are the clicked
    /// column's, [`App::chronicle_column`]. Which of two frames shows is not approximate either:
    /// 0x508819's test is 0x4C7C90,
    /// `[this+0x10C] & 4`, bit 2 of the state mask — **hover** (§4.109) — so entry 1 is the
    /// hovered frame and the draw picks it.
    ///
    /// The draw also puts `[[this+0x148]+0x24]`, the entry's own name, under the icon in the
    /// class's `Univers7b.ffn` (0x5089AD), but only when that name is non-empty (0x508965) — and
    /// the `Chron` packs' entries carry none, so nothing is drawn there. Read, not invented.
    pub(crate) fn bind_chronicle_icons(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let icons = l.numbered_widgets("NFS5.ChroniclePictureIcon");
        if icons.is_empty() {
            return;
        }
        let files = self.chronicle_media();
        let wanted: Vec<(usize, String)> = icons
            .into_iter()
            .map(|(i, number)| (i, files.get(number.max(0) as usize).cloned().unwrap_or_default()))
            .collect();
        let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) else { return };
        for (index, file) in wanted {
            // entry 0 is the resting frame; the draw takes the next one when the pointer is on
            // the icon, which is what 0x508819's 0x4C7C90 tests (§4.109)
            l.set_pack_picture(r, &self.paths, "Chron", index, &file, 1);
            l.set_pack_picture(r, &self.paths, "Chron", index, &file, 0);
        }
    }

    /// `chronicle.lay`'s five model-group buttons carry the **`selected`** flag, one at a time.
    /// 0x5077D0 clears it on all five through the by-name helper 0x50D9A0 and then, on
    /// `[0x5DBA44]` through the jump table at 0x507894, sets it on exactly one (§4.111).
    pub(crate) fn bind_chronicle_group(&mut self) {
        let Some(l) = self.fe.layout_preview.as_ref() else { return };
        let want = l.widgets_named(&CHRONICLE_GROUP_BUTTONS);
        if want.is_empty() {
            return;
        }
        // and the timeline's own five columns, which the same screen carries (§4.122). The walk
        // always starts at the opening column (0x507E20 never rewrites it, §4.121) — a click only
        // picks a column to open a sub-screen from (§4.120), it does not move the timeline itself.
        let cols: Vec<(u32, usize)> = self.chronicle_columns();
        // the icons of the ten cells: `0x507340(year, row, variant, hot)` for rows 0 and 1
        // (0x507DDB loops `ebx < 2`), each answering the resting frame and the hovered one
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let pack = format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)]);
        let entries = self.chronicle_entries();
        // the caption above the `фото` row, one per column — computed before the mutable borrow
        // below, like `cols`/`entries` (§4.173)
        let captions = self.chronicle_captions();
        let mut cells: std::collections::BTreeMap<(usize, usize), (usize, usize)> = Default::default();
        for (col, (year, var)) in cols.iter().enumerate() {
            for row in 0..2 {
                if let Some((_, _, _, f)) = entries.iter().find(|(y, k, v, _)| y == year && *k == row && v == var) {
                    cells.insert((col, row), (f[0], if f[1] == usize::MAX { f[0] } else { f[1] }));
                }
            }
        }
        let frames: Vec<usize> = cells.values().flat_map(|(a, b)| [*a, *b]).collect();
        if let (Some(l), Some(r)) = (self.fe.layout_preview.as_mut(), self.renderer.as_mut()) {
            for n in frames {
                l.load_pack_entry(r, &self.paths, "Chron", &pack, n);
            }
        }
        let (year_idx, year_count) = self.chronicle_year_index();
        if let Some(l) = self.fe.layout_preview.as_mut() {
            if let Some(g) = l.chronicle_slider() {
                l.set_knob(g.index, Some((year_idx, year_count)));
            }
            l.set_chronicle_icons(pack, cells);
            l.set_chronicle_columns(cols);
            l.set_chronicle_captions(captions);
        }
        let group = self.fe.chronicle_group;
        if let Some(l) = self.fe.layout_preview.as_mut() {
            for (index, which) in want {
                l.set_selected_flag(index, which == group);
            }
        }
    }

    /// **The chronicle's entry index**, as 0x506FE0 builds it.
    ///
    /// The index is a flat table of 5 × 53 × 3 × 5 × 2 dwords at **0x692038** — group, year − 48,
    /// kind, variant, and a flag — and 0x507340 reads it with
    /// `[0x692038 + (2*(5*(3*(53*group + year − 48) + kind) + variant) + (flag != 0)) * 4]`.
    /// 0x507065's `rep stosd` of **0x1F0E** dwords is 7950, which is that product exactly.
    ///
    /// It is built from the **five icon packs**, not from the picture files: 0x50701E formats
    /// `"%s%s.fsh"` over the names at 0x5B8AB4, stepping 0x14 up to 0x5B8B18 — `iconsrace`,
    /// `icons911`, `icons356`, `icons924`, `icons914`, which is group order and agrees with both
    /// `"CBADE"` (§4.107) and the five buttons (§4.111). For each entry 0x537F60 hands back the
    /// four-character directory tag and 0x5070CF…0x5070FB parses it:
    ///
    /// ```text
    /// 0x5070FB  year = 10*tag[0] + tag[1] - 0x210     ; two ASCII digits; "00" becomes 100
    /// 0x5070CA  kind = table[tag[2] - 0x4D]           ; 0x507234 -> the jump table 0x507224
    ///                'p' -> 0, 'm' -> 1, 'v' -> 2, anything else -> 0
    /// 0x507132  the slot is found by **linear probing along the flat table** from
    ///           (group, year, kind, 0, 0): 0x50714C walks `eax += 4` while the dword is
    ///           taken, counting in `ebx`, and gives up at ten — which is the product of the
    ///           two fastest dimensions, 5 variants x 2 flags. So the k-th entry with the same
    ///           (group, year, kind) lands at `variant = k / 2`, `flag = k % 2`, and the pairs
    ///           are exactly §4.107's two frames, the resting one and the hovered one.
    /// ```
    ///
    /// so `icons356.fsh`'s `"48p "` is the 1948 picture of the 356 line. §4.118.
    pub(crate) fn chronicle_index(&self) -> Vec<(u32, usize, usize)> {
        self.chronicle_entries().into_iter().map(|(y, k, v, _)| (y, k, v)).collect()
    }

    /// The same walk, keeping the **pack entry number** each slot holds — which is what
    /// 0x507340 returns and what the timeline's icons are (§4.123). The two slots of a
    /// (year, kind, variant) are the resting frame and the hovered one (§4.121).
    fn chronicle_entries(&self) -> Vec<(u32, usize, usize, [usize; 2])> {
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let dir = self.paths.fe_data().join("Chron");
        let Some(path) = GamePaths::find_file(&dir, &format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)])) else {
            return Vec::new();
        };
        let Some(pack) = std::fs::read(&path).ok().and_then(|d| nfs_formats::fsh::Fsh::parse(&d).ok()) else {
            return Vec::new();
        };
        let mut out: Vec<(u32, usize, usize, [usize; 2])> = Vec::new();
        for (n, e) in pack.entries.iter().enumerate() {
            let t = e.name.as_bytes();
            if t.len() < 3 || !t[0].is_ascii_digit() || !t[1].is_ascii_digit() {
                continue;
            }
            let year = match 10 * u32::from(t[0]) + u32::from(t[1]) - 0x210 {
                0 => 100,
                y => y,
            };
            let kind = match t[2] {
                b'p' | b'P' => 0,
                b'm' | b'M' => 1,
                b'v' => 2,
                _ => 0,
            };
            // 0x50714C's probe: the k-th entry of a (year, kind) lands at variant k/2 and
            // frame k%2, so a group with a free second frame takes this one, and otherwise a
            // fresh variant opens
            match out.iter_mut().find(|(y, kk, _, f)| *y == year && *kk == kind && f[1] == usize::MAX) {
                Some(slot) => slot.3[1] = n,
                None => {
                    let v = out.iter().filter(|(y, kk, _, _)| *y == year && *kk == kind).count();
                    out.push((year, kind, v, [n, usize::MAX]));
                }
            }
        }
        out.sort();
        out
    }

    /// One caption per timeline column — the `фото`-row (kind 0) entry's own name, the FSH pack's
    /// `0x6F` text attachment (`nfs_formats::fsh::Fsh::entry_caption`) — always the **resting**
    /// frame's, `[0]` of `chronicle_entries`' pair: 0x507340's fourth argument, `hot`, is a
    /// hardcoded literal `0` at the caption's own call site (0x507C3B), never the hovered frame,
    /// `[1]`.
    ///
    /// **A dead special case, read and not ported.** The disassembly (0x507C51…0x507C66) compares
    /// that name against the exact literal `"356 Roadster No1"` (mixed case, `strcmp` 0x5AE3C0 —
    /// case-sensitive) and, on a match, draws a small badge icon (`"356R"` of `weeicons.fsh`) in
    /// place of the text instead. Because the lookup's `hot` is always `0`, this can only ever see
    /// a **resting**-frame name, and every shipped resting-frame name is upper-case
    /// (`"356 ROADSTER NO1"`, `"356"`, `"356A"`, `"356B"`, `"COUPE"`, `"CABRIO"`, …, confirmed by
    /// reading every text attachment of all five `FEData/Chron/icons*.fsh` packs) — the mixed-case
    /// literal is only the *hover*-frame name of `icons356.fsh`'s 1948 entry, which this lookup
    /// never reaches. So the badge branch cannot fire for any shipped pack; §4.173.
    fn chronicle_captions(&self) -> Vec<Option<String>> {
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let cols = self.chronicle_columns();
        let dir = self.paths.fe_data().join("Chron");
        let Some(path) = GamePaths::find_file(&dir, &format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)])) else {
            return vec![None; cols.len()];
        };
        let Some(data) = std::fs::read(&path).ok() else { return vec![None; cols.len()] };
        let Some(pack) = nfs_formats::fsh::Fsh::parse(&data).ok() else { return vec![None; cols.len()] };
        let entries = self.chronicle_entries();
        cols.iter()
            .map(|(year, variant)| {
                entries
                    .iter()
                    .find(|(y, k, v, _)| y == year && *k == 0 && v == variant)
                    .and_then(|(_, _, _, f)| pack.entry_caption(&data, f[0]))
            })
            .collect()
    }

    /// `Chronicle.Year` on the chronicle screen. The accessor is a range one of its own (0x508A24:
    /// 1948…1996, not the career's calendar year `[0x5DB764]`), and both the screen's constructor
    /// (0x506E30…0x506E98) and every group button (0x507690…) set it the same way: its range to
    /// `1948 … cursor[g]` through 0x4BA4F0 with 0x79C, then its value to the group's **earliest**
    /// year through slot 11, 0x4BC240, which clamps the index to `0 … count − 1`.
    ///
    /// Both years come out of 0x506FE0's walk over the group's icon pack: the earliest is the
    /// smallest `1900 + yy` of any tag, `yy = 0` being 2000 (0x507157…0x507180); the cursor starts
    /// at 1996 (0x50708B) and gains one for every `p` tag of a year at or past it that is the third,
    /// fifth… of its (year, kind) — an even count of at least two slots already filled, under ten
    /// (0x507186…0x5071B0).
    pub(crate) fn chronicle_year(&self) -> i32 {
        let (idx, _) = self.chronicle_year_index();
        1948 + idx
    }

    /// `Chronicle.Year`'s index and the count of its range (`1948 … cursor`): the accessor's
    /// slots 0x28 and 0x24 (0x4D0B90 reads both to place the knob). The index is what a group
    /// button (0x507690: the earliest year, slot 11) or the slider (0x4D0F20, slot 0x2c) last set,
    /// clamped to `0 … count − 1` like the setter 0x4BC240.
    fn chronicle_year_index(&self) -> (i32, i32) {
        let (earliest, cursor) = self.chronicle_year_range();
        let count = cursor - 1948 + 1;
        let idx = self.fe.chronicle_year_idx.unwrap_or(earliest - 1948);
        (idx.clamp(0, (count - 1).max(0)), count)
    }

    /// The group's earliest year and its year cursor — 0x506FE0's walk over the icon pack
    /// (see [`Self::chronicle_year`]).
    fn chronicle_year_range(&self) -> (i32, i32) {
        const PACKS: [&str; 5] = ["iconsrace", "icons911", "icons356", "icons924", "icons914"];
        let tags: Vec<String> = GamePaths::find_file(&self.paths.fe_data().join("Chron"), &format!("{}.fsh", PACKS[self.fe.chronicle_group.min(4)]))
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|d| nfs_formats::fsh::Fsh::parse(&d).ok())
            .map(|pack| pack.entries.iter().map(|e| e.name.clone()).collect())
            .unwrap_or_default();
        let (mut cursor, mut earliest) = (1996i32, None::<i32>);
        let mut filled: std::collections::HashMap<(i32, usize), u32> = std::collections::HashMap::new();
        for t in tags.iter().map(|t| t.as_bytes()) {
            if t.len() < 3 {
                continue;
            }
            let yy = match 10 * i32::from(t[0] as i8) + i32::from(t[1] as i8) - 0x210 {
                0 => 100,
                y => y,
            };
            let kind = match t[2] {
                b'p' | b'P' => 0,
                b'm' | b'M' => 1,
                b'v' => 2,
                _ => 0,
            };
            let year = 1900 + yy;
            earliest = Some(earliest.map_or(year, |e: i32| e.min(year)));
            let n = filled.entry((yy, kind)).or_insert(0);
            if *n < 10 {
                if year >= cursor && *n >= 2 && *n % 2 == 0 && kind == 0 {
                    cursor += 1;
                }
                *n += 1;
            }
        }
        let count = cursor - 1948 + 1;
        (1948 + (earliest.unwrap_or(1948) - 1948).clamp(0, (count - 1).max(0)), cursor)
    }

    /// `FEHorizontalSlider`'s helper on `chronicle.lay`'s `NFS5.CHRONICLESLIDER`
    /// (`accessor=Chronicle.Year`) — the press, 0x4D0900 (through the wrapper's slot 5, 0x4D06C0):
    /// 0x4D0CC0 sorts the pointer into a region — the knob part's rectangle is 3, the rest of the
    /// track 4 or 5 (the two arrow buttons of the helper's other users, regions 1 / 2, are not in
    /// this template: 0x4D0750 finds no `decrement_button` / `increment_button` part). On the
    /// knob the anchor `[+0x100]` is the pointer's x and `[+0x108]` the index at that moment
    /// (0x4D0973…0x4D098A). On the track the knob is put under the pointer first — its left edge is
    /// the anchor (0x4D0995…0x4D09F6, the knob's own left) and the region is worked out again
    /// (0x4D0A5C): only a press whose y is also on the knob rectangle leaves region 3, the one
    /// slot 8 drags in (0x4D0F50 tests `[+0x10C] == 3`), so a press lower on the track only jumps.
    ///
    /// **The event 7 (read 2026-09-30).** A track press ends in `0x4D0A82: call 0x4D6140`
    /// (`bVar4`, set only in that arm): it builds a `MouseMoveEvent` (vtable 0x5B6870, type 7,
    /// `[ev+8]`, `[ev+0xC]` = the pointer's x, y from the device state 0x5372A0(6) + 4 / + 8) and
    /// posts it through the owner's `[+0xD4]` slot 6 (`EventPump::post`, controls.md §4.40). It lands
    /// on the slider's own slot 8 (0x4D0F20) at once, which — the press region `[+0x10C]` being 3 when
    /// the pointer's y is on the knob — sets the index from the anchor the press just stored: so the
    /// press itself moves the knob, in the same frame, not the first mouse move after it. Ported: the
    /// track press runs the move handler with the press's own pointer. (0x4D6140's other callers,
    /// 0x4C7DBD — the combo box's letting go — and 0x4DB119 / 0x4E0A83, are the same "re-hover the
    /// widget under a pointer that has not moved" post.)
    pub(crate) fn chronicle_slider_press(&mut self) {
        let Some((cx, cy)) = self.fe.cursor else { return };
        let Some(g) = self.fe.layout_preview.as_ref().and_then(|l| l.chronicle_slider()) else { return };
        let (idx, count) = self.chronicle_year_index();
        self.fe.chronicle_slider_drag = if g.on_knob(cx, cy, idx, count) {
            Some((cx as i32, idx))
        } else if g.on_track(cx, cy) {
            // the knob's left edge moves to the pointer: it is on the knob again only when the
            // pointer's y is inside the knob's rectangle
            let drag = (cy >= g.y && cy < g.y + g.knob_h).then_some((g.knob_left(idx, count) as i32, idx));
            self.fe.chronicle_slider_drag = drag;
            // 0x4D0A82: the event 7 the press posts, at the pointer where it is
            self.chronicle_slider_move();
            return;
        } else {
            None
        };
    }

    /// Slot 8, 0x4D0F20: while the press was on the knob (`[+0x10C] == 3`) and the pointer is still
    /// inside the track (`[+0x110]` 3, 4 or 5), the accessor takes
    /// `(x − anchor) · (count − 1) / (track − knob) + start` (`idiv`), which the setter 0x4BC240
    /// clamps, and the helper re-places the knob (slot 0x2c = 0x4D0B90); the screen's timeline
    /// follows the accessor at its next draw (0x507999).
    pub(crate) fn chronicle_slider_move(&mut self) {
        let Some((anchor, start)) = self.fe.chronicle_slider_drag else { return };
        let Some((cx, cy)) = self.fe.cursor else { return };
        let Some(g) = self.fe.layout_preview.as_ref().and_then(|l| l.chronicle_slider()) else { return };
        if !g.on_track(cx, cy) {
            return;
        }
        let (cur, count) = self.chronicle_year_index();
        let want = g.dragged_index(cx, anchor, start, count).clamp(0, (count - 1).max(0));
        if want != cur {
            self.fe.chronicle_year_idx = Some(want);
            self.bind_chronicle_group();
        }
    }

    /// 0x5074A0's list for the screen the port is on: `<kind><group><yy><i><variant>.fsh` for
    /// `i` = 1…4, stopping at the first one that is not on disk.
    ///
    /// **The timeline's five columns — re-read 2026-09-28, correcting the paragraph this replaces.**
    /// The old reading ("the index's distinct (year, variant) pairs") was wrong: it collapsed
    /// every year with *no* data at all, so the timeline only ever showed years the pack has
    /// something for, back to back. Disassembling 0x507B4F itself (not just its neighbours) shows
    /// that is not what the original does.
    ///
    /// The loop that builds one column, **0x507B59…0x507B93**:
    ///
    /// ```text
    /// 0x507B59  for kind in 0..3: 0x507340(year, kind, variant, hot=0)
    /// 0x507B70    art != 0            -> skip the counter (this kind has something)
    /// 0x507B78    variant <= 0        -> skip the counter (variant 0 is never rolled over)
    /// 0x507B7A    else                -> miss += 1
    /// 0x507B8C  miss == 3 (all three kinds empty, and variant was > 0)
    /// 0x507B93    -> year += 1, variant = 0
    /// 0x507BA0  write (year, variant) as this column, regardless of whether anything is there
    /// ```
    ///
    /// So the roll-over test **never fires at variant 0** — `jle 0x507B78` skips the `miss`
    /// increment outright — which means a year with nothing in it is still shown, blank, as long
    /// as the walk reaches it at variant 0. The only years the walk ever skips are *extra*
    /// variants (1, 2, …) of a year that turns out to have nothing beyond variant 0.
    ///
    /// The outer per-column step, **0x507DE8…0x507E0E** (the loop back to 0x507B4B): `variant`
    /// is incremented **unconditionally** for the next column — there is no search for "the next
    /// column with data" anywhere in this function. Read together, the walk is: start at
    /// `(chronicle_year(), 0)`; for each of the five columns, roll `(year, variant)` over to
    /// `(year + 1, 0)` only when `variant > 0` and none of the three kinds has anything there;
    /// emit the (possibly rolled) `(year, variant)`; then bump `variant` by one for the next
    /// column. On this installation's 356 pack that gives `1948, 1949, 1950, 1951, 1952` —
    /// consecutive years, most of them with nothing in the `фото`/`постер` rows because 1949,
    /// 1951 and 1952 have no chronicle files at all — which is the original's own on-screen
    /// behaviour, not a career gate: 0x506FE0 (§4.118, re-read the same day) never touches a
    /// career global, and rendering this port's own `chronicle` golden from a completely empty
    /// `NFS5_SAVE_DIR` byte-for-byte matches the pristine-career render, confirming the screen is
    /// state-independent end to end.
    fn chronicle_columns(&self) -> Vec<(u32, usize)> {
        let entries = self.chronicle_entries();
        let has = |yy: u32, variant: usize| entries.iter().any(|(y, _, v, _)| *y == yy && *v == variant);
        let start = ((self.chronicle_year() - 1900) % 100) as u32;
        let (mut yy, mut variant) = (start, 0usize);
        let mut out = Vec::with_capacity(5);
        for _ in 0..5 {
            if variant > 0 && !has(yy, variant) {
                yy += 1;
                variant = 0;
            }
            out.push((yy, variant));
            variant += 1;
        }
        out
    }

    /// The (year, variant) the chronicle is on: the column a click has picked (0x5080C0's row
    /// handler reads it off `[this+0x160/0x174 + 4*col]`, the very table 0x507B4F fills), else the
    /// timeline's own opening column, index 0 of [`Self::chronicle_columns`]. §4.120, §4.121.
    fn chronicle_column(&self) -> Option<(u32, usize)> {
        let cols = self.chronicle_columns();
        self.fe.chronicle_col.and_then(|c| cols.get(c).copied()).or_else(|| cols.first().copied())
    }

    pub(crate) fn chronicle_media(&self) -> Vec<String> {
        // 0x507508's three letters, by the kind each screen passes (0x5080F8, 0x508137) — and
        // the same three the index's tags carry (§4.118)
        self.chronicle_media_for(match self.fe.layout_name.as_deref().unwrap_or("").to_ascii_lowercase().as_str() {
            "chroniclemovie" => 2usize,
            "chroniclemedia" => 1,
            _ => 0,
        })
    }

    /// 0x5074A0 for one kind — what a timeline cell's row asks for before it opens a screen
    /// (0x5080F8, 0x508137).
    pub(crate) fn chronicle_media_for(&self, k: usize) -> Vec<String> {
        let kind = [b'p', b'm', b'v'][k.min(2)] as char;
        let letter = CHRONICLE_GROUP_LETTERS.as_bytes()[self.fe.chronicle_group.min(4)] as char;
        // the year and the variant are the clicked column's (below); the picture the big widget
        // draws is `chronicle_sel` of the list this builds (`chronicle_picture`)
        // the column the timeline is on — 0x5080E0 reads the year and the variant out of
        // `[this+0x160 + 4*col]` and `[this+0x174 + 4*col]`, and a click on a cell moves the
        // column (§4.120)
        let Some((year, var)) = self.chronicle_column() else { return Vec::new() };
        let yy = format!("{:02}", year % 100);
        let variant = (b'a' + var.min(4) as u8) as char;
        let dir = self.paths.fe_data().join("Chron");
        let mut out = Vec::new();
        for i in 1..=4 {
            let name = format!("{kind}{letter}{yy}{i}{variant}.fsh");
            if GamePaths::find_file(&dir, &name).is_none() {
                break;
            }
            out.push(name);
        }
        out
    }
}

/// `0x5B8B18` — the model-group letter the chronicle's file names use, indexed by the group at
/// `[0x5DBA44]`. Five groups, and the five `icons*.fsh` packs in `FEData/Chron` match them.
const CHRONICLE_GROUP_LETTERS: &str = "CBADE";

/// `[0x5DBA44]` starts at 2, so the chronicle opens on `'A'` — the 356.
pub(crate) const CHRONICLE_GROUP: usize = 2;

/// `chronicle.lay`'s five model-group buttons, in the order the jump table at **0x507894** puts
/// them: 0x507839 `ID_RACEMODEL`, 0x50784B `ID_911BUTTON`, 0x50785D `ID_356BUTTON`, 0x50786F
/// `ID_924BUTTON`, 0x507881 `ID_914BUTTON`. The index is `[0x5DBA44]`, which is also what
/// [`CHRONICLE_GROUP_LETTERS`] is indexed by — so group 2 is `ID_356BUTTON` and letter `'A'`,
/// which is what the port already had. Each arm sets the flag on its own button and 0x5077D0
/// clears it on all five first. §4.111.
pub(crate) const CHRONICLE_GROUP_BUTTONS: [&str; 5] = ["ID_RACEMODEL", "ID_911BUTTON", "ID_356BUTTON", "ID_924BUTTON", "ID_914BUTTON"];
