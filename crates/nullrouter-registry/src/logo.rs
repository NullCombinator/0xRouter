//! Plugin logos (spec 009 research R10, contracts/plugin-logo.md). A plugin only names its logo
//! file; the core finds it in the `logos/` directory beside the plugin's set, checks it, and keeps
//! the bytes of one that passes. The check reads the file's length and its first 33 bytes (the
//! PNG signature and the `IHDR` chunk) and never decodes the image, so the core needs no image
//! library. A logo that fails is ignored: the plugin loads without it.

use std::borrow::Cow;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/logos.rs"));

/// The directory, beside a plugin set, that holds its logos.
pub const DIR: &str = "logos";

/// At most 64 KiB.
pub const MAX_BYTES: u64 = 64 * 1024;

/// Width and height each at most 256 px.
pub const MAX_SIDE: u32 = 256;

/// What the check reads: the 8-byte signature, then the `IHDR` chunk's length, type, width and
/// height, bit depth, colour type, compression, filter and interlace bytes.
const HEAD: usize = 33;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The first chunk: 13 data bytes, type `IHDR`.
const IHDR: [u8; 8] = [0, 0, 0, 13, b'I', b'H', b'D', b'R'];

/// The rule a `logo` value breaks, after the value.
pub const NAME_RULE: &str = "must be a bare file name ending in .png: no /, \\ or ..";

/// Whether `name` may be a `logo` value: a bare file name ending in `.png`, with no `/`, `\` or
/// `..`, so it can only name a file in the logos directory itself.
pub fn is_file_name(name: &str) -> bool {
    name.len() > ".png".len()
        && name.ends_with(".png")
        && !name.contains(['/', '\\'])
        && !name.contains("..")
        && !name.chars().any(char::is_control)
}

/// Why a logo was ignored. Its `Display` is the reason `check` prints after
/// `note: logo ignored: <id>: `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogoProblem {
    /// The declared file isn't in the logos directory (or isn't a file).
    NotFound(String),
    /// The file couldn't be read.
    Unreadable { file: String, error: String },
    /// Over [`MAX_BYTES`]; the file's length.
    TooLarge(u64),
    /// No PNG signature, or no `IHDR` chunk first.
    NotPng,
    /// Width or height over [`MAX_SIDE`].
    TooWide { width: u32, height: u32 },
}

impl fmt::Display for LogoProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(file) => write!(f, "file not found: {DIR}/{file}"),
            Self::Unreadable { file, error } => write!(f, "can't read {DIR}/{file}: {error}"),
            Self::TooLarge(len) => write!(f, "{} KiB, over 64 KiB", len.div_ceil(1024)),
            Self::NotPng => f.write_str("not a PNG"),
            Self::TooWide { width, height } => write!(f, "{width} × {height} px, over {MAX_SIDE} px"),
        }
    }
}

/// The four checks on a logo `len` bytes long that starts with `head` (contracts/plugin-logo.md),
/// in the contract's order after "the file exists": size, PNG signature and `IHDR`, then width
/// and height. Reads at most the first 33 bytes of `head`. Returns the width and height.
pub fn check(len: u64, head: &[u8]) -> Result<(u32, u32), LogoProblem> {
    if len > MAX_BYTES {
        return Err(LogoProblem::TooLarge(len));
    }
    let Some(h) = head.get(..HEAD) else { return Err(LogoProblem::NotPng) };
    if h[..8] != SIGNATURE || h[8..16] != IHDR {
        return Err(LogoProblem::NotPng);
    }
    let width = u32::from_be_bytes([h[16], h[17], h[18], h[19]]);
    let height = u32::from_be_bytes([h[20], h[21], h[22], h[23]]);
    // PNG forbids a zero side, so a header that says so is not a PNG.
    if width == 0 || height == 0 {
        return Err(LogoProblem::NotPng);
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(LogoProblem::TooWide { width, height });
    }
    Ok((width, height))
}

/// The embedded logo `name` from `set` (the bundled or the community logos), checked.
pub fn embedded(set: &'static [(&'static str, &'static [u8])], name: &str) -> Result<&'static [u8], LogoProblem> {
    let (_, bytes) = set.iter().find(|(n, _)| *n == name).ok_or_else(|| LogoProblem::NotFound(name.to_owned()))?;
    check(bytes.len() as u64, bytes)?;
    Ok(*bytes)
}

/// The embedded community logo `name`, unchecked, for `plugins install` to copy.
pub fn community(name: &str) -> Option<&'static [u8]> {
    COMMUNITY_LOGOS.iter().find(|(n, _)| *n == name).map(|(_, bytes)| *bytes)
}

