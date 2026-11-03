//! TEST GLUE `NFS_GPU_PROFILE=1`: a per-pass GPU timer (diagnostic, off by default; not the original's).
//!
//! `NFS_GPU_PROFILE=minimal` is the other value of the variable: the plain device (no optional feature, so no timestamp
//! queries), which the start-up crash guard asks for in safe mode. It never turns the profiler on.
//!
//! Every render / compute pass of the renderer asks [`rp`] / [`cp`] for its `timestamp_writes`; with the env var
//! unset (or an adapter without `TIMESTAMP_QUERY`) both return `None` and the pass is exactly what it was. With it
//! set, each pass writes its begin / end tick into a query set; the encoder that holds the passes resolves them
//! ([`resolve`]) and [`end_frame`] reads them back (waiting for the device) and logs, every 120 frames, one line per
//! pass label: the mean ms per frame and the mean number of runs per frame, plus the frame's GPU total. The wait
//! makes the frame slower than it is - read the ms of a pass, not the fps. Nothing here touches what is drawn.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

const MAX_PASSES: u32 = 512;

struct Prof {
    qs: &'static wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    /// `NFS_GPU_PROFILE` is set: the blocking per-pass log
    log: bool,
    /// the live read-back's `map_async` has completed (set by its callback)
    mapped: std::sync::Arc<AtomicBool>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    labels: Vec<String>,
    ctx: &'static str,
    resolved: u32,
    frames: u32,
    acc: BTreeMap<String, (f64, u32)>,
    /// live mode: a read-back of `pending.len()` passes is mapped (or being mapped); no pass is timed meanwhile
    pending: Vec<String>,
}

static PROF: OnceLock<Prof> = OnceLock::new();

/// PORT-DEVIATION D63 (the race panel): the panel is open. While it is, the passes are timed and the draws counted, without
/// waiting for the GPU; closed, none of it runs (a pass asks for its `timestamp_writes` and gets `None`, a draw costs one load).
static LIVE: AtomicBool = AtomicBool::new(false);
static DRAWS: AtomicU32 = AtomicU32::new(0);
static TRIS: AtomicU64 = AtomicU64::new(0);

/// What the panel shows of the renderer: the last frame's draw calls and triangles submitted, and the last GPU timing read back.
#[derive(Debug, Clone, Default)]
pub struct LiveStats {
    pub draws: u32,
    pub tris: u64,
    /// the sum of the frame's timed passes, milliseconds; `None` = the adapter has no timestamp queries (or no read-back has come yet)
    pub gpu_total_ms: Option<f64>,
    /// the passes by label (runs of one label added up), milliseconds
    pub gpu_passes: Vec<(String, f64)>,
}

static LAST: Mutex<LiveStats> = Mutex::new(LiveStats { draws: 0, tris: 0, gpu_total_ms: None, gpu_passes: Vec::new() });

/// Turn the panel's live statistics on or off.
pub fn live_set(on: bool) {
    if LIVE.swap(on, Relaxed) != on {
        DRAWS.store(0, Relaxed);
        TRIS.store(0, Relaxed);
        if let Some(p) = PROF.get() && let Ok(mut s) = p.state.lock() && s.pending.is_empty() {
            s.labels.clear();
            s.resolved = 0;
        }
        if !on && let Ok(mut l) = LAST.lock() {
            *l = LiveStats::default();
        }
    }
}

/// Whether the GPU can time passes (the device was created with timestamp queries).
pub fn timestamps_supported() -> bool {
    PROF.get().is_some()
}

/// The last frame's numbers (see [`LiveStats`]).
pub fn live_stats() -> LiveStats {
    LAST.lock().map(|l| l.clone()).unwrap_or_default()
}

/// One draw call of `tris` triangles (only counted while the panel is open).
#[inline]
pub(crate) fn count_draw(tris: u64) {
    if LIVE.load(Relaxed) {
        DRAWS.fetch_add(1, Relaxed);
        TRIS.fetch_add(tris, Relaxed);
    }
}

