//! Embedding vector encodings: a JSON float array, or base64 of little-endian `f32`s
//! (OpenAI's `encoding_format = "base64"`).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nullrouter_registry::schema::EmbeddingVector;
use serde_json::Value;

/// The vector as floats, from either encoding.
pub fn decode(v: &Value) -> Option<Vec<f32>> {
    match v {
        Value::Array(a) => a.iter().map(|x| x.as_f64().map(|f| f as f32)).collect(),
        Value::String(s) => {
            let bytes = STANDARD.decode(s).ok()?;
            if bytes.len() % 4 != 0 {
                return None;
            }
            Some(bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect())
        }
        _ => None,
    }
}

pub fn encode(form: EmbeddingVector, v: &[f32]) -> Value {
    match form {
        EmbeddingVector::Float => Value::Array(v.iter().map(|f| Value::from(f64::from(*f))).collect()),
        EmbeddingVector::Base64F32le => {
            let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
            Value::String(STANDARD.encode(bytes))
        }
    }
}

/// Re-encodes `v` into `form` when it isn't already there; leaves it alone if unreadable.
pub fn convert(form: EmbeddingVector, v: &Value) -> Value {
    let already = matches!(
        (form, v),
        (EmbeddingVector::Float, Value::Array(_)) | (EmbeddingVector::Base64F32le, Value::String(_))
    );
    if already {
        return v.clone();
    }
    decode(v).map_or_else(|| v.clone(), |f| encode(form, &f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn base64_round_trips_through_floats() {
        let v = [0.5f32, -1.25, 3.0];
        let b = encode(EmbeddingVector::Base64F32le, &v);
        assert_eq!(decode(&b).unwrap(), v);
        assert_eq!(convert(EmbeddingVector::Float, &b), json!([0.5, -1.25, 3.0]));
        assert_eq!(convert(EmbeddingVector::Base64F32le, &json!([0.5, -1.25, 3.0])), b);
    }

    #[test]
    fn a_bad_vector_is_left_alone() {
        assert_eq!(convert(EmbeddingVector::Float, &json!("@@")), json!("@@"));
        assert_eq!(decode(&json!("AAA=")), None, "not a whole number of f32s");
    }
}
