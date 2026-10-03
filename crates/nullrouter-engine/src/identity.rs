//! Client identity headers (research R7): the installation id and the closed set of values
//! the core fills for `[identity]` placeholders (spec Clarifications Q5).
//!
//! Every value is true information the core owns. Nothing here hashes the request, copies
//! another machine's id or lets a plugin compute a value.

use std::borrow::Cow;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Mutex;

use indexmap::IndexMap;
use nullrouter_registry::schema::{HeaderValue, IdentityDecl, Placeholder};
use nullrouter_wire::ir::{self, Part, Role};

use crate::files::{self, FileError};
use crate::keys::AgentId;
use crate::redact::Redactor;
use crate::tokens::Claims;

pub const INSTALL_ID_FILE: &str = "install-id";
/// Agents remembered for `{session.id}` before the oldest is forgotten.
pub const MAX_AGENT_SESSIONS: usize = 4096;

/// A random UUID v4, lower-case and hyphenated.
pub fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("the OS random source is available");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// This installation's id: `home/install-id` (mode 0600), created with a random UUID v4
/// the first time and read after. Two processes creating it at once agree: the file is
/// linked into place only if absent.
pub fn install_id(home: &Path) -> Result<String, FileError> {
    let path = home.join(INSTALL_ID_FILE);
    if let Some(text) = files::read_private(&path)? {
        let id = text.trim();
        return if looks_like_uuid(id) {
            Ok(id.to_owned())
        } else {
            Err(FileError::invalid(&path, "not a UUID; delete the file to create a new installation id"))
        };
    }
    let io = |source| FileError::Io { path: path.clone(), source };
    fs::create_dir_all(home).map_err(io)?;
    let id = uuid_v4();
    let tmp = home.join(format!(".{INSTALL_ID_FILE}.tmp-{}-{}", std::process::id(), &id[..8]));
    let written = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
        f.write_all(format!("{id}\n").as_bytes())?;
        f.sync_all()?;
        fs::hard_link(&tmp, &path)
    })();
    let _ = fs::remove_file(&tmp);
    match written {
        Ok(()) => Ok(id),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => install_id(home),
        Err(e) => Err(io(e)),
    }
}

/// `{session.id}` for agents that send no session: a random id per agent key, kept in
/// memory (oldest forgotten past [`MAX_AGENT_SESSIONS`]).
#[derive(Debug, Default)]
pub struct AgentSessions {
    ids: Mutex<IndexMap<String, String>>,
}

impl AgentSessions {
    /// The agent's session id when it sent one (slice 003), else its kept random id.
    pub fn id_for(&self, agent: &AgentId) -> String {
        if let Some(s) = &agent.session {
            return s.clone();
        }
        self.kept_id(agent)
    }

    /// [`id_for`](Self::id_for) for an upstream header: the client's session value passes
    /// the same checks a same-style forwarded header does (no control characters, no
    /// account secret, nothing shaped like an agent key); one that fails is replaced by the
    /// agent's kept random id.
    pub fn id_for_upstream(&self, agent: &AgentId, redactor: &Redactor) -> String {
        match &agent.session {
            Some(s) if !s.chars().any(char::is_control) && matches!(redactor.redact(s), Cow::Borrowed(_)) => s.clone(),
            _ => self.kept_id(agent),
        }
    }

    fn kept_id(&self, agent: &AgentId) -> String {
        let mut ids = self.ids.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = ids.get(&agent.key) {
            return id.clone();
        }
        if ids.len() >= MAX_AGENT_SESSIONS {
            ids.shift_remove_index(0);
        }
        let id = uuid_v4();
        ids.insert(agent.key.clone(), id.clone());
        id
    }
}

/// The user turns in a request's conversation: user messages that carry more than tool
/// results (so every client style counts the same).
pub fn user_turns(req: &ir::Request) -> usize {
    req.messages
        .iter()
        .filter(|m| m.role == Role::User && m.parts.iter().any(|p| !matches!(p, Part::ToolResult { .. })))
        .count()
}

/// What the placeholders of one upstream request are filled from.
#[derive(Debug, Clone, Copy)]
pub struct FillContext<'a> {
    /// `{session.id}`: from [`AgentSessions::id_for`].
    pub session_id: &'a str,
    /// `{request.id}`: one fresh [`uuid_v4`] per upstream request.
    pub request_id: &'a str,
    /// `{session.turn}`: from [`user_turns`].
    pub turns: usize,
    /// `{model.upstream}`.
    pub upstream_model: &'a str,
    /// The signed-in account's claims; `None` for a key account.
    pub claims: Option<&'a Claims>,
    /// `{install.id}`: from [`install_id`].
    pub install_id: &'a str,
}

/// The value of `placeholder`, or `None` when the account has no such claim (the header
/// is then omitted).
pub fn fill(placeholder: Placeholder, ctx: &FillContext<'_>) -> Option<String> {
    let claim = |f: fn(&Claims) -> &Option<String>| ctx.claims.and_then(|c| f(c).clone()).filter(|v| !v.is_empty());
    match placeholder {
        Placeholder::SessionId => Some(ctx.session_id.to_owned()),
        Placeholder::RequestId => Some(ctx.request_id.to_owned()),
        Placeholder::SessionTurn => Some(ctx.turns.to_string()),
        Placeholder::ModelUpstream => Some(ctx.upstream_model.to_owned()),
        Placeholder::AccountEmail => claim(|c| &c.email),
        Placeholder::AccountUserId => claim(|c| &c.user_id),
        Placeholder::InstallId => Some(ctx.install_id.to_owned()),
    }
}

