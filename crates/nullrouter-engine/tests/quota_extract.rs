//! The quota extractor (spec 005 T066, research R12): one case per 9router parser branch
//! (`ref/9router/open-sse/services/usage/{claude,grok-cli,opencode-go,opencode-zen,shared}.js`).

use nullrouter_engine::quota::{self, QuotaWindow, extract};
use nullrouter_engine::testkit::mock_quota::samples;
use nullrouter_registry::schema::{QuotaDecl, QuotaUnit, ResetsFormat, WindowRule};
use serde_json::{Value, json};

fn rules(windows: &str) -> Vec<WindowRule> {
    #[derive(serde::Deserialize)]
    struct Doc {
        quota: QuotaDecl,
    }
    let text = format!("[quota]\naccounts = \"any\"\nrequest = {{ url = \"https://q.example\" }}\n{windows}");
    toml::from_str::<Doc>(&text).unwrap().quota.primary.windows
}

fn run(windows: &str, body: Value) -> Vec<QuotaWindow> {
    extract::extract(&rules(windows), &body)
}

fn at(s: &str) -> String {
    s.to_owned()
}

/// `(name, used, limit, remaining, resets_at)`.
type Row = (String, Option<f64>, Option<f64>, Option<f64>, Option<String>);

fn rows(ws: &[QuotaWindow]) -> Vec<Row> {
    ws.iter()
        .map(|w| (w.name.clone(), w.used, w.limit, w.remaining, w.resets_at.map(extract::rfc3339_millis)))
        .collect()
}

const ANTHROPIC: &str = r#"
[[quota.window]]
path = "five_hour"
name = "5-hour"
unit = "percent"
used = "utilization"
resets_at = "resets_at"

[[quota.window]]
path = "seven_day"
name = "weekly"
unit = "percent"
used = "utilization"
resets_at = "resets_at"

[[quota.window]]
path = "seven_day_*"
name = "weekly {1}"
unit = "percent"
used = "utilization"
resets_at = "resets_at"

[[quota.window]]
path = "limits[*]"
where = { kind = "weekly_scoped" }
name = "weekly {scope.model.display_name|lower}"
unit = "percent"
used = "percent"
resets_at = "resets_at"
"#;

#[test]
fn anthropic_windows_models_and_scoped_limits() {
    let ws = run(ANTHROPIC, samples::anthropic_usage());
    assert!(ws.iter().all(|w| w.unit == QuotaUnit::Percent));
    assert_eq!(
        rows(&ws),
        [
            (at("5-hour"), Some(87.0), Some(100.0), Some(13.0), Some(at("2026-10-02T18:00:00.000Z"))),
            (at("weekly"), Some(40.0), Some(100.0), Some(60.0), Some(at("2026-10-07T00:00:00.000Z"))),
            (at("weekly opus"), Some(12.5), Some(100.0), Some(87.5), Some(at("2026-10-07T00:00:00.000Z"))),
            (at("weekly fable"), Some(30.0), Some(100.0), Some(70.0), Some(at("2026-10-07T00:00:00.000Z"))),
        ]
    );
}

#[test]
fn anthropic_missing_utilization_and_blank_scope_are_skipped() {
    let body = json!({
        "five_hour": { "resets_at": "2026-10-02T18:00:00Z" },
        "seven_day": { "utilization": null },
        "seven_day_oauth_apps": null,
        "seven_day_sonnet": { "utilization": 3, "resets_at": 1791331200 },
        "limits": [
            { "kind": "weekly_scoped", "percent": 140, "scope": { "model": { "display_name": " Opus 4 " } } },
            { "kind": "weekly_scoped", "percent": -5, "scope": { "model": { "display_name": "Haiku" } } },
            { "kind": "weekly_scoped", "percent": 10, "scope": { "model": {} } },
            { "kind": "weekly_scoped", "percent": 10, "scope": { "model": { "display_name": "  " } } },
            { "kind": "other", "percent": 99, "scope": { "model": { "display_name": "Other" } } }
        ]
    });
    assert_eq!(
        rows(&run(ANTHROPIC, body)),
        [
            (at("weekly sonnet"), Some(3.0), Some(100.0), Some(97.0), Some(at("2026-10-07T00:00:00.000Z"))),
            // Over the limit: shown as reported (SC-006), remaining floored at 0.
            (at("weekly opus 4"), Some(140.0), Some(100.0), Some(0.0), None),
            (at("weekly haiku"), Some(0.0), Some(100.0), Some(100.0), None),
        ]
    );
    assert!(run(ANTHROPIC, json!({})).is_empty());
}

