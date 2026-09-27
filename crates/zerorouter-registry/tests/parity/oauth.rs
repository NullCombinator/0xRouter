use crate::{bundled, fixture};

#[test]
fn oauth_urls_match_9router() {
    let reg = bundled();
    let got = serde_json::to_value(reg.oauth_urls_view()).unwrap();
    let want = fixture("oauth-urls");
    for section in ["oauthEndpoints", "tokenUrls", "authUrls", "refreshUrls", "clientIds"] {
        assert_eq!(got[section], want[section], "{section}");
    }
    assert_eq!(got, want);
}
