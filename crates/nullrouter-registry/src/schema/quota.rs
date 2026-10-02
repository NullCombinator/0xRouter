//! `[quota]`: how the core reads an account's quota (contracts/signin-quota-schema.md
//! § `[quota]`, research R12). Data only: a request, then window rules built from three
//! extractor tools (`*` paths, `first_of` alternatives, `{ val = n }` unwrapping).

use std::fmt;

use indexmap::IndexMap;
use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize, Serializer};

use super::enums::closed_enum;

closed_enum!(
    /// Which account kinds a `[quota]` section applies to.
    QuotaAccounts, "quota account kind" {
        Signin = "signin",
        Key = "key",
        Any = "any",
    }
);

closed_enum!(
    /// The body of a quota request.
    #[derive(Default)]
    QuotaBody, "quota request body" {
        #[default]
        None = "none",
        GrpcWebEmpty = "grpc_web_empty",
    }
);

closed_enum!(
    /// The unit a quota window counts in.
    QuotaUnit, "quota unit" {
        Percent = "percent",
        Credits = "credits",
        Requests = "requests",
        Tokens = "tokens",
    }
);

closed_enum!(
    /// How a window's reset time is written. `auto`: epoch seconds below 1e12, else
    /// milliseconds, else RFC 3339 (9router `parseResetTime`).
    #[derive(Default)]
    ResetsFormat, "reset time format" {
        #[default]
        Auto = "auto",
        EpochS = "epoch_s",
        EpochMs = "epoch_ms",
        Rfc3339 = "rfc3339",
    }
);

closed_enum!(
    /// How a quota response is decoded. `grpc_web_ratio` is the core's grok-cli credits
    /// decoder (research R12).
    #[derive(Default)]
    QuotaDecoder, "quota decoder" {
        #[default]
        Json = "json",
        GrpcWebRatio = "grpc_web_ratio",
    }
);

/// A JSON value path with `first_of` alternatives: `"billingPeriodEnd | currentPeriod.end"`.
/// The first alternative that resolves wins. `.` is the document root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuePath(pub Vec<String>);

impl ValuePath {
    /// Splits on `|` and checks each alternative's shape.
    pub fn parse(s: &str) -> Result<Self, String> {
        let alts: Vec<String> = s.split('|').map(|a| a.trim().to_owned()).collect();
        for a in &alts {
            if a.is_empty() {
                return Err(format!("{s:?}: empty alternative in a value path"));
            }
            if a == "." {
                continue;
            }
            let ok = a.chars().all(|c| c.is_ascii_alphanumeric() || "_-.*[]".contains(c));
            if !ok || a.starts_with('.') || a.ends_with('.') || a.contains("..") {
                return Err(format!("{a:?} is not a value path such as `a.b`, `items[*]` or `.`"));
            }
        }
        Ok(Self(alts))
    }

    pub fn alternatives(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl fmt::Display for ValuePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.join(" | "))
    }
}

impl<'de> Deserialize<'de> for ValuePath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

/// The quota request.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaRequest {
    pub url: String,
    #[serde(default = "get")]
    pub method: String,
    /// Static, non-secret headers. The account's credential is added by the core.
    #[serde(default)]
    pub headers: IndexMap<String, String>,
    #[serde(default)]
    pub body: QuotaBody,
}

fn get() -> String {
    "GET".into()
}

/// One window rule (research R12).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowRule {
    /// Where windows sit: a key, or a path with `*` over object keys or array items
    /// (`seven_day_*`, `limits[*]`). Each `*` binds `{1}`, `{2}`, … in `name`.
    pub path: String,
    /// Equality filter on fields of the matched value.
    #[serde(default, rename = "where")]
    pub filter: IndexMap<String, toml::Value>,
    /// Name template: literal text, `{1}`…, and `{path}` or `{path|lower}` read from the match.
    pub name: String,
    pub unit: QuotaUnit,
    pub used: Option<ValuePath>,
    pub limit: Option<ValuePath>,
    pub remaining: Option<ValuePath>,
    pub resets_at: Option<ValuePath>,
    #[serde(default)]
    pub resets_format: ResetsFormat,
    /// Read protobuf-JSON `{ val = n }` numbers.
    #[serde(default)]
    pub unwrap_val: bool,
}

