//! ENHANCEMENT E22 level «auto»: is the machine running on its battery? One question, one answer per platform; render /
//! pacing only (the frame cap), nothing here reaches the simulation, a replay or the net.
//!
//! `Some(true)` on battery, `Some(false)` on mains power, `None` when the platform does not say (a desktop without a
//! battery, an unknown system): the caller then treats it as mains power. macOS is tested; the Linux and Windows
//! arms are compile-checked only.

pub(crate) fn on_battery() -> Option<bool> {
    imp::on_battery()
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{c_char, c_void};

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> *const c_void;
        fn IOPSGetProvidingPowerSourceType(snapshot: *const c_void) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringGetCString(s: *const c_void, buf: *mut c_char, size: isize, encoding: u32) -> u8;
        fn CFRelease(p: *const c_void);
    }
    const UTF8: u32 = 0x0800_0100;

    /// `IOPSGetProvidingPowerSourceType`: "AC Power", "Battery Power" or "UPS Power".
    pub(super) fn on_battery() -> Option<bool> {
        // SAFETY: plain CoreFoundation / IOKit calls; the snapshot is released, the type string is borrowed from it
        // (the "Get" rule) and copied out before that.
        unsafe {
            let snap = IOPSCopyPowerSourcesInfo();
            if snap.is_null() {
                return None;
            }
            let s = IOPSGetProvidingPowerSourceType(snap);
            let mut out = None;
            if !s.is_null() {
                let mut buf = [0 as c_char; 64];
                if CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0 {
                    let name = std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy();
                    out = Some(name.starts_with("Battery"));
                }
            }
            CFRelease(snap);
            out
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    /// `/sys/class/power_supply`: a battery that is discharging = on battery; a mains supply that is online = not.
    pub(super) fn on_battery() -> Option<bool> {
        let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
        let (mut mains_online, mut discharging, mut any) = (false, false, false);
        for e in dir.flatten() {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).map(|s| s.trim().to_string()).unwrap_or_default();
            match read("type").as_str() {
                "Mains" => {
                    any = true;
                    mains_online |= read("online") == "1";
                }
                "Battery" => {
                    any = true;
                    discharging |= read("status") == "Discharging";
                }
                _ => {}
            }
        }
        any.then_some(discharging && !mains_online)
    }
}

#[cfg(windows)]
mod imp {
    #[repr(C)]
    struct SystemPowerStatus {
        ac_line_status: u8,
        battery_flag: u8,
        battery_life_percent: u8,
        system_status_flag: u8,
        battery_life_time: u32,
        battery_full_life_time: u32,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemPowerStatus(status: *mut SystemPowerStatus) -> i32;
    }

    /// `GetSystemPowerStatus`: `ACLineStatus` 0 offline (battery), 1 online, 255 unknown.
    pub(super) fn on_battery() -> Option<bool> {
        let mut s = SystemPowerStatus { ac_line_status: 255, battery_flag: 0, battery_life_percent: 0, system_status_flag: 0, battery_life_time: 0, battery_full_life_time: 0 };
        // SAFETY: the struct matches SYSTEM_POWER_STATUS and outlives the call
        if unsafe { GetSystemPowerStatus(&mut s) } == 0 {
            return None;
        }
        match s.ac_line_status {
            0 => Some(true),
            1 => Some(false),
            _ => None,
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod imp {
    pub(super) fn on_battery() -> Option<bool> {
        None
    }
}
