//! `LayoutScreen` methods split out of the parent module (a move, no logic changed).

use super::*;

impl LayoutScreen {
    /// What one part of a widget actually covers: a `SHAPE`'s `.fsh` entry at its own pixel size,
    /// or a `TEXT`/`LOCALIZEDTEXT`'s string measured in the font that part draws it in.
    pub(super) fn part_extent(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, depth: u32) -> Option<(f64, f64)> {
        match p.kind() {
            nfs_formats::ctr::PartKind::Shape => {
                // the same resolution `shape_parts` uses — the part attribute's screen key against
                // the block, else the template's own value. `Graphic Tab Button.ctr`'s
                // `selectedbg` has an **empty** `shapePathName.value` and only the screen key
                // `selectedbgSmallPathName` reaches its pack, so without this the four unopened
                // tabs could not measure the art they centre their captions over.
                let file = self.part_attr(e, p, "shapePathName").filter(|v| !v.is_empty())?;
                let index = match self.part_attr(e, p, "shapeIndex") {
                    Some(v) => nfs_formats::ctr::eval(v, &|n| self.reference(e, n, depth + 1)).filter(|n| *n >= 0)? as usize,
                    None => 0,
                };
                let &(_, w, h) = self.shapes.get(&(file.to_string(), index))?;
                Some((w as f64, h as f64))
            }
            nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText => {
                // `FETextPart`'s load **0x4E7160**, after the base update 0x52B250 → 0x52A4F0 has
                // evaluated every attribute: the font `fontPathName` (`[+0xDC]`, rebuilt by
                // 0x4E37D0 only when the name changed), the part's text `[+0x19C]` measured by
                // 0x536970 (`BitmapFont::measure`: its third and fourth outputs, 0x4E727C /
                // 0x4E7285), both **truncated** (`_ftol` 0x5A0F98) and written over the part's
                // own `size` with the `"%d,%d"` setter 0x5276E0 (0x4E72A0). Whatever the template
                // says, a text part's `size` is the measure of the string it holds.
                let text = self.part_text(e, p, depth)?;
                let named = e
                    .get("fontPathName")
                    .filter(|f| !f.is_empty())
                    .or_else(|| p.value("fontPathName").filter(|f| !f.is_empty()))
                    .map(|f| f.to_lowercase());
                let f = named.and_then(|n| self.fonts.get(&n)).or_else(|| self.fonts.get(DEFAULT_FONT))?;
                let (w, h) = f.measure(&text);
                Some((w.trunc() as f64, h.trunc() as f64))
            }
            _ => None,
        }
    }

    /// The string a `TEXT` / `LOCALIZEDTEXT` part holds, `[part+0x19C]`: the `text` attribute's
    /// value after `StringAttribute::slot4` **0x526B60** has spliced each `%name%` with that
    /// macro's text (0x52DCE0; a name the dictionary does not hold stays as written). `None` when
    /// a name in it is one the port cannot answer.
    ///
    /// * `LOCALIZEDTEXT`'s load **0x4E66E0** first writes the text from its `stringID`: `atoi`
    ///   (0x5A1F4A) of the attribute's value, then **0x4E06D0** — id 0 is the empty string
    ///   0x5E8E50, an id no table holds is `"Not Found"` (0x5D9B38). 0x4E06D0 never answers null,
    ///   so 0x4E66E0's `"stringID = "` arm (0x4E6772) is dead code.
    /// * `NFS5.AccessorWatch`'s tick writes the accessor's answer into `label.text` (0x4BD32C);
    ///   the port keeps what its draw last showed (`watch_text`), and before that the part holds
    ///   the template's `WATCH`.
    /// * a `TEXT` part whose template gives no text keeps its constructor's `"Hello, World!"`
    ///   (0x5DDB84, pushed at 0x52B036).
    pub(super) fn part_text(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, depth: u32) -> Option<String> {
        use nfs_formats::ctr::PartKind;
        if depth > 4 {
            return None;
        }
        // 0x526B60's loop, read to the instruction 2026-09-26: the pair finder 0x526DF0 takes the
        // **first** `%` of the whole string at `a` and gives up — ending the loop — when `a + 2`
        // is not inside the string or `s[a+1]` is `%` (0x526E2D…0x526E3A); the closing `%` is the
        // first from `a + 2` (0x526E3C…). The name between is expanded (0x52DCE0) and **every**
        // `%name%` replaced with it by 0x526E80, which searches from the string's start after each
        // replacement (0x526F85…0x526FC4, 0x4E1920 from `[esi]`); then the search starts over
        // (0x526D4D). After the loop 0x526E80 turns every `%%` (0x5DD9E0) into `%` (0x5CCC90), the
        // same way (0x526D62…0x526DB6).
        let replace_all = |s: &mut String, pat: &str, with: &str| {
            // a replacement that brings its own pattern back would spin the original forever;
            // the port stops (guard of its own)
            for _ in 0..256 {
                let Some(k) = s.find(pat) else { break };
                s.replace_range(k..k + pat.len(), with);
            }
        };
        let splice = |v: &str| -> Option<String> {
            let mut s = v.to_string();
            for _ in 0..256 {
                let Some(a) = s.find('%') else { break };
                if a + 2 >= s.len() || s.as_bytes()[a + 1] == b'%' {
                    break;
                }
                let Some(b) = s[a + 2..].find('%').map(|k| k + a + 2) else { break };
                let name = s[a + 1..b].to_string();
                let x = nfs_formats::expr::expand(&name, &|n| self.reference(e, n, depth + 1));
                if x.contains(nfs_formats::expr::UNKNOWN) {
                    return None;
                }
                replace_all(&mut s, &format!("%{name}%"), &x);
            }
            replace_all(&mut s, "%%", "%");
            Some(s)
        };
        if p.kind() == PartKind::LocalizedText {
            let raw = splice(self.part_attr(e, p, "stringID").unwrap_or(""))?;
            // `atoi`: leading white space, a sign, then the digits
            let t = raw.trim_start();
            let (neg, t) = match t.as_bytes().first() {
                Some(b'-') => (true, &t[1..]),
                Some(b'+') => (false, &t[1..]),
                _ => (false, t),
            };
            let n = t.bytes().take_while(u8::is_ascii_digit).fold(0i32, |a, c| a.wrapping_mul(10).wrapping_add((c - b'0') as i32));
            let id = if neg { n.wrapping_neg() } else { n };
            if id == 0 {
                return Some(String::new());
            }
            return Some(self.fe_strings.get(id as u32).unwrap_or("Not Found").to_string());
        }
        if e.class.eq_ignore_ascii_case("NFS5.AccessorWatch")
            && let Some(i) = self.layout.elements.iter().position(|x| std::ptr::eq(x, e))
            && let Some(t) = self.watch_text.borrow().get(&i)
        {
            return Some(t.clone());
        }
        match self.part_attr(e, p, "text") {
            Some(v) => splice(v),
            None => Some("Hello, World!".to_string()),
        }
    }

