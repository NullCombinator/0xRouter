//! One entry in a provider's catalog (data-model § Model).

use serde::de::{self, Deserialize, Deserializer, MapAccess, Visitor};
use std::fmt;

use super::enums::{ContentKind, ModelKind, WireFormat};

#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub name: Option<String>,
    /// `None` = no declared type (FR-004). Never defaulted to `llm`.
    pub kind: Option<ModelKind>,
    /// May carry a preset thinking suffix, e.g. `claude-opus-4(high)`.
    pub upstream_id: Option<String>,
    pub target_format: Option<WireFormat>,
    pub supported_formats: Option<Vec<WireFormat>>,
    pub quota_family: Option<String>,
    pub strip: Option<Vec<ContentKind>>,
    pub context_length: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub dimensions: Option<u64>,
    pub rate_multiplier: Option<f64>,
    /// Model feature flags (`edit`, `mask`, …), not capability kinds.
    pub capabilities: Option<Vec<String>>,
    /// Request parameters the model accepts. Names only; secret-like names are rejected.
    pub params: Option<Vec<String>>,
    pub description: Option<String>,
    pub thinking: Option<bool>,
    pub image_gen: Option<bool>,
    /// Schema 2: wire styles this model accepts, in preference order after the native pair.
    pub wires: Option<Vec<String>>,
}

/// A `models` element: a full table or a bare ID string (9router `normalizeModel`).
struct Entry(Model);

impl<'de> Deserialize<'de> for Entry {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Entry;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a model id string or a model table")
            }
            fn visit_str<E: de::Error>(self, id: &str) -> Result<Entry, E> {
                Ok(Entry(Model { id: id.to_owned(), ..Model::default() }))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Entry, A::Error> {
                Model::deserialize(de::value::MapAccessDeserializer::new(map)).map(Entry)
            }
        }
        d.deserialize_any(V)
    }
}

pub(crate) fn de_models<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<Model>>, D::Error> {
    let entries = Option::<Vec<Entry>>::deserialize(d)?;
    Ok(entries.map(|v| v.into_iter().map(|e| e.0).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Doc {
        #[serde(default, deserialize_with = "de_models")]
        models: Option<Vec<Model>>,
    }

    #[test]
    fn bare_and_table_entries() {
        let doc: Doc = toml::from_str(r#"models = ["m-a", { id = "m-b", kind = "tts" }]"#).unwrap();
        let models = doc.models.unwrap();
        assert_eq!(models[0], Model { id: "m-a".into(), ..Model::default() });
        assert_eq!(models[1].kind, Some(ModelKind::Tts));
    }

    #[test]
    fn absent_empty_and_bad_entries() {
        assert!(toml::from_str::<Doc>("").unwrap().models.is_none());
        assert_eq!(toml::from_str::<Doc>("models = []").unwrap().models, Some(vec![]));
        let err = toml::from_str::<Doc>("models = [1]").err().unwrap().to_string();
        assert!(err.contains("a model id string or a model table"), "{err}");
    }
}
