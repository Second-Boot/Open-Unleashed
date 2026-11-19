//! The front end's pad / arrow-key targets (PORT-DEVIATION D54, extending D10): the original's
//! front end is pointer-only (`RE/notes/controls.md` §4.38, §4.169.5), so the port's keys move a
//! virtual pointer between the places a click would land. This module lists those places — every
//! widget the mouse can operate, with the point a click on it goes to — and picks the neighbour in
//! a direction. Nothing here is the original's; the clicks themselves are the mouse path's.

use super::*;

/// What a [`PadTarget`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadKind {
    /// a button that leads somewhere, runs a callback or steps an accessor
    Button,
    /// a tab of a strip (`NFS5.TABBUTTON`, `NFS5.GRAPHICTABBUTTON`)
    Tab,
    /// a closed `NFS5.ComboBoxSlider` / `NFS5.ColorBox`; a click on its value part opens the list
    Combo,
    /// one visible row of an open drop-down; a click on it commits the row
    ComboRow,
    /// one visible row of an `NFS5.ListBox`
    ListRow,
    /// any other widget that takes a click (check box, controller row, camera pictures, …)
    Other,
    /// `NFS5.CarSettingSlider`: Left / Right step its accessor by one, as the arrow buttons beside it do
    CarSlider,
    /// a `data.changeable` `NFS5.GouraudIndicatorBar`: Left / Right move its accessor by [`bar_step`]
    Bar,
    /// an `NFS5.DEADZONE` bar: Left / Right move the active handle pair, A switches the pair
    DeadZone,
    /// `NFS5.CHRONICLESLIDER`: Left / Right move the year by one
    ChronicleSlider,
}

impl PadKind {
    /// A widget a drag sets: on it Left / Right **adjust** the value (PORT-DEVIATION D54, PAD-SLIDERS) instead of moving.
    pub fn adjustable(self) -> bool {
        matches!(self, PadKind::CarSlider | PadKind::Bar | PadKind::DeadZone | PadKind::ChronicleSlider)
    }
}

/// How many values one press moves an indicator bar of `n` values: a twentieth of the range, at least one (the bar sets
/// any of its `n` values from the pointer, so the pad takes a coarser, quick step; a held key repeats it).
pub fn bar_step(n: i32) -> i32 {
    (n / 20).max(1)
}

/// The index the pad's step leaves a bar of `n` values on, from `cur`: `dir` is -1 or 1, the ends hold.
pub fn bar_step_index(n: i32, cur: i32, dir: i32) -> i32 {
    (cur + dir * bar_step(n)).clamp(0, (n - 1).max(0))
}

/// The fraction of the bar that `set_accessor_fraction` turns back into index `idx` of `n` (the middle of its cell,
/// so the pointer's truncation `trunc(n * f)` lands exactly there).
pub fn bar_fraction(n: i32, idx: i32) -> f32 {
    (idx as f32 + 0.5) / n.max(1) as f32
}

/// One press of the dead-zone bar's Left / Right, in units of the 0…128 scale (a thirty-second of it).
pub const DEADZONE_PAD_STEP: i32 = 4;
/// One press of the colour editor's d-pad: a thirty-second of the wheel's width (hue) or height (saturation).
pub const PICKER_PAD_STEP: f32 = 1.0 / 32.0;
/// One press of the colour editor's shoulders: a twentieth of the brightness ramp.
pub const PICKER_LUM_STEP: f32 = 0.05;

/// One place the pad can stand: the widget's flattened index (the one `hovered` answers), the box
/// it fills and the design point a click on it is aimed at.
#[derive(Debug, Clone, PartialEq)]
pub struct PadTarget {
    pub index: usize,
    pub kind: PadKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// where a click lands (design coordinates)
    pub click: (f32, f32),
    /// the row of a [`PadKind::ComboRow`] / [`PadKind::ListRow`], else 0
    pub row: usize,
}

impl PadTarget {
    pub fn centre(&self) -> (f32, f32) {
        (self.x + self.w * 0.5, self.y + self.h * 0.5)
    }
}