#[test]
fn first_window_of_a_name_wins() {
    let body = json!({
        "seven_day_fable": { "utilization": 5 },
        "limits": [{ "kind": "weekly_scoped", "percent": 30, "scope": { "model": { "display_name": "Fable" } } }]
    });
    assert_eq!(rows(&run(ANTHROPIC, body)), [(at("weekly fable"), Some(5.0), Some(100.0), Some(95.0), None)]);
}

const GROK: &str = r#"
[[quota.window]]
path = "."
name = "monthly included"
unit = "credits"
limit = "config.monthlyLimit | monthlyLimit"
used = "config.includedUsed | includedUsed | config.totalUsed | totalUsed"
resets_at = "config.billingPeriodEnd | config.billing_period_end | config.currentPeriod.end | billingPeriodEnd | billing_period_end | resetAt"
unwrap_val = true

[[quota.window]]
path = "."
name = "on-demand"
unit = "credits"
limit = "config.onDemandCap | onDemandCap"
used = "config.onDemandUsed | onDemandUsed"
resets_at = "config.billingPeriodEnd | config.billing_period_end | config.currentPeriod.end | billingPeriodEnd | billing_period_end | resetAt"
unwrap_val = true

[[quota.window]]
path = "."
name = "prepaid"
unit = "credits"
limit = "config.prepaidBalance | prepaidBalance"
remaining = "config.prepaidBalance | prepaidBalance"
unwrap_val = true

[[quota.window]]
path = "."
name = "weekly SuperGrok"
unit = "percent"
used = "config.creditUsagePercent | creditUsagePercent"
resets_at = "config.billingPeriodEnd | config.currentPeriod.end"
unwrap_val = true

[[quota.window]]
path = "credits"
name = "credits"
unit = "credits"
limit = "total | limit | cap"
used = "used | spent"
remaining = "remaining | balance"
resets_at = "resetAt | end"
unwrap_val = true
"#;

#[test]
fn grok_billing_val_numbers_monthly_on_demand_prepaid() {
    let ws = run(GROK, samples::grok_billing());
    let reset = Some(at("2026-11-01T00:00:00.000Z"));
    assert_eq!(
        rows(&ws),
        [
            (at("monthly included"), Some(1200.0), Some(2500.0), Some(1300.0), reset.clone()),
            (at("on-demand"), Some(250.0), Some(1000.0), Some(750.0), reset),
            (at("prepaid"), Some(0.0), Some(500.0), Some(500.0), None),
        ]
    );
    assert!(ws.iter().all(|w| w.unit == QuotaUnit::Credits));
}

#[test]
fn grok_each_reset_alternative() {
    let on_demand = |extra: Value| {
        let mut config = json!({ "onDemandCap": { "val": 10 }, "onDemandUsed": { "val": 1 } });
        config.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        let ws = run(GROK, json!({ "config": config }));
        ws.iter().find(|w| w.name == "on-demand").unwrap().resets_at.map(extract::rfc3339_millis)
    };
    let want = Some(at("2026-09-21T14:13:20.000Z"));
    assert_eq!(on_demand(json!({ "billingPeriodEnd": "2026-09-21T14:13:20Z" })), want);
    assert_eq!(on_demand(json!({ "billing_period_end": 1_790_000_000 })), want);
    assert_eq!(on_demand(json!({ "currentPeriod": { "end": "2026-09-21T14:13:20+00:00" } })), want);
    // An alternative that doesn't parse gives way to the next.
    assert_eq!(on_demand(json!({ "billingPeriodEnd": "", "billing_period_end": "1790000000000" })), want);
    assert_eq!(on_demand(json!({})), None);
    // Top-level fields when there is no `config`.
    let ws = run(GROK, json!({ "monthlyLimit": 400, "totalUsed": "120", "billing_period_end": 1_790_000_000 }));
    assert_eq!(rows(&ws), [(at("monthly included"), Some(120.0), Some(400.0), Some(280.0), want)]);
}

