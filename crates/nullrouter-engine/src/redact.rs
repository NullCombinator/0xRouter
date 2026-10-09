//! Secret redaction (research R23): every account secret and anything shaped like an agent
//! key becomes `***` in error bodies, log lines and record fields. Sign-in tokens are
//! masked too, the current generation and the one before it (spec 005, research R9), so a
//! token echoed after a refresh is still caught.
//!
//! Agent keys are stored as digests, so they are matched by shape (`0r-` + 43 base64url
//! characters) rather than by value.

use std::borrow::Cow;
use std::io::{self, Write};
use std::sync::Arc;

use aho_corasick::{AhoCorasick, MatchKind};
use arc_swap::ArcSwap;
use nullrouter_registry::SecretString;
use tracing_subscriber::fmt::MakeWriter;

use crate::accounts::Accounts;
use crate::keys::PREFIX;
use crate::tokens::TokenCells;

pub const MASK: &str = "***";
/// Shorter secrets would mask ordinary words.
pub const MIN_SECRET_LEN: usize = 8;
const KEY_BODY_LEN: usize = 43;

#[derive(Debug, Default)]
pub struct Redactor {
    secrets: Option<AhoCorasick>,
}

fn key_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

impl Redactor {
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a SecretString>) -> Self {
        let patterns: Vec<String> =
            secrets.into_iter().filter(|s| s.len() >= MIN_SECRET_LEN).map(|s| s.with_exposed(str::to_owned)).collect();
        let secrets = (!patterns.is_empty()).then(|| {
            AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .build(&patterns)
                .expect("literal patterns build")
        });
        Self { secrets }
    }

    /// Every key account's secret, plus the current and previous access and refresh tokens
    /// of every sign-in account.
    pub fn for_state(accounts: &Accounts, tokens: &TokenCells) -> Self {
        Self::for_state_with(accounts, tokens, &[])
    }

    /// [`for_state`](Self::for_state), plus `extra` (the proxies' passwords and usernames).
    pub fn for_state_with(accounts: &Accounts, tokens: &TokenCells, extra: &[SecretString]) -> Self {
        let views = tokens.views();
        Self::new(
            accounts
                .iter()
                .filter_map(|a| a.secret.as_ref())
                .chain(views.iter().flat_map(|v| v.secrets()))
                .chain(extra),
        )
    }

    pub fn redact<'t>(&self, text: &'t str) -> Cow<'t, str> {
        let text = match &self.secrets {
            Some(ac) if ac.is_match(text) => Cow::Owned(ac.replace_all(text, &[MASK].repeat(ac.patterns_len()))),
            _ => Cow::Borrowed(text),
        };
        match mask_keys(&text) {
            Some(masked) => Cow::Owned(masked),
            None => text,
        }
    }

    /// [`redact`](Self::redact), plus `key=` query values (Gemini's key carrier).
    pub fn redact_url<'t>(&self, url: &'t str) -> Cow<'t, str> {
        let url = self.redact(url);
        let Some(q) = url.find('?') else { return url };
        let (head, query) = url.split_at(q + 1);
        let mut changed = false;
        let parts: Vec<Cow<str>> = query
            .split('&')
            .map(|p| match p.split_once('=') {
                Some(("key", v)) if v != MASK => {
                    changed = true;
                    Cow::Owned(format!("key={MASK}"))
                }
                _ => Cow::Borrowed(p),
            })
            .collect();
        if !changed {
            return url;
        }
        Cow::Owned(format!("{head}{}", parts.join("&")))
    }
}

/// A snapshot's redactor, swappable on its own so a token refresh can extend it without a
/// full reload (`Engine::rebuild_redactor`).
#[derive(Debug, Default)]
pub struct SharedRedactor(ArcSwap<Redactor>);

impl SharedRedactor {
    pub fn new(r: Redactor) -> Self {
        Self(ArcSwap::from_pointee(r))
    }

    /// The redactor now, for callers that take `&Redactor`.
    pub fn current(&self) -> Arc<Redactor> {
        self.0.load_full()
    }

    pub fn store(&self, r: Arc<Redactor>) {
        self.0.store(r);
    }

    /// [`Redactor::redact`] with the current redactor.
    pub fn redact<'t>(&self, text: &'t str) -> Cow<'t, str> {
        self.0.load().redact(text)
    }

    /// [`Redactor::redact_url`] with the current redactor.
    pub fn redact_url<'t>(&self, url: &'t str) -> Cow<'t, str> {
        self.0.load().redact_url(url)
    }
}

