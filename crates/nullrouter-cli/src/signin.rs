//! `nullrouter accounts signin` (contracts/operator-cli.md § `accounts signin` output; research
//! R3, R4, R5). The engine runs the flow and writes nothing; this module talks to the operator,
//! then writes the token entry (under `tokens.lock`) and the account, and asks a running
//! server to reload.
//!
//! Nothing here prints a token, code or verifier. The email is shown shortened.

use std::cell::Cell;
use std::io::Write;
use std::time::{Duration, SystemTime};

use nullrouter_engine::accounts::{self, Account, AccountKind, Accounts};
use nullrouter_engine::signin::{self, Begun, DeviceSignIn, PkceSignIn, SignInError, SignInHttp, printable};
use nullrouter_engine::tokens::{self, TokenEntry};
use nullrouter_registry::schema::RedirectKind;
use nullrouter_registry::{OperatorHome, ProviderEntity, SecretString};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio_util::sync::CancellationToken;

/// Exit code for a sign-in that was refused, expired or abandoned.
pub const EXIT_ENDED: u8 = 5;
/// Exit code for invalid input or a file that can't be read or written.
pub const EXIT_INVALID: u8 = 1;

/// Research R4, shown before the link at every sign-in of a provider that declares
/// `terms_warning` (Clarifications Q2).
pub const TERMS_WARNING: &str = "Anthropic's terms limit the use of Claude Pro/Max subscriptions outside Anthropic's own\n\
apps. Anthropic may refuse these requests or act on your account. You carry that risk.\n\
Continue? [y/N] ";

/// The prompt for a pasted redirect address or code.
pub const PASTE_PROMPT: &str = "Paste the address or code: ";

/// One sign-in, as the operator asked for it.
pub struct SignIn<'a> {
    pub home: &'a OperatorHome,
    pub provider: &'a ProviderEntity,
    pub name: &'a str,
    pub http: &'a SignInHttp,
    /// `--accept-terms-risk`: answers the terms question; the warning is still printed.
    pub accept_terms_risk: bool,
    /// Opens a link in the browser; `None` with no display, `--no-browser` or `--paste`.
    pub browser: Option<&'a dyn Fn(&str)>,
}

/// A finished sign-in, written and applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Done {
    pub provider: String,
    pub name: String,
    /// Shortened (`a…@example.com`).
    pub email: Option<String>,
    pub tier: Option<String>,
    /// The kind of the account this one replaced.
    pub replaced: Option<AccountKind>,
    /// `applied`, or `saved; applies at next start`.
    pub status: &'static str,
}

impl Done {
    /// `xai/main: signed in as a…@example.com (SuperGrok); applied`. The provider's email
    /// and tier lose any control characters.
    pub fn line(&self) -> String {
        let who = self.email.as_deref().map_or_else(|| "account".to_owned(), printable);
        let tier = self.tier.as_deref().map(|t| format!(" ({})", printable(t))).unwrap_or_default();
        let replaced = match self.replaced {
            Some(AccountKind::Signin) => "replaced the earlier sign-in; ",
            Some(AccountKind::Key) => "replaced the key account; ",
            None => "",
        };
        format!("{}/{}: signed in as {who}{tier}; {replaced}{}", self.provider, self.name, self.status)
    }
}

/// Why a sign-in ended without being written (or, for a reload refusal, after).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub code: u8,
    pub message: String,
}

/// `a…@example.com`: the first character of the local part, then the domain.
pub fn short_email(email: &str) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    let first: String = local.chars().take(1).collect();
    if domain.is_empty() { format!("{first}…") } else { format!("{first}…@{domain}") }
}

/// Whether a link can be opened here: macOS, or a graphical session on other systems.
pub fn has_display() -> bool {
    cfg!(target_os = "macos")
        || ["DISPLAY", "WAYLAND_DISPLAY"].iter().any(|v| std::env::var_os(v).is_some_and(|s| !s.is_empty()))
}