#[test]
fn grok_weekly_percent_and_credit_bags() {
    let ws = run(
        GROK,
        json!({ "config": { "creditUsagePercent": 99, "onDemandCap": { "val": 0 }, "onDemandUsed": { "val": 0 },
                            "prepaidBalance": { "val": 0 }, "billingPeriodEnd": "2026-07-24T12:42:26.494595+00:00" } }),
    );
    assert_eq!(
        rows(&ws),
        [(at("weekly SuperGrok"), Some(99.0), Some(100.0), Some(1.0), Some(at("2026-07-24T12:42:26.494Z")))],
        "a zero cap and a zero prepaid balance are no windows"
    );
    let ws = run(
        GROK,
        json!({ "credits": { "total": { "val": 50 }, "remaining": { "val": 20 }, "resetAt": "2026-09-01T00:00:00Z" } }),
    );
    assert_eq!(rows(&ws), [(at("credits"), Some(30.0), Some(50.0), Some(20.0), Some(at("2026-09-01T00:00:00.000Z")))]);
    let ws = run(GROK, json!({ "credits": { "balance": 0 } }));
    assert_eq!(rows(&ws), [(at("credits"), None, None, Some(0.0), None)]);
}

#[test]
fn grok_zero_caps_yield_no_window() {
    let body = json!({ "config": { "onDemandCap": { "val": 0 }, "onDemandUsed": { "val": 0 }, "prepaidBalance": { "val": 0 } } });
    assert!(run(GROK, body).is_empty(), "so the poller reads the gRPC-web fallback");
}

fn opencode() -> String {
    ["rolling", "weekly", "monthly"]
        .map(|p| {
            format!(
                "[[quota.window]]\npath = \"usage.{p}\"\nname = \"{p}\"\nunit = \"percent\"\nused = \"percent\"\nresets_at = \"resetsAt\"\n"
            )
        })
        .concat()
}

#[test]
fn opencode_rolling_weekly_monthly() {
    assert_eq!(
        rows(&run(&opencode(), samples::opencode_usage())),
        [
            (at("rolling"), Some(13.0), Some(100.0), Some(87.0), Some(at("2026-09-04T14:28:02.617Z"))),
            (at("weekly"), Some(5.0), Some(100.0), Some(95.0), Some(at("2026-09-07T00:00:00.617Z"))),
            (at("monthly"), Some(2.0), Some(100.0), Some(98.0), Some(at("2026-10-02T12:14:24.617Z"))),
        ]
    );
}

#[test]
fn opencode_string_percents_floors_and_skips() {
    let body = json!({ "usage": {
        "rolling": { "percent": "40.5", "resetsAt": 1_790_000_000 },
        "weekly": { "percent": 130 },
        "monthly": { "percent": -3, "resetsAt": "1790000000000" },
        "future": { "percent": 10 }
    } });
    assert_eq!(
        rows(&run(&opencode(), body)),
        [
            (at("rolling"), Some(40.5), Some(100.0), Some(59.5), Some(at("2026-09-21T14:13:20.000Z"))),
            (at("weekly"), Some(130.0), Some(100.0), Some(0.0), None),
            (at("monthly"), Some(0.0), Some(100.0), Some(100.0), Some(at("2026-09-21T14:13:20.000Z"))),
        ]
    );
    let none = json!({ "usage": { "rolling": { "status": "ok" }, "weekly": { "percent": "" }, "monthly": { "percent": "n/a" } } });
    assert!(run(&opencode(), none).is_empty());
    assert!(run(&opencode(), json!({ "data": [] })).is_empty());
}