/// `text` with agent-key-shaped tokens masked, or `None` when there are none.
fn mask_keys(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut from = 0;
    while let Some(at) = text[from..].find(PREFIX).map(|i| i + from) {
        let body = at + PREFIX.len();
        let len = bytes[body..].iter().take_while(|b| key_char(**b)).count();
        let starts_token = at == 0 || !key_char(bytes[at - 1]);
        if starts_token && len == KEY_BODY_LEN {
            let o = out.get_or_insert_with(String::new);
            o.push_str(&text[copied..at]);
            o.push_str(MASK);
            copied = body + len;
        }
        from = body + len;
    }
    out.map(|mut o| {
        o.push_str(&text[copied..]);
        o
    })
}

/// A `tracing_subscriber` writer that passes each formatted event through the current
/// redactor. Use with `tracing_subscriber::fmt::layer().with_writer(...)`.
#[derive(Clone)]
pub struct RedactWriter<W> {
    redactor: Arc<ArcSwap<Redactor>>,
    inner: W,
}

impl<W> RedactWriter<W> {
    pub fn new(redactor: Arc<ArcSwap<Redactor>>, inner: W) -> Self {
        Self { redactor, inner }
    }
}

/// One event's bytes, redacted and written when the formatter drops it.
pub struct EventBuf<'r, W: Write> {
    redactor: &'r ArcSwap<Redactor>,
    buf: Vec<u8>,
    out: W,
}

impl<W: Write> Write for EventBuf<'_, W> {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return self.out.flush();
        }
        let text = String::from_utf8_lossy(&self.buf);
        let redacted = self.redactor.load().redact(&text).into_owned();
        self.buf.clear();
        self.out.write_all(redacted.as_bytes())?;
        self.out.flush()
    }
}

impl<W: Write> Drop for EventBuf<'_, W> {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

impl<'a, W: MakeWriter<'a>> MakeWriter<'a> for RedactWriter<W> {
    type Writer = EventBuf<'a, W::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        EventBuf { redactor: &self.redactor, buf: Vec::new(), out: self.inner.make_writer() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn redactor() -> Redactor {
        let s = [
            SecretString::new("sk-ant-SENTINEL-1"),
            SecretString::new("short"),
            SecretString::new("sk-ant-SENTINEL-12"),
        ];
        Redactor::new(&s)
    }

    #[test]
    fn masks_secrets_and_key_shapes() {
        let r = redactor();
        assert_eq!(r.redact("bad key sk-ant-SENTINEL-12!"), "bad key ***!");
        assert_eq!(r.redact("short stays"), "short stays");
        let key = crate::keys::generate();
        assert_eq!(r.redact(&format!("Bearer {key}.")), "Bearer ***.");
        assert_eq!(r.redact("0r-tooshort"), "0r-tooshort");
        assert!(matches!(r.redact("nothing here"), Cow::Borrowed(_)));
        assert_eq!(r.redact_url("https://h/v1?alt=sse&key=AIzaXYZ&x=1"), "https://h/v1?alt=sse&key=***&x=1");
        assert_eq!(Redactor::default().redact("sk-ant-SENTINEL-1"), "sk-ant-SENTINEL-1");
    }

    #[test]
    fn masks_both_token_generations() {
        let tokens = TokenCells::default();
        tokens.replace(crate::tokens::tests::entry("xai", "main", "xai-gen1-SENTINEL", &[]));
        tokens.replace(crate::tokens::tests::entry("xai", "main", "xai-gen2-SENTINEL", &[]));
        tokens.replace(crate::tokens::tests::entry("xai", "main", "xai-gen3-SENTINEL", &[]));
        let accounts = Accounts::parse(
            "schema = 2\n[[account]]\nprovider = \"a\"\nname = \"m\"\nsecret = \"sk-key-SENTINEL\"\n",
            std::path::Path::new("accounts.toml"),
            |_| None,
        )
        .unwrap();
        let r = SharedRedactor::new(Redactor::for_state(&accounts, &tokens));
        let text = "a xai-gen3-SENTINEL b xai-gen2-SENTINEL-refresh c sk-key-SENTINEL d xai-gen1-SENTINEL";
        assert_eq!(r.redact(text), "a *** b *** c *** d xai-gen1-SENTINEL", "two generations, not three");
    }

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Sink {
        type Writer = Sink;
        fn make_writer(&'a self) -> Sink {
            self.clone()
        }
    }

    #[test]
    fn log_lines_go_through_the_current_redactor() {
        let current = Arc::new(ArcSwap::from_pointee(Redactor::default()));
        let sink = Sink::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(RedactWriter::new(current.clone(), sink.clone()))
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(sub, || {
            tracing::warn!("upstream said sk-ant-SENTINEL-1");
            current.store(Arc::new(redactor()));
            tracing::warn!("upstream said sk-ant-SENTINEL-1");
        });
        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].ends_with("sk-ant-SENTINEL-1"), "{out}");
        assert!(lines[1].ends_with("upstream said ***"), "{out}");
    }
}
