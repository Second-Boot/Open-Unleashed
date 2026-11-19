//! Test glue: the network race dumps (`NFS_NET_DUMP`, `NFS_REMOTE_DUMP`).

use crate::*;

impl App {
    /// **Test glue** (`NFS_NET_DUMP`): every human car of an input-exchange race after the tick, one line
    /// each in race-index order — `tick,index,` then the position, velocity, orientation, spin, rpm and
    /// steer as `f32` bit patterns, and the gear.
    pub(crate) fn net_dump_tick(out: &mut std::io::BufWriter<std::fs::File>, local: Option<u32>, seat_index: &[Option<u32>], v: &Vehicle, opponents: &[RaceCar], tick: u64) {
        use std::io::Write;
        let mut cars: Vec<(u32, &Vehicle)> = Vec::new();
        if let Some(i) = local {
            cars.push((i, v));
        }
        for (k, o) in opponents.iter().enumerate() {
            if let Some(i) = seat_index.get(k).copied().flatten() {
                cars.push((i, &o.vehicle));
            }
        }
        cars.sort_by_key(|c| c.0);
        for (i, c) in cars {
            let mut f: Vec<f32> = Vec::new();
            f.extend(c.position.to_array());
            f.extend(c.velocity.to_array());
            f.extend(c.orientation.to_cols_array());
            f.extend(c.angular_velocity.to_array());
            f.push(c.rpm);
            f.push(c.steer as f32);
            let bits: Vec<String> = f.iter().map(|x| format!("{:08x}", x.to_bits())).collect();
            let _ = writeln!(out, "{},{},{},{}", tick, i, bits.join(","), c.gear);
        }
    }
}

/// **TEST GLUE** (`NFS_REMOTE_DUMP=<file>`): one CSV line per tick for every remote seat after the dead
/// reckoning — tick, seat, position, speed estimate, the last block's tick, the lag book's `+0x68` /
/// `+0x5C` and the stale flag — what the smooth-motion proof reads (`network.md` 8.4).
pub(crate) fn remote_dump(tick: u64, seat: usize, v: &Vehicle, n: &netplay::NetPlay) {
    use std::io::Write;
    static OUT: std::sync::OnceLock<Option<std::sync::Mutex<std::io::BufWriter<std::fs::File>>>> = std::sync::OnceLock::new();
    let out = OUT.get_or_init(|| {
        let path = nfs_game::config::cfg().nfs_remote_dump.as_os_str()?;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).ok()?);
        let _ = writeln!(f, "tick,seat,x,y,z,speed_est,block_tick,lag,lag_avg,stale");
        Some(std::sync::Mutex::new(f))
    });
    let (Some(out), Some(r)) = (out.as_ref(), n.remotes.values().find(|r| r.seat == seat)) else { return };
    if let Ok(mut f) = out.lock() {
        let _ = writeln!(f, "{tick},{seat},{:.4},{:.4},{:.4},{:.3},{},{},{},{}", v.position.x, v.position.y, v.position.z, v.speed_est, r.state.tick, r.lag, r.lag_avg, r.stale as u8);
        let _ = f.flush();
    }
}
