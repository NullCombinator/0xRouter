//! The pieces every page is built from, each drawn after the 9router component it names
//! (contracts/style-guide.md "Components"). Pages arrange these and add no styling of their own:
//! every class here is a `[component.*]` in `style/tokens.toml`, styled in `style/dashboard.css`
//! from tokens only.
//!
//! Class names are `<component>`, `<component>__<part>` and `<component>--<variant>`; the style
//! test maps each class to its component by the part before `__` or `--`.

use maud::{Markup, html};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        /// Material Symbols Outlined (Apache 2.0, `assets/LICENSES/`), one `<path>` each, drawn
        /// inline so they take the text colour. `<name>_fill` is the filled form the current
        /// sidebar entry shows.
        pub const ICONS: &[(&str, &str)] = &[$(($name, include_str!(concat!("../assets/icons/", $name, ".svg")))),*];
    };
}

icons!(
    "add",
    "alt_route",
    "api",
    "api_fill",
    "apps",
    "bar_chart",
    "bar_chart_fill",
    "block",
    "brush",
    "chat",
    "check",
    "chevron_left",
    "chevron_right",
    "close",
    "data_array",
    "data_usage",
    "data_usage_fill",
    "dns",
    "dns_fill",
    "error",
    "expand_more",
    "extension",
    "folder",
    "history",
    "hourglass_top",
    "hub",
    "info",
    "key",
    "lan",
    "lan_fill",
    "layers",
    "layers_fill",
    "link",
    "lock",
    "menu",
    "mic",
    "monitor",
    "movie",
    "notifications",
    "open_in_new",
    "psychology",
    "record_voice_over",
    "remove",
    "route",
    "schedule",
    "search",
    "settings",
    "settings_fill",
    "smart_toy",
    "terminal",
    "terminal_fill",
    "warning",
);

/// The `d` of an embedded icon; `None` for a name not in [`ICONS`].
pub fn icon_path(name: &str) -> Option<&'static str> {
    let svg = ICONS.iter().find(|(n, _)| *n == name)?.1;
    let start = svg.find(" d=\"")? + 4;
    let len = svg[start..].find('"')?;
    Some(&svg[start..start + len])
}

/// An icon at the size its place gives it (`.icon` in a nav entry, a button, a tile...).
pub fn icon(name: &str) -> Markup {
    let d = icon_path(name).unwrap_or_default();
    html! { svg class="icon" viewBox="0 -960 960 960" aria-hidden="true" { path d=(d) {} } }
}

/// How a status is coloured (contracts/style-guide.md "Status → badge").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Error,
    Info,
    Default,
    Primary,
}

impl Tone {
    pub const fn class(self) -> &'static str {
        match self {
            Self::Success => "badge badge--success",
            Self::Warning => "badge badge--warning",
            Self::Error => "badge badge--error",
            Self::Info => "badge badge--info",
            Self::Default => "badge badge--default",
            Self::Primary => "badge badge--primary",
        }
    }

    /// The tone of a status as the CLI words it, from its leading words; anything the table
    /// doesn't name is `Default`.
    pub fn of(status: &str) -> Self {
        let s = status.trim().to_ascii_lowercase().replace('_', " ");
        let starts = |words: &[&str]| words.iter().any(|w| s.starts_with(w));
        if starts(&["active", "signed in", "polled", "served", "loaded", "resolved", "ok"]) {
            Self::Success
        } else if starts(&["needs sign-in", "needs sign in", "refused", "failed", "records not kept", "error"]) {
            Self::Error
        } else if starts(&["cooling", "stale", "pending first poll", "estimated", "fallback", "warning", "refreshing"]) {
            Self::Warning
        } else if starts(&["in progress", "in flight", "note"]) {
            Self::Info
        } else {
            Self::Default
        }
    }
}

