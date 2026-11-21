//! "SDR content brightness" (Settings > Display > HDR) via the DisplayConfig API.
//!
//! Reading is documented (GET_SDR_WHITE_LEVEL = 11). Writing uses the undocumented
//! SET_SDR_WHITE_LEVEL (0xFFFFFFEE) — the same call the Settings app makes.
//! Level is stored as nits / 80 * 1000. The Settings slider maps 0..100 to 80..480 nits.

use std::mem::size_of;

const QDC_ONLY_ACTIVE_PATHS: u32 = 2;
const GET_ADVANCED_COLOR_INFO: u32 = 9;
const GET_SDR_WHITE_LEVEL: u32 = 11;
const SET_SDR_WHITE_LEVEL: u32 = 0xFFFF_FFEE;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Header {
    kind: u32,
    size: u32,
    adapter_low: u32,
    adapter_high: i32,
    id: u32,
}

#[repr(C)]
struct GetLevel {
    h: Header,
    level: u32,
}

#[repr(C)]
struct SetLevel {
    h: Header,
    level: u32,
    final_value: u8,
}

#[repr(C)]
struct ColorInfo {
    h: Header,
    flags: u32, // bit0 supported, bit1 enabled
    encoding: u32,
    bpc: u32,
}

// Opaque, correctly-sized buffers for QueryDisplayConfig.
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct PathInfo([u8; 72]);
#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct ModeInfo([u8; 64]);

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDisplayConfigBufferSizes(flags: u32, paths: *mut u32, modes: *mut u32) -> i32;
    fn QueryDisplayConfig(
        flags: u32,
        np: *mut u32,
        paths: *mut PathInfo,
        nm: *mut u32,
        modes: *mut ModeInfo,
        topology: *mut u32,
    ) -> i32;
    fn DisplayConfigGetDeviceInfo(h: *mut Header) -> i32;
    fn DisplayConfigSetDeviceInfo(h: *mut Header) -> i32;
}

/// Active display targets that currently have HDR enabled.
fn hdr_targets() -> Vec<Header> {
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != 0 {
            return vec![];
        }
        let mut paths = vec![PathInfo([0; 72]); np as usize];
        let mut modes = vec![ModeInfo([0; 64]); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        ) != 0
        {
            return vec![];
        }
        paths
            .iter()
            .take(np as usize)
            .filter_map(|p| {
                // DISPLAYCONFIG_PATH_TARGET_INFO starts at byte 20: LUID adapterId, UINT32 id.
                let b = &p.0;
                let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                let target = Header {
                    adapter_low: u(20),
                    adapter_high: u(24) as i32,
                    id: u(28),
                    ..Default::default()
                };
                let mut ci = ColorInfo {
                    h: Header {
                        kind: GET_ADVANCED_COLOR_INFO,
                        size: size_of::<ColorInfo>() as u32,
                        ..target
                    },
                    flags: 0,
                    encoding: 0,
                    bpc: 0,
                };
                let ok = DisplayConfigGetDeviceInfo(&mut ci.h) == 0 && ci.flags & 0b10 != 0;
                ok.then_some(target)
            })
            .collect()
    }
}

fn level_to_slider(level: u32) -> u8 {
    let nits = level as f32 * 80.0 / 1000.0;
    ((nits - 80.0) / 4.0).round().clamp(0.0, 100.0) as u8
}

fn slider_to_level(v: u8) -> u32 {
    (80 + 4 * v.min(100) as u32) * 1000 / 80
}

/// Current slider value (0..=100), or None when no display has HDR on.
pub fn get() -> Option<u8> {
    let t = *hdr_targets().first()?;
    let mut g = GetLevel {
        h: Header {
            kind: GET_SDR_WHITE_LEVEL,
            size: size_of::<GetLevel>() as u32,
            ..t
        },
        level: 0,
    };
    (unsafe { DisplayConfigGetDeviceInfo(&mut g.h) } == 0).then(|| level_to_slider(g.level))
}

/// Apply to every HDR display. `commit=false` previews (while dragging), `true` persists.
pub fn set(value: u8, commit: bool) -> Result<(), String> {
    let targets = hdr_targets();
    if targets.is_empty() {
        return Err("HDR is off".into());
    }
    for t in targets {
        let mut s = SetLevel {
            h: Header {
                kind: SET_SDR_WHITE_LEVEL,
                size: size_of::<SetLevel>() as u32,
                ..t
            },
            level: slider_to_level(value),
            final_value: commit as u8,
        };
        let rc = unsafe { DisplayConfigSetDeviceInfo(&mut s.h) };
        if rc != 0 {
            return Err(format!("DisplayConfigSetDeviceInfo failed: {rc}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapping_round_trips() {
        assert_eq!(slider_to_level(30), 2500); // 200 nits
        assert_eq!(level_to_slider(2500), 30);
        assert_eq!(slider_to_level(0), 1000);
        assert_eq!(slider_to_level(100), 6000);
        for v in 0..=100 {
            assert_eq!(level_to_slider(slider_to_level(v)), v);
        }
    }
}
