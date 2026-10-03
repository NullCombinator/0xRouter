//! PKCE verifier, challenge, state and authorize URL; parsing what the operator pastes back.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nullrouter_registry::SecretString;
use nullrouter_registry::schema::{RedirectKind, SignInDecl, SignInParamValue};
use sha2::{Digest, Sha256};

fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("the OS random source is available");
    b
}

/// A PKCE verifier of `bytes` random bytes, base64url without padding (RFC 7636 § 4.1).
pub fn verifier(bytes: u32) -> SecretString {
    SecretString::new(URL_SAFE_NO_PAD.encode(random(bytes as usize)))
}

/// The S256 challenge of `verifier`.
pub fn challenge(verifier: &SecretString) -> String {
    verifier.with_exposed(|v| URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes())))
}

/// A random `state`: 32 bytes, base64url.
pub fn state() -> String {
    URL_SAFE_NO_PAD.encode(random(32))
}

/// `{random.hex16}`: 16 random bytes, hex.
pub fn hex16() -> String {
    random(16).iter().map(|b| format!("{b:02x}")).collect()
}

/// JavaScript's `encodeURIComponent`: spaces become `%20`, never `+`.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A declared parameter's value for this sign-in.
pub fn param_value(v: &SignInParamValue) -> String {
    match v {
        SignInParamValue::Fixed(s) => s.clone(),
        SignInParamValue::RandomHex16 => hex16(),
    }
}

/// `base?response_type=code&client_id&redirect_uri&scope&code_challenge&S256&state`, then
/// the declared `params` in declaration order.
pub fn authorize_url(base: &str, decl: &SignInDecl, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let scope = decl.scopes.join(" ");
    let mut pairs: Vec<(&str, String)> = vec![
        ("response_type", "code".into()),
        ("client_id", decl.client_id.clone()),
        ("redirect_uri", redirect_uri.into()),
        ("scope", scope),
        ("code_challenge", challenge.into()),
        ("code_challenge_method", "S256".into()),
        ("state", state.into()),
    ];
    pairs.extend(decl.params.iter().map(|(k, v)| (k.as_str(), param_value(v))));
    let qs: Vec<String> = pairs.iter().map(|(k, v)| format!("{k}={}", encode_component(v))).collect();
    let sep = if base.contains('?') { '&' } else { '?' };
    format!("{base}{sep}{}", qs.join("&"))
}

/// Why a paste was refused. Never carries the code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasteError {
    #[error("nothing was pasted")]
    Empty,
    #[error("the pasted address has no `code`")]
    NoCode,
    #[error("the pasted state doesn't match this sign-in; start again and paste the address it gives")]
    StateMismatch,
    #[error("paste the whole address from the browser's address bar, not just the code")]
    BareCode,
    #[error("the provider refused the sign-in: {0}")]
    Provider(String),
}

/// The code from a callback's query: `code`, `state` and `error` parameters.
pub(crate) fn from_query(query: &str, expected_state: &str) -> Result<SecretString, PasteError> {
    let (mut code, mut state, mut error) = (None, None, None);
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match &*k {
            "code" => code = Some(SecretString::new(v.into_owned())),
            "state" => state = Some(v.into_owned()),
            "error" => error = Some(v.into_owned()),
            _ => {}
        }
    }
    // State first (security review L1): an `error` from anything but this sign-in's
    // provider redirect (another local process, a web page hitting the fixed port) is a
    // state mismatch, not a message to print.
    if state.as_deref() != Some(expected_state) {
        return Err(PasteError::StateMismatch);
    }
    if let Some(e) = error {
        // The provider's error code is short and public; cap it in case it isn't, and keep
        // control characters off the terminal.
        return Err(PasteError::Provider(super::printable(&e).chars().take(80).collect()));
    }
    code.filter(|c| !c.is_empty()).ok_or(PasteError::NoCode)
}