/// A pill with a leading dot (`Badge.js`, `dot`).
pub fn badge(tone: Tone, text: &str) -> Markup {
    html! { span class=(tone.class()) { span class="badge__dot" {} (text) } }
}

/// A badge whose tone follows the status words.
pub fn status(text: &str) -> Markup {
    badge(Tone::of(text), text)
}

/// A card's header row (`Card.js`: icon tile, title, subtitle, an action on the right).
pub struct Head<'a> {
    pub icon: &'a str,
    pub title: &'a str,
    pub subtitle: Option<&'a str>,
    pub action: Option<Markup>,
}

impl<'a> Head<'a> {
    pub fn new(icon: &'a str, title: &'a str) -> Self {
        Self { icon, title, subtitle: None, action: None }
    }

    pub fn subtitle(mut self, s: &'a str) -> Self {
        self.subtitle = Some(s);
        self
    }

    pub fn action(mut self, m: Markup) -> Self {
        self.action = Some(m);
        self
    }
}

/// `Card.js` at `padding = "md"`, with an optional header.
pub fn card(head: Option<Head<'_>>, body: Markup) -> Markup {
    html! {
        section class="card" {
            @if let Some(h) = head {
                div class="card__head" {
                    span class="card__tile" { (icon(h.icon)) }
                    div class="card__titles" {
                        h3 class="card__title" { (h.title) }
                        @if let Some(s) = h.subtitle { p class="card__subtitle" { (s) } }
                    }
                    @if let Some(a) = h.action { div class="card__action" { (a) } }
                }
            }
            (body)
        }
    }
}

/// `Card.Section`: an inset panel inside a card.
pub fn inset(body: Markup) -> Markup {
    html! { div class="inset" { (body) } }
}

/// A heading row above a group of cards, with controls on the right (9router's section bars).
pub fn section_bar(title: &str, right: Markup) -> Markup {
    html! { div class="section-bar" { h2 class="section-bar__title" { (title) } div class="section-bar__controls" { (right) } } }
}

/// A label and a value on one line.
pub fn kv(label: &str, value: Markup) -> Markup {
    html! { div class="kv" { span class="kv__label" { (label) } span class="kv__value" { (value) } } }
}

/// A link styled as `Button.js` (`sm`); `primary` is the brand fill.
pub fn link_button(href: &str, label: &str, icon_name: Option<&str>, primary: bool) -> Markup {
    let class = if primary { "button button--primary" } else { "button button--secondary" };
    html! { a class=(class) href=(href) { @if let Some(i) = icon_name { (icon(i)) } (label) } }
}

/// A control that isn't built yet or lives in the CLI (research R15): a disabled button and the
/// hint beside it. There is nothing to submit.
pub fn disabled(label: &str, icon_name: Option<&str>, hint: &str) -> Markup {
    html! {
        span class="control" {
            button class="button button--secondary" type="button" disabled { @if let Some(i) = icon_name { (icon(i)) } (label) }
            span class="hint" { (prose(hint)) }
        }
    }
}

/// A switch drawn off and disabled, with its hint (plugin enable, disable, hide).
pub fn disabled_switch(label: &str, hint: &str) -> Markup {
    html! {
        span class="control" title=(hint) {
            span class="switch" role="switch" aria-checked="true" aria-disabled="true" aria-label=(label) { span class="switch__knob" {} }
        }
    }
}

/// `Modal.js`, opened by its address (research R4): the page underneath stays, the backdrop and
/// the close button are links back to `close`.
pub fn modal(title: &str, close: &str, body: Markup) -> Markup {
    html! {
        div class="modal" role="dialog" aria-modal="true" aria-label=(title) {
            a class="modal__backdrop" href=(close) aria-label="Close" {}
            div class="modal__box" {
                div class="modal__head" {
                    h2 class="modal__title" { (title) }
                    a class="modal__close" href=(close) aria-label="Close" { (icon("close")) }
                }
                div class="modal__body" { (body) }
            }
        }
    }
}