/// A strip of tab buttons: their flattened indices left to right, which one is the current tab
/// (when the screen says), and how many panels deep the strip lies (0 = the screen's own).
#[derive(Debug, Clone, PartialEq)]
pub struct TabStrip {
    pub tabs: Vec<usize>,
    pub current: Option<usize>,
    pub depth: usize,
}

/// A direction of the d-pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadDir {
    Up,
    Down,
    Left,
    Right,
}

/// The target to move to from `from` in direction `dir`: of those whose centre lies past `from`'s in
/// that direction, the one with the least `distance along + 2 × distance across` (so a control in
/// line wins over a nearer one off to the side); with none that way, the pad **wraps** to the far
/// side — the target farthest the opposite way, in line first. `None` only when there is no other
/// target.
pub fn pad_pick(targets: &[PadTarget], from: usize, dir: PadDir) -> Option<usize> {
    let cur = targets.get(from)?;
    let (cx, cy) = cur.centre();
    // (along, across) of a target's centre in the direction's frame; along > 0 is "that way"
    let frame = |t: &PadTarget| {
        let (x, y) = t.centre();
        let (dx, dy) = (x - cx, y - cy);
        match dir {
            PadDir::Right => (dx, dy.abs()),
            PadDir::Left => (-dx, dy.abs()),
            PadDir::Down => (dy, dx.abs()),
            PadDir::Up => (-dy, dx.abs()),
        }
    };
    let others = || targets.iter().enumerate().filter(|(i, _)| *i != from);
    // a hair of slack so two stacked rows of one column (equal centres on the cross axis) count as "that way"
    if let Some((i, _)) = others()
        .filter_map(|(i, t)| {
            let (along, across) = frame(t);
            (along > 1.0).then_some((i, along + 2.0 * across))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
    {
        return Some(i);
    }
    // nothing that way: the far side, the one most behind, closest in line
    others()
        .map(|(i, t)| {
            let (along, across) = frame(t);
            (i, 2.0 * across + along)
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The place the pad first stands on a screen it has not stood on yet: the first target in reading order (top row first,
/// left first within a row; equal centres keep their list order).
pub fn pad_start(targets: &[PadTarget]) -> Option<usize> {
    let mut order: Vec<usize> = (0..targets.len()).collect();
    order.sort_by(|&a, &b| {
        let (ca, cb) = (targets[a].centre(), targets[b].centre());
        ca.1.total_cmp(&cb.1).then(ca.0.total_cmp(&cb.0))
    });
    order.first().copied()
}

/// The pick from `from` among the targets `rest` (the ones outside its own list / strip): `from` leads the set so
/// [`pad_pick`] measures from it.
pub fn pad_pick_outside(from: &PadTarget, rest: &[PadTarget], dir: PadDir) -> Option<PadTarget> {
    let mut v = vec![from.clone()];
    v.extend(rest.iter().cloned());
    pad_pick(&v, 0, dir).map(|i| v[i].clone())
}

/// What a direction key does from the target `from` (an index into `targets`), by [`pad_step`].
#[derive(Debug, Clone, PartialEq)]
pub enum PadStep {
    /// stand on this target
    To(PadTarget),
    /// nowhere to go
    Stay,
    /// the end of a list's / drop-down's visible rows: the caller scrolls by `step` rows (a drop-down from row `first`),
    /// asks [`pad_targets`](LayoutScreen::pad_targets) again, and settles with [`pad_after_scroll`]; `before` = the first
    /// and last visible row now
    ScrollRows { step: i32, first: i32, before: (Option<usize>, Option<usize>) },
}

/// The step of a direction key from `targets[from]`: a tab walks its strip along the strip's own line — the graphic
/// tabs side by side with Left / Right, the staircase of text tabs with Up / Down — and any other key leaves the strip,
/// past its (overlapping) boxes; inside a list or an open drop-down Up / Down walk the rows and run out into a scroll;
/// everything else is [`pad_pick`]. `strips` = the screen's [`TabStrip`]s (only read for a tab).
pub fn pad_step(targets: &[PadTarget], strips: &[TabStrip], from: usize, dir: PadDir) -> PadStep {
    let cur = &targets[from];
    let vertical = matches!(dir, PadDir::Up | PadDir::Down);
    let strip: Option<&Vec<usize>> = (cur.kind == PadKind::Tab).then(|| strips.iter().find(|s| s.tabs.contains(&cur.index)).map(|s| &s.tabs)).flatten();
    if let Some(tabs) = strip {
        let ys: Vec<f32> = targets.iter().filter(|t| tabs.contains(&t.index)).map(|t| t.y).collect();
        let staircase = ys.iter().cloned().fold(f32::MIN, f32::max) - ys.iter().cloned().fold(f32::MAX, f32::min) > 20.0;
        let along = if staircase { vertical } else { !vertical };
        let at = tabs.iter().position(|t| *t == cur.index).unwrap_or(0);
        let step: i32 = if matches!(dir, PadDir::Down | PadDir::Right) { 1 } else { -1 };
        let to = at as i32 + step;
        if along
            && to >= 0
            && (to as usize) < tabs.len()
            && let Some(t) = targets.iter().find(|t| t.index == tabs[to as usize])
        {
            return PadStep::To(t.clone());
        }
        let rest: Vec<PadTarget> = targets.iter().filter(|t| !tabs.contains(&t.index)).cloned().collect();
        return pad_pick_outside(cur, &rest, dir).map_or(PadStep::Stay, PadStep::To);
    }
    if matches!(cur.kind, PadKind::ComboRow | PadKind::ListRow) && vertical {
        // the rows of this list, top to bottom
        let mut rows: Vec<&PadTarget> = targets.iter().filter(|t| t.index == cur.index && t.kind == cur.kind).collect();
        rows.sort_by_key(|t| t.row);
        let at = rows.iter().position(|t| t.row == cur.row).unwrap_or(0);
        let step: i32 = if dir == PadDir::Down { 1 } else { -1 };
        let to = at as i32 + step;
        if to >= 0 && (to as usize) < rows.len() {
            PadStep::To(rows[to as usize].clone())
        } else {
            let first = rows.first().map_or(0, |t| t.row) as i32;
            PadStep::ScrollRows { step, first, before: (rows.first().map(|t| t.row), rows.last().map(|t| t.row)) }
        }
    } else if cur.kind == PadKind::ComboRow {
        // an open drop-down answers to Up and Down alone
        PadStep::Stay
    } else if cur.kind == PadKind::ListRow {
        let rest: Vec<PadTarget> = targets.iter().filter(|t| t.index != cur.index).cloned().collect();
        pad_pick_outside(cur, &rest, dir).map_or(PadStep::Stay, PadStep::To)
    } else {
        // a list's rows count once for the pick: the row level with the pad's centre is what matters
        pad_pick(targets, from, dir).map_or(PadStep::Stay, |i| PadStep::To(targets[i].clone()))
    }
}

/// Did the scroll of a [`PadStep::ScrollRows`] bring a new row in? `after` = the list's rows now (empty when it scrolled
/// past the last one: the caller then scrolls back).
pub fn pad_rows_moved(before: (Option<usize>, Option<usize>), after: &[PadTarget]) -> bool {
    !after.is_empty() && (after.first().map(|t| t.row), after.last().map(|t| t.row)) != before
}

/// The step after a [`PadStep::ScrollRows`]: the row that came in (`moved`); a list's end (not a drop-down's) goes on to
/// the next control that way, outside the list; a drop-down's end stays.
pub fn pad_after_scroll(targets: &[PadTarget], from: &PadTarget, dir: PadDir, moved: bool, after: &[PadTarget]) -> Option<PadTarget> {
    if moved {
        if dir == PadDir::Down { after.last().cloned() } else { after.first().cloned() }
    } else if from.kind == PadKind::ListRow {
        let rest: Vec<PadTarget> = targets.iter().filter(|t| t.index != from.index).cloned().collect();
        pad_pick_outside(from, &rest, dir)
    } else {
        None
    }
}

impl LayoutScreen {
    /// Every place the pad can stand on this screen, panels included, in the flattened index space
    /// `hovered` uses. An open drop-down takes the pad whole: its visible rows are all there is.
    pub fn pad_targets(&self) -> Vec<PadTarget> {
        let mut out = Vec::new();
        self.pad_collect(&mut out, 0.0, 0.0, 0);
        if out.iter().any(|t| t.kind == PadKind::ComboRow) {
            out.retain(|t| t.kind == PadKind::ComboRow);
        }
        out
    }

    fn pad_collect(&self, out: &mut Vec<PadTarget>, ox: f32, oy: f32, add: usize) {
        use WidgetClass as C;
        for (i, e) in self.layout.elements.iter().enumerate().skip(1) {
            let kind = match self.classes[i] {
                C::GotoButton | C::CarSettingButton => PadKind::Button,
                C::TabButton | C::GraphicTabButton => PadKind::Tab,
                C::ComboBoxSlider | C::ColorBox => PadKind::Combo,
                C::ListBox => PadKind::ListRow,
                C::CarSettingSlider => PadKind::CarSlider,
                C::ChronicleSlider => PadKind::ChronicleSlider,
                C::GouraudIndicatorBar if self.attr_num(e, "changeable").is_some_and(|v| v != 0.0) && e.get("accessor").or_else(|| e.get("othersource")).is_some_and(|a| !a.is_empty()) => PadKind::Bar,
                C::DeadZone if self.any_analogue() => PadKind::DeadZone,
                C::CheckBox | C::ControllerSetup | C::CameraSelect | C::CameraPicture | C::ChroniclePictureIcon | C::KnockoutTrackList => PadKind::Other,
                _ => continue,
            };
            // the hit scan's own refusals (`hovered_scan_owner`): no box, hidden, disabled
            if self.attr_num(e, "nosize").is_some_and(|v| v != 0.0) || self.state_is_hidden(e) || self.hidden_flags.contains(&i) || self.widget_disabled(i) {
                continue;
            }
            let (x, y) = self.position_of(e);
            let (w, h) = self.box_hit(e);
            if w < 1.0 || h < 1.0 {
                continue;
            }
            let (x, y) = (x + ox, y + oy);
            let t = |kind, x, y, w, h, click, row| PadTarget { index: add + i, kind, x, y, w, h, click, row };
            match kind {
                PadKind::Combo => {
                    if self.open_combo == Some(i) {
                        if let Some((px, py, rw, rh)) = self.drop_down_rect(e, i, x - ox, y - oy)
                            && let Some((rows, _)) = self.lists.get(&i)
                        {
                            let first = self.scroll.get(&i).copied().unwrap_or(0);
                            let mut acc = py;
                            for (n, r) in rows.iter().enumerate().skip(first) {
                                let rh_ = r.height();
                                if acc + rh_ > py + rh + 0.5 && acc > py {
                                    break;
                                }
                                let c = (px + ox + rw * 0.5, acc + oy + rh_ * 0.5);
                                out.push(t(PadKind::ComboRow, px + ox, acc + oy, rw, rh_, c, n));
                                acc += rh_;
                            }
                        }
                    } else {
                        // a click in the caption strip steps the accessor on (0x4C6B7E); the value part opens the list
                        let strip = self.combo_strip(e).min(w - 2.0).max(0.0);
                        let c = (x + strip + (w - strip) * 0.5, y + h * 0.5);
                        out.push(t(PadKind::Combo, x, y, w, h, c, 0));
                    }
                }
                PadKind::ListRow => {
                    let Some((rows, _)) = self.lists.get(&i) else { continue };
                    let max_h = self.attr_num(e, "selectionboxmaxheight").unwrap_or(0.0) as f32;
                    let full: f32 = rows.iter().map(|r| r.height()).sum();
                    let width = self.attr_num(e, "width").unwrap_or(0.0) as f32;
                    let panel_w = if full >= max_h { width - LIST_SCROLLBAR_W } else { width };
                    let (wx, wy) = self.position_of(e);
                    // the rows' box, as `list_row_at` puts it
                    let (px, py) = match self.controls.get(&e.class).and_then(|c| c.part("selectionbox")).and_then(|p| p.value("pos")) {
                        Some(v) => self.ev_pair(e, v, 0).map(|(a, b)| (wx + a as f32, wy + b as f32)).unwrap_or((wx, wy)),
                        None => (wx, wy),
                    };
                    let first = self.scroll.get(&i).copied().unwrap_or(0);
                    let mut acc = py;
                    for (n, r) in rows.iter().enumerate().skip(first) {
                        let rh_ = r.height();
                        if rh_ <= 0.0 || (acc + rh_ > py + max_h + 0.5 && acc > py) {
                            break;
                        }
                        let c = (px + ox + panel_w * 0.5, acc + oy + rh_ * 0.5);
                        out.push(t(PadKind::ListRow, px + ox, acc + oy, panel_w, rh_, c, n));
                        acc += rh_;
                    }
                }
                // the dead-zone bar: the pointer goes on the active handle, so the bar paints it hot as for the mouse
                PadKind::DeadZone => {
                    let c = self.deadzone_pad_point(i).map_or((x + w * 0.5, y + h * 0.5), |(px, py)| (px + ox, py + oy));
                    out.push(t(kind, x, y, w, h, c, 0))
                }
                _ => out.push(t(kind, x, y, w, h, (x + w * 0.5, y + h * 0.5), 0)),
            }
        }
        // the panels and strips this screen holds, at their place
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if !self.closed.contains(&k)
                && let Some(pe) = self.layout.elements.get(k)
            {
                let (gx, gy) = self.position_of(pe);
                g.pad_collect(out, ox + gx, oy + gy, add + base);
            }
            base += g.span();
        }
    }

    /// The accessor name of the widget `index` (flattened) — a bar's `accessor` / `othersource`, a slider's `accessor`.
    pub fn widget_accessor(&self, index: usize) -> Option<String> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.widget_accessor(j);
        }
        let e = self.layout.elements.get(index)?;
        e.get("accessor").or_else(|| e.get("othersource")).filter(|a| !a.is_empty()).map(str::to_string)
    }

    /// The row a list or drop-down has selected (the one its accessor holds), by its flattened index.
    pub fn list_current(&self, index: usize) -> Option<usize> {
        if let Some((k, j)) = self.route(index) {
            return self.groups.get(&k)?.list_current(j);
        }
        self.lists.get(&index).and_then(|(_, cur)| *cur)
    }

    /// The tab strips of this screen — each the flattened indices of the tab buttons that share one
    /// panel, left to right — and, for each, which one is the current tab.
    pub fn tab_strips(&self) -> Vec<TabStrip> {
        let mut out = Vec::new();
        self.tab_strips_into(&mut out, 0, 0);
        out
    }

    fn tab_strips_into(&self, out: &mut Vec<TabStrip>, add: usize, depth: usize) {
        let is_tab = |c: WidgetClass| matches!(c, WidgetClass::TabButton | WidgetClass::GraphicTabButton);
        let mut tabs: Vec<(f32, f32, usize)> = self
            .layout
            .elements
            .iter()
            .enumerate()
            .filter(|(i, _)| is_tab(self.classes[*i]) && !self.widget_disabled(*i) && !self.hidden_flags.contains(i))
            .map(|(i, e)| {
                let (x, y) = self.position_of(e);
                (x, y, i)
            })
            .collect();
        if tabs.len() > 1 {
            // a strip reads along its longer side: left to right, or — a staircase of text tabs — top to bottom
            let span = |f: fn(&(f32, f32, usize)) -> f32| {
                tabs.iter().map(f).fold(f32::MIN, f32::max) - tabs.iter().map(f).fold(f32::MAX, f32::min)
            };
            if span(|t| t.1) > 20.0 {
                tabs.sort_by(|a, b| a.1.total_cmp(&b.1));
            } else {
                tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
            }
            let cur = tabs.iter().position(|(_, _, n)| {
                self.selected_flags.contains(n) || (self.layout.elements[*n].get("state") == Some("selected") && !self.unselected.contains(n))
            });
            out.push(TabStrip { tabs: tabs.iter().map(|(_, _, n)| add + n).collect(), current: cur, depth });
        }
        let mut keys: Vec<usize> = self.groups.keys().copied().collect();
        keys.sort_unstable();
        let mut base = self.layout.elements.len();
        for k in keys {
            let g = &self.groups[&k];
            if !self.closed.contains(&k) {
                g.tab_strips_into(out, add + base, depth + 1);
            }
            base += g.span();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(index: usize, x: f32, y: f32) -> PadTarget {
        PadTarget { index, kind: PadKind::Button, x, y, w: 100.0, h: 20.0, click: (x + 50.0, y + 10.0), row: 0 }
    }

    /// two columns of two, the right one lower: Down stays in the column, Right goes across
    #[test]
    fn the_pick_keeps_to_the_column_and_the_row() {
        let v = vec![t(0, 0.0, 0.0), t(1, 0.0, 40.0), t(2, 200.0, 10.0), t(3, 200.0, 50.0)];
        assert_eq!(pad_pick(&v, 0, PadDir::Down), Some(1));
        assert_eq!(pad_pick(&v, 0, PadDir::Right), Some(2));
        assert_eq!(pad_pick(&v, 2, PadDir::Down), Some(3));
        assert_eq!(pad_pick(&v, 3, PadDir::Left), Some(1));
        assert_eq!(pad_pick(&v, 3, PadDir::Up), Some(2));
    }

    /// a target nearer but off to the side loses against one in line
    #[test]
    fn a_target_in_line_beats_a_nearer_one_aside() {
        let v = vec![t(0, 100.0, 0.0), t(1, 0.0, 30.0), t(2, 100.0, 100.0)];
        assert_eq!(pad_pick(&v, 0, PadDir::Down), Some(2));
    }

    /// PAD-SLIDERS: a bar of 128 values moves by 6, a short one by 1; the ends hold; the fraction is the cell's middle
    #[test]
    fn a_bar_steps_by_a_twentieth_and_the_ends_hold() {
        assert_eq!(bar_step(128), 6);
        assert_eq!(bar_step(10), 1);
        assert_eq!(bar_step(1), 1);
        assert_eq!(bar_step_index(128, 0, -1), 0);
        assert_eq!(bar_step_index(128, 125, 1), 127);
        assert_eq!(bar_step_index(128, 60, 1), 66);
        assert_eq!(bar_step_index(10, 9, 1), 9);
        for n in [2, 10, 101, 128] {
            for i in 0..n {
                // `set_accessor_fraction`'s own truncation gives the index back
                assert_eq!((n as f32 * bar_fraction(n, i)) as i32, i);
            }
        }
    }

    #[test]
    fn only_the_drag_set_widgets_adjust() {
        assert!([PadKind::CarSlider, PadKind::Bar, PadKind::DeadZone, PadKind::ChronicleSlider].iter().all(|k| k.adjustable()));
        assert!([PadKind::Button, PadKind::Combo, PadKind::ListRow, PadKind::Other, PadKind::Tab].iter().all(|k| !k.adjustable()));
    }

    #[test]
    fn the_pick_wraps_to_the_far_side() {
        let v = vec![t(0, 0.0, 0.0), t(1, 0.0, 40.0), t(2, 0.0, 80.0)];
        assert_eq!(pad_pick(&v, 2, PadDir::Down), Some(0));
        assert_eq!(pad_pick(&v, 0, PadDir::Up), Some(2));
        assert_eq!(pad_pick(&v[..1], 0, PadDir::Up), None);
    }

    fn kt(index: usize, kind: PadKind, x: f32, y: f32, row: usize) -> PadTarget {
        PadTarget { index, kind, x, y, w: 100.0, h: 20.0, click: (x + 50.0, y + 10.0), row }
    }

    /// the pad's first place is the top-left one in reading order, whatever the list order
    #[test]
    fn the_start_is_the_first_in_reading_order() {
        let v = vec![t(0, 200.0, 40.0), t(1, 100.0, 0.0), t(2, 0.0, 0.0), t(3, 0.0, 40.0)];
        assert_eq!(pad_start(&v), Some(2));
        assert_eq!(pad_start(&[]), None);
    }

    /// side-by-side tabs walk with Left / Right and leave with Up / Down; a staircase the other way round
    #[test]
    fn a_tab_walks_its_strip_along_its_line() {
        let row = vec![kt(0, PadKind::Tab, 0.0, 0.0, 0), kt(1, PadKind::Tab, 100.0, 0.0, 0), kt(2, PadKind::Tab, 200.0, 0.0, 0), t(3, 100.0, 60.0)];
        let strips = vec![TabStrip { tabs: vec![0, 1, 2], current: Some(0), depth: 0 }];
        assert_eq!(pad_step(&row, &strips, 1, PadDir::Right), PadStep::To(row[2].clone()));
        assert_eq!(pad_step(&row, &strips, 1, PadDir::Left), PadStep::To(row[0].clone()));
        // the strip's end holds along the line: it leaves to the pick outside, which wraps back to the one other target
        assert_eq!(pad_step(&row, &strips, 2, PadDir::Right), PadStep::To(row[3].clone()));
        assert_eq!(pad_step(&row, &strips, 1, PadDir::Down), PadStep::To(row[3].clone()));
        let stairs = vec![kt(0, PadKind::Tab, 0.0, 0.0, 0), kt(1, PadKind::Tab, 0.0, 30.0, 0), kt(2, PadKind::Tab, 0.0, 60.0, 0)];
        let strips = vec![TabStrip { tabs: vec![0, 1, 2], current: None, depth: 0 }];
        assert_eq!(pad_step(&stairs, &strips, 0, PadDir::Down), PadStep::To(stairs[1].clone()));
        assert_eq!(pad_step(&stairs, &strips, 2, PadDir::Up), PadStep::To(stairs[1].clone()));
        // Left / Right leave the staircase: nothing else there
        assert_eq!(pad_step(&stairs, &strips, 1, PadDir::Right), PadStep::Stay);
    }

    /// rows of a list: Down walks, the end asks for a scroll; a drop-down row ignores Left / Right
    #[test]
    fn list_rows_walk_then_scroll() {
        let v = vec![kt(5, PadKind::ListRow, 0.0, 0.0, 3), kt(5, PadKind::ListRow, 0.0, 20.0, 4), t(9, 0.0, 100.0)];
        assert_eq!(pad_step(&v, &[], 0, PadDir::Down), PadStep::To(v[1].clone()));
        assert_eq!(pad_step(&v, &[], 1, PadDir::Down), PadStep::ScrollRows { step: 1, first: 3, before: (Some(3), Some(4)) });
        assert_eq!(pad_step(&v, &[], 0, PadDir::Up), PadStep::ScrollRows { step: -1, first: 3, before: (Some(3), Some(4)) });
        // a list row leaves sideways to the controls outside the list
        assert_eq!(pad_step(&v, &[], 0, PadDir::Left), PadStep::To(v[2].clone()));
        let d = vec![kt(5, PadKind::ComboRow, 0.0, 0.0, 0), kt(5, PadKind::ComboRow, 0.0, 20.0, 1)];
        assert_eq!(pad_step(&d, &[], 0, PadDir::Right), PadStep::Stay);
    }

    /// after a scroll: a new row comes in, else the list's end goes on outside the list, a drop-down's end stays
    #[test]
    fn the_scroll_settles_on_the_row_that_came_in() {
        let v = vec![kt(5, PadKind::ListRow, 0.0, 0.0, 3), kt(5, PadKind::ListRow, 0.0, 20.0, 4), t(9, 0.0, 100.0)];
        let after = vec![kt(5, PadKind::ListRow, 0.0, 0.0, 4), kt(5, PadKind::ListRow, 0.0, 20.0, 5)];
        let before = (Some(3), Some(4));
        assert!(pad_rows_moved(before, &after));
        assert!(!pad_rows_moved(before, &v[..2]));
        assert!(!pad_rows_moved(before, &[]));
        assert_eq!(pad_after_scroll(&v, &v[1], PadDir::Down, true, &after), Some(after[1].clone()));
        assert_eq!(pad_after_scroll(&v, &v[0], PadDir::Up, true, &after), Some(after[0].clone()));
        assert_eq!(pad_after_scroll(&v, &v[1], PadDir::Down, false, &v[..2]), Some(v[2].clone()));
        let d = vec![kt(5, PadKind::ComboRow, 0.0, 0.0, 0)];
        assert_eq!(pad_after_scroll(&d, &d[0], PadDir::Down, false, &d), None);
    }
}
