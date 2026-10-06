//! Combo, Console Log and Proxy Pools say what isn't built and what exists instead (T042,
//! research R15).

use nullrouter_dashboard::pages::{combo, console_log, proxy_pools};
use nullrouter_dashboard::page::ViewName;
use serde_json::json;

use crate::common::{Dash, text_of};

#[tokio::test(flavor = "multi_thread")]
async fn each_not_built_entry_says_so_and_names_what_exists() {
    let d = Dash::dashboard().await;
    let combo_page = text_of(&d.ok("/combo").await);
    assert!(combo_page.contains(combo::TITLE), "{combo_page}");
    assert!(combo_page.contains("[[unified_model]]") && combo_page.contains("nullrouter unified"), "{combo_page}");

    let log = d.ok("/console-log").await;
    assert!(text_of(&log).contains(console_log::TITLE) && text_of(&log).contains(console_log::HINT));
    assert!(log.contains("href=\"/usage\""), "Console Log links to the Usage page");

    let pools = text_of(&d.ok("/proxy-pools").await);
    assert!(pools.contains(proxy_pools::TITLE) && pools.contains(proxy_pools::HINT), "{pools}");
}

/// Combo lists its `combo` notices and no unified model list.
#[tokio::test(flavor = "multi_thread")]
async fn combo_lists_its_notices_and_no_unified_models() {
    let d = Dash::dashboard().await;
    let check = d.view(ViewName::Check, json!({})).await;
    let page = text_of(&d.ok("/combo").await);
    let mine: Vec<&str> = check["notices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["subject"] == "combo")
        .map(|n| n["text"].as_str().unwrap())
        .collect();
    assert!(!mine.is_empty(), "the fixture has a dropped unified model or a limits note");
    for text in mine {
        let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(page.contains(&one_line), "{one_line:?} is on Combo\n{page}");
    }
    let unified = d.view(ViewName::Check, json!({})).await["unified_models"].clone();
    assert!(!page.contains("unified models:"), "no unified model list: {unified}");
}