/// Whether a link from a provider may be handed to the browser (security review L2): an
/// `https` link with no control characters or spaces. Any other link (`file:`, a custom
/// scheme) is only printed.
pub fn openable(link: &str) -> bool {
    link.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://"))
        && !link.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// Opens `url` with `open` (macOS) or `xdg-open`, ignoring any failure: the link is printed
/// either way. Only [`openable`] links are opened.
pub fn open_browser(url: &str) {
    if !openable(url) {
        return;
    }
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

macro_rules! say {
    ($out:expr, $($arg:tt)*) => {{
        let _ = write!($out, $($arg)*);
        let _ = $out.flush();
    }};
}

/// `in 15 min`, or seconds under a minute.
fn expires_in(at: SystemTime) -> String {
    let left = at.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO).as_secs();
    if left < 60 { format!("{left} s") } else { format!("{} min", (left + 30) / 60) }
}

impl SignIn<'_> {
    fn ended(&self, code: u8, reason: impl std::fmt::Display) -> Ended {
        // The reason may carry a provider's error code: no control characters (L1).
        let reason = printable(&reason.to_string());
        Ended { code, message: format!("{}/{}: sign-in ended: {reason}; nothing saved", self.provider.id, self.name) }
    }

    /// Hands `link` to the browser when there is one and the link is [`openable`];
    /// otherwise says why it wasn't opened (the link is printed already).
    fn open(&self, link: &str, out: &mut impl Write) {
        let Some(open) = self.browser else { return };
        if openable(link) {
            open(link);
        } else {
            say!(out, "(Not opened in the browser: not an https link.)\n");
        }
    }

    fn failed(&self, e: &SignInError) -> Ended {
        match e {
            SignInError::Denied(code) => self.ended(EXIT_ENDED, code),
            SignInError::Expired => self.ended(EXIT_ENDED, "the code expired"),
            SignInError::TimedOut(d) => self.ended(EXIT_ENDED, format!("no answer within {} min", d.as_secs() / 60)),
            SignInError::Cancelled => self.ended(EXIT_ENDED, "cancelled"),
            SignInError::NoInput => self.ended(EXIT_ENDED, "no address or code was pasted"),
            SignInError::Rejected { code, .. } => self.ended(EXIT_ENDED, code),
            e => self.ended(EXIT_INVALID, e),
        }
    }

    /// Runs the sign-in: reads operator lines from `input` (`None` once input ended), writes
    /// prompts to `out`, and stops with nothing written when `cancel` fires.
    pub async fn run(
        &self,
        input: &mut UnboundedReceiver<String>,
        out: &mut impl Write,
        cancel: &CancellationToken,
    ) -> Result<Done, Ended> {
        let (pid, name) = (self.provider.id.as_str(), self.name);
        if !accounts::valid_name(name) {
            return Err(Ended {
                code: EXIT_INVALID,
                message: accounts::AccountError::BadName(name.into()).to_string(),
            });
        }
        let Some(decl) = &self.provider.signin else {
            return Err(Ended {
                code: EXIT_INVALID,
                message: format!("{pid} has no sign-in; add a key with `nullrouter accounts add {pid} {name}`"),
            });
        };
        let file = self.home.path().join(accounts::FILE);
        let invalid = |e: &dyn std::fmt::Display| Ended { code: EXIT_INVALID, message: e.to_string() };
        // Read now, so a broken file stops the sign-in before the operator approves anything.
        let replaced = Accounts::load(&file).map_err(|e| invalid(&e))?.get(pid, name).map(|a| a.kind);

        if decl.terms_warning {
            say!(out, "{TERMS_WARNING}");
            if self.accept_terms_risk {
                say!(out, "y (--accept-terms-risk)\n");
            } else {
                let answer = tokio::select! {
                    () = cancel.cancelled() => return Err(self.failed(&SignInError::Cancelled)),
                    line = input.recv() => line.unwrap_or_default(),
                };
                if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                    return Err(self.ended(EXIT_ENDED, "the terms risk was not accepted"));
                }
            }
        }

        let begun = tokio::select! {
            () = cancel.cancelled() => return Err(self.failed(&SignInError::Cancelled)),
            b = signin::begin(self.http, self.provider) => b.map_err(|e| self.failed(&e))?,
        };
        let entry = match begun {
            Begun::Device(d) => self.device(d, out, cancel).await?,
            Begun::Pkce(p) => self.pkce(p, input, out, cancel).await?,
        };
        let email = entry.claims.email.as_deref().map(short_email);
        let tier = entry.claims.tier.clone();
        self.save(entry).map_err(|e| invalid(&e))?;
        let status = crate::apply(self.home).map_err(|e| Ended { code: EXIT_INVALID, message: e })?;
        Ok(Done { provider: pid.into(), name: name.into(), email, tier, replaced, status })
    }

    async fn device(
        &self,
        mut d: DeviceSignIn,
        out: &mut impl Write,
        cancel: &CancellationToken,
    ) -> Result<TokenEntry, Ended> {
        // The device endpoint's text is the provider's: printed without control characters.
        say!(out, "Open this page on any device and approve:\n  {}\n", printable(d.link()));
        say!(out, "Code: {} (expires in {})\n", printable(&d.user_code), expires_in(d.expires_at));
        self.open(d.link(), out);
        say!(out, "Waiting for approval… ");
        match d.poll(self.http, self.name, cancel).await {
            Ok(e) => {
                say!(out, "done.\n");
                Ok(e)
            }
            Err(e) => {
                say!(out, "\n");
                Err(self.failed(&e))
            }
        }
    }

    async fn pkce(
        &self,
        mut p: PkceSignIn,
        input: &mut UnboundedReceiver<String>,
        out: &mut impl Write,
        cancel: &CancellationToken,
    ) -> Result<TokenEntry, Ended> {
        say!(out, "Open this link in a browser (on any device):\n  {}\n", printable(p.authorize_url()));
        match p.redirect_kind() {
            RedirectKind::CodePage => say!(out, "After you approve, the page shows a code; paste it here.\n"),
            RedirectKind::Loopback => {
                say!(
                    out,
                    "If the browser ends on a page that doesn't load, copy its full address and paste it here.\n"
                );
                if let Some(why) = p.port_busy() {
                    say!(out, "(No local listener: {why}.)\n");
                }
            }
        }
        self.open(p.authorize_url(), out);
        let code: SecretString = loop {
            say!(out, "{PASTE_PROMPT}");
            let pasted = Cell::new(false);
            let line = async {
                let l = input.recv().await;
                pasted.set(true);
                l
            };
            match p.wait_code(line, cancel).await {
                Ok(code) => {
                    if !pasted.get() {
                        say!(out, "received from the browser.\n");
                    }
                    break code;
                }
                Err(SignInError::Paste(e)) => {
                    say!(out, "That didn't work: {}. Try again.\n", printable(&e.to_string()))
                }
                Err(e) => {
                    say!(out, "\n");
                    return Err(self.failed(&e));
                }
            }
        };
        tokio::select! {
            () = cancel.cancelled() => Err(self.failed(&SignInError::Cancelled)),
            r = p.complete(self.http, self.name, code) => r.map_err(|e| self.failed(&e)),
        }
    }

    /// Writes the token entry under the writers' lock, then the account (`kind = "signin"`).
    /// Tokens go first: an entry without its account is ignored, an account without its
    /// tokens would need signing in again.
    fn save(&self, entry: TokenEntry) -> Result<(), String> {
        let (pid, name) = (self.provider.id.as_str(), self.name);
        tokens::update(self.home.path(), pid, name, |slot| *slot = Some(entry)).map_err(|e| e.to_string())?;
        let mut list = Accounts::load(&self.home.path().join(accounts::FILE)).map_err(|e| e.to_string())?;
        // A replaced account keeps its place; a new one goes after the provider's others.
        let order = list
            .get(pid, name)
            .map(|a| a.order)
            .or_else(|| list.iter().filter(|a| a.provider == pid).map(|a| a.order).max())
            .unwrap_or(0);
        let mut account = Account::signin(pid, name, order);
        account.poll_interval = list.get(pid, name).and_then(|a| a.poll_interval);
        list.add(account).map_err(|e| e.to_string())?;
        list.save().map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_links_are_opened() {
        assert!(openable("https://accounts.x.ai/device?user_code=ABCD"));
        assert!(openable("HTTPS://auth.example/a"));
        for bad in
            ["http://127.0.0.1:1/device", "file:///etc/passwd", "x-custom:open", "https://a\u{1b}[2J", "https://a b"]
        {
            assert!(!openable(bad), "{bad:?}");
        }
    }

    #[test]
    fn provider_text_in_the_done_line_loses_control_characters() {
        let d = Done {
            provider: "xai".into(),
            name: "main".into(),
            email: Some("a\u{1b}[2J…@example.com".into()),
            tier: Some("Super\u{7}Grok".into()),
            replaced: None,
            status: "applied",
        };
        assert_eq!(d.line(), "xai/main: signed in as a[2J…@example.com (SuperGrok); applied");
    }

    #[test]
    fn emails_are_shortened() {
        assert_eq!(short_email("alice@example.com"), "a…@example.com");
        assert_eq!(short_email("bob"), "b…");
    }

    #[test]
    fn the_done_line_names_tier_and_replacement() {
        let d = Done {
            provider: "grok-cli".into(),
            name: "work".into(),
            email: Some("a…@example.com".into()),
            tier: Some("SuperGrok".into()),
            replaced: None,
            status: "applied",
        };
        assert_eq!(d.line(), "grok-cli/work: signed in as a…@example.com (SuperGrok); applied");
        let d = Done { email: None, tier: None, replaced: Some(AccountKind::Signin), ..d };
        assert_eq!(d.line(), "grok-cli/work: signed in as account; replaced the earlier sign-in; applied");
    }
}
