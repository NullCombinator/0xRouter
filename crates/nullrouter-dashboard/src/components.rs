//! The pieces every page is built from, each drawn after the 9router component it names
//! (contracts/style-guide.md "Components"). Pages arrange these and add no styling of their own:
//! every class here is a `[component.*]` in `style/tokens.toml`, styled in `style/dashboard.css`
//! from tokens only.
//!
//! Class names are `<component>`, `<component>__<part>` and `<component>--<variant>`; the style
//! test maps each class to its component by the part before `__` or `--`.

use maud::{Markup, html};
use nullrouter_engine::tasks::TaskInput;

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
    "play_arrow",
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
        } else if starts(&["cooling", "stale", "pending first poll", "estimated", "fallback", "warning", "refreshing"])
        {
            Self::Warning
        } else if starts(&["in progress", "in flight", "note"]) {
            Self::Info
        } else {
            Self::Default
        }
    }
}

/// The palette token of an agent: `agent-1` to `agent-7` by the key's position in `keys list`
/// order (creation order, revoked keys included), so a colour never moves when keys are added
/// (spec 010 R8).
pub fn agent_colour(index: usize) -> &'static str {
    const NAMES: [&str; 7] = ["agent-1", "agent-2", "agent-3", "agent-4", "agent-5", "agent-6", "agent-7"];
    NAMES[index % NAMES.len()]
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
    card_as("card", head, body)
}

/// A card's head over a body with no card around it: the body brings its own frame.
pub fn bare_card(head: Head<'_>, body: Markup) -> Markup {
    card_as("card card--bare", Some(head), body)
}

