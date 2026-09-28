//! Closed sets of names a plugin may use to *select* core behaviour (Constitution I).
//! An unknown value is rejected with the allowed values listed.

use std::fmt;

use serde::de::{Deserialize, Deserializer, Error as _};
use serde::{Serialize, Serializer};

macro_rules! closed_enum {
    ($(#[$meta:meta])* $name:ident, $what:literal { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            /// Every accepted spelling, in declaration order.
            pub const ALLOWED: &[&str] = &[$($text),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            pub fn parse(s: &str) -> Option<Self> {
                match s {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                Self::parse(&s).ok_or_else(|| {
                    D::Error::custom(format!(
                        "unknown {} {s:?}; allowed: {}",
                        $what,
                        Self::ALLOWED.join(", ")
                    ))
                })
            }
        }
    };
}
pub(crate) use closed_enum;

closed_enum!(
    /// Provider category (FR-006). Spellings follow 9router.
    Category, "category" {
        Apikey = "apikey",
        Oauth = "oauth",
        FreeTier = "freeTier",
        Free = "free",
        WebCookie = "webCookie",
    }
);

closed_enum!(
    /// Wire format of an upstream endpoint; each names a core translator/executor.
    WireFormat, "format" {
        Openai = "openai",
        OpenaiResponses = "openai-responses",
        Claude = "claude",
        Gemini = "gemini",
        GeminiCli = "gemini-cli",
        Vertex = "vertex",
        Antigravity = "antigravity",
        Kiro = "kiro",
        Cursor = "cursor",
        Commandcode = "commandcode",
        Ollama = "ollama",
        GrokWeb = "grok-web",
        PerplexityWeb = "perplexity-web",
    }
);

impl WireFormat {
    /// The slice 002 format for a schema 2 wire style id, for the four bundled styles.
    pub fn from_wire(wire: &str) -> Option<Self> {
        match wire {
            "openai-chat" => Some(Self::Openai),
            "anthropic-messages" => Some(Self::Claude),
            "openai-responses" => Some(Self::OpenaiResponses),
            "gemini" => Some(Self::Gemini),
            _ => None,
        }
    }
}

closed_enum!(
    /// Request-shaping behaviours built into the core.
    Quirk, "quirk" {
        PreserveCacheControl = "preserve_cache_control",
        DropClientMetadata = "drop_client_metadata",
        ClineEnvelope = "cline_envelope",
        DropOutputConfig = "drop_output_config",
        RequireClaudeToolType = "require_claude_tool_type",
        CloakToolsOnOauth = "cloak_tools_on_oauth",
    }
);

closed_enum!(
    /// Header hooks built into the core (9router `HEADER_HOOKS`).
    AuthHook, "auth hook" {
        ClineHeaders = "cline_headers",
        KimiHeaders = "kimi_headers",
        KilocodeOrg = "kilocode_org",
    }
);

closed_enum!(
    /// How the user authenticates (9router `authType` / `authModes`).
    AuthKind, "auth kind" {
        Apikey = "apikey",
        Oauth = "oauth",
        Cookie = "cookie",
        None = "none",
    }
);

closed_enum!(
    /// How the core renders the credential into the auth header. `user_id_access_token`
    /// is cursor's `"<user_id> <access_token>"` template, selected by name.
    AuthScheme, "auth scheme" {
        Bearer = "bearer",
        Raw = "raw",
        UserIdAccessToken = "<user_id> <access_token>",
    }
);

closed_enum!(
    /// A capability a provider can offer (every 9router `serviceKinds` value).
    #[non_exhaustive]
    CapabilityKind, "capability kind" {
        Llm = "llm",
        Image = "image",
        ImageToText = "image_to_text",
        Video = "video",
        Tts = "tts",
        Stt = "stt",
        Embedding = "embedding",
        WebSearch = "web_search",
        WebFetch = "web_fetch",
        Systemone = "systemone",
    }
);

/// A model's declared type. Same value set as [`CapabilityKind`].
pub type ModelKind = CapabilityKind;

closed_enum!(
    /// Request content a model cannot accept (9router `strip`).
    ContentKind, "content kind" {
        Image = "image",
        Audio = "audio",
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_value_lists_allowed() {
        let err =
            toml::from_str::<std::collections::BTreeMap<String, Quirk>>("q = \"run_script\"").unwrap_err().to_string();
        assert!(err.contains("unknown quirk \"run_script\"; allowed: preserve_cache_control"), "{err}");
    }

    #[test]
    fn round_trips_spelling() {
        assert_eq!(Category::parse("freeTier"), Some(Category::FreeTier));
        assert_eq!(WireFormat::ALLOWED.len(), 13);
        assert_eq!(CapabilityKind::WebSearch.to_string(), "web_search");
    }
}
