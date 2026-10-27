//! The post chain of the float scene: the motion blur (E27), the depth of field (the photo mode, D46 / E31), the
//! windscreen drops (E40), the bloom (E10) and the eye adaptation's measure (E21), in that order, each taking the scene
//! image the passes before it left ([`PostChain`]) and the frame's [`PostCtx`]. Which of them run this frame is decided
//! first ([`PostPlan`], from the switches and the scene); the tone map after them reads [`PostChain::current`].

use crate::gfx::{DepthSrc, PostCtx};
use super::*;

/// The post chain's passes (each built at its first use) and the lens dirt.
#[derive(Default)]
pub(super) struct PostPasses {
    /// ENHANCEMENT E10: the bloom chain (rebuilt when the level changes)
    pub bloom: Option<crate::bloom::BloomPass>,
    /// ENHANCEMENT E21: the eye adaptation's state (built at first use)
    pub eye: Option<crate::adapt::EyePass>,
    /// ENHANCEMENT E27: the motion blur pass (built at first use)
    pub mblur: Option<crate::camfx::MotionBlur>,
    /// ENHANCEMENTS E31 / the photo mode (D46): the depth-of-field pass, built at the first use
    pub dof: Option<crate::dof::DofPass>,
    /// ENHANCEMENT E40: the windscreen drops' pass and the water on the glass (built on first use)
    pub drops: Option<crate::drops::DropsPass>,
    /// ENHANCEMENT E28: the generated lens dirt (built at first use)
    pub dirt: Option<crate::camfx::Dirt>,
}

/// Which passes of the chain run this frame, and with what.
pub(super) struct PostPlan {
    /// ENHANCEMENT E27: the blur's level and the previous frame
    pub blur: Option<(u8, crate::MotionPrev)>,
    /// ENHANCEMENTS E31 / the photo mode (D46): the lens
    pub dof: Option<crate::Dof>,
    /// ENHANCEMENT E40: the windscreen, its glass on the screen and the frame's look
    pub drops: Option<(crate::drops::Drops, crate::drops::Glass, crate::drops::Look)>,
    /// ENHANCEMENT E10: the bloom's level (0 = none) and E28's dirt on its last add
    pub bloom_level: u8,
    pub dirt: bool,
    /// ENHANCEMENT E21: the eye adaptation measures the finished image
    pub eye: bool,
}

/// The image the chain carries as the scene: the scaled 3-D image's own (`base`, its bind group `base_bg`) until a pass
/// writes one of its own (`out`).
pub(super) struct PostChain {
    base_bg: Option<wgpu::BindGroup>,
    base_view: wgpu::TextureView,
    pub out: Option<(wgpu::BindGroup, wgpu::TextureView)>,
}

impl PostChain {
    pub fn new(base_bg: Option<&wgpu::BindGroup>, base_view: &wgpu::TextureView) -> Self {
        Self { base_bg: base_bg.cloned(), base_view: base_view.clone(), out: None }
    }

    /// The scene's bind group a pass reads (`None`: no image to read, the pass is skipped).
    fn src(&self) -> Option<wgpu::BindGroup> {
        self.out.as_ref().map(|o| o.0.clone()).or_else(|| self.base_bg.clone())
    }

    /// A pass wrote the scene into an image of its own: the passes after it take that one.
    fn set(&mut self, (bg, view): (&wgpu::BindGroup, &wgpu::TextureView)) {
        self.out = Some((bg.clone(), view.clone()));
    }

    /// The scene as it stands: its bind group and view.
    pub fn current(&self) -> (Option<&wgpu::BindGroup>, &wgpu::TextureView) {
        match self.out.as_ref() {
            Some((bg, view)) => (Some(bg), view),
            None => (self.base_bg.as_ref(), &self.base_view),
        }
    }
}

/// ENHANCEMENT E1: the depth the chain reads — the multisampled one of a multisampled frame, else the single-sample `depth`.
pub(super) fn depth_src<'a>(ms: Option<&'a Ms>, depth: &'a wgpu::TextureView) -> DepthSrc<'a> {
    match ms {
        Some(m) => DepthSrc { view: &m.depth, multisampled: true },
        None => DepthSrc { view: depth, multisampled: false },
    }
}

impl PostPasses {
    /// Run the planned passes in order over `chain`; `scene` / `scene_view`: the scaled 3-D image itself (the drops draw
    /// into it in place while no pass before them made an image of its own).
    pub fn run(&mut self, cx: &mut PostCtx, plan: PostPlan, chain: &mut PostChain, scene: &wgpu::Texture, scene_view: &wgpu::TextureView) {
        // ENHANCEMENT E27: the camera-motion blur of the float scene, into an image of its own, which the
        // bloom, the eye adaptation and the tone map then take as the scene
        if let (Some((level, prev)), Some(src)) = (plan.blur, chain.src()) {
            let m = self.mblur.get_or_insert_with(|| crate::camfx::MotionBlur::new(cx.device, cx.layout));
            chain.set(m.run(cx, &src, &prev, level));
        }
        // ENHANCEMENTS E31 / the photo mode (D46): the depth of field of the photo mode / a replay, on whatever the blur left
        if let (Some(lens), Some(src)) = (plan.dof, chain.src()) {
            let d = self.dof.get_or_insert_with(|| crate::dof::DofPass::new(cx.device, cx.layout));
            chain.set(d.run(cx, &src, lens));
        }
        // ENHANCEMENT E40: the raindrops on the windscreen of the driver view, over whatever the passes above left (before
        // the bloom, so a lamp seen through a drop blooms)
        let mut drops_on = false;
        if let Some((dr, gl, look)) = plan.drops {
            let d = self.drops.get_or_insert_with(|| crate::drops::DropsPass::new(cx.device, cx.layout));
            if d.prepare(&dr, &gl).is_some() {
                match chain.out.as_ref().map(|b| b.0.clone()) {
                    // an earlier pass left the scene in an image of its own: a new image the size of the frame
                    Some(src) => chain.set(d.run(cx, &src, &gl, look)),
                    // the scene's own image (the usual case): only the glass's box is copied and drawn over
                    None => d.run_in_place(cx, scene, scene_view, &gl, look),
                }
                drops_on = true;
            }
        }
        if !drops_on && let Some(d) = self.drops.as_mut() {
            // off the windscreen (or a clean glass): the water is forgotten, the next look starts from a fresh pre-roll
            d.sim = None;
        }
        let (img_bg, img_view) = chain.current();
        // ENHANCEMENT E10: the bloom of the float scene, added before the tone map
        if plan.bloom_level > 0 {
            // ENHANCEMENT E28: the last add through the lens dirt
            if plan.dirt && self.dirt.is_none() {
                self.dirt = Some(crate::camfx::Dirt::new(cx.device, cx.queue, cx.layout, cx.sampler));
            }
            let mut bloom = match self.bloom.take() {
                Some(b) if b.level == plan.bloom_level && b.dirt == plan.dirt => b,
                _ => crate::bloom::BloomPass::new(cx.device, cx.layout, plan.bloom_level, plan.dirt),
            };
            if let Some(bg) = img_bg {
                bloom.encode(cx, bg, img_view, self.dirt.as_ref().map(|d| &d.bind_group));
            }
            self.bloom = Some(bloom);
        }
        // ENHANCEMENT E21: measure the finished float image (bloom included), then the tone map reads the exposure
        if plan.eye && let (Some(e), Some(bg)) = (self.eye.as_mut(), img_bg) {
            e.encode(cx, bg);
        }
    }
}
