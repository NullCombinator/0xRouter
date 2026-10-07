//! `[[rejections]]`: an upstream answer a provider uses to say a model is definitively not
//! servable (spec 011 research R4). Its shape matches `[[signin.refused]]`; the gate refuses
//! 408, 429 and anything outside 400–499.

use serde::Deserialize;

use super::enums::RejectionReason;
use super::signin::de_statuses;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectionRule {
    #[serde(deserialize_with = "de_statuses")]
    pub status: Vec<u16>,
    /// Case-sensitive, like `[[signin.refused]]`.
    pub body_contains: Option<String>,
    pub reason: RejectionReason,
}

impl RejectionRule {
    pub fn matches(&self, status: u16, body: &str) -> bool {
        self.status.contains(&status) && self.body_contains.as_deref().is_none_or(|t| body.contains(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct File {
        rejections: Vec<RejectionRule>,
    }

    #[test]
    fn parses_and_matches() {
        let f: File = toml::from_str(
            "[[rejections]]\nstatus = [400, 404]\nbody_contains = \"model_retired\"\nreason = \"model_not_found\"\n\
             [[rejections]]\nstatus = 403\nreason = \"model_not_available\"\n",
        )
        .unwrap();
        let [a, b] = &f.rejections[..] else { panic!() };
        assert_eq!(a.reason, RejectionReason::ModelNotFound);
        assert!(a.matches(404, "error: model_retired"));
        assert!(!a.matches(404, "error: MODEL_RETIRED"));
        assert!(!a.matches(403, "model_retired"));
        assert!(b.matches(403, "anything"));
        let err = toml::from_str::<File>("[[rejections]]\nstatus = 400\nreason = \"gone\"\n").unwrap_err().to_string();
        assert!(err.contains("unknown reason \"gone\""), "{err}");
    }
}