#[test]
fn reset_formats() {
    let p = |v: Value, f| extract::parse_reset(&v, f).map(extract::rfc3339_millis);
    let t = Some(at("2026-09-21T14:13:20.000Z"));
    assert_eq!(p(json!(1_790_000_000), ResetsFormat::Auto), t);
    assert_eq!(p(json!(1_790_000_000_000_u64), ResetsFormat::Auto), t);
    assert_eq!(p(json!("1790000000"), ResetsFormat::Auto), t);
    assert_eq!(p(json!("2026-09-21T14:13:20Z"), ResetsFormat::Auto), t);
    assert_eq!(p(json!("2026-09-21T16:13:20.000+02:00"), ResetsFormat::Auto), t);
    assert_eq!(p(json!("2026-09-21"), ResetsFormat::Auto), Some(at("2026-09-21T00:00:00.000Z")));
    // Explicit formats don't guess.
    assert_eq!(p(json!(1_790_000_000), ResetsFormat::EpochS), t);
    assert_eq!(p(json!(1_790_000_000_000_u64), ResetsFormat::EpochMs), t);
    assert_eq!(p(json!(1_790_000), ResetsFormat::EpochMs), Some(at("1970-01-01T00:29:50.000Z")));
    assert_eq!(p(json!("2026-09-21T14:13:20Z"), ResetsFormat::EpochS), None);
    assert_eq!(p(json!(1_790_000_000), ResetsFormat::Rfc3339), None);
    assert_eq!(p(json!("2026-09-21T14:13:20Z"), ResetsFormat::Rfc3339), t);
    // No reset.
    for v in [json!(""), json!(null), json!(0), json!("soon"), json!(true), json!({})] {
        assert_eq!(p(v.clone(), ResetsFormat::Auto), None, "{v}");
    }
}

#[test]
fn templates_bind_stars_and_filters_compare_types() {
    let body = json!({
        "pools": [
            { "kind": "a", "tier": 2, "on": true, "used": 1, "cap": 4, "label": "Pool A" },
            { "kind": "a", "tier": 3, "on": true, "used": 1, "cap": 4, "label": "Pool B" },
            { "kind": "a", "tier": 2, "on": false, "used": 1, "cap": 4, "label": "Pool C" }
        ],
        "by_region": { "eu_x": { "n": 5 }, "us_x": { "n": 6 }, "eu_y": { "n": 7 } }
    });
    let ws = run(
        r#"
[[quota.window]]
path = "pools[*]"
where = { kind = "a", tier = 2, on = true }
name = "{label|lower} #{1}"
unit = "requests"
used = "used"
limit = "cap"

[[quota.window]]
path = "by_region.*_x"
name = "region {1}"
unit = "tokens"
remaining = "n"
"#,
        body,
    );
    assert_eq!(
        rows(&ws),
        [
            (at("pool a #0"), Some(1.0), Some(4.0), Some(3.0), None),
            (at("region eu"), None, None, Some(5.0), None),
            (at("region us"), None, None, Some(6.0), None),
        ]
    );
    assert_eq!(ws[0].unit, QuotaUnit::Requests);
}

#[test]
fn read_dispatches_on_the_decoder_and_windows_serialize() {
    #[derive(serde::Deserialize)]
    struct Doc {
        quota: QuotaDecl,
    }
    let q = toml::from_str::<Doc>(&format!(
        "[quota]\naccounts = \"key\"\nrequest = {{ url = \"https://q.example\" }}\n{}\n[quota.fallback]\nrequest = {{ url = \"https://g.example\", method = \"POST\", body = \"grpc_web_empty\" }}\ndecoder = \"grpc_web_ratio\"\nname = \"credits\"\nunit = \"percent\"\n",
        opencode()
    ))
    .unwrap()
    .quota;
    let body = serde_json::to_vec(&samples::opencode_usage()).unwrap();
    assert_eq!(quota::read(&q.primary, &body).len(), 3);
    assert!(quota::read(&q.primary, b"not json").is_empty());
    let fb = q.fallback.as_ref().unwrap();
    let ws = quota::read(fb, &samples::grok_credits_frame(0.35, 1_790_000_000));
    assert_eq!(rows(&ws), [(at("credits"), Some(35.0), Some(100.0), Some(65.0), Some(at("2026-09-21T14:13:20.000Z")))]);
    assert!(quota::read(fb, b"").is_empty());

    let v = serde_json::to_value(&ws[0]).unwrap();
    assert_eq!(
        v,
        json!({ "name": "credits", "unit": "percent", "used": 35.0, "limit": 100.0, "remaining": 65.0,
                "resets_at": "2026-09-21T14:13:20.000Z" })
    );
    assert_eq!(serde_json::from_value::<QuotaWindow>(v).unwrap(), ws[0]);
    let bare = QuotaWindow {
        name: "x".into(),
        unit: QuotaUnit::Credits,
        used: None,
        limit: None,
        remaining: Some(1.0),
        resets_at: None,
    };
    assert_eq!(serde_json::to_value(&bare).unwrap(), json!({ "name": "x", "unit": "credits", "remaining": 1.0 }));
}