/// `NFS_GPU_PROFILE=minimal`: a device with no optional feature.
pub(crate) fn minimal() -> bool {
    std::env::var("NFS_GPU_PROFILE").is_ok_and(|v| v.eq_ignore_ascii_case("minimal"))
}

/// Whether the feature is to be requested at device creation: only with the env var set to something but `minimal` / `0`.
pub(crate) fn wanted() -> bool {
    std::env::var("NFS_GPU_PROFILE").is_ok_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("minimal"))
}

/// Whether the adapter's timestamp queries are to be requested at device creation: the profiler wants them, or the race panel may
/// (PORT-DEVIATION D63); never in safe mode. Requesting the feature changes nothing a draw does: a pass gets `timestamp_writes` only
/// while the log is on or the panel is open.
pub(crate) fn request_timestamps() -> bool {
    !minimal()
}

pub(crate) fn init(device: &wgpu::Device, queue: &wgpu::Queue) {
    if minimal() || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return;
    }
    let qs = Box::leak(Box::new(device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("prof"), ty: wgpu::QueryType::Timestamp, count: MAX_PASSES * 2 })));
    let bytes = MAX_PASSES as u64 * 2 * 8;
    let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof resolve"), size: bytes, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof readback"), size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let _ = PROF.set(Prof { qs, resolve, readback, period_ns: queue.get_timestamp_period() as f64, log: wanted(), mapped: Default::default(), state: Mutex::new(State::default()) });
    if wanted() {
        tracing::info!("NFS_GPU_PROFILE: per-pass timestamps on");
    }
}

fn slot(label: &'static str) -> Option<(&'static wgpu::QuerySet, u32, u32)> {
    let p = PROF.get()?;
    if !p.log && !LIVE.load(Relaxed) {
        return None;
    }
    let mut s = p.state.lock().ok()?;
    // live mode: the read-back buffer is still on its way, so this frame is not timed
    if !s.pending.is_empty() {
        return None;
    }
    let i = s.labels.len() as u32;
    if i >= MAX_PASSES {
        return None;
    }
    let l = format!("{label}{}", s.ctx);
    s.labels.push(l);
    Some((p.qs, i * 2, i * 2 + 1))
}

/// What the passes recorded from now on are for (a suffix of their label): "" the main view.
pub(crate) fn ctx(c: &'static str) {
    if let Some(p) = PROF.get() && let Ok(mut s) = p.state.lock() {
        s.ctx = c;
    }
}

