//! TEST GLUE `NFS_GPU_PROFILE=1`: a per-pass GPU timer (diagnostic, off by default; not the original's).
//!
//! `NFS_GPU_PROFILE=minimal` is the other value of the variable: the plain device (no optional feature, so no timestamp
//! queries), which the start-up crash guard asks for in safe mode. It never turns the profiler on.
//!
//! Every render pass of the renderer asks [`rp`] for its `timestamp_writes`; with the env var
//! unset (or an adapter without `TIMESTAMP_QUERY`) it returns `None` and the pass is exactly what it was. With it
//! set, each pass writes its begin / end tick into a query set; the encoder that holds the passes resolves them
//! ([`resolve`]) and [`end_frame`] reads them back (waiting for the device) and logs, every 120 frames, one line per
//! pass label: the mean ms per frame and the mean number of runs per frame, plus the frame's GPU total. The wait
//! makes the frame slower than it is - read the ms of a pass, not the fps. Nothing here touches what is drawn.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

const MAX_PASSES: u32 = 512;

struct Prof {
    qs: &'static wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    period_ns: f64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    labels: Vec<String>,
    ctx: &'static str,
    resolved: u32,
    frames: u32,
    acc: BTreeMap<String, (f64, u32)>,
}

static PROF: OnceLock<Prof> = OnceLock::new();

/// `NFS_GPU_PROFILE=minimal`: a device with no optional feature.
pub(crate) fn minimal() -> bool {
    std::env::var("NFS_GPU_PROFILE").is_ok_and(|v| v.eq_ignore_ascii_case("minimal"))
}

/// Whether the feature is to be requested at device creation: only with the env var set to something but `minimal` / `0`.
pub(crate) fn wanted() -> bool {
    std::env::var("NFS_GPU_PROFILE").is_ok_and(|v| !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("minimal"))
}

pub(crate) fn init(device: &wgpu::Device, queue: &wgpu::Queue) {
    if !wanted() || !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
        return;
    }
    let qs = Box::leak(Box::new(device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("prof"), ty: wgpu::QueryType::Timestamp, count: MAX_PASSES * 2 })));
    let bytes = MAX_PASSES as u64 * 2 * 8;
    let resolve = device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof resolve"), size: bytes, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof readback"), size: bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
    let _ = PROF.set(Prof { qs, resolve, readback, period_ns: queue.get_timestamp_period() as f64, state: Mutex::new(State::default()) });
    tracing::info!("NFS_GPU_PROFILE: per-pass timestamps on");
}

fn slot(label: &'static str) -> Option<(&'static wgpu::QuerySet, u32, u32)> {
    let p = PROF.get()?;
    let mut s = p.state.lock().ok()?;
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

/// Resolve the passes recorded since the last call; call before `encoder.finish()` of every encoder with timed passes.
pub(crate) fn resolve(encoder: &mut wgpu::CommandEncoder) {
    let Some(p) = PROF.get() else { return };
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
    let Some(p) = PROF.get() else { return };
    let Ok(mut s) = p.state.lock() else { return };
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