/// What the operator pasted: the full redirect address (its `state` must match), or, for a
/// `code_page` redirect only, `code#state` or a bare code.
pub fn parse_paste(input: &str, kind: RedirectKind, expected_state: &str) -> Result<SecretString, PasteError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(PasteError::Empty);
    }
    if s.contains("://") || s.starts_with('/') || s.starts_with('?') {
        let base = url::Url::parse("http://paste.invalid/").expect("static URL");
        let u = base.join(s).map_err(|_| PasteError::NoCode)?;
        return from_query(u.query().unwrap_or_default(), expected_state);
    }
    if kind != RedirectKind::CodePage {
        return Err(PasteError::BareCode);
    }
    match s.split_once('#') {
        Some((code, state)) if !state.is_empty() => {
            if state != expected_state {
                return Err(PasteError::StateMismatch);
            }
            if code.is_empty() { Err(PasteError::NoCode) } else { Ok(SecretString::new(code)) }
        }
        Some((code, _)) if !code.is_empty() => Ok(SecretString::new(code)),
        Some(_) => Err(PasteError::NoCode),
        None => Ok(SecretString::new(s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7636_appendix_b() {
        let v = SecretString::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge(&v), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn random_values_have_their_lengths() {
        assert_eq!(verifier(32).len(), 43);
        assert_eq!(verifier(96).len(), 128);
        assert_eq!(state().len(), 43);
        let n = hex16();
        assert!(n.len() == 32 && n.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(hex16(), hex16());
        assert!(verifier(32).with_exposed(|v| !v.contains(['=', '+', '/'])));
    }

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(encode_component("a b:c/d?e=f&g'(h)*~!"), "a%20b%3Ac%2Fd%3Fe%3Df%26g'(h)*~!");
        assert_eq!(encode_component("é"), "%C3%A9");
    }

    #[test]
    fn pastes() {
        use RedirectKind::{CodePage, Loopback};
        let ok = |r: Result<SecretString, PasteError>, want: &str| assert!(r.unwrap().matches(want));
        ok(parse_paste("http://127.0.0.1:1/callback?code=c1&state=s", Loopback, "s"), "c1");
        ok(parse_paste(" /callback?state=s&code=c%2B2 ", Loopback, "s"), "c+2");
        ok(parse_paste("c3#s", CodePage, "s"), "c3");
        ok(parse_paste("c4", CodePage, "s"), "c4");
        ok(parse_paste("c5#", CodePage, "s"), "c5");
        let err = |r: Result<SecretString, PasteError>| r.unwrap_err();
        assert_eq!(err(parse_paste("c4", Loopback, "s")), PasteError::BareCode);
        assert_eq!(err(parse_paste("c4#s", Loopback, "s")), PasteError::BareCode);
        assert_eq!(err(parse_paste("c4#x", CodePage, "s")), PasteError::StateMismatch);
        assert_eq!(err(parse_paste("http://h/cb?code=c&state=x", CodePage, "s")), PasteError::StateMismatch);
        assert_eq!(err(parse_paste("http://h/cb?code=c", CodePage, "s")), PasteError::StateMismatch);
        assert_eq!(err(parse_paste("http://h/cb?state=s", CodePage, "s")), PasteError::NoCode);
        assert_eq!(err(parse_paste("  ", CodePage, "s")), PasteError::Empty);
        assert_eq!(
            err(parse_paste("http://h/cb?error=access_denied&state=s", Loopback, "s")),
            PasteError::Provider("access_denied".into())
        );
        // L1: an error without this sign-in's state is not shown, and control characters
        // never reach the terminal.
        assert_eq!(err(parse_paste("http://h/cb?error=Run%20rm%20-rf", Loopback, "s")), PasteError::StateMismatch);
        assert_eq!(err(parse_paste("http://h/cb?error=x&state=other", Loopback, "s")), PasteError::StateMismatch);
        assert_eq!(
            err(parse_paste("http://h/cb?error=denied%1B%5B2J&state=s", Loopback, "s")),
            PasteError::Provider("denied[2J".into())
        );
    }
}