/// Reads `<dir>/<name>` and checks it. A file over the limit is refused on its length without
/// being read; otherwise the check runs on the first 33 bytes read, and only a file that passes
/// is read whole. The bytes returned are the ones checked.
pub fn read(dir: &Path, name: &str) -> Result<Vec<u8>, LogoProblem> {
    let path = dir.join(name);
    let unreadable = |e: io::Error| LogoProblem::Unreadable { file: name.to_owned(), error: e.to_string() };
    // Not a regular file (a directory, a FIFO that would block the open): as if missing.
    match fs::metadata(&path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(LogoProblem::NotFound(name.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(LogoProblem::NotFound(name.to_owned())),
        Err(e) => return Err(unreadable(e)),
    }
    let mut file = File::open(&path).map_err(unreadable)?;
    let len = file.metadata().map_err(unreadable)?.len();
    if len > MAX_BYTES {
        return Err(LogoProblem::TooLarge(len));
    }
    let mut bytes = Vec::with_capacity(HEAD);
    (&mut file).take(HEAD as u64).read_to_end(&mut bytes).map_err(unreadable)?;
    check(len, &bytes)?;
    // One byte past the limit is enough to tell a file that grew since its length was read.
    file.take(MAX_BYTES + 1 - bytes.len() as u64).read_to_end(&mut bytes).map_err(unreadable)?;
    check(bytes.len() as u64, &bytes)?;
    Ok(bytes)
}

/// A logo that passed the check, as the registry snapshot keeps it.
#[derive(Clone, PartialEq, Eq)]
pub struct Logo {
    bytes: Cow<'static, [u8]>,
    hash: String,
}

impl Logo {
    pub(crate) fn new(bytes: Cow<'static, [u8]>) -> Self {
        let hash = content_hash(&bytes);
        Self { bytes, hash }
    }

    /// The PNG file, at most 64 KiB.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// A hash of the bytes (16 hex digits), for an address that changes when the logo does.
    pub fn hash(&self) -> &str {
        &self.hash
    }
}

impl fmt::Debug for Logo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logo").field("len", &self.bytes.len()).field("hash", &self.hash).finish()
    }
}

