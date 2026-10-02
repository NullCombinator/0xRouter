//! Translation parity with 9router (T043, research R4, R26): every
//! `tests/fixtures/9router/translate/**` case run through the bundled styles and codecs,
//! compared leaf by leaf, with differences allowed only through `tests/parity/deviations.toml`.
//!
//! Requests take the engine's translated path: decode in the client's style, encode for the
//! wire, then the stream switches a streamed attempt sets. Streams read the provider's events
//! with the wire's reader and write them with the client's writer.

mod oracle;

use oracle::{Deviations, Fixture, bundled, fixtures, style_id};
use serde_json::Value;
use zerorouter_wire::codec::request::{self, Edits};
use zerorouter_wire::stream::{Frame, StreamReader, StreamWriter};

fn translate_request(f: &Fixture) -> Value {
    let d = &f.data;
    let (client, wire) = (bundled(style_id(d["from"].as_str().unwrap())), bundled(style_id(d["to"].as_str().unwrap())));
    let stream = d["stream"].as_bool().unwrap();
    let mut ir = request::decode(&client, &d["input"]).unwrap_or_else(|e| panic!("{}/{}: decode: {e}", f.pair, f.case));
    ir.model = "m1".into();
    ir.stream = stream;
    let enc = match request::encode(&ir, &wire, &client.id) {
        Ok(enc) => enc,
        // A request the wire can't carry skips the target (R4): no body to compare.
        Err(e) => return serde_json::json!({ "0router": format!("refused: {e}") }),
    };
    request::forward(&enc.body, &wire, &Edits { include_usage: stream, ..Edits::default() }).unwrap()
}

/// The client's frames for the provider's events, as `{ event?, data }` (`[DONE]` left out,
/// as in the oracle).
fn translate_stream(f: &Fixture) -> Value {
    let d = &f.data;
    let (wire, client) =
        (bundled(style_id(d["wire"].as_str().unwrap())), bundled(style_id(d["client"].as_str().unwrap())));
    let mut reader = StreamReader::new(&wire).unwrap();
    let mut writer =
        StreamWriter::new(&client, &serde_json::json!({"stream": true}), "req-1", "", 1_700_000_000).unwrap();
    let mut text = String::new();
    for ev in d["upstream"].as_array().unwrap() {
        let frame = match ev {
            Value::String(s) => Frame { event: None, data: s.clone() },
            v => Frame { event: v["type"].as_str().map(str::to_owned), data: v.to_string() },
        };
        for e in reader.read(&frame).unwrap_or_else(|e| panic!("{}/{}: read: {e}", f.pair, f.case)) {
            text += &writer.write(&e);
        }
    }
    for e in reader.finish() {
        text += &writer.write(&e);
    }
    text += &writer.end();
    let mut frames = Vec::new();
    for block in text.split("\n\n").filter(|b| !b.trim().is_empty()) {
        let mut frame = serde_json::Map::new();
        for line in block.lines() {
            if let Some(e) = line.strip_prefix("event: ") {
                frame.insert("event".into(), e.into());
            } else if let Some(d) = line.strip_prefix("data: ").filter(|d| *d != "[DONE]") {
                frame.insert("data".into(), serde_json::from_str(d).unwrap());
            }
        }
        if frame.contains_key("data") {
            frames.push(Value::Object(frame));
        }
    }
    Value::Array(frames)
}

#[test]
fn every_oracle_case_matches_or_is_a_listed_deviation() {
    let dev = Deviations::load();
    let mut failures = Vec::new();
    let all = fixtures();
    for f in &all {
        let (got, want) = if f.data.get("upstream").is_some() {
            (translate_stream(f), f.data["client_frames"].clone())
        } else {
            (translate_request(f), f.data["output"].clone())
        };
        // `ZR_ORACLE_DUMP=<dir>` writes 0router's side, to review a pair against the oracle.
        if let Some(dir) = std::env::var_os("ZR_ORACLE_DUMP") {
            let dir = std::path::Path::new(&dir).join(&f.pair);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{}.json", f.case)), serde_json::to_string_pretty(&got).unwrap()).unwrap();
        }
        failures.extend(dev.compare(&f.pair, &f.case, &got, &want));
    }
    failures.extend(dev.stale());
    assert!(failures.is_empty(), "{} differences over {} cases:\n\n{}", failures.len(), all.len(), failures.join("\n"));
}
