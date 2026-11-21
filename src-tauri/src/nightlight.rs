//! Night Light strength, stored by Windows as a CloudStore binary blob in HKCU.
//! Windows watches this value and applies changes immediately.
//!
//! Blob layout (verified on build 26300):
//!   43 42 01 00 0A 02 01 00 2A 06 <timestamp varint> 2A 2B 0E <inner len varint> <inner> <trailer>
//!   inner = 43 42 01 00 ... CF 28 <kelvin*2 varint> ... [C2 46 01] 00
//! `C2 46 01` is the preview flag Settings sets while the slider is being dragged.
//! The timestamp must increase on each write or Windows ignores the change.
//! Strength % maps linearly: kelvin = 6500 - 53 * strength (0 => 6500K, 100 => 1200K).

use crate::reg;

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.bluelightreduction.settings\windows.data.bluelightreduction.settings";
const PREFIX: [u8; 10] = [0x43, 0x42, 0x01, 0x00, 0x0A, 0x02, 0x01, 0x00, 0x2A, 0x06];
const INNER_TAG: [u8; 3] = [0x2A, 0x2B, 0x0E];
const TEMP_TAG: [u8; 2] = [0xCF, 0x28];
const PREVIEW: [u8; 3] = [0xC2, 0x46, 0x01];

fn read_varint(b: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let (mut v, mut shift) = (0u64, 0);
    loop {
        let byte = *b.get(i)?;
        v |= ((byte & 0x7F) as u64) << shift;
        i += 1;
        if byte & 0x80 == 0 {
            return Some((v, i));
        }
        shift += 7;
    }
}

fn write_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

struct Blob<'a> {
    timestamp: u64,
    inner: &'a [u8],
    trailer: &'a [u8],
}

fn parse(b: &[u8]) -> Option<Blob<'_>> {
    if !b.starts_with(&PREFIX) {
        return None;
    }
    let (timestamp, i) = read_varint(b, PREFIX.len())?;
    if b.get(i..i + 3)? != INNER_TAG {
        return None;
    }
    let (len, i) = read_varint(b, i + 3)?;
    let end = i.checked_add(len as usize)?;
    Some(Blob {
        timestamp,
        inner: b.get(i..end)?,
        trailer: b.get(end..)?,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn kelvin_to_strength(k: u64) -> u8 {
    ((6500.0 - k as f64) / 53.0).round().clamp(0.0, 100.0) as u8
}

fn strength_to_kelvin(s: u8) -> u64 {
    6500 - 53 * s.min(100) as u64
}

fn decode(b: &[u8]) -> Option<u8> {
    let inner = parse(b)?.inner;
    let t = find(inner, &TEMP_TAG)? + 2;
    Some(kelvin_to_strength(read_varint(inner, t)?.0 / 2))
}

fn encode(b: &[u8], strength: u8, preview: bool, now: u64) -> Option<Vec<u8>> {
    let blob = parse(b)?;
    let t = find(blob.inner, &TEMP_TAG)? + 2;
    let (_, t_end) = read_varint(blob.inner, t)?;

    let mut inner = blob.inner[..t].to_vec();
    write_varint(strength_to_kelvin(strength) * 2, &mut inner);
    let mut rest = &blob.inner[t_end..];
    // Strip any existing preview flag (it sits right before the final 00).
    let had_preview = rest.len() >= 4 && rest[rest.len() - 4..rest.len() - 1] == PREVIEW;
    let tail: Vec<u8>;
    if had_preview {
        tail = [&rest[..rest.len() - 4], &rest[rest.len() - 1..]].concat();
        rest = &tail;
    }
    let (body, last) = rest.split_at(rest.len().checked_sub(1)?);
    inner.extend_from_slice(body);
    if preview {
        inner.extend_from_slice(&PREVIEW);
    }
    inner.extend_from_slice(last);

    let mut out = PREFIX.to_vec();
    write_varint(now.max(blob.timestamp + 1), &mut out);
    out.extend_from_slice(&INNER_TAG);
    write_varint(inner.len() as u64, &mut out);
    out.extend_from_slice(&inner);
    out.extend_from_slice(blob.trailer);
    Some(out)
}

pub fn get() -> Option<u8> {
    decode(&reg::get(KEY, "Data")?)
}

/// `commit=false` previews (while dragging), `true` saves.
pub fn set(strength: u8, commit: bool) -> Result<(), String> {
    let cur = reg::get(KEY, "Data").ok_or("Night Light settings not found")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let new = encode(&cur, strength, !commit, now).ok_or("Unrecognised Night Light data format")?;
    reg::set(KEY, "Data", reg::REG_BINARY, &new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        s.split_whitespace()
            .map(|h| u8::from_str_radix(h, 16).unwrap())
            .collect()
    }

    // Captured from Settings on build 26300.
    const SAVED: &str = "43 42 01 00 0A 02 01 00 2A 06 AF AD 87 D8 06 2A 2B 0E 19 43 42 01 00 CA 14 0E 15 00 CA 1E 0E 07 00 CF 28 92 59 CA 32 00 CA 3C 00 00 00 00 00";
    const DRAGGING: &str = "43 42 01 00 0A 02 01 00 2A 06 E8 FC 87 D8 06 2A 2B 0E 1C 43 42 01 00 CA 14 0E 15 00 CA 1E 0E 07 00 CF 28 E0 12 CA 32 00 CA 3C 00 C2 46 01 00 00 00 00";

    #[test]
    fn decodes_captured_blobs() {
        assert_eq!(decode(&hex(SAVED)), Some(15)); // 5705K
        assert_eq!(decode(&hex(DRAGGING)), Some(100)); // 1200K
    }

    #[test]
    fn preview_then_commit_matches_settings_layout() {
        let saved = hex(SAVED);
        let ts = parse(&saved).unwrap().timestamp;
        let preview = encode(&saved, 100, true, 0).unwrap();
        // Same as what Settings wrote while dragging, except the timestamp.
        let expect = hex(DRAGGING);
        assert_eq!(
            parse(&preview).unwrap().inner,
            parse(&expect).unwrap().inner
        );
        assert_eq!(parse(&preview).unwrap().timestamp, ts + 1);

        let committed = encode(&preview, 15, false, 0).unwrap();
        assert_eq!(
            parse(&committed).unwrap().inner,
            parse(&saved).unwrap().inner
        );
        assert_eq!(committed.len(), saved.len());
    }
}
