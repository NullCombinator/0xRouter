//! The edit model: the only way an adapter changes content.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One step of a concrete path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Seg {
    Key(String),
    Index(usize),
}

/// A concrete path into a JSON body, such as `messages[3].content[1]`. `$` is the empty path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Path(pub Vec<Seg>);

fn plain_key(k: &str) -> bool {
    !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl Path {
    pub fn root() -> Self {
        Path(Vec::new())
    }

    pub fn child(&self, key: &str) -> Path {
        let mut p = self.clone();
        p.0.push(Seg::Key(key.to_owned()));
        p
    }

    pub fn index(&self, i: usize) -> Path {
        let mut p = self.clone();
        p.0.push(Seg::Index(i));
        p
    }

    /// True when `self` is `other` or a prefix of it.
    pub fn is_prefix_of(&self, other: &Path) -> bool {
        other.0.len() >= self.0.len() && other.0[..self.0.len()] == self.0[..]
    }

    /// Parses the `Display` form back. Keys that aren't plain are written `["a.b"]`.
    pub fn parse(s: &str) -> Result<Path, String> {
        if s == "$" {
            return Ok(Path::root());
        }
        let b = s.as_bytes();
        let mut segs = Vec::new();
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'.' if i > 0 => {
                    i += 1;
                    if matches!(b.get(i), None | Some(b'.') | Some(b'[')) {
                        return Err("empty segment".into());
                    }
                }
                b'[' => {
                    i += 1;
                    if b.get(i) == Some(&b'"') {
                        let start = i;
                        i += 1;
                        while i < b.len() && !(b[i] == b'"' && b[i - 1] != b'\\') {
                            i += 1;
                        }
                        let key: String = serde_json::from_str(&s[start..=i.min(b.len() - 1)])
                            .map_err(|e| format!("bad quoted key: {e}"))?;
                        i += 1;
                        if b.get(i) != Some(&b']') {
                            return Err("unclosed bracket".into());
                        }
                        i += 1;
                        segs.push(Seg::Key(key));
                    } else {
                        let start = i;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                        if start == i || b.get(i) != Some(&b']') {
                            return Err("bad index".into());
                        }
                        segs.push(Seg::Index(s[start..i].parse().map_err(|_| "bad index")?));
                        i += 1;
                    }
                }
                _ => {
                    let start = i;
                    while i < b.len() && b[i] != b'.' && b[i] != b'[' {
                        i += 1;
                    }
                    if start == i {
                        return Err("empty segment".into());
                    }
                    segs.push(Seg::Key(s[start..i].to_owned()));
                }
            }
        }
        Ok(Path(segs))
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return f.write_str("$");
        }
        for (n, s) in self.0.iter().enumerate() {
            match s {
                Seg::Key(k) if plain_key(k) => {
                    if n > 0 {
                        f.write_str(".")?;
                    }
                    f.write_str(k)?;
                }
                Seg::Key(k) => write!(f, "[{}]", serde_json::to_string(k).unwrap_or_default())?,
                Seg::Index(i) => write!(f, "[{i}]")?,
            }
        }
        Ok(())
    }
}

impl Serialize for Path {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Path {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Path::parse(&s).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Removed,
    Converted,
}

/// Why an edit was made. Closed: adding a code is a minor kit version, removing one a major.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    TargetRejectsField,
    TargetCannotCarryBlock,
    ForeignBlock,
    FormatConversion,
    ParamUnsupportedByModel,
    EmptyAfterRemoval,
    DuplicateTool,
    RoleNotAccepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Remove,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub op: Op,
    pub path: Path,
    pub kind: Kind,
    pub reason: Reason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// What an adapter returns. Serialises as `{"edits": [...]}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Edits {
    pub edits: Vec<Edit>,
}

impl Edits {
    pub fn remove(&mut self, path: &Path, reason: Reason) {
        self.edits.push(Edit { op: Op::Remove, path: path.clone(), kind: Kind::Removed, reason, value: None });
    }

    pub fn convert(&mut self, path: &Path, value: serde_json::Value, reason: Reason) {
        self.edits.push(Edit { op: Op::Replace, path: path.clone(), kind: Kind::Converted, reason, value: Some(value) });
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_and_parse_round_trip() {
        for s in ["$", "a[1].b", "messages[3].content[1]", "[\"a.b\"].c", "x-y.z_1"] {
            let p = Path::parse(s).unwrap();
            assert_eq!(p.to_string(), s);
        }
    }

    #[test]
    fn edit_json_shape() {
        let mut e = Edits::default();
        e.remove(&Path::root().child("messages").index(2).child("reasoning_content"), Reason::TargetRejectsField);
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"edits":[{"op":"remove","path":"messages[2].reasoning_content","kind":"removed","reason":"target_rejects_field"}]}"#
        );
    }
}
