//! The garage's car set-up screen (`carsetup.lay`): dragging a `NFS5.CarSettingSlider`'s knob.
//! The arrows and the pad go through `set_accessor` (`fe_screens.rs`); the values live on the
//! garage car (`OwnedCar::setup`, `career.rs`).

use crate::*;

impl App {
    /// The slider under the pointer, as `(element index, setting, geometry)`.
    fn carsetup_slider_at(&self, cx: f32, cy: f32) -> Option<(usize, usize, nfs_game::layout::ChronicleSliderGeom)> {
        let l = self.fe.layout_preview.as_ref()?;
        l.car_setting_sliders().into_iter().find_map(|(index, accessor)| {
            let base = accessor.split_once('(').map_or(accessor.as_str(), |(b, _)| b.trim());
            let k = nfs_game::career::CAR_SETUP_NAMES.iter().position(|n| *n == base)?;
            let g = l.car_setting_slider_geom(index)?;
            (g.on_track(cx, cy) || g.on_knob(cx, cy, 0, 1)).then_some((index, k, g))
        })
    }

    /// The setting's index and list length on the garage car.
    fn carsetup_index(&self, k: usize) -> Option<(i32, i32)> {
        let (_, o) = self.garage_car()?;
        Some(nfs_game::career::car_setup_index(k, o.setup[k]))
    }

    /// The commit **0x4F14B0** on the garage car ([`nfs_game::career::OwnedCar::commit_setup`]): the
    /// screen's destructor 0x4F1A00, `SAVESETTINGS` 0x4F1A30 and `TESTDRIVECHECK` 0x4F1A90 run it.
    pub(crate) fn carsetup_commit(&mut self) {
        if let Some(o) = self.garage_car_mut() {
            o.commit_setup();
        }
    }

    /// The press on a `NFS5.CarSettingSlider` — the same helper as `NFS5.CHRONICLESLIDER`'s
    /// (0x4D0900, [`App::chronicle_slider_press`]): a press on the knob anchors at the pointer's x
    /// with the index at that moment; a press on the track puts the knob under the pointer and
    /// anchors at its old left edge, then the move it posts (0x4D0A82) sets the index.
    pub(crate) fn carsetup_slider_press(&mut self) {
        self.fe.carsetup_slider_drag = None;
        let Some((cx, cy)) = self.fe.cursor else { return };
        let Some((index, k, g)) = self.carsetup_slider_at(cx, cy) else { return };
        let Some((idx, count)) = self.carsetup_index(k) else { return };
        if g.on_knob(cx, cy, idx, count) {
            self.fe.carsetup_slider_drag = Some((index, k, cx as i32, idx));
        } else if cy >= g.y && cy < g.y + g.knob_h {
            self.fe.carsetup_slider_drag = Some((index, k, g.knob_left(idx, count) as i32, idx));
            self.carsetup_slider_move();
        }
    }

    /// Slot 8, 0x4D0F20: while the pointer is inside the track the setting takes
    /// `(x − anchor) · (count − 1) / (track − knob) + start`, which 0x4BC240 clamps to the list.
    pub(crate) fn carsetup_slider_move(&mut self) {
        let Some((index, k, anchor, start)) = self.fe.carsetup_slider_drag else { return };
        let Some((cx, cy)) = self.fe.cursor else { return };
        let Some(g) = self.fe.layout_preview.as_ref().and_then(|l| l.car_setting_slider_geom(index)) else { return };
        if !g.on_track(cx, cy) {
            return;
        }
        let Some((cur, count)) = self.carsetup_index(k) else { return };
        let want = g.dragged_index(cx, anchor, start, count);
        if want != cur
            && let Some(o) = self.garage_car_mut()
        {
            o.set_setup_index(k, want);
            self.bind_after_accessor();
        }
    }
}