/// `timestamp_writes` for a render pass.
pub(crate) fn rp(label: &'static str) -> Option<wgpu::RenderPassTimestampWrites<'static>> {
    slot(label).map(|(q, b, e)| wgpu::RenderPassTimestampWrites { query_set: q, beginning_of_pass_write_index: Some(b), end_of_pass_write_index: Some(e) })
}

/// ENHANCEMENT (the enhancements' compute passes): `timestamp_writes` for a compute pass.
pub(crate) fn cp(label: &'static str) -> Option<wgpu::ComputePassTimestampWrites<'static>> {
    slot(label).map(|(q, b, e)| wgpu::ComputePassTimestampWrites { query_set: q, beginning_of_pass_write_index: Some(b), end_of_pass_write_index: Some(e) })
}

/// Resolve the passes recorded since the last call; call before `encoder.finish()` of every encoder with timed passes.
pub(crate) fn resolve(encoder: &mut wgpu::CommandEncoder) {
    let Some(p) = PROF.get() else { return };
    if !p.log && !LIVE.load(Relaxed) {
        return;
    }
    let Ok(mut s) = p.state.lock() else { return };
    let (from, to) = (s.resolved, s.labels.len() as u32);
    if from >= to {
        return;
    }
    encoder.resolve_query_set(p.qs, from * 2..to * 2, &p.resolve, 0);
    encoder.copy_buffer_to_buffer(&p.resolve, 0, &p.readback, from as u64 * 16, (to - from) as u64 * 16);
    s.resolved = to;
}

/// The frame's end: read the resolved timestamps (the device is waited for) and add them up.
pub(crate) fn end_frame(device: &wgpu::Device) {
    if LIVE.load(Relaxed) {
        let mut l = LAST.lock();
        if let Ok(l) = l.as_mut() {
            l.draws = DRAWS.swap(0, Relaxed);
            l.tris = TRIS.swap(0, Relaxed);
        }
    }
    let Some(p) = PROF.get() else { return };
    if !p.log && !LIVE.load(Relaxed) {
        return;
    }
    let Ok(mut s) = p.state.lock() else { return };
    if !p.log {
        end_frame_live(device, p, &mut s);
        return;
    }
    let n = s.resolved;
    if n == 0 {
        return;
    }
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let slice = p.readback.slice(..n as u64 * 16);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let Ok(view) = slice.get_mapped_range() else { return };
    let data: Vec<u64> = bytemuck::cast_slice::<u8, u64>(&view).to_vec();
    drop(view);
    p.readback.unmap();
    let labels = std::mem::take(&mut s.labels);
    let mut total = 0.0;
    for (i, l) in labels.iter().take(n as usize).enumerate() {
        let (b, e) = (data[i * 2], data[i * 2 + 1]);
        let ms = if e > b { (e - b) as f64 * p.period_ns / 1.0e6 } else { 0.0 };
        total += ms;
        let a = s.acc.entry(l.clone()).or_default();
        a.0 += ms;
        a.1 += 1;
    }
    s.acc.entry("TOTAL (sum of passes)".to_string()).or_default().0 += total;
    s.resolved = 0;
    s.frames += 1;
    if s.frames >= 120 {
        let f = s.frames as f64;
        let mut line = String::new();
        for (l, (ms, runs)) in &s.acc {
            line.push_str(&format!("\n  {:<40} {:7.3} ms/frame  x{:.2}", l, ms / f, *runs as f64 / f));
        }
        tracing::info!("GPU profile, mean of {} frames:{}", s.frames, line);
        s.acc.clear();
        s.frames = 0;
    }
}

/// The live read-back, without a wait: collect the one that has come (a poll that does not block), then start the next.
fn end_frame_live(device: &wgpu::Device, p: &Prof, s: &mut State) {
    if !s.pending.is_empty() {
        let _ = device.poll(wgpu::PollType::Poll);
        if !p.mapped.load(Relaxed) {
            return;
        }
        let labels = std::mem::take(&mut s.pending);
        let slice = p.readback.slice(..labels.len() as u64 * 16);
        if let Ok(view) = slice.get_mapped_range() {
            let data: &[u64] = bytemuck::cast_slice(&view);
            let mut passes: BTreeMap<String, f64> = BTreeMap::new();
            let mut total = 0.0;
            for (i, l) in labels.iter().enumerate() {
                let (b, e) = (data[i * 2], data[i * 2 + 1]);
                let ms = if e > b { (e - b) as f64 * p.period_ns / 1.0e6 } else { 0.0 };
                total += ms;
                *passes.entry(l.clone()).or_default() += ms;
            }
            if let Ok(mut l) = LAST.lock() {
                l.gpu_total_ms = Some(total);
                l.gpu_passes = passes.into_iter().collect();
            }
        }
        p.readback.unmap();
        p.mapped.store(false, Relaxed);
        return;
    }
    let n = s.resolved;
    if n == 0 {
        s.labels.clear();
        return;
    }
    p.mapped.store(false, Relaxed);
    let mapped = p.mapped.clone();
    p.readback.slice(..n as u64 * 16).map_async(wgpu::MapMode::Read, move |r| {
        if r.is_ok() {
            mapped.store(true, Relaxed);
        }
    });
    s.pending = std::mem::take(&mut s.labels);
    s.pending.truncate(n as usize);
    s.resolved = 0;
}
