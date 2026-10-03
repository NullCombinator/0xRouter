//! The `grpc_web_ratio` decoder for grok-cli's credits RPC (research R12), ported from
//! `ref/9router/open-sse/services/usage/grokCliQuotaFrame.js`.
//!
//! The response is gRPC-web: frames of `flag (1 byte) + length (4 bytes, big-endian) +
//! payload`, flag bit `0x80` marking a trailer. The first data frame holds a protobuf message
//! whose field 1 is the credits info: nested field 1 is the used ratio (fixed32 float or fixed64
//! double; absent = 0% used, as proto3 omits zeros) and nested field 5 a `Timestamp {seconds,
//! nanos}` for the pool's reset. A buffer that doesn't start with a valid frame header is read
//! as a bare protobuf message. Anything malformed gives `None`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nullrouter_registry::schema::QuotaUnit;

use super::extract::QuotaWindow;

/// The decoded credits report.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ratio {
    /// Used share of the pool, 0–100 (a ratio above 1 reads as 100).
    pub percent_used: f64,
    /// When the pool resets, at millisecond precision.
    pub resets_at: Option<SystemTime>,
}

impl Ratio {
    /// The one window the decoder yields: `name` and `unit` come from the plugin. `used` is
    /// rounded to two decimals (a fixed32 0.35 reads as 34.99999940395355).
    pub fn window(&self, name: &str, unit: QuotaUnit) -> QuotaWindow {
        let used = (self.percent_used * 100.0).round() / 100.0;
        QuotaWindow {
            name: name.to_owned(),
            unit,
            used: Some(used),
            limit: Some(100.0),
            remaining: Some((100.0 - used).max(0.0)),
            resets_at: self.resets_at,
        }
    }
}

/// Decodes a `GetGrokCreditsConfig` response body.
pub fn decode(buf: &[u8]) -> Option<Ratio> {
    if buf.is_empty() {
        return None;
    }
    let payload = if frame_header(buf, 0).is_some() { data_frame(buf)? } else { buf };
    let top = Fields::read(payload)?;
    let Field::Bytes(info) = top.get(1)? else { return None };
    let info = Fields::read(info)?;
    let ratio = match info.get(1) {
        None => 0.0,
        Some(Field::Fixed32(b)) => f64::from(f32::from_le_bytes(b)),
        Some(Field::Fixed64(b)) => f64::from_le_bytes(b),
        Some(_) => return None,
    };
    if !ratio.is_finite() || ratio < 0.0 {
        return None;
    }
    Some(Ratio { percent_used: (ratio * 100.0).min(100.0), resets_at: info.get(5).and_then(timestamp) })
}

/// A frame header at `at`: `(flag, payload start, payload length)`, if the flag is one gRPC-web
/// uses and the payload fits in the buffer.
fn frame_header(buf: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let head = buf.get(at..at.checked_add(5)?)?;
    let flag = head[0];
    if !matches!(flag, 0x00 | 0x01 | 0x80 | 0x81) {
        return None;
    }
    let len = u32::from_be_bytes([head[1], head[2], head[3], head[4]]) as usize;
    let start = at + 5;
    (len <= buf.len() - start).then_some((flag, start, len))
}

/// The payload of the first data (non-trailer) frame.
fn data_frame(buf: &[u8]) -> Option<&[u8]> {
    let mut at = 0;
    while at < buf.len() {
        let (flag, start, len) = frame_header(buf, at)?;
        if flag & 0x80 == 0 {
            return Some(&buf[start..start + len]);
        }
        at = start + len;
    }
    None
}

#[derive(Debug, Clone, Copy)]
enum Field<'a> {
    Varint(u128),
    Fixed64([u8; 8]),
    Bytes(&'a [u8]),
    Fixed32([u8; 4]),
}

/// A message's fields in wire order; a repeated field number reads as its last value.
struct Fields<'a>(Vec<(u64, Field<'a>)>);

impl<'a> Fields<'a> {
    fn read(buf: &'a [u8]) -> Option<Self> {
        let mut out = Vec::new();
        let mut at = 0;
        while at < buf.len() {
            let (tag, next) = varint(buf, at)?;
            // 9router reads the tag through JavaScript's `>>> 3`, a 32-bit shift.
            let tag = tag as u32;
            let (number, wire) = (u64::from(tag >> 3), tag & 7);
            if number == 0 {
                return None;
            }
            let (field, next) = match wire {
                0 => varint(buf, next).map(|(v, n)| (Field::Varint(v), n))?,
                1 => {
                    let b = buf.get(next..next.checked_add(8)?)?;
                    (Field::Fixed64(b.try_into().ok()?), next + 8)
                }
                2 => {
                    let (len, start) = varint(buf, next)?;
                    let end = start.checked_add(usize::try_from(len).ok()?)?;
                    (Field::Bytes(buf.get(start..end)?), end)
                }
                5 => {
                    let b = buf.get(next..next.checked_add(4)?)?;
                    (Field::Fixed32(b.try_into().ok()?), next + 4)
                }
                _ => return None,
            };
            out.push((number, field));
            at = next;
        }
        Some(Self(out))
    }

    fn get(&self, number: u64) -> Option<Field<'a>> {
        self.0.iter().rev().find(|(n, _)| *n == number).map(|(_, f)| *f)
    }
}

/// A base-128 varint at `at`: `(value, next offset)`. At most 11 bytes, as 9router allows.
fn varint(buf: &[u8], at: usize) -> Option<(u128, usize)> {
    let mut value = 0u128;
    let mut shift = 0u32;
    let mut pos = at;
    loop {
        let byte = *buf.get(pos)?;
        value |= u128::from(byte & 0x7f) << shift;
        pos += 1;
        if byte & 0x80 == 0 {
            return Some((value, pos));
        }
        shift += 7;
        if shift > 70 {
            return None;
        }
    }
}

/// A protobuf `Timestamp` at millisecond precision; 9router's `Date` range bounds it.
fn timestamp(field: Field<'_>) -> Option<SystemTime> {
    let Field::Bytes(b) = field else { return None };
    let ts = Fields::read(b)?;
    let part = |n| match ts.get(n) {
        Some(Field::Varint(v)) => v as f64,
        _ => 0.0,
    };
    let millis = part(1) * 1000.0 + (part(2) / 1_000_000.0).round();
    (millis <= 8.64e15).then(|| UNIX_EPOCH + Duration::from_millis(millis as u64))
}
