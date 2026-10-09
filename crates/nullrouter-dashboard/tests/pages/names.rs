//! Long names (T057, FR-046 edge case): a 200-character model and key name, and an account name
//! as long as `accounts add` takes (32 characters), are shown in full or cut with the full name in
//! a `title`; two names that differ only in their last character stay apart.

use std::fs;

use nullrouter_engine::files::write_private;
use nullrouter_engine::keys::{self, Keys};
use nullrouter_engine::testkit::homes;

use crate::common::{Dash, attrs, text_of};

/// Two names of `len` characters that differ only in the last one.
fn pair(stem: &str, len: usize) -> [String; 2] {
    let body: String = stem.chars().cycle().take(len - 1).collect();
    [format!("{body}a"), format!("{body}b")]
}

fn shown(html: &str, name: &str) -> bool {
    text_of(html).contains(name) && attrs(html, "title").iter().any(|t| t == name)
}

#[tokio::test(flavor = "multi_thread")]
async fn long_names_are_shown_whole_or_titled_and_stay_apart() {
    let models = pair("long-model-", 200);
    let accounts = pair("account-", 32);
    let keys_ = pair("long-key-", 200);
    let dir = homes::dashboard();
    let h = dir.path();
    fs::write(
        h.join("plugins/longnames.toml"),
        format!(
            "schema = 2\nid = \"longnames\"\ncategory = \"apikey\"\n\n[auth]\nkind = \"apikey\"\nheader = \"Authorization\"\n\
             scheme = \"bearer\"\n\n[endpoints.text]\nurl = \"https://api.longnames.example/v1/chat/completions\"\n\
             wire = \"openai-chat\"\n\n[[models]]\nid = \"{}\"\n\n[[models]]\nid = \"{}\"\n",
            models[0], models[1]
        ),
    )
    .unwrap();
    let file = h.join(nullrouter_engine::accounts::FILE);
    let mut text = fs::read_to_string(&file).unwrap();
    for (i, a) in accounts.iter().enumerate() {
        text += &format!("\n[[account]]\nprovider = \"longnames\"\nname = \"{a}\"\nsecret = \"sk-long-{i}\"\norder = {}\n", 20 + i);
    }
    write_private(&file, &text).unwrap();
    let mut keys = Keys::load(&h.join(keys::FILE)).unwrap();
    for k in &keys_ {
        keys.issue(k, None).unwrap();
    }
    write_private(&h.join(keys::FILE), &keys.to_toml()).unwrap();
    let d = Dash::start(dir).await;

    let window = d.ok("/providers/longnames").await;
    let endpoint = d.ok("/endpoint").await;
    for (html, names, what) in [(&window, &models, "model"), (&window, &accounts, "account"), (&endpoint, &keys_, "key")] {
        assert_ne!(names[0], names[1]);
        for n in names {
            assert!(shown(html, n), "the {what} name {n:?} is not shown whole with its title");
        }
    }
}