    /// The widget's box. `Control::computeBox` 0x5253C0, re-read to the instruction 2026-09-30:
    /// `eax = [this+0xF0]` (the class template's bound `autosize` attribute object) then
    /// `ecx = [eax+0x38]` (its `value`, `[[this+0xF0]+0x38]`) — when that is nonzero
    /// (0x52540a…0x525516) the box is **unconditionally** the union of the parts the current
    /// state draws (0x5253E9…0x5254FF, zeroed then grown from the origin, exactly
    /// [`parts_union`](Self::parts_union)); the `.lay` block's own `size=`, if it carries one,
    /// is never read on that branch — not `[this+0xE4]` (the size attribute) nor anything
    /// derived from it. Only when autosize is **0** (0x525501: eight of the seventy-two
    /// templates that declare the key, §4.62) does the box come from `[this+0xE4]`'s pair —
    /// the `.lay`'s `size=` if it gives one, else the template's own `size.value` expression
    /// (`data.width,data.height` for a button, `data.width,data.selectionboxmaxheight` for a
    /// list, …).
    ///
    /// **Until 2026-09-30 this only took the union when the `.lay` block carried no `size=` of
    /// its own** — an extra guard the disassembly does not have, added when `parts_union` was
    /// first ported and never audited against every autosizing class. `NFS_BOX_SWEEP=1`
    /// measured the guard's effect over all 98 screens once it was lifted (`box_disagreements`,
    /// below, now audits every autosizing widget rather than only the no-`size=` subset): 875
    /// widget-instances across 16 classes differ from the guarded answer, `NFS5.PICTURE` (512)
    /// and `NFS5.AccessorWatch` (277) by far the largest groups. Two shapes turned up:
    ///
    /// * **The box changes and nothing reads it.** `NFS5.PICTURE`/`NFS5.NonLocalizedLabel`/
    ///   `NFS5.ExtraCircle` (`draw_template_classes`) and a matched `NFS5.AccessorWatch`
    ///   (`draw_default`'s `watch_pos`, `NFS5.POPUPCIRCLE`'s and `NFS5.PLAYERDRIVERPICTURE`'s
    ///   picture-only arms) draw every part at that part's **own** `pos`/`size`, never the
    ///   widget's outer box — `box_of` only matters to these for hit-testing, which
    ///   [`box_hit`](Self::box_hit) already implemented the *un*guarded rule for independently
    ///   (§4.62), so lifting the guard here changes nothing they draw.
    /// * **The box changes and something did read it, wrongly** — `NFS5.CarLogo`
    ///   (`draw_car_logo`) and `NFS5.ACCOUNTTHING` (`draw_account_thing`) both centred/sized
    ///   against the guarded box and were patched locally (33d21f13, 2c864da4) rather than here,
    ///   for exactly the reason this comment used to give: touching `box_of` looked like it would
    ///   touch every other autosizing class too. Both local patches are redundant now and are
    ///   removed — they read `d.w`/`d.h`, which is this function's own answer.
    ///
    /// **One more shape, not in the 875**: `NFS5.WORDWRAPCONTROL`/`NFS5.WORDWRAPACCESSORWATCH`
    /// (wrap width, `draw_default`) and `NFS5.MOVIEBOX`/`NFS5.MOVIECIRCLE` (movie blit size,
    /// `draw_movie`) also autosize, but their own draws were **already** reading something other
    /// than `box_of`'s depth-0 answer for their size — the wrap engine measures the `area` part
    /// (`size.value=control.size.width,control.size.height`, the attribute, §4.64) and the movie
    /// player's box is read straight off the widget's own `size` attribute at bind time
    /// (0x4CD034). Both keep reading the attribute (`box_of(e, 1)`, depth > 0) rather than this
    /// function's union, which is right for them but would have been very wrong for a long
    /// paragraph's `label` part (measured unwrapped, hundreds of pixels wide) had they read `w`.
    ///
    /// **Only at depth 0**, which is to say only for the widget's own box and never for a
    /// `control.size.*` term inside a part's expression. §4.64: those two are not the same
    /// thing in the original either — 31 of the templates that reference `control.size.width`
    /// autosize, and if that term went through the box the union would re-enter itself before
    /// `computeBox` sets its valid flag (0x525516) and never come back. The term is the `size`
    /// attribute; the box is the union.
    pub(super) fn box_of(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
        if depth == 0
            && self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() != "0").unwrap_or(false)
            && let Some(u) = self.parts_union(e)
        {
            return u;
        }
        self.box_sized(e, depth)
    }

    /// The box a widget is **hit-tested** against, which is the template's `size.value` with the
    /// part-relative terms measured — `box_of` measures them too since 2026-09-30 (the
    /// unmeasured pass was the port's own recursion guard, not a rule of the original), and the
    /// difference only ever shows on a class whose size is written over its parts.
    ///
    /// Two of them have a hover state and so can be hit at all: `BottomBarButton.ctr`'s
    /// `(label.size.width + picture.size.width + data.space), data.height` and
    /// `Graphic Tab Button.ctr`'s `hoverbg.size.width, hoverbg.size.height`. Without this the box
    /// came out (0, 0) and the 27 of them could not be pointed at (§4.53, §4.54).
    ///
    /// **Only where `size.value` is live.** §4.62: `Control::getSize` 0x525360 and
    /// `computeBox` 0x5253C0 both branch on the `autosize` attribute (`[this+0xF0]`, bound at
    /// 0x52498A), and when it is set the box is the **union of the parts the current state
    /// draws**, not the `size` expression at all. Seven of the seventy shipped templates set it to
    /// 0 — `BottomBarButton`, `ComboBox`, `ColorBox`, `GoToButton`, `Graphic Tab Button`,
    /// `MultiAccessor` and `OvalThing` — and only those seven have a `size.value` the original
    /// ever evaluates.
    pub(super) fn box_hit(&self, e: &nfs_formats::lay::Element) -> (f32, f32) {
        let live = self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() == "0").unwrap_or(false);
        if !live && let Some(p) = self.parts_union(e) {
            return p;
        }
        self.box_sized(e, 0)
    }

    /// Every **autosizing** widget whose box disagrees with the union of its own parts —
    /// `box_of` now returns that union unconditionally at depth 0 (§4.62's rule, re-read
    /// 2026-09-30), so since this walks every autosizing widget rather than only the ones a
    /// `.lay` block leaves `size=`-less, it should always come back empty; a non-empty result
    /// means `box_of` and `parts_union` have drifted apart again. `NFS_BOX_SWEEP=1` prints it.
    pub fn box_disagreements(&self) -> Vec<(String, (f32, f32), (f32, f32))> {
        let mut out = Vec::new();
        for e in self.layout.elements.iter().skip(1) {
            let autosizes = self.controls.get(&e.class).and_then(|c| c.value("autosize")).map(|v| v.trim() != "0").unwrap_or(false);
            if !autosizes {
                continue;
            }
            let Some(u) = self.parts_union(e) else { continue };
            let b = self.box_of(e, 0);
            if (b.0 - u.0).abs() > 1.0 || (b.1 - u.1).abs() > 1.0 {
                out.push((e.class.clone(), b, u));
            }
        }
        for g in self.groups.values() {
            out.extend(g.box_disagreements());
        }
        out
    }

    /// The box an **autosizing** widget gets: the union of the rects of the parts its state draws,
    /// started at the origin — `Control::computeBox` 0x5253C0, the branch at 0x525446.
    ///
    /// ```text
    /// left = top = right = bottom = 0                      ; 0x5253E9…0x525403
    /// for each name in the state's part list:
    ///     part = lookupPart(this, name)                     ; 0x525620, skipped when < 0
    ///     r    = part->slot2()                              ; the part's own rect
    ///     left = min(left, r.left);  top    = min(top,    r.top)
    ///     right= max(right, r.right); bottom = max(bottom, r.bottom)
    /// ```
    ///
    /// Because it starts at zero the box always contains the widget's own origin, so it can only
    /// grow from there. The port takes each part's `pos` and `size` the way it draws them, falling
    /// back to what the part measures when the template gives it no size (§4.54's `part_extent`).
    fn parts_union(&self, e: &nfs_formats::lay::Element) -> Option<(f32, f32)> {
        let c = self.controls.get(&e.class)?;
        let state = e.get("state").unwrap_or("normal");
        let (mut r, mut b) = (0.0f32, 0.0f32);
        let (mut l, mut t) = (0.0f32, 0.0f32);
        let mut any = false;
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            // only what the state **draws**. A `DATA` part is the widget's attribute block, not a
            // rect, and its `pos` carries a screen key that resolves to the widget's own `pos=` —
            // unioning that would drag the box back to the screen's origin.
            if !matches!(
                p.kind(),
                nfs_formats::ctr::PartKind::Rect
                    | nfs_formats::ctr::PartKind::Shape
                    | nfs_formats::ctr::PartKind::Text
                    | nfs_formats::ctr::PartKind::LocalizedText
            ) {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let (px, py) = at("pos").unwrap_or((0, 0));
            // a text part's size is its text's measure whatever the template says (0x4E72A0)
            let text_part = matches!(p.kind(), nfs_formats::ctr::PartKind::Text | nfs_formats::ctr::PartKind::LocalizedText);
            let (pw, ph) = match at("size").filter(|_| !text_part) {
                Some((w, h)) => (w as f64, h as f64),
                None => self.part_extent(e, p, 0).unwrap_or((0.0, 0.0)),
            };
            l = l.min(px as f32);
            t = t.min(py as f32);
            r = r.max(px as f32 + pw as f32);
            b = b.max(py as f32 + ph as f32);
            any = true;
        }
        (any && r - l >= 1.0 && b - t >= 1.0).then_some((r - l, b - t))
    }

    fn box_sized(&self, e: &nfs_formats::lay::Element, depth: u32) -> (f32, f32) {
        if let Some(p) = e.pair("size") {
            return p;
        }
        // **A `size` with one usable component overwrites only the width.** 0x4B5936 skips an
        // empty token without emitting a value, so `size=150,` — the three identical
        // `NFS5.Rectangle`s of `location.lay` §12, `locationMP.lay` §16 and `locationQK.lay` §16
        // — parses to the single number 150, and the height stays whatever the template put
        // there: `Rectangle.ctr`'s `size.value=50,50`. §4.116.
        let given = e.parts_of("size");
        if given.len() == 1 {
            let h = self.box_template(e, depth).map(|(_, h)| h).unwrap_or(0.0);
            return (given[0], h);
        }
        // **A `size=` key present but carrying no token at all is (0, 0), not the template's
        // default.** The one shipped instance is `carcompare.lay` §14, a stray duplicate of
        // §11/12's hairline divider (same `pos=322,~370`, same `color=0x80184258`) whose `size=`
        // line was left blank — the only blank `size=` in any of the 98 shipped `.lay` files
        // (checked by grep over all of them). **Why (0, 0), read 2026-09-30 (the note of
        // 2026-09-29 that put this on an "unconditional" pair parser 0x527BC0 was wrong):** the
        // pair parser is `PointAttribute::slot4` **0x527B80** (vtable 0x5BB0AC + 0x10) and it
        // cuts the value at its first comma (`strchr` 0x527BA9) — **no comma, nothing is
        // evaluated** (`je 0x527CAD` at 0x527BB7), so a blank `size=` leaves the attribute what
        // its constructor **0x527590** made it: the two dwords `[+0x38]`, `[+0x3C]` copied from the
        // default the control's constructor 0x5246E0 passes for `size` (0x5247FB), a point whose
        // every stack cell that constructor zeroes (0x5247AF…0x5247E1) — (0, 0). The template's
        // own `size.value` is never evaluated in its place, because the attribute's string is the
        // `.lay` value (0x52C000) and that is the empty one. This agrees with the reference
        // capture (`RE/reference/2026-09-28/pass6_carcompare.jpg`: no rectangle behind the
        // "PORSCHE" name graphic, where this port drew a 50×50 teal-grey box before this fix,
        // `crop_carcompare_badge_port.png`). The *evaluator's* answer for an empty half of a
        // comma pair (`size=150,`) is a different path and also 0: 0x52F84A…0x52F880 → the operand
        // reader 0x5309E0 → 0x530F0A → 0x530F2F, which only reports error 5 (0x531C30) and returns
        // `xor eax, eax` (0x530F41). CONFIRMED. `layouts.md` "carcompare, read 2026-09-29".
        if given.is_empty() && e.get("size").is_some_and(|v| v.trim().is_empty()) {
            return (0.0, 0.0);
        }
        if let Some(p) = self.box_template(e, depth) {
            return p;
        }
        (e.float("width").unwrap_or(0.0), e.float("height").unwrap_or(0.0))
    }

    /// The class template's own `size.value`, evaluated.
    fn box_template(&self, e: &nfs_formats::lay::Element, depth: u32) -> Option<(f32, f32)> {
        if depth > 4 {
            return None;
        }
        let c = self.controls.get(&e.class)?;
        let v = c.value("size")?;
        nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, depth + 1)).map(|(w, h)| (w as f32, h as f32))
    }

    /// The widget's position: the block's own `pos`, or the class template's `[control] pos.value`
    /// — `NFS5.HEADER` carries no `pos` at all and `Header.ctr` puts it at (6, 4).
    pub(super) fn position_of(&self, e: &nfs_formats::lay::Element) -> (f32, f32) {
        if let Some(p) = e.pair("pos") {
            return p;
        }
        if let Some(c) = self.controls.get(&e.class)
            && let Some(v) = c.value("pos")
            && let Some((x, y)) = nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0))
        {
            return (x as f32, y as f32);
        }
        (0.0, 0.0)
    }

    /// Whether the widget's `state` selects **no drawable part at all**, in which case it draws
    /// nothing however much its `.lay` block carries.
    ///
    /// A `.lay` block's `state=` is not decoration: it is the name of one of the template's
    /// `[state.<name>]` sections, and that section's `parts=` *is* the widget's draw list.
    /// The chain in the original:
    ///
    /// * the nine state names live in a 25-byte-stride table at **0x5D81FC**, ending at 0x5D82DD —
    ///   `normal`, `stringnotused`, `hover`, `selected`, `disabled`, `hidden`, `focused`,
    ///   `focusedhover`, `selectedhover`, in that order;
    /// * **0x4C778F…0x4C7818** walks that table and, for each name the template's `states=` list
    ///   carries, sets bit `1 << i` in `[widget+0x110]` — the states the class supports;
    /// * **0x4C7820…0x4C78B2** clears `[widget+0x10C]` and matches the widget's own `state`
    ///   attribute against names 2…8, setting that name's bit on a hit;
    /// * **0x4C82A0** turns those flags back into a name by priority — 0x20 → `"hidden"`
    ///   (0x4C82FE), 0x10 → `"disabled"`, 0x40+0x04 → `"focusedhover"`, 0x40 → `"focused"`,
    ///   0x08+0x04 → `"selectedhover"`, 0x08 → `"selected"`, 0x04 → `"hover"`, else `"normal"`;
    /// * **0x525285…0x5252F5** matches that name against the loaded `[state.*]` sections and hands
    ///   the index to 0x525580 → **0x5271F0**, which *copies that state's `parts` vector over the
    ///   widget's active part list* (`[[widget+0xF4]+0x10]`).
    ///
    /// So when the chosen section lists only `DATA` parts — and `DATA` draws nothing, it is the
    /// attribute block (`RE/notes/controls.md` §1) — the widget is erased. Three shipped widgets
    /// are in exactly that case, all `NFS5.PANEL` with `state=hidden` against `Panel.ctr`'s
    /// `[state.hidden] parts=data`: `carsetup.lay` §77, `carsetupdesc.lay` §11 and
    /// `usedcars.lay` §19. Each carries `color=0x80ffFF00`, so drawing them anyway paints a
    /// half-transparent yellow slab over the middle of those three screens.
    pub(super) fn state_draws_nothing(&self, e: &nfs_formats::lay::Element) -> bool {
        let Some(c) = self.controls.get(&e.class) else { return false };
        let Some(state) = e.get("state") else { return false };
        // an unknown state name leaves the ctor's own list in place, which is every part
        if !c.has_state(state) {
            return false;
        }
        !c.state_parts(state).iter().filter_map(|n| c.part(n)).any(|p| p.kind() != nfs_formats::ctr::PartKind::Data)
    }

    /// An embedded layout's sprites, moved into the widget's own place. The offset is applied in
    /// the same normalised space [`place`](Self::place) maps into, so it carries the letterbox
    /// factor with it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn embedded(&self, g: &LayoutScreen, aspect: f32, strings: &Strings, accessor: &dyn Fn(&str) -> Option<String>, blank: &mut Vec<(String, String)>, x: f32, y: f32) -> Vec<Sprite> {
        // an embedded layout is one of the 98 the sweep already walks on its own, so its gaps are
        // reported there and not again here — see the note on the background above
        let _ = &blank;
        g.inherited.set(self.widget_state());
        let mut inner = g.sprites_reporting(aspect, strings, None, accessor, &mut Vec::new());
        if x != 0.0 || y != 0.0 {
            let design_aspect = DESIGN.0 / DESIGN.1;
            let fw = if aspect > design_aspect { design_aspect / aspect } else { 1.0 };
            let (dx, dy) = (x / DESIGN.0 * fw, y / DESIGN.1);
            for sp in &mut inner {
                sp.pos[0] += dx;
                sp.pos[1] += dy;
                // a per-corner triangle carries its own points
                if let Some((t, _)) = sp.tri.as_mut() {
                    for v in t {
                        *v = [v[0] + dx, v[1] + dy];
                    }
                }
            }
        }
        inner
    }

    /// The `RECT` parts of the widget's class template that `state` draws, each resolved to a
    /// rectangle and a colour (`RE/notes/controls.md` §1). This is how a class that is nothing but
    /// boxes — `NFS5.CONTROLLERSETUP` is a filled box with a one-pixel border — draws without the
    /// port knowing anything about it. The state is named rather than taken from the block, because
    /// a widget that can change state draws the parts of the state it is in now.
    pub(super) fn rect_parts_in(&self, e: &nfs_formats::lay::Element, state: &str) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let Some((x, y)) = at("pos") else { continue };
            let Some((w, h)) = at("size") else { continue };
            // a colour may be a literal or a reference to one of the `data` attributes
            let colour = self
                .part_attr(e, p, "color")
                .and_then(|v| parse_color(v).or_else(|| v.split_once('.').and_then(|(_, key)| self.attr(e, key)).and_then(parse_color)));
            let Some(colour) = colour else { continue };
            out.push((x as f32, y as f32, w as f32, h as f32, colour));
        }
        out
    }

    /// A part's colour attribute: a literal `0x…`, or an **expression** over the widget's own
    /// attributes — `GoToButton.ctr` writes its caption's colour as
    /// `data.medium?0x00000000:0xffffffff`, so which of its three labels is opaque is arithmetic.
    pub(super) fn part_colour(&self, e: &nfs_formats::lay::Element, p: &nfs_formats::ctr::Part, key: &str) -> Option<[u8; 4]> {
        let v = self.part_attr(e, p, key)?;
        if let Some(c) = parse_color(v) {
            return Some(c);
        }
        // a 32-bit pattern: `0xffffffff` is −1 to the evaluator and every bit of it the colour
        let n = nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0))? as u32;
        Some([((n >> 16) & 0xFF) as u8, ((n >> 8) & 0xFF) as u8, (n & 0xFF) as u8, (n >> 24) as u8])
    }

    /// The `RECT` parts of one state of the widget's class, each resolved to a rectangle and a
    /// colour, with the fully transparent ones dropped. Unlike [`rect_parts_in`](Self::rect_parts_in)
    /// it evaluates the colour as an expression — a button's highlight is `[state.selected]`,
    /// which the `.lay` block never says, and its colour can be arithmetic.
    pub(super) fn state_rects(&self, e: &nfs_formats::lay::Element, state: &str) -> Vec<(f32, f32, f32, f32, [u8; 4])> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            if p.kind() != nfs_formats::ctr::PartKind::Rect {
                continue;
            }
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let (Some((px, py)), Some((pw, ph))) = (at("pos"), at("size")) else { continue };
            let Some(col) = self.part_colour(e, p, "color").filter(|c| c[3] != 0) else { continue };
            out.push((px as f32, py as f32, pw as f32, ph as f32, col));
        }
        out
    }

    /// One part attribute with the `%…%` indirection resolved: `%<part>.<key>%` is that part's
    /// value for that key, which is how `GoToButton.ctr`'s two spare captions say
    /// `stringID.value=%label.stringID%` — "the same string the first one shows".
    fn part_ref<'a>(&'a self, e: &'a nfs_formats::lay::Element, p: &'a nfs_formats::ctr::Part, key: &str) -> Option<String> {
        let v = self.part_attr(e, p, key)?;
        let Some(inner) = v.strip_prefix('%').and_then(|v| v.strip_suffix('%')) else {
            return Some(v.to_string());
        };
        let (part, k) = inner.split_once('.')?;
        if part == "data" {
            return e.get(k).map(str::to_string);
        }
        let q = self.controls.get(&e.class)?.part(part)?;
        self.part_attr(e, q, k).map(str::to_string)
    }

    /// Draw a widget entirely out of its class template: the `RECT` parts of the state, then its
    /// `SHAPE` parts, then its `TEXT` / `LOCALIZEDTEXT` parts — each where the template puts it,
    /// in the font and colour it names, and skipping any whose colour is fully transparent. That
    /// last rule is how a class carries several captions and shows one (`controls.md` §4.3).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn template_widget(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, state: &str, x: f32, y: f32, strings: &Strings) {
        self.template_widget_at(out, aspect, e, state, x, y, strings, None)
    }

    /// The same, with the `picture` part pinned to one entry of its pack — the animation of §4.53.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn template_widget_at(&self, out: &mut Vec<Sprite>, aspect: f32, e: &nfs_formats::lay::Element, state: &str, x: f32, y: f32, strings: &Strings, frame: Option<usize>) {
        for (px, py, pw, ph, col) in self.state_rects(e, state) {
            out.push(self.quad(aspect, x + px, y + py, pw.max(1.0), ph.max(1.0), tint(col)));
        }
        // the art of the state the widget is in **now**, like its boxes and its text: the draw
        // 0x4C8260 ends in 0x525830, which walks the part list of the current state
        // (`[[this+0xD4]] + 0x525570()·4`, 0x525876…0x525886) — the index `updateState` 0x4C82A0
        // sets through 0x525580 — not the `.lay` block's `state=`
        for (px, py, file, index, flip) in self.shape_parts_at(e, state, frame) {
            let Some(&(tex, aw, ah)) = self.shapes.get(&(file, index)) else { continue };
            let (pos, size) = self.place(aspect, x + px, y + py, aw, ah);
            let (u0, u1) = if flip & 1 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            let (v0, v1) = if flip & 2 != 0 { (1.0, 0.0) } else { (0.0, 1.0) };
            out.push(Sprite { texture: tex, pos, size, uv_min: [u0, v0], uv_max: [u1, v1], color: [1.0, 1.0, 1.0, 1.0], rotation: 0.0, pivot: [0.5, 0.5], tri: None, corner_colors: None, depth: None });
        }
        let Some(c) = self.controls.get(&e.class) else { return };
        for p in c.state_parts(state).iter().filter_map(|n| c.part(n)) {
            let text = match p.kind() {
                nfs_formats::ctr::PartKind::LocalizedText => {
                    let id = self.part_ref(e, p, "stringID").and_then(|v| v.trim().parse::<u32>().ok());
                    match id.filter(|n| *n != 0).and_then(|n| strings.get(n)) {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => continue,
                    }
                }
                nfs_formats::ctr::PartKind::Text => match self.part_ref(e, p, "text").filter(|t| !t.is_empty()) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            };
            let Some(col) = self.part_colour(e, p, "foreColor").filter(|c| c[3] != 0) else { continue };
            let (px, py) = self
                .part_attr(e, p, "pos")
                .and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)))
                .unwrap_or((0, 0));
            let Some(f) = self
                .part_attr(e, p, "fontPathName")
                .map(|f| f.to_lowercase())
                .and_then(|f| self.fonts.get(&f))
                .or_else(|| self.fonts.get(DEFAULT_FONT))
            else {
                continue;
            };
            self.text(out, aspect, f, &text, x + px as f32, y + py as f32, 0.0, 0, tint(col));
        }
    }

    /// One attribute of one part, resolved the way the original does: the `.lay` block's value for
    /// the attribute's **screen key** when it has one, else the template's `<key>.value`
    /// (`nfs_formats::ctr::Part::screen_key`, 0x52BC90 / 0x52BFB4 / 0x52C000).
    pub(super) fn part_attr<'a>(&self, e: &'a nfs_formats::lay::Element, p: &'a nfs_formats::ctr::Part, key: &str) -> Option<&'a str> {
        if let Some(k) = p.screen_key(key)
            && let Some(v) = e.get(&k).filter(|v| !v.is_empty())
        {
            return Some(v);
        }
        // an **empty** `<key>.value` is not a value: `Graphic Tab Button.ctr` writes `size.value=`
        // on the parts whose size is their art's, and `shapePathName.value=` on the two whose pack
        // only the screen key names. Reading either as a value can only fail.
        p.value(key).filter(|v| !v.trim().is_empty())
    }

    /// The `SHAPE` parts of the widget's class template that `state` draws, each resolved to a
    /// place and a `.fsh` pack entry (`RE/notes/controls.md` §1), with the `picture` part's entry
    /// replaced by `frame` — what `NFS5.BOTTOMBARBUTTON`'s tick does every frame through
    /// `0x5271F0([this+0x17C], frame)` (§4.53).
    ///
    /// This is the other half of [`rect_parts_in`](Self::rect_parts_in): a class whose art is entirely in
    /// its template draws through here without the port knowing anything about it.
    /// `NFS5.CARSETTINGBUTTON` is the plainest case — `CarSettingButton.ctr` gives its `picnonsel`
    /// part `shapePathName=weeicons.fsh` and `shapeIndex=data.nonselindex`, so the widget's own
    /// `nonselindex` picks the arrow. The same shape covers `NFS5.POPUPCIRCLE`,
    /// `NFS5.ExtraCircle`, the tab buttons and the rest of the art-from-template classes.
    ///
    /// `state` is the **current** one for every draw that ends in 0x525830 (§2026-09-27 in
    /// `layouts.md`); there is no caller left that takes the `.lay` block's `state=` (the old
    /// `shape_parts`, retired 2026-09-27).
    pub(super) fn shape_parts_at(&self, e: &nfs_formats::lay::Element, state: &str, frame: Option<usize>) -> Vec<(f32, f32, String, usize, i64)> {
        // `NFS5.POPUPCIRCLE` **is drawn**, and the reading that said otherwise rested on the
        // draw order being the wrong way round (§4.67).
        //
        // `PopUp.ctr` pins it at `pos=113,100` (`HIDDEN|READONLY`) and tiles **one** 207×175
        // quarter four ways — `p1`…`p4` at (0,0), (207,0), (0,175), (207,175) with `flip` 0…3 —
        // a 414×350 oval over the middle of a 640×480 screen. Its states are `normal` and
        // `hidden`, and `[state.hidden]` draws only `data`, which is nothing.
        //
        // **Nothing can put it in that state.** Neither the class id `NFS5.POPUPCIRCLE` nor the
        // widget name `ID_POPUPCIRCLE` appears anywhere in the executable — a byte search of the
        // whole image finds neither — so there is no class code and no lookup by name that could
        // reach `setHidden`. And all **19** shipped blocks carry `state=normal`. It is therefore
        // on, on every one of those screens, always.
        //
        // The old note argued from its being the **last** section in 18 of the 19 that it "draws
        // over everything else". That was true only while this port painted sections forwards; the
        // original walks the child vector backwards (§4.67), so the last section is painted
        // **first** — the oval goes *under* the screen, which is what a background vignette does.
        //
        // **Not** gated on `swrenderoff` either: that key reaches `[widget+0x13C]` at 0x4C7733 and
        // 0x4C7750 and a scan of the whole `.text` for the displacement finds those two writes and
        // **no read at all**, so it is dead in this build (CONFIRMED by exhaustion).
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        self.shape_parts_in(e, c.state_parts(state), frame)
    }

    /// The `SHAPE` parts among `names`, in that order, whatever state the widget is in — for a
    /// class whose draw holds its parts by pointer instead of walking a state's list (the movie
    /// circle's nine stencil parts, 0x4CDB2C…0x4CDB57).
    pub(super) fn shape_parts_in(&self, e: &nfs_formats::lay::Element, names: &[String], frame: Option<usize>) -> Vec<(f32, f32, String, usize, i64)> {
        let Some(c) = self.controls.get(&e.class) else { return Vec::new() };
        let mut out = Vec::new();
        for (name, p) in names.iter().filter_map(|n| c.part(n).map(|p| (n.clone(), p))) {
            if p.kind() != nfs_formats::ctr::PartKind::Shape {
                continue;
            }
            // A part's `shapePathName.value` and `shapeIndex.value` are that part's **defaults**,
            // and the `.lay` block's own keys override them — the ordinary attribute rule of
            // `RE/notes/controls.md` §1, nothing special about these two. Five classes name their
            // own pack: `NFS5.PICTURE` (484 of them) also names the entry, so its single part
            // resolves to exactly the pair the widget carries; `NFS5.BOTTOMBARBUTTON`,
            // `NFS5.CheckBox`, `NFS5.AnimatingPicture` and `NFS5.CarSettingSlider` name only the
            // pack and keep the part's index — `data.startframe`, `data.onIndex`,
            // `data.start_frame` and the literal 8 of `CarSettingSlider.ctr`'s knob.
            //
            // The port used to give up on the whole part list as soon as a widget named a pack and
            // draw the widget's own pair instead, which is the same picture for a `NFS5.PICTURE`
            // and the **wrong entry** for the other four: every bottom-bar button drew `logoAn`
            // entry 0 where its `startframe` says 11, and every car-setting slider entry 0 of
            // `weeicons.fsh` where its knob is entry 8.
            // `%<part>.<key>%` is the indirection of `controls.md` §2 — that other part's value
            // for that key — and `CarSettingButton.ctr` is the one template that uses it for art:
            // its `picsel` and `picseldown` both take `%picnonsel.shapePathName%`, so the hover
            // and pressed arrows come out of the same pack as the plain one.
            let Some(file) = self.part_ref(e, p, "shapePathName").filter(|v| !v.is_empty()) else { continue };
            // `shapeIndex` is a literal or a `data.<key>` reference to the widget's own attribute
            let index = match frame.filter(|_| name == "picture") {
                Some(n) => n,
                None => match self.part_attr(e, p, "shapeIndex") {
                    Some(v) => match nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0)) {
                        Some(n) if n >= 0 => n as usize,
                        _ => continue,
                    },
                    None => 0,
                },
            };
            let at = |key: &str| self.part_attr(e, p, key).and_then(|v| nfs_formats::ctr::eval_pair(v, &|n| self.reference(e, n, 0)));
            let (px, py) = at("pos").unwrap_or((0, 0));
            let (px, py) = (px as f64, py as f64);
            // No size: a `SHAPE` part is always drawn at its **art's** size. `FEShapePart`'s load
            // (vtable 0x5B6E64 slot 8, **0x4E6AA0**) fetches the entry (0x4D7A10 at 0x4E6EBE) and
            // writes its width and height (`+0x14`, `+0x18`) over the part's `size` attribute
            // (0x5276E0 on `[part+0x60]` at 0x4E6EE2), and the draw 0x4E6F60 hands the entry to
            // 0x4D9AF0, which reads the same two fields for the quad's extent (0x4D9B04, 0x4D9B0E).
            // The only `SHAPE` parts that write a `size.value` are `CarSettingSlider.ctr`'s three
            // knobs, `6,11` — which is the art (`weeicons.fsh` 8, 34, 35 are 6×11).
            // **`data.picoffy`** — `NFS5.BOTTOMBARBUTTON`'s own draw, 0x4C9B30, adds it to the
            // `picture` part's y and nothing else: `[this+0x1D4]` (the bind at 0x4C9639) plus the
            // part's own `pos.y` from `0x4E7060([this+0x178])` plus the widget's y, all three at
            // 0x4C9BDE…0x4C9BED, with the x left alone (0x4C9BFD). Three of `bottombar.lay`'s four
            // buttons — back, options and exit, the three with an icon — carry **6**; the other
            // nineteen shipped bar buttons carry 0. §4.80.
            let py = py
                + if name == "picture" && e.class == "NFS5.BOTTOMBARBUTTON" {
                    self.attr_num(e, "picoffy").unwrap_or(0.0)
                } else {
                    0.0
                };
            // `flip` is the part's own attribute — `Graphic Tab Button.ctr` writes
            // `flip.value=data.flipit`, so the widget's `flipit` mirrors the tab's art
            let flip = self
                .part_attr(e, p, "flip")
                .and_then(|v| nfs_formats::ctr::eval(v, &|n| self.reference(e, n, 0)))
                .filter(|n| *n >= 0)
                .map(|n| n as i64)
                .unwrap_or_else(|| e.int("flip").unwrap_or(0).max(0));
            out.push((px as f32, py as f32, file.to_string(), index, flip));
        }
        out
    }

    /// The first drawable part of the widget's class template that carries `key`, e.g. the
    /// `fontPathName` of a header's `label` part.
    pub(super) fn part_value<'a>(&'a self, e: &'a nfs_formats::lay::Element, key: &str) -> Option<&'a str> {
        let c = self.controls.get(&e.class)?;
        let state = e.get("state").unwrap_or("normal");
        c.state_parts(state).iter().filter_map(|n| c.part(n)).find_map(|p| p.value(key)).filter(|v| !v.is_empty())
    }
}