/// The right side panel (Client adapters, Provider plugins), always rendered; its chevron is a
/// `<details>` summary, so it opens and closes without a script.
pub fn side_panel(icon_name: &str, title: &str, sections: Markup) -> Markup {
    html! {
        details class="side-panel" open {
            summary class="side-panel__head" {
                (icon(icon_name)) b class="side-panel__title" { (title) }
                span class="side-panel__chevron side-panel__chevron--open" { (icon("chevron_right")) }
                span class="side-panel__chevron side-panel__chevron--closed" { (icon("chevron_left")) }
            }
            div class="side-panel__body" { (sections) }
        }
    }
}

/// A titled group inside a side panel.
pub fn side_section(heading: &str, body: Markup) -> Markup {
    html! { div class="side-section" { h5 class="side-section__heading" { (heading) } (body) } }
}

/// What a slot says (research R12).
pub const ARRIVES: &str = "Arrives with the next dashboard slice.";

/// A place kept for slice 2: the card shape, a dashed border, muted text, no numbers.
pub fn slot(title: &str) -> Markup {
    html! { div class="slot" { span class="slot__title" { (title) } span class="slot__label" { (ARRIVES) } } }
}

/// An empty state: icon tile, a bold title, a muted hint (9router's Combos and Quota pages).
pub fn empty(icon_name: &str, title: &str, hint: &str) -> Markup {
    html! {
        div class="empty" {
            span class="empty__tile" { (icon(icon_name)) }
            h3 class="empty__title" { (title) }
            @if !hint.is_empty() { p class="empty__hint" { (prose(hint)) } }
        }
    }
}

/// One `check` notice in its own words, coloured by level. A multi-line notice (a skipped plugin
/// with its errors) keeps its lines.
pub fn notice(level: &str, text: &str) -> Markup {
    let class = match level {
        "error" => "notice notice--error",
        "warning" => "notice notice--warning",
        _ => "notice notice--note",
    };
    let icon_name = match level {
        "error" => "error",
        "warning" => "warning",
        _ => "info",
    };
    html! { div class=(class) role="status" { (icon(icon_name)) pre class="notice__text" { (text) } } }
}

/// A name shown in full, or cut with an ellipsis and the full name in its `title`.
pub fn name(text: &str) -> Markup {
    html! { span class="truncate" title=(text) { (text) } }
}

/// Text with `backticked` spans set as code: the hints of research R15 name CLI commands.
pub fn prose(text: &str) -> Markup {
    html! {
        @for (i, part) in text.split('`').enumerate() {
            @if i % 2 == 1 { code class="code" { (part) } } @else { (part) }
        }
    }
}

/// A command the operator can run, as selectable text.
pub fn command(text: &str) -> Markup {
    html! { code class="code" { (text) } }
}

/// The square text icon 9router draws for a provider without a logo: its first two letters.
pub fn text_icon(alias: &str) -> Markup {
    let letters: String = alias.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
    html! { span class="text-icon" aria-hidden="true" { (letters) } }
}

/// A provider's logo from `/logos/<hash>/<id>.png`, or its text icon.
pub fn logo(src: Option<&str>, alias: &str) -> Markup {
    match src {
        Some(src) => html! { img class="logo" src=(src) alt="" width="32" height="32"; },
        None => text_icon(alias),
    }
}

