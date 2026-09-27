//! Positioned validation errors (FR-010): file, position, field path, and rule.

use std::fmt;

/// One segment of a field path: a table key or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    Index(usize),
}

/// A field path such as `transport.headers.Authorization` or `models[3].id`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FieldPath(pub Vec<Seg>);

impl FieldPath {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn key(&self, k: impl Into<String>) -> Self {
        let mut p = self.clone();
        p.0.push(Seg::Key(k.into()));
        p
    }

    pub fn index(&self, i: usize) -> Self {
        let mut p = self.clone();
        p.0.push(Seg::Index(i));
        p
    }

    /// Builds a path from dotted keys, e.g. `"transport.headers"`.
    pub fn of(dotted: &str) -> Self {
        Self(dotted.split('.').filter(|s| !s.is_empty()).map(|s| Seg::Key(s.to_owned())).collect())
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for FieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.0.iter().enumerate() {
            match seg {
                Seg::Key(k) if i == 0 => f.write_str(k)?,
                Seg::Key(k) => write!(f, ".{k}")?,
                Seg::Index(n) => write!(f, "[{n}]")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ValidationError {
    pub file: String,
    /// 1-based; 0 when the position is unknown.
    pub line: usize,
    pub col: usize,
    pub path: FieldPath,
    pub rule: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.file)?;
        if self.line > 0 {
            write!(f, ":{}:{}", self.line, self.col)?;
        }
        if self.path.is_root() { write!(f, ": {}", self.rule) } else { write!(f, " {}: {}", self.path, self.rule) }
    }
}

/// Converts a byte offset into a 1-based (line, column), counting columns in chars.
pub fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(src.len());
    while !src.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_file_position_path_rule() {
        let e = ValidationError {
            file: "acme.toml".into(),
            line: 9,
            col: 13,
            path: FieldPath::of("transport.headers").key("Authorization"),
            rule: "credential-bearing header not allowed in plugins".into(),
        };
        assert_eq!(
            e.to_string(),
            "acme.toml:9:13 transport.headers.Authorization: credential-bearing header not allowed in plugins"
        );
        let parse = ValidationError { path: FieldPath::root(), rule: "bad".into(), line: 4, col: 7, ..e };
        assert_eq!(parse.to_string(), "acme.toml:4:7: bad");
    }

    #[test]
    fn index_segments() {
        assert_eq!(FieldPath::of("models").index(3).key("id").to_string(), "models[3].id");
    }

    #[test]
    fn line_col_counts_chars() {
        let src = "a = 1\nbé = 2\n";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 6), (2, 1));
        assert_eq!(line_col(src, src.find('=').unwrap()), (1, 3));
        assert_eq!(line_col(src, src.rfind('=').unwrap()), (2, 4));
    }
}
