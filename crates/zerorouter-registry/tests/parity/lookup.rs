use serde_json::json;

use crate::{bundled, fixture};

/// SC-004: every `lookup.json` row, through the parity conventions (`quota_family: None` ↔
/// `"normal"`, `strip: None` ↔ `[]`, `kind: None` ↔ `null`).
///
/// The generator leaves out the cx `-review` and muse-spark special cases of 9router's
/// `findModelName` (documented deviation), so the fixture has no rows for them.
///
/// Deviation: 9router keys passthrough by alias, so mimo-free's `passthroughModels` leaks
/// onto the token `mmf`, whose catalog is the separate mmf provider's. In 0router `mmf`
/// names only the mmf provider, which is not passthrough: its undeclared models are not
/// declared. `mimo-free/<anything>` is.
#[test]
fn lookup_matches_9router() {
    let reg = bundled();
    let rows = fixture("lookup");
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2493);

    let mut diffs = Vec::new();
    for row in rows {
        let (alias, model) = (row["alias"].as_str().unwrap(), row["model"].as_str().unwrap());
        let info = reg.model(alias, model).unwrap_or_else(|e| panic!("{alias}/{model}: {e}"));
        let got = json!({
            "isValidModel": info.declared,
            "upstreamId": info.upstream_id,
            "type": info.kind.map(|k| k.as_str()),
            "targetFormat": info.target_format.map(|f| f.as_str()),
            "supportedFormats": info.supported_formats.map(|f| f.iter().map(|x| x.as_str()).collect::<Vec<_>>()),
            "quotaFamily": info.quota_family.unwrap_or("normal"),
            "strip": info.strip.unwrap_or_default().iter().map(|x| x.as_str()).collect::<Vec<_>>(),
            "name": info.name,
        });
        assert_eq!(reg.upstream_id(alias, model).unwrap(), info.upstream_id);
        let mut want = row.clone();
        if alias == "mmf" && info.model.is_none() {
            assert_eq!(row["isValidModel"], true, "{alias}/{model}");
            assert!(reg.model("mimo-free", model).unwrap().declared, "mimo-free/{model}");
            want["isValidModel"] = false.into();
        }
        let want_map = want.as_object_mut().unwrap();
        for k in ["alias", "model", "edge"] {
            want_map.remove(k);
        }
        if got != want {
            diffs.push(format!("{alias}/{model:?} [{}]:\n  got  {got}\n  want {want}", row["edge"]));
        }
    }
    assert!(diffs.is_empty(), "{} of {} rows differ:\n{}", diffs.len(), rows.len(), diffs[..diffs.len().min(20)].join("\n"));
}

#[test]
fn unknown_provider_is_not_found() {
    let reg = bundled();
    assert!(reg.model("no-such-provider", "x").is_err());
    assert!(reg.upstream_id("no-such-provider", "x").is_err());
}