/// A bar of `percent` (0 to 100) left, coloured by how much is left, as 9router's quota rows
/// are: green above 50 %, yellow above 20 %, red below. The CSP allows no inline style, so the
/// width is one of 21 classes in steps of 5 %; the exact figure is the text beside the bar.
pub fn meter(percent: f64) -> Markup {
    let p = if percent.is_finite() { percent.clamp(0.0, 100.0) } else { 0.0 };
    let tone = if p > 50.0 {
        "meter--good"
    } else if p > 20.0 {
        "meter--low"
    } else {
        "meter--out"
    };
    let step = ((p / 5.0).round() as u32) * 5;
    let class = format!("meter {tone} meter--w{step}");
    html! { span class=(class) role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow=(format!("{p:.0}")) { span class="meter__fill" {} } }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_one_path() {
        for (name, svg) in ICONS {
            let d = icon_path(name).unwrap_or_else(|| panic!("{name} has no path"));
            assert!(d.len() > 10, "{name}");
            assert_eq!(svg.matches("<path").count(), 1, "{name}");
            assert!(!svg.contains("<script") && !svg.contains("href"), "{name} is plain path data");
        }
    }

    /// R11: every icon a page names exists, so none renders as an empty box.
    #[test]
    fn every_icon_the_source_names_is_embedded() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let mut files = vec![std::path::PathBuf::from(dir)];
        let mut named = Vec::new();
        while let Some(p) = files.pop() {
            if p.is_dir() {
                files.extend(std::fs::read_dir(&p).unwrap().map(|e| e.unwrap().path()));
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap();
            for pat in ["icon(\"", "Head::new(\"", "empty(\"", "side_panel(\""] {
                for (i, _) in text.match_indices(pat) {
                    let rest = &text[i + pat.len()..];
                    let n: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').collect();
                    if !n.is_empty() && rest[n.len()..].starts_with('"') {
                        named.push((p.display().to_string(), n));
                    }
                }
            }
        }
        assert!(!named.is_empty());
        for (file, n) in named {
            assert!(icon_path(&n).is_some(), "{file} names the icon {n:?}, which isn't in assets/icons/");
        }
    }

    #[test]
    fn statuses_take_the_contracts_badges() {
        for (s, t) in [
            ("active", Tone::Success),
            ("signed in", Tone::Success),
            ("polled", Tone::Success),
            ("served", Tone::Success),
            ("loaded", Tone::Success),
            ("needs sign-in since 2026-10-03 14:02 (refresh failed)", Tone::Error),
            ("needs_sign_in", Tone::Error),
            ("refused by provider", Tone::Error),
            ("failed", Tone::Error),
            ("records not kept", Tone::Error),
            ("cooling gpt-5 12 s", Tone::Warning),
            ("stale", Tone::Warning),
            ("pending first poll", Tone::Warning),
            ("estimated", Tone::Warning),
            ("fallback", Tone::Warning),
            ("pay-as-you-go", Tone::Default),
            ("disabled", Tone::Default),
            ("revoked", Tone::Default),
            ("default", Tone::Default),
            ("never", Tone::Default),
            ("not built yet", Tone::Default),
            ("in progress", Tone::Info),
        ] {
            assert_eq!(Tone::of(s), t, "{s}");
        }
    }

    #[test]
    fn backticks_become_code_and_text_is_escaped() {
        let m = prose("In the CLI: `nullrouter keys issue <name>`").into_string();
        assert_eq!(m, "In the CLI: <code class=\"code\">nullrouter keys issue &lt;name&gt;</code>");
    }

    #[test]
    fn a_long_name_keeps_its_full_text_in_the_title() {
        let long = "a".repeat(199) + "z";
        let m = name(&long).into_string();
        assert!(m.contains(&format!("title=\"{long}\"")) && m.contains(&format!(">{long}<")), "{m}");
    }

    #[test]
    fn a_slot_shows_no_digits() {
        let m = slot("Agent traffic").into_string();
        assert!(!m.chars().any(|c| c.is_ascii_digit()), "{m}");
        assert!(m.contains(ARRIVES));
    }

    #[test]
    fn a_disabled_control_has_nothing_to_submit() {
        let m = disabled("Add Agent", Some("add"), "In the CLI: `nullrouter keys issue <name>`").into_string();
        assert!(m.contains("disabled") && !m.contains("<form") && !m.contains("type=\"submit\""), "{m}");
    }
}