fn card_as(class: &str, head: Option<Head<'_>>, body: Markup) -> Markup {
    html! {
        section class=(class) {
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

/// A section bar with a muted note before its controls (the mockups' "Agents" bar): where a group
/// of disabled controls shares one hint, the hint is the note.
pub fn section_bar_noted(title: &str, note: Markup, right: Markup) -> Markup {
    html! {
        div class="section-bar" {
            h2 class="section-bar__title" { (title) }
            div class="section-bar__controls" { span class="section-bar__note" { (note) } (right) }
        }
    }
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

/// A disabled button alone, for a group whose hint is its section bar's note
/// ([`section_bar_noted`]); `primary` draws it in the brand fill, as the mockups' first button.
pub fn disabled_button(label: &str, icon_name: Option<&str>, primary: bool) -> Markup {
    let class = if primary { "button button--primary" } else { "button button--secondary" };
    html! { button class=(class) type="button" disabled { @if let Some(i) = icon_name { (icon(i)) } (label) } }
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

/// The hint on a panel's search, which isn't built yet.
pub const SEARCH_HINT: &str = "Not built yet: the search comes with the functional dashboard.";

/// The right side panel (Client adapters, Provider plugins), always rendered and closed at first,
/// as the mockups draw it: a chevron tab at the page's edge. The tab is a `<details>` summary, so
/// it opens and closes without a script. Under the title: a search (disabled) and the gear that
/// opens the [`settings`] window `gear`.
pub fn side_panel(icon_name: &str, title: &str, search: &str, gear: &str, sections: Markup) -> Markup {
    html! {
        details class="side-panel" {
            summary class="side-panel__head" {
                (icon(icon_name)) b class="side-panel__title" { (title) }
                span class="side-panel__chevron side-panel__chevron--open" { (icon("chevron_right")) }
                span class="side-panel__chevron side-panel__chevron--closed" { (icon("chevron_left")) }
            }
            div class="side-panel__tools" {
                label class="side-panel__search" title=(SEARCH_HINT) {
                    (icon("search"))
                    input class="side-panel__input" type="search" placeholder=(search) aria-label=(search) disabled;
                }
                label class="side-panel__gear" for=(gear) title="Settings" aria-label="Settings" { (icon("settings")) }
            }
            div class="side-panel__body" { (sections) }
        }
    }
}

/// A titled group inside a side panel.
pub fn side_section(heading: &str, body: Markup) -> Markup {
    html! { div class="side-section" { h5 class="side-section__heading" { (heading) } (body) } }
}

/// One row of a side panel: a logo tile, the name, a short line under it, and a status badge.
pub fn side_row(tile: Markup, title: &str, line: &str, state: Markup) -> Markup {
    html! {
        div class="plugin-row" {
            span class="plugin-row__logo" { (tile) }
            div class="plugin-row__text" {
                b class="plugin-row__id" { (name(title)) }
                span class="plugin-row__source" { (line) }
            }
            (state)
        }
    }
}

/// A settings window opened by a panel's gear, over the page and the open panel: a hidden
/// checkbox `id` that the gear, the backdrop, the close button and Cancel are labels for (the
/// mockups' CSS toggle). There is no script; each form in the body saves on its own, so the foot
/// only closes the window.
pub fn settings(id: &str, title: &str, body: Markup) -> Markup {
    html! {
        input class="modal-toggle" type="checkbox" id=(id) aria-label=(title);
        div class="modal modal--toggled" role="dialog" aria-modal="true" aria-label=(title) {
            label class="modal__backdrop" for=(id) {}
            div class="modal__box" {
                div class="modal__head" {
                    h2 class="modal__title" { (title) }
                    label class="modal__close" for=(id) aria-label="Close" { (icon("close")) }
                }
                div class="modal__body" { (body) }
                div class="modal__foot" {
                    label class="button button--secondary" for=(id) { "Close" }
                }
            }
        }
    }
}

/// [`modal`] wide, with a slim head that holds only a muted line (`what`) and the close link;
/// the body carries its own title.
pub fn wide_modal(title: &str, what: &str, close: &str, body: Markup) -> Markup {
    html! {
        div class="modal" role="dialog" aria-modal="true" aria-label=(title) {
            a class="modal__backdrop" href=(close) aria-label="Close" {}
            div class="modal__box modal__box--wide" {
                div class="modal__head modal__head--slim" {
                    span class="modal__what" { (what) }
                    a class="modal__close" href=(close) aria-label="Close" { (icon("close")) }
                }
                div class="modal__body modal__body--wide" { (body) }
            }
        }
    }
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

/// The answer to a form, shown once at the top of the page it returns to (contracts/dashboard-forms.md):
/// a change that was done, or one that was refused. Named apart from [`notice`], which takes a level.
pub fn flash_notice(text: &str, refused: bool) -> Markup {
    let (class, icon_name) = if refused { ("notice notice--error", "error") } else { ("notice notice--note", "check") };
    html! { div class=(class) role="status" { (icon(icon_name)) p class="notice__text" { (text) } } }
}

/// A POST form to `action` (contracts/dashboard-forms.md "Every form"). The form token and the page
/// to return to travel as hidden fields; the body holds the fields and the submit.
pub fn form(action: &str, back: &str, token: &str, body: Markup) -> Markup {
    html! {
        form class="form" method="post" action=(action) {
            input type="hidden" name="_t" value=(token);
            input type="hidden" name="_back" value=(back);
            (body)
        }
    }
}

fn input_class(error: Option<&str>) -> &'static str {
    if error.is_some() { "input input--error" } else { "input" }
}

fn field_error(error: Option<&str>) -> Markup {
    html! { @if let Some(e) = error { span class="field__error" { (icon("error")) (e) } } }
}

/// A text field under its label, with the value it had (none for a new field) and its error beside it.
pub fn field(name: &str, label: &str, value: Option<&str>, error: Option<&str>) -> Markup {
    html! {
        label class="field" {
            span class="field__label" { (label) }
            input class=(input_class(error)) type="text" name=(name) value=[value];
            (field_error(error))
        }
    }
}

/// A multi-line text field under its label, with the value it had and its error beside it (members,
/// one per line).
pub fn textarea(name: &str, label: &str, value: Option<&str>, error: Option<&str>) -> Markup {
    html! {
        label class="field" {
            span class="field__label" { (label) }
            textarea class=(input_class(error)) name=(name) rows="4" { (value.unwrap_or_default()) }
            (field_error(error))
        }
    }
}

/// A drop-down under its label; `selected` is the option's value that is chosen.
pub fn select(name: &str, label: &str, options: &[(&str, &str)], selected: Option<&str>) -> Markup {
    html! {
        label class="field" {
            span class="field__label" { (label) }
            select class="input" name=(name) {
                @for (value, text) in options {
                    option value=(value) selected[selected == Some(*value)] { (text) }
                }
            }
        }
    }
}

/// A live on/off switch: a checkbox drawn as a track and a knob, submitted under `name` when on.
pub fn switch(name: &str, label: &str, on: bool) -> Markup {
    html! {
        label class="switch-field" {
            input class="switch-field__input" type="checkbox" name=(name) value="on" checked[on];
            span class="switch-field__track" { span class="switch-field__knob" {} }
            span class="switch-field__label" { (label) }
        }
    }
}

/// A password field for a write-only value (`secret`, `password`, `code`): it never has a value
/// attribute, so nothing the server holds is echoed back into the page.
pub fn secret_field(name: &str, label: &str, error: Option<&str>) -> Markup {
    html! {
        label class="field" {
            span class="field__label" { (label) }
            input class=(input_class(error)) type="password" name=(name) autocomplete="off";
            (field_error(error))
        }
    }
}

/// A value shown once, selectable, with the warning that it won't be shown again (a new key, a
/// proxy's one-time code).
pub fn once_box(label: &str, value: &str) -> Markup {
    html! {
        div class="once" {
            span class="once__label" { (label) }
            pre class="once__value" { (value) }
            p class="once__warning" { (icon("warning")) "This is the only time it is shown." }
        }
    }
}

/// A confirm window (research R9): the modal names what is lost, and holds the real form with its
/// Confirm button beside a Cancel link that closes the window.
pub fn confirm_window(title: &str, loss_text: &str, cancel_href: &str, form: Markup) -> Markup {
    let body = html! {
        div class="confirm" {
            (notice("warning", loss_text))
            div class="confirm__actions" {
                (link_button(cancel_href, "Cancel", None, false))
                (form)
            }
        }
    };
    modal(title, cancel_href, body)
}

/// Where a long task stands (research R4). A running task's page refreshes every 3 s, with no script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState<'a> {
    Running,
    Done(&'a str),
    Failed(&'a str),
}

/// A task's panel: its subject, its state, its progress lines, and its result once it has one.
pub fn task_panel(subject: &str, started: &str, progress: &[String], result: TaskState<'_>) -> Markup {
    let (state, outcome, refresh) = match result {
        TaskState::Running => (badge(Tone::Info, "in progress"), None, true),
        TaskState::Done(text) => (badge(Tone::Success, "done"), Some(flash_notice(text, false)), false),
        TaskState::Failed(text) => (badge(Tone::Error, "failed"), Some(flash_notice(text, true)), false),
    };
    html! {
        div class="task" {
            @if refresh { meta http-equiv="refresh" content="3"; }
            div class="task__head" {
                span class="task__subject" { (subject) }
                (state)
            }
            p class="task__started" { "started at " (started) }
            @if !progress.is_empty() {
                ul class="task__progress" { @for line in progress { li { (progress_line(line)) } } }
            }
            @if let Some(o) = outcome { (o) }
        }
    }
}

/// A progress line; a link the provider sent (the sign-in's authorize address) is copyable text in
/// a `code` element, never an `href`: the dashboard refers only to its own addresses.
fn progress_line(line: &str) -> Markup {
    let trimmed = line.trim();
    html! {
        @if trimmed.starts_with("https://") { code class="code" { (trimmed) } } @else { (line) }
    }
}

/// What a sign-in task waits for, under its panel: a form that posts the pasted code (write-only,
/// a password field), or the device code and its link as copyable text. `back` is where the form's
/// redirect returns, and `token` is the form token.
pub fn task_input(task: &str, input: &TaskInput, back: &str, token: &str) -> Markup {
    match input {
        TaskInput::Paste => form(
            "/do/accounts/signin-paste",
            back,
            token,
            html! {
                input type="hidden" name="task" value=(task);
                (secret_field("code", "Pasted code or address", None))
                button class="button button--primary" type="submit" { "Send code" }
            },
        ),
        TaskInput::Device { code, link } => html! {
            div class="task__input" {
                span class="kv__label" { "Approve this code" }
                code class="code" { (code) }
                span class="kv__label" { "on this page" }
                code class="code" { (link) }
            }
        },
    }
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

/// How a share left is coloured, as 9router's quota rows colour it (`getColorClasses`): `good`
/// above 70 %, `low` from 30 %, `out` below.
pub fn left_tone(percent: f64) -> &'static str {
    if percent > 70.0 {
        "good"
    } else if percent >= 30.0 {
        "low"
    } else {
        "out"
    }
}

/// A bar of `percent` (0 to 100) left, coloured by [`left_tone`]. The CSP allows no inline style,
/// so the width is one of 21 classes in steps of 5 %; the exact figure is the text beside the bar.
pub fn meter(percent: f64) -> Markup {
    let p = if percent.is_finite() { percent.clamp(0.0, 100.0) } else { 0.0 };
    let tone = format!("meter--{}", left_tone(p));
    let step = ((p / 5.0).round() as u32) * 5;
    let class = format!("meter {tone} meter--w{step}");
    html! { span class=(class) role="meter" aria-valuemin="0" aria-valuemax="100" aria-valuenow=(format!("{p:.0}")) { span class="meter__fill" {} } }
}

#[cfg(test)]
mod tests {
    use super::agent_colour;

    #[test]
    fn agent_colours_cycle_through_seven_and_never_move_when_keys_are_appended() {
        let before: Vec<_> = (0..5).map(agent_colour).collect();
        let after: Vec<_> = (0..9).map(agent_colour).collect();
        assert_eq!(before, after[..5], "appending keys leaves the earlier colours alone");
        assert_eq!(agent_colour(0), "agent-1");
        assert_eq!(agent_colour(6), "agent-7");
        assert_eq!(agent_colour(7), "agent-1", "the eighth key wraps");
    }

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

    #[test]
    fn a_secret_field_is_a_password_that_never_has_a_value() {
        for error in [None, Some("refused")] {
            let m = secret_field("secret", "API key", error).into_string();
            assert!(m.contains(r#"type="password""#) && m.contains(r#"autocomplete="off""#), "{m}");
            assert!(!m.contains("value"), "a write-only field has no value attribute: {m}");
        }
    }

    #[test]
    fn a_form_carries_its_token_and_the_page_it_returns_to() {
        let m = form("/do/keys/issue", "/endpoint", "tok123", maud::html! { p { "x" } }).into_string();
        assert!(m.contains(r#"method="post""#) && m.contains(r#"action="/do/keys/issue""#), "{m}");
        assert!(m.contains(r#"<input type="hidden" name="_t" value="tok123">"#), "{m}");
        assert!(m.contains(r#"<input type="hidden" name="_back" value="/endpoint">"#), "{m}");
    }

    #[test]
    fn a_running_task_refreshes_and_a_finished_one_does_not() {
        let progress = vec!["step one".to_owned()];
        let running = task_panel("Test anthropic/work", "14:02", &progress, TaskState::Running).into_string();
        assert!(running.contains(r#"<meta http-equiv="refresh" content="3">"#), "{running}");
        assert!(running.contains("started at 14:02"), "{running}");
        for done in [TaskState::Done("ok"), TaskState::Failed("boom")] {
            let m = task_panel("Test", "14:02", &progress, done).into_string();
            assert!(!m.contains("http-equiv"), "{m}");
        }
    }

    #[test]
    fn a_once_value_comes_with_the_only_time_warning() {
        let m = once_box("Key", "nr_abc123").into_string();
        assert!(m.contains("This is the only time it is shown."), "{m}");
        assert!(m.contains("nr_abc123"), "{m}");
    }

    #[test]
    fn a_paste_task_takes_a_write_only_code_for_its_task() {
        let m = task_input("tk_abcdefghijklmnop", &TaskInput::Paste, "/providers", "tok").into_string();
        assert!(m.contains(r#"action="/do/accounts/signin-paste""#), "{m}");
        assert!(m.contains(r#"name="task" value="tk_abcdefghijklmnop""#), "{m}");
        assert!(m.contains(r#"type="password" name="code""#) && !m.contains("value=\"\""), "{m}");
    }

    #[test]
    fn a_device_task_shows_its_code_and_link_as_text_not_as_a_link() {
        let input = TaskInput::Device { code: "ABCD-1234".into(), link: "https://auth.example/device".into() };
        let m = task_input("tk_abcdefghijklmnop", &input, "/providers", "tok").into_string();
        assert!(m.contains("<code class=\"code\">ABCD-1234</code>"), "{m}");
        assert!(m.contains("<code class=\"code\">https://auth.example/device</code>"), "{m}");
        assert!(!m.contains("href"), "no link a browser follows: {m}");
    }

    #[test]
    fn a_progress_link_is_copyable_text() {
        let progress = vec!["  https://auth.example/authorize?x=1  ".to_owned(), "waiting".to_owned()];
        let m = task_panel("Sign in", "14:02", &progress, TaskState::Running).into_string();
        assert!(m.contains("<code class=\"code\">https://auth.example/authorize?x=1</code>"), "{m}");
        assert!(m.contains("<li>waiting</li>"), "{m}");
    }
}