/// FNV-1a, 64 bits: enough to tell one version of a provider's logo from the next.
fn content_hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::PluginSource;
    use crate::validate::validate;

    /// A PNG header of `width` × `height` (33 bytes), padded with zeros to `len` bytes.
    fn png(width: u32, height: u32, len: usize) -> Vec<u8> {
        let mut b = SIGNATURE.to_vec();
        b.extend_from_slice(&IHDR);
        b.extend_from_slice(&width.to_be_bytes());
        b.extend_from_slice(&height.to_be_bytes());
        // Bit depth, colour type, compression, filter, interlace; then the CRC, which isn't checked.
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b.resize(len.max(b.len()), 0);
        b
    }

    fn in_dir(files: &[(&str, &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (name, bytes) in files {
            fs::write(dir.path().join(name), bytes).unwrap();
        }
        dir
    }

    #[test]
    fn a_valid_png_passes() {
        let bytes = png(128, 64, 4096);
        assert_eq!(check(bytes.len() as u64, &bytes), Ok((128, 64)));
        let dir = in_dir(&[("ok.png", &bytes)]);
        assert_eq!(read(dir.path(), "ok.png"), Ok(bytes));
        let edge = png(256, 256, 1);
        assert_eq!(check(edge.len() as u64, &edge), Ok((256, 256)));
        let tiny = png(1, 1, 1);
        assert_eq!(check(tiny.len() as u64, &tiny), Ok((1, 1)));
    }

    #[test]
    fn at_most_65536_bytes() {
        let at = png(16, 16, 65_536);
        assert_eq!(check(at.len() as u64, &at), Ok((16, 16)));
        let over = png(16, 16, 65_537);
        assert_eq!(check(over.len() as u64, &over), Err(LogoProblem::TooLarge(65_537)));
        assert_eq!(LogoProblem::TooLarge(65_537).to_string(), "65 KiB, over 64 KiB");
        assert_eq!(LogoProblem::TooLarge(790_000).to_string(), "772 KiB, over 64 KiB");
        let dir = in_dir(&[("at.png", &at), ("over.png", &over)]);
        assert_eq!(read(dir.path(), "at.png").map(|b| b.len()), Ok(65_536));
        assert_eq!(read(dir.path(), "over.png"), Err(LogoProblem::TooLarge(65_537)));
    }

    #[test]
    fn a_jpeg_named_png_is_not_a_png() {
        let jpeg: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0];
        let mut jpeg = jpeg.to_vec();
        jpeg.resize(2048, 0);
        assert_eq!(check(jpeg.len() as u64, &jpeg), Err(LogoProblem::NotPng));
        let dir = in_dir(&[("photo.png", &jpeg)]);
        let got = read(dir.path(), "photo.png").unwrap_err();
        assert_eq!((got.clone(), got.to_string()), (LogoProblem::NotPng, "not a PNG".to_owned()));
        // A signature alone, a chunk other than IHDR first, and a zero side aren't PNGs either.
        let full = png(16, 16, 1);
        assert_eq!(full.len(), HEAD);
        let short = &full[..20];
        assert_eq!(check(short.len() as u64, short), Err(LogoProblem::NotPng));
        let mut other = png(16, 16, 64);
        other[12..16].copy_from_slice(b"tEXt");
        assert_eq!(check(64, &other), Err(LogoProblem::NotPng));
        assert_eq!(check(64, &png(0, 16, 64)), Err(LogoProblem::NotPng));
    }

    #[test]
    fn over_256_px_on_a_side() {
        let wide = png(257, 10, 64);
        let err = check(64, &wide).unwrap_err();
        assert_eq!(err, LogoProblem::TooWide { width: 257, height: 10 });
        assert_eq!(err.to_string(), "257 × 10 px, over 256 px");
        assert_eq!(check(64, &png(10, 257, 64)).unwrap_err().to_string(), "10 × 257 px, over 256 px");
        assert_eq!(check(64, &png(2700, 1392, 64)).unwrap_err().to_string(), "2700 × 1392 px, over 256 px");
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let dir = in_dir(&[]);
        let err = read(dir.path(), "gone.png").unwrap_err();
        assert_eq!(err.to_string(), "file not found: logos/gone.png");
        fs::create_dir(dir.path().join("folder.png")).unwrap();
        assert_eq!(read(dir.path(), "folder.png"), Err(LogoProblem::NotFound("folder.png".into())));
        assert_eq!(embedded(&[], "x.png"), Err(LogoProblem::NotFound("x.png".into())));
    }

    #[test]
    fn the_logo_field_is_a_bare_png_name() {
        let src = |logo: &str| format!("schema = 2\nid = \"p\"\ncategory = \"apikey\"\nlogo = {logo:?}\n");
        let ok = validate(&src("p.png"), PluginSource::Bundled, "t.toml").unwrap();
        assert_eq!(ok.logo.as_deref(), Some("p.png"));
        let schema1 = "schema = 1\nid = \"p\"\ncategory = \"apikey\"\nlogo = \"p.png\"\n";
        assert!(validate(schema1, PluginSource::Bundled, "t.toml").is_ok(), "generated community plugins carry it");
        let bare = "schema = 2\nid = \"p\"\ncategory = \"apikey\"\n";
        assert_eq!(validate(bare, PluginSource::Bundled, "t.toml").unwrap().logo, None);
        for bad in ["../x.png", "a/b.png", "x.svg", "a\\b.png", "..png", ".png", "x.PNG", "x.png\n"] {
            let errs = validate(&src(bad), PluginSource::Bundled, "t.toml").unwrap_err();
            assert_eq!(errs.len(), 1, "{bad:?}: {errs:#?}");
            assert_eq!(errs[0].path.to_string(), "logo", "{bad:?}");
            assert!(errs[0].rule.ends_with(NAME_RULE), "{bad:?}: {}", errs[0].rule);
            assert_eq!(errs[0].line, 4, "{bad:?}: positioned on the field");
        }
    }

    #[test]
    fn the_hash_follows_the_bytes() {
        let a = Logo::new(Cow::Owned(png(16, 16, 64)));
        let b = Logo::new(Cow::Owned(png(16, 17, 64)));
        assert_eq!(a.hash().len(), 16);
        assert_ne!(a.hash(), b.hash());
        assert_eq!(a.hash(), Logo::new(Cow::Owned(png(16, 16, 64))).hash());
        assert!(format!("{a:?}").contains("len: 64"), "{a:?}");
    }

    /// Every shipped logo passes the check the core runs at load.
    #[test]
    fn every_embedded_logo_passes() {
        for (name, bytes) in BUNDLED_LOGOS.iter().chain(COMMUNITY_LOGOS) {
            assert!(is_file_name(name), "{name}");
            assert!(check(bytes.len() as u64, bytes).is_ok(), "{name}: {:?}", check(bytes.len() as u64, bytes));
        }
    }
}
