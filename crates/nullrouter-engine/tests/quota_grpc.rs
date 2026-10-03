//! The `grpc_web_ratio` decoder (spec 005 T067), ported from
//! `ref/9router/open-sse/services/usage/grokCliQuotaFrame.js`.

use nullrouter_engine::quota::extract::rfc3339_millis;
use nullrouter_engine::quota::grpc_web::{Ratio, decode};
use nullrouter_engine::testkit::mock_quota::samples;

fn varint(mut v: u64, out: &mut Vec<u8>) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn len_field(number: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![(number << 3) | 2];
    varint(body.len() as u64, &mut out);
    out.extend(body);
    out
}

fn timestamp(secs: u64, nanos: u64) -> Vec<u8> {
    let mut ts = vec![0x08];
    varint(secs, &mut ts);
    if nanos > 0 {
        ts.push(0x10);
        varint(nanos, &mut ts);
    }
    len_field(5, &ts)
}

fn frame(flag: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![flag];
    out.extend((payload.len() as u32).to_be_bytes());
    out.extend(payload);
    out
}

fn trailer() -> Vec<u8> {
    frame(0x80, b"grpc-status:0\r\n")
}

fn message(info: &[u8]) -> Vec<u8> {
    len_field(1, info)
}

fn fixed32(ratio: f32) -> Vec<u8> {
    let mut v = vec![0x0d];
    v.extend(ratio.to_le_bytes());
    v
}

fn fixed64(ratio: f64) -> Vec<u8> {
    let mut v = vec![0x09];
    v.extend(ratio.to_le_bytes());
    v
}

fn reset(r: &Ratio) -> Option<String> {
    r.resets_at.map(rfc3339_millis)
}

const RESET: u64 = 1_784_825_940; // 2026-07-23T16:59:00Z

#[test]
fn framed_with_and_without_trailer() {
    let info = [fixed32(0.5), timestamp(RESET, 0)].concat();
    for buf in [frame(0, &message(&info)), [frame(0, &message(&info)), trailer()].concat()] {
        let r = decode(&buf).unwrap();
        assert_eq!(r.percent_used, 50.0);
        assert_eq!(reset(&r).as_deref(), Some("2026-07-23T16:59:00.000Z"));
    }
    // A trailer first, then the data frame.
    let r = decode(&[trailer(), frame(0, &message(&info))].concat()).unwrap();
    assert_eq!(r.percent_used, 50.0);
}

#[test]
fn unframed_buffer_is_raw_protobuf() {
    let r = decode(&message(&[fixed32(0.75), timestamp(RESET, 868_000_000)].concat())).unwrap();
    assert_eq!(r.percent_used, 75.0);
    assert_eq!(reset(&r).as_deref(), Some("2026-07-23T16:59:00.868Z"));
}

#[test]
fn fixed32_float_and_fixed64_double() {
    let r = decode(&frame(0, &message(&fixed32(0.35)))).unwrap();
    assert!((r.percent_used - 34.999_999_403_953_55).abs() < 1e-12, "{r:?}");
    assert_eq!(r.resets_at, None);
    let r = decode(&frame(0, &message(&fixed64(0.35)))).unwrap();
    assert!((r.percent_used - 35.0).abs() < 1e-9);
    assert_eq!(decode(&frame(0, &message(&fixed32(1.5)))).unwrap().percent_used, 100.0, "capped at 100");
}

#[test]
fn missing_ratio_is_zero_used() {
    let r = decode(&frame(0, &message(&timestamp(RESET, 0)))).unwrap();
    assert_eq!(r.percent_used, 0.0);
    assert!(r.resets_at.is_some());
    assert_eq!(decode(&frame(0, &message(&[]))).unwrap().percent_used, 0.0);
}

#[test]
fn timestamp_rounds_nanos_to_millis() {
    let r = decode(&frame(0, &message(&[fixed32(0.1), timestamp(RESET, 868_500_000)].concat()))).unwrap();
    assert_eq!(reset(&r).as_deref(), Some("2026-07-23T16:59:00.869Z"));
}

#[test]
fn window_rounds_used_and_fills_limit() {
    let r = decode(&samples::grok_credits_frame(0.35, 1_759_622_400)).unwrap();
    let w = r.window("weekly SuperGrok", nullrouter_registry::schema::QuotaUnit::Percent);
    assert_eq!((w.used, w.limit, w.remaining), (Some(35.0), Some(100.0), Some(65.0)));
    assert_eq!(w.resets_at.map(rfc3339_millis).as_deref(), Some("2025-10-05T00:00:00.000Z"));
}

#[test]
fn trailer_only_and_malformed_yield_nothing() {
    let good = frame(0, &message(&[fixed32(0.5), timestamp(RESET, 0)].concat()));
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("trailer only", trailer()),
        ("truncated", good[..good.len() - 3].to_vec()),
        ("negative ratio", frame(0, &message(&fixed32(-0.1)))),
        ("nan ratio", frame(0, &message(&fixed32(f32::NAN)))),
        ("ratio as bytes", frame(0, &message(&len_field(1, b"not-a-float")))),
        ("ratio as varint", frame(0, &message(&[0x08, 0x01]))),
        ("top field 1 is a varint", frame(0, &[0x08, 0x2a])),
        ("no top field 1", frame(0, &[0x48, 0x01])),
        ("field number 0", frame(0, &[0x02, 0x00])),
        ("group wire type", frame(0, &[0x0b])),
        ("length past the end", frame(0, &[0x0a, 0x7f, 0x00])),
        ("varint too long", frame(0, &[0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01])),
        ("garbage", vec![1, 2, 3]),
        ("frame length past the end", [vec![0, 0, 0, 0, 0x27], vec![0x0f, 1, 2]].concat()),
    ];
    for (name, buf) in cases {
        assert_eq!(decode(&buf), None, "{name}");
    }
}