/// A request and how to read it: the primary source or the fallback.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaSource {
    pub request: QuotaRequest,
    #[serde(default)]
    pub decoder: QuotaDecoder,
    /// Window rules (`json` decoder).
    #[serde(default, rename = "window")]
    pub windows: Vec<WindowRule>,
    /// The one window's name and unit (`grpc_web_ratio` decoder).
    pub name: Option<String>,
    pub unit: Option<QuotaUnit>,
}

/// `[quota]`. `fallback` is read only when the primary source yields no window.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(from = "RawQuota")]
pub struct QuotaDecl {
    pub accounts: QuotaAccounts,
    pub primary: QuotaSource,
    pub fallback: Option<QuotaSource>,
}

impl QuotaDecl {
    /// The primary source, then the fallback.
    pub fn sources(&self) -> impl Iterator<Item = &QuotaSource> {
        std::iter::once(&self.primary).chain(&self.fallback)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawQuota {
    accounts: QuotaAccounts,
    request: QuotaRequest,
    #[serde(default)]
    decoder: QuotaDecoder,
    #[serde(default)]
    window: Vec<WindowRule>,
    name: Option<String>,
    unit: Option<QuotaUnit>,
    fallback: Option<QuotaSource>,
}

impl From<RawQuota> for QuotaDecl {
    fn from(r: RawQuota) -> Self {
        let primary =
            QuotaSource { request: r.request, decoder: r.decoder, windows: r.window, name: r.name, unit: r.unit };
        Self { accounts: r.accounts, primary, fallback: r.fallback }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Doc {
        quota: QuotaDecl,
    }

    #[test]
    fn parses_the_contract_example() {
        let d: Doc = toml::from_str(
            r#"
[quota]
accounts = "signin"
request = { url = "https://api.anthropic.com/api/oauth/usage", headers = { anthropic-beta = "oauth-2025-04-20" } }

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
resets_at = "billingPeriodEnd | billing_period_end | currentPeriod.end"
resets_format = "epoch_ms"

[quota.fallback]
request = { url = "https://grok.com/x", method = "POST", body = "grpc_web_empty" }
decoder = "grpc_web_ratio"
name = "credits"
unit = "percent"
"#,
        )
        .unwrap();
        let q = d.quota;
        assert_eq!(q.accounts, QuotaAccounts::Signin);
        assert_eq!(q.primary.request.method, "GET");
        assert_eq!(q.primary.windows.len(), 2);
        let w = &q.primary.windows[1];
        assert_eq!(w.filter["kind"].as_str(), Some("weekly_scoped"));
        assert_eq!(w.resets_at.as_ref().unwrap().0.len(), 3);
        assert_eq!(w.resets_format, ResetsFormat::EpochMs);
        let fb = q.fallback.as_ref().unwrap();
        assert_eq!((fb.decoder, fb.request.body), (QuotaDecoder::GrpcWebRatio, QuotaBody::GrpcWebEmpty));
        assert_eq!(q.sources().count(), 2);
    }

    #[test]
    fn value_paths() {
        assert_eq!(ValuePath::parse("data | models | .").unwrap().0, ["data", "models", "."]);
        assert!(ValuePath::parse("a || b").is_err());
        assert!(ValuePath::parse("a.{b}").is_err());
        assert!(ValuePath::parse(".a").is_err());
    }

    #[test]
    fn unknown_keys_and_values_are_refused() {
        let base = "[quota]\naccounts = \"signin\"\nrequest = { url = \"https://a.example\" }\n";
        assert!(toml::from_str::<Doc>(&format!("{base}extra = 1\n")).is_err());
        let err = toml::from_str::<Doc>(&base.replace("signin", "cookie")).err().unwrap().to_string();
        assert!(err.contains("allowed: signin, key, any"), "{err}");
    }
}