/// Every `[identity]` header with its value, in declared order; headers whose placeholder
/// has no value are left out.
pub fn headers<'d>(decl: &'d IdentityDecl, ctx: &FillContext<'_>) -> Vec<(&'d str, String)> {
    decl.headers
        .iter()
        .filter_map(|(name, v)| {
            let value = match v {
                HeaderValue::Fixed(s) => s.clone(),
                HeaderValue::Core(p) => fill(*p, ctx)?,
            };
            Some((name.as_str(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn install_id_is_created_once_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let id = install_id(&home).unwrap();
        assert!(looks_like_uuid(&id) && id.as_bytes()[14] == b'4', "{id}");
        let path = home.join(INSTALL_ID_FILE);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(install_id(&home).unwrap(), id, "read after");
        assert_eq!(fs::read_dir(&home).unwrap().count(), 1, "no temporary file left");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(install_id(&home).unwrap_err().to_string().contains("chmod 600"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(&path, "machine-id\n").unwrap();
        assert!(install_id(&home).is_err());
    }

    #[test]
    fn concurrent_creators_agree() {
        let dir = tempfile::tempdir().unwrap();
        let ids: Vec<String> = (0..4)
            .map(|_| {
                let home = dir.path().to_owned();
                std::thread::spawn(move || install_id(&home).unwrap())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|t| t.join().unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]), "{ids:?}");
    }

    #[test]
    fn session_ids_per_agent() {
        let s = AgentSessions::default();
        assert_eq!(s.id_for(&AgentId::new("ak_1", Some("sess-9"))), "sess-9");
        let a = s.id_for(&AgentId::new("ak_1", None));
        assert_eq!(s.id_for(&AgentId::new("ak_1", None)), a, "kept per agent");
        assert_ne!(s.id_for(&AgentId::new("ak_2", None)), a);
        for i in 0..MAX_AGENT_SESSIONS {
            s.id_for(&AgentId::new(format!("k{i}"), None));
        }
        assert_eq!(s.ids.lock().unwrap().len(), MAX_AGENT_SESSIONS, "capped");
        assert_ne!(s.id_for(&AgentId::new("ak_1", None)), a, "the oldest was forgotten");
    }

    /// The `{session.id}` value sent upstream passes the forwarding filter.
    #[test]
    fn a_client_session_carrying_a_secret_is_not_sent_upstream() {
        let s = AgentSessions::default();
        let secret = nullrouter_registry::SecretString::new("sk-ant-SENTINEL-SESSION");
        let r = Redactor::new([&secret]);
        assert_eq!(s.id_for_upstream(&AgentId::new("ak_1", Some("sess-9")), &r), "sess-9");
        let kept = s.id_for(&AgentId::new("ak_1", None));
        let key = format!("0r-{}", "A".repeat(43));
        for bad in ["x sk-ant-SENTINEL-SESSION", key.as_str(), "sess\u{1b}[2J"] {
            assert_eq!(s.id_for_upstream(&AgentId::new("ak_1", Some(bad)), &r), kept, "{bad:?}");
        }
    }

    #[test]
    fn fills_the_seven_placeholders() {
        let claims = Claims { email: Some("a@example.com".into()), user_id: None, tier: None };
        let ctx = FillContext {
            session_id: "sess-1",
            request_id: "req-1",
            turns: 3,
            upstream_model: "grok-4",
            claims: Some(&claims),
            install_id: "inst-1",
        };
        let got: Vec<Option<String>> = Placeholder::ALL.iter().map(|p| fill(*p, &ctx)).collect();
        let want = ["sess-1", "req-1", "3", "grok-4", "a@example.com", "", "inst-1"];
        for (p, (g, w)) in Placeholder::ALL.iter().zip(got.iter().zip(want)) {
            assert_eq!(g.as_deref(), (!w.is_empty()).then_some(w), "{p}");
        }
        assert_eq!(fill(Placeholder::AccountEmail, &FillContext { claims: None, ..ctx }), None);

        let decl: IdentityDecl = toml::from_str(
            "[headers]\nUser-Agent = \"grok-shell/1\"\nx-userid = \"{account.user_id}\"\nx-email = \"{account.email}\"\nx-grok-req-id = \"{request.id}\"\n",
        )
        .unwrap();
        assert_eq!(
            headers(&decl, &ctx),
            [
                ("User-Agent", "grok-shell/1".to_owned()),
                ("x-email", "a@example.com".into()),
                ("x-grok-req-id", "req-1".into())
            ],
            "a missing claim omits its header"
        );
    }

    #[test]
    fn counts_user_turns_not_tool_results() {
        let msg = |role, parts| ir::Message { role, parts };
        let result = Part::ToolResult {
            id: "t".into(),
            name: None,
            content: ir::ResultContent::Text(String::new()),
            is_error: false,
            cache_control: None,
        };
        let req = ir::Request {
            messages: vec![
                msg(Role::User, vec![Part::text("hi")]),
                msg(Role::Assistant, vec![Part::text("yo")]),
                msg(Role::User, vec![result]),
                msg(Role::User, vec![Part::text("again")]),
            ],
            ..Default::default()
        };
        assert_eq!(user_turns(&req), 2);
        assert!(uuid_v4() != uuid_v4());
    }
}
