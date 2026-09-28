//! The style gate (contracts/api-style-schema.md). Style files are data the core
//! interprets, so they pass the same kind of gate as plugins: unknown keys, unknown
//! placeholders, expressions, secrets and ambiguous decode rules are all rejected, and
//! every error is reported.

use std::collections::BTreeMap;

use super::errors::{FieldPath, ValidationError};
use super::gate::{parse, positioned};
use crate::schema::{
    FinishReason, MatchRule, ModelType, ParamDecl, PartKind, ResponseFormatForm, Route, RouteOp, StreamEventDecl,
    StyleFile, TextCodec, ThinkingForm,
};
use crate::template::{FieldPath as Selector, Piece, PlaceholderSet, Template};

/// Placeholder sets per template context. The wire interpreter binds exactly these.
pub mod placeholders {
    use crate::template::PlaceholderSet;

    const CACHE: &str = "part.cache_control";
    pub const TEXT_PART: PlaceholderSet = PlaceholderSet(&["part.text", CACHE]);
    pub const MEDIA_PART: PlaceholderSet = PlaceholderSet(&["media", "media.mime", "media.data", "media.url", CACHE]);
    pub const TOOL_CALL_PART: PlaceholderSet = PlaceholderSet(&["call.id", "call.name", "call.arguments", CACHE]);
    pub const TOOL_RESULT_PART: PlaceholderSet =
        PlaceholderSet(&["result.id", "result.name", "result.content", "result.is_error", CACHE]);
    pub const THINKING_PART: PlaceholderSet = PlaceholderSet(&["part.text", "part.signature"]);
    pub const MESSAGE: PlaceholderSet = PlaceholderSet(&["message.role", "message.content"]);
    pub const TOOL: PlaceholderSet =
        PlaceholderSet(&["tool.name", "tool.description", "tool.parameters", "tool.cache_control"]);
    pub const TOOLS_WRAP: PlaceholderSet = PlaceholderSet(&["tools.list"]);
    pub const TOOL_CHOICE: PlaceholderSet = PlaceholderSet(&["tool.name"]);
    pub const RESPONSE: PlaceholderSet = PlaceholderSet(&[
        "response.id",
        "response.model",
        "response.created",
        "response.status",
        "response.content",
        "response.text",
        "response.thinking",
        "response.tool_calls",
        "response.finish",
        "usage.*",
    ]);
    pub const STREAM: PlaceholderSet = PlaceholderSet(&[
        "response.id",
        "response.model",
        "response.created",
        "response.rendered",
        "block.index",
        "block.id",
        "block.name",
        "block.full_text",
        "block.full_arguments",
        "block.arguments_json",
        "block.signature",
        "tool.ordinal",
        "output.index",
        "sequence.number",
        "delta.text",
        "delta.thinking",
        "delta.signature",
        "delta.arguments",
        "finish",
        "usage.*",
        "error.type",
        "error.message",
    ]);
    pub const ERROR_BODY: PlaceholderSet =
        PlaceholderSet(&["error.type", "error.message", "error.details", "error.status", "error.code"]);
    pub const ERROR_EVENT: PlaceholderSet = PlaceholderSet(&["error.body", "error.type", "error.message"]);
    pub const NONE: PlaceholderSet = PlaceholderSet(&[]);
    pub const COUNT: PlaceholderSet = PlaceholderSet(&["count.input"]);
    /// Non-text codecs and inline provider bodies.
    pub const TYPE_CODEC: PlaceholderSet = PlaceholderSet(&[
        "model",
        "model.upstream_id",
        "input.*",
        "params.*",
        "output.*",
        "usage.*",
        "response.id",
        "response.created",
        "response.model",
    ]);
    /// Inline provider endpoint bodies (contracts/provider-schema-v2.md).
    pub const PROVIDER_BODY: PlaceholderSet =
        PlaceholderSet(&["model.upstream_id", "input.*", "params.*", "output.*"]);
    pub const JOB: PlaceholderSet = PlaceholderSet(&["job.id", "job.status", "job.error", "job.created", "job.model"]);
}

/// IR request parameters a style may locate.
pub const IR_PARAMS: &[&str] = &[
    "max_tokens",
    "temperature",
    "top_p",
    "top_k",
    "stop",
    "seed",
    "presence_penalty",
    "frequency_penalty",
    "user",
    "parallel_tool_calls",
    "thinking",
    "response_format",
    "metadata",
];

const IR_ROLES: &[&str] = &["system", "user", "assistant", "tool"];
const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE"];
const REQUIRED_ERROR_STATUSES: &[&str] = &["400", "401", "404", "429", "500", "503"];

type Found = Vec<(FieldPath, String)>;

/// Parses and checks one style file.
pub fn validate_style(src: &str, file: &str) -> Result<StyleFile, Vec<ValidationError>> {
    let style = parse::<StyleFile>(src, file)?;
    let errors = semantic_errors(&style);
    if errors.is_empty() {
        Ok(style)
    } else {
        Err(errors.into_iter().map(|(path, rule)| positioned(src, file, path, rule)).collect())
    }
}

/// Route collisions across every loaded style: `(file, source, style)` per file.
pub fn check_route_collisions(styles: &[(&str, &str, &StyleFile)]) -> Vec<ValidationError> {
    let mut groups: BTreeMap<(String, String), Vec<(usize, usize)>> = BTreeMap::new();
    for (f, (_, _, s)) in styles.iter().enumerate() {
        for (r, route) in s.routes.iter().enumerate() {
            groups.entry((route.method.to_ascii_uppercase(), shape(&route.path))).or_default().push((f, r));
        }
    }
    let mut out = Vec::new();
    for ((method, _), members) in groups.into_iter().filter(|(_, m)| m.len() > 1) {
        let route = |&(f, r): &(usize, usize)| &styles[f].2.routes[r];
        let defaults = members.iter().filter(|m| route(m).discriminator.is_none()).count();
        let disjoint = members.iter().enumerate().all(|(i, a)| {
            members[i + 1..].iter().all(|b| match (&route(a).discriminator, &route(b).discriminator) {
                (Some(x), Some(y)) => disjoint(x, y),
                _ => true,
            })
        });
        if defaults == 1 && disjoint {
            continue;
        }
        let names: Vec<String> =
            members.iter().map(|&(f, r)| format!("{}:routes[{r}] {}", styles[f].0, styles[f].2.routes[r].path)).collect();
        for &(f, r) in &members {
            let (file, src, _) = styles[f];
            out.push(positioned(
                src,
                file,
                FieldPath::of("routes").index(r),
                format!(
                    "route collision on {method}: {}; routes may share a path only with disjoint discriminators and exactly one default",
                    names.join(", ")
                ),
            ));
        }
    }
    out
}

/// A path with placeholder names erased, so `/v1/{id}` and `/v1/{x}` collide.
fn shape(path: &str) -> String {
    let mut out = String::new();
    let mut in_hole = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_hole = true;
                out.push('{');
            }
            '}' => {
                in_hole = false;
                out.push('}');
            }
            '*' if in_hole => out.push('*'),
            _ if in_hole => {}
            _ => out.push(c),
        }
    }
    out
}

fn disjoint(a: &MatchRule, b: &MatchRule) -> bool {
    match (&a.path_equals, &b.path_equals) {
        (Some(x), Some(y)) => x.len() == 2 && y.len() == 2 && x[0] == y[0] && x[1] != y[1],
        _ => false,
    }
}

fn semantic_errors(s: &StyleFile) -> Found {
    let mut out = Found::new();
    let mut err = |path: FieldPath, rule: String| out.push((path, rule));

    if s.schema != 1 {
        err(FieldPath::of("schema"), format!("unsupported style schema {}; expected 1", s.schema));
    }
    if s.kind != "api-style" {
        err(FieldPath::of("kind"), format!("{:?} must be \"api-style\"", s.kind));
    }
    if !is_token(&s.id) {
        err(FieldPath::of("id"), format!("{:?} must match [a-z0-9][a-z0-9-]*", s.id));
    }
    if s.forwarding.is_some() {
        err(FieldPath::of("forwarding"), "a style file may not declare forwarding".into());
    }

    check_carriers(s, &mut err);
    for (i, route) in s.routes.iter().enumerate() {
        check_route(s, route, &FieldPath::of("routes").index(i), &mut err);
    }
    if let Some(text) = &s.text {
        check_text(text, &FieldPath::of("text"), &mut err);
    }
    for t in [ModelType::Embeddings, ModelType::Image, ModelType::Tts, ModelType::Stt, ModelType::Video] {
        let Some(c) = s.type_codec(t) else { continue };
        let base = FieldPath::of(t.as_str());
        check_template(&c.request, placeholders::TYPE_CODEC, &base.key("request"), &mut err);
        check_template(&c.response, placeholders::TYPE_CODEC, &base.key("response"), &mut err);
        if c.vector.is_some() && t != ModelType::Embeddings {
            err(base.key("vector"), "only the embeddings codec has a vector encoding".into());
        }
        match (&c.job, t) {
            (Some(j), ModelType::Video) => check_template(&j.status, placeholders::JOB, &base.key("job").key("status"), &mut err),
            (Some(_), _) => err(base.key("job"), "only the video codec runs asynchronous jobs".into()),
            _ => {}
        }
        if let Some(u) = &c.usage {
            check_usage_paths(u, &base.key("usage"), &mut err);
        }
    }
    check_errors(s, &mut err);
    out
}

fn check_carriers(s: &StyleFile, err: &mut impl FnMut(FieldPath, String)) {
    let base = FieldPath::of("access_key").key("carriers");
    if s.access_key.carriers.is_empty() {
        err(base.clone(), "at least one access-key carrier is required".into());
    }
    for (i, c) in s.access_key.carriers.iter().enumerate() {
        let p = base.index(i);
        match (&c.header, &c.query) {
            (Some(h), None) if !is_header_name(h) => err(p.key("header"), format!("{h:?} is not a header name")),
            (Some(_), None) => {}
            (None, Some(_)) if c.scheme != crate::schema::KeyScheme::Raw => {
                err(p.key("scheme"), "a query carrier takes the raw key; scheme must be \"raw\"".into());
            }
            (None, Some(q)) if !is_token_underscore(q) => err(p.key("query"), format!("{q:?} is not a query name")),
            (None, Some(_)) => {}
            _ => err(p, "a carrier names exactly one of `header` or `query`".into()),
        }
    }
    let base = FieldPath::of("session").key("carriers");
    for (i, c) in s.session.carriers.iter().enumerate() {
        let p = base.index(i);
        let set = usize::from(c.header.is_some()) + usize::from(c.path.is_some()) + usize::from(c.extractor.is_some());
        if set != 1 {
            err(p, "a session carrier names exactly one of `header`, `path` or `extractor`".into());
            continue;
        }
        if let Some(h) = c.header.as_deref().filter(|h| !is_header_name(h)) {
            err(p.key("header"), format!("{h:?} is not a header name"));
        }
        if let Some(Err(e)) = c.path.as_deref().map(Selector::parse) {
            err(p.key("path"), e);
        }
    }
}

fn check_route(s: &StyleFile, r: &Route, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    if !METHODS.contains(&r.method.as_str()) {
        err(base.key("method"), format!("{:?} is not one of {}", r.method, METHODS.join(", ")));
    }
    let params = match path_params(&r.path) {
        Ok(p) => p,
        Err(e) => {
            err(base.key("path"), e);
            Vec::new()
        }
    };
    if let Some(m) = &r.model {
        match (&m.body, &m.path) {
            (Some(b), None) => {
                if let Err(e) = Selector::parse(b) {
                    err(base.key("model").key("body"), e);
                }
            }
            (None, Some(p)) if !params.contains(p) => {
                err(base.key("model").key("path"), format!("{p:?} is not a parameter of the route path"));
            }
            (None, Some(_)) => {}
            _ => err(base.key("model"), "names exactly one of `body` or `path`".into()),
        }
    }
    if let Some(st) = &r.stream {
        match (&st.body, &st.path_suffix) {
            (Some(b), None) => {
                if let Err(e) = Selector::parse(b) {
                    err(base.key("stream").key("body"), e);
                }
            }
            (None, Some(suffix)) if !r.path.ends_with(suffix.as_str()) => {
                err(base.key("stream").key("path_suffix"), format!("{suffix:?} is not a suffix of the route path"));
            }
            (None, Some(_)) => {}
            _ => err(base.key("stream"), "names exactly one of `body` or `path_suffix`".into()),
        }
    }
    if let Some(d) = &r.discriminator {
        check_match_rule(d, &base.key("discriminator"), err);
    }
    let needs_codec = !matches!(r.op, RouteOp::ListModels | RouteOp::GetModel);
    if needs_codec && !s.has_codec(r.model_type) {
        err(base.key("type"), format!("route type {} has no [{}] codec section", r.model_type, r.model_type));
    }
    if r.op == RouteOp::CountTokens && s.text.as_ref().is_none_or(|t| t.count_tokens.is_none()) {
        err(base.key("op"), "count_tokens route needs [text.count_tokens]".into());
    }
    if matches!(r.op, RouteOp::JobSubmit | RouteOp::JobGet | RouteOp::JobContent)
        && s.type_codec(r.model_type).is_none_or(|c| c.job.is_none())
    {
        err(base.key("op"), format!("{} route needs a job declaration in [{}]", r.op, r.model_type));
    }
}

/// Parameter names in a route path. `{name}` may sit inside a segment with literal text
/// around it (`{model*}:generateContent`); `{name*}` must be in the final segment.
fn path_params(path: &str) -> Result<Vec<String>, String> {
    if !path.starts_with('/') {
        return Err(format!("route path {path:?} must start with `/`"));
    }
    let segs: Vec<&str> = path[1..].split('/').collect();
    let mut names = Vec::new();
    for (i, seg) in segs.iter().enumerate() {
        let opens = seg.matches('{').count();
        if opens != seg.matches('}').count() || opens > 1 {
            return Err(format!("path segment {seg:?}: at most one `{{name}}` per segment"));
        }
        let Some(open) = seg.find('{') else {
            if seg.contains(['*', ' ', '?', '#']) {
                return Err(format!("path segment {seg:?} has a reserved character"));
            }
            continue;
        };
        let close = seg.find('}').filter(|c| *c > open).ok_or_else(|| format!("path segment {seg:?} is malformed"))?;
        let inner = &seg[open + 1..close];
        let (name, rest) = match inner.strip_suffix('*') {
            Some(n) => (n, true),
            None => (inner, false),
        };
        if rest && i + 1 != segs.len() {
            return Err(format!("`{{{inner}}}` must be in the last path segment"));
        }
        if !is_token_underscore(name) {
            return Err(format!("`{{{inner}}}`: path placeholders are `{{name}}` or `{{name*}}`"));
        }
        names.push(name.to_owned());
    }
    Ok(names)
}

fn check_match_rule(m: &MatchRule, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    if m.header_present.is_none() && m.path_present.is_none() && m.path_equals.is_none() {
        err(base.clone(), "a match rule needs `header_present`, `path_present` or `path_equals`".into());
    }
    if let Some(Err(e)) = m.path_present.as_deref().map(Selector::parse) {
        err(base.key("path_present"), e);
    }
    match m.path_equals.as_deref() {
        Some([path, _]) => {
            if let Err(e) = Selector::parse(path) {
                err(base.key("path_equals"), e);
            }
        }
        Some(_) => err(base.key("path_equals"), "must be [path, value]".into()),
        None => {}
    }
}

fn check_text(t: &TextCodec, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    let layout = base.key("layout");
    for (k, v) in [("messages", &t.layout.messages), ("role", &t.layout.role), ("content", &t.layout.content)] {
        if let Err(e) = Selector::parse(v) {
            err(layout.key(k), e);
        }
    }
    for role in t.layout.roles.keys().filter(|r| !IR_ROLES.contains(&r.as_str())) {
        err(layout.key("roles").key(role.as_str()), format!("unknown role; allowed: {}", IR_ROLES.join(", ")));
    }
    if let Some(m) = &t.layout.message {
        check_template(m, placeholders::MESSAGE, &layout.key("message"), err);
    }

    let parts = base.key("parts");
    if !t.parts.contains_key(&PartKind::Text) {
        err(parts.clone(), "a text codec needs [text.parts.text]".into());
    }
    for (kind, p) in &t.parts {
        let pb = parts.key(kind.as_str());
        let set = match kind {
            PartKind::Text => placeholders::TEXT_PART,
            PartKind::Image | PartKind::Audio => placeholders::MEDIA_PART,
            PartKind::ToolCall => placeholders::TOOL_CALL_PART,
            PartKind::ToolResult => placeholders::TOOL_RESULT_PART,
            PartKind::Thinking => placeholders::THINKING_PART,
        };
        let media = matches!(kind, PartKind::Image | PartKind::Audio);
        match (media, p.codec) {
            (true, None) => err(pb.key("codec"), "a media part needs a codec".into()),
            (false, Some(_)) => err(pb.key("codec"), "only image and audio parts take a codec".into()),
            _ => {}
        }
        check_template(&p.data, set, &pb.key("data"), err);
        if let Some(m) = &p.match_ {
            check_template(m, set, &pb.key("match"), err);
        }
        for (role, v) in &p.roles {
            if !IR_ROLES.contains(&role.as_str()) {
                err(pb.key("roles").key(role.as_str()), format!("unknown role; allowed: {}", IR_ROLES.join(", ")));
            }
            check_template(v, set, &pb.key("roles").key(role.as_str()), err);
        }
        check_reversible(&p.data, &pb.key("data"), err);
    }

    if let Some(tools) = &t.tools {
        let tb = base.key("tools");
        if let Err(e) = Selector::parse(&tools.path) {
            err(tb.key("path"), e);
        }
        check_template(&tools.data, placeholders::TOOL, &tb.key("data"), err);
        if let Some(w) = &tools.wrap {
            check_template(w, placeholders::TOOLS_WRAP, &tb.key("wrap"), err);
        }
        if let Some(c) = &tools.choice {
            let cb = tb.key("choice");
            if let Err(e) = Selector::parse(&c.path) {
                err(cb.key("path"), e);
            }
            for (k, v) in [("auto", Some(&c.auto)), ("required", Some(&c.required)), ("none", c.none.as_ref())] {
                if let Some(v) = v {
                    check_template(v, placeholders::NONE, &cb.key(k), err);
                }
            }
            check_template(&c.named, placeholders::TOOL_CHOICE, &cb.key("named"), err);
        }
        for kind in [PartKind::ToolCall, PartKind::ToolResult] {
            if !t.parts.contains_key(&kind) {
                err(parts.clone(), format!("a style with tools needs [text.parts.{kind}]"));
            }
        }
    }

    for (name, decl) in &t.params {
        let pb = base.key("params").key(name.as_str());
        if !IR_PARAMS.contains(&name.as_str()) {
            err(pb.clone(), format!("unknown request parameter; allowed: {}", IR_PARAMS.join(", ")));
        }
        if let Err(e) = Selector::parse(decl.path()) {
            err(pb.key("path"), e);
        }
        match (name.as_str(), decl) {
            ("thinking", ParamDecl::Form { form, .. }) => {
                if ThinkingForm::parse(form).is_none() {
                    err(pb.key("form"), unknown("thinking form", form, ThinkingForm::ALLOWED));
                }
            }
            ("response_format", ParamDecl::Form { form, .. }) => {
                if ResponseFormatForm::parse(form).is_none() {
                    err(pb.key("form"), unknown("response format form", form, ResponseFormatForm::ALLOWED));
                }
            }
            ("thinking" | "response_format", ParamDecl::Path(_)) => {
                err(pb, format!("`{name}` needs `{{ path, form }}`"));
            }
            (_, ParamDecl::Form { .. }) => err(pb.key("form"), format!("`{name}` takes a plain field path")),
            ("max_tokens", ParamDecl::Default { .. }) => {}
            (_, ParamDecl::Default { .. }) => err(pb.key("default"), format!("only `max_tokens` takes a default, not `{name}`")),
            _ => {}
        }
    }

    check_finish(t, &base.key("finish"), err);
    check_usage_paths(&t.usage, &base.key("usage"), err);
    check_template(&t.response.body, placeholders::RESPONSE, &base.key("response").key("body"), err);
    check_reversible(&t.response.body, &base.key("response").key("body"), err);
    if let Some(m) = &t.response.match_ {
        check_template(m, placeholders::RESPONSE, &base.key("response").key("match"), err);
        check_reversible(m, &base.key("response").key("match"), err);
    }
    check_stream_events(&t.stream.events, &base.key("stream").key("events"), err);
    if let Some(c) = &t.count_tokens {
        check_template(&c.response, placeholders::COUNT, &base.key("count_tokens").key("response"), err);
    }
}

/// `finish` decodes every reason the style emits, and every IR reason has an outbound
/// style reason (explicit in `finish_out`, or the unique inverse of `finish`).
fn check_finish(t: &TextCodec, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    let mut missing = Vec::new();
    for reason in FinishReason::ALLOWED.iter().filter_map(|r| FinishReason::parse(r)) {
        if t.finish_out.contains_key(&reason) {
            continue;
        }
        if t.finish.values().filter(|v| **v == reason).count() != 1 {
            missing.push(reason.as_str());
        }
    }
    if !missing.is_empty() {
        err(
            base.clone(),
            format!(
                "finish map is not total: no unique style reason for IR {}; add them to finish_out",
                missing.join(", ")
            ),
        );
    }
    for (reason, style) in &t.finish_out {
        if !t.finish.contains_key(style) {
            err(
                base.key(style.as_str()),
                format!("finish_out.{reason} = {style:?} is not a reason in the finish map"),
            );
        }
    }
}

fn check_usage_paths(u: &crate::schema::UsageDecl, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    for (k, v) in [
        ("input", &u.input),
        ("output", &u.output),
        ("cache_read", &u.cache_read),
        ("cache_write", &u.cache_write),
        ("reasoning", &u.reasoning),
    ] {
        if let Some(Err(e)) = v.as_deref().map(Selector::parse) {
            err(base.key(k), e);
        }
    }
}

/// Stream decode rules must reverse unambiguously: no two rules with the same event name
/// and the same decode shape map to different IR events, and a rule without an event name
/// must pin at least one literal.
fn check_stream_events(events: &[StreamEventDecl], base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    let mut decoded: Vec<(usize, Option<&str>, Template)> = Vec::new();
    for (i, e) in events.iter().enumerate() {
        let eb = base.index(i);
        check_template(&e.data, placeholders::STREAM, &eb.key("data"), err);
        if let Some(m) = &e.match_ {
            check_template(m, placeholders::STREAM, &eb.key("match"), err);
        }
        if let Some(Err(er)) = e.when_request.as_deref().map(Selector::parse) {
            err(eb.key("when_request"), er);
        }
        let Ok(tpl) = Template::parse(e.match_.as_ref().unwrap_or(&e.data)) else { continue };
        check_reversible(e.match_.as_ref().unwrap_or(&e.data), &eb.key("data"), err);
        if e.event.is_none() && !is_selective(&tpl, true) && events.len() > 1 {
            err(
                eb.clone(),
                "ambiguous-stream-rules: a rule without an event name must pin a literal or a required field".into(),
            );
        }
        for (j, ev, other) in &decoded {
            if *ev == e.event.as_deref() && *other == tpl && events[*j].on != e.on {
                err(
                    eb.clone(),
                    format!(
                        "ambiguous-stream-rules: decodes the same frames as events[{j}] ({} vs {})",
                        events[*j].on, e.on
                    ),
                );
            }
        }
        decoded.push((i, e.event.as_deref(), tpl));
    }
}

fn check_errors(s: &StyleFile, err: &mut impl FnMut(FieldPath, String)) {
    let base = FieldPath::of("errors");
    check_template(&s.errors.body, placeholders::ERROR_BODY, &base.key("body"), err);
    if let Ok(t) = Template::parse(&s.errors.body) {
        for need in ["error.message", "error.details"] {
            if !t.mentions(need) {
                err(base.key("body"), format!("the error body must place {{{need}}}"));
            }
        }
    }
    for status in s.errors.type_map.keys() {
        if !status.parse::<u16>().is_ok_and(|n| (100..=599).contains(&n)) {
            err(base.key("type_map").key(status.as_str()), "keys are HTTP statuses 100-599".into());
        }
    }
    let missing: Vec<&str> =
        REQUIRED_ERROR_STATUSES.iter().copied().filter(|st| !s.errors.type_map.contains_key(*st)).collect();
    if !missing.is_empty() {
        err(base.key("type_map"), format!("type_map must cover {}", missing.join(", ")));
    }
    check_template(&s.errors.stream_event.data, placeholders::ERROR_EVENT, &base.key("stream_event").key("data"), err);
    if let Some(k) = &s.errors.keepalive {
        check_template(&k.data, placeholders::NONE, &base.key("keepalive").key("data"), err);
    }
}

fn check_template(v: &toml::Value, set: PlaceholderSet, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    let at = |e: crate::template::TemplateError| {
        if e.at.is_empty() { base.clone() } else { base.key(e.at) }
    };
    match Template::parse(v) {
        Err(e) => {
            let rule = e.rule.clone();
            err(at(e), rule);
        }
        Ok(t) => {
            if let Err(e) = t.check(set) {
                let rule = e.rule.clone();
                err(at(e), rule);
            }
            if let Some(lit) = credential_literal(&t) {
                err(base.clone(), format!("literal {lit:?} looks like a credential; style files carry no secrets"));
            }
        }
    }
}

/// Interpolations with two placeholders not separated by a literal can't be split back.
fn check_reversible(v: &toml::Value, base: &FieldPath, err: &mut impl FnMut(FieldPath, String)) {
    fn go(t: &Template) -> bool {
        match t {
            Template::Interp(pieces) => {
                pieces.windows(2).all(|w| !matches!(w, [Piece::Hole(_), Piece::Hole(_)]))
            }
            Template::Array(items) => items.iter().all(go),
            Template::Object(fields) => fields.iter().all(|(_, t)| go(t)),
            _ => true,
        }
    }
    if let Ok(t) = Template::parse(v)
        && !go(&t)
    {
        err(base.clone(), "ambiguous-stream-rules: adjacent placeholders can't be decoded".into());
    }
}

/// Whether a decode template rejects some frames: it pins a literal, or a required
/// placeholder below the root (a field that must be present).
fn is_selective(t: &Template, root: bool) -> bool {
    match t {
        Template::Bool(_) | Template::Int(_) | Template::Float(_) | Template::Str(_) | Template::Interp(_) => true,
        Template::Hole { optional, .. } => !root && !optional,
        Template::Array(items) => items.iter().any(|t| is_selective(t, false)),
        Template::Object(fields) => fields.iter().any(|(_, t)| is_selective(t, false)),
        Template::Null => false,
    }
}

/// A literal that looks like a key or bearer token (`sk-…`, `0r-…`, `AIza…`, `Bearer …`).
fn credential_literal(t: &Template) -> Option<String> {
    let looks = |s: &str| {
        let long = s.len() >= 20 && !s.contains(' ');
        s.starts_with("Bearer ") || (long && ["sk-", "0r-", "AIza", "xai-", "gsk_"].iter().any(|p| s.starts_with(p)))
    };
    match t {
        Template::Str(s) if looks(s) => Some(s.clone()),
        Template::Interp(pieces) => pieces.iter().find_map(|p| match p {
            Piece::Lit(s) if looks(s) => Some(s.clone()),
            _ => None,
        }),
        Template::Array(items) => items.iter().find_map(credential_literal),
        Template::Object(fields) => fields.iter().find_map(|(_, t)| credential_literal(t)),
        _ => None,
    }
}

fn unknown(what: &str, value: &str, allowed: &[&str]) -> String {
    format!("unknown {what} {value:?}; allowed: {}", allowed.join(", "))
}

fn is_token(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_token_underscore(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Letters, digits, `-` and `_` (Codex sends `session_id`).
fn is_header_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal valid style, shared with the loader tests.
    pub(crate) const BASE: &str = r#"
schema = 1
kind = "api-style"
id = "mini"

[access_key]
carriers = [{ header = "authorization", scheme = "bearer" }]

[[routes]]
method = "POST"
path = "/v1/chat"
op = "generate"
type = "text"
model = { body = "model" }
stream = { body = "stream" }

[[routes]]
method = "GET"
path = "/v1/models/{model*}"
op = "get_model"
type = "text"
model = { path = "model" }

[text.layout]
system = "first_message"
messages = "messages"
content = "content"
tool_calls = "message_field"
tool_results = "tool_role_message"
arguments = "json_string"

[text.parts.text]
data = { type = "text", text = "{part.text}" }

[text.finish]
stop = "stop"
length = "length"
tool_calls = "tool_calls"
content_filter = "content_filter"

[text.finish_out]
error = "stop"

[text.usage]
input = "usage.prompt_tokens"
input_semantics = "includes_cache"

[text.response]
body = { id = "{response.id}", choices = [{ message = { content = "{response.text}" }, finish_reason = "{response.finish}" }] }

[text.stream]
framing = "sse_data_done"
blocks = "implicit"
tool_arguments = "fragments"

[[text.stream.events]]
on = "text_delta"
data = { choices = [{ delta = { content = "{delta.text}" } }] }

[[text.stream.events]]
on = "finish"
data = { choices = [{ finish_reason = "{finish}" }] }

[errors]
body = { error = { type = "{error.type}", message = "{error.message}" }, zerorouter = "{error.details}" }
type_map = { 400 = "invalid_request_error", 401 = "authentication_error", 404 = "not_found_error", 429 = "rate_limit_error", 500 = "api_error", 503 = "api_error" }
stream_event = { data = "{error.body}" }
"#;

    fn rules(src: &str) -> Vec<String> {
        validate_style(src, "mini.toml").err().unwrap_or_default().iter().map(ToString::to_string).collect()
    }

    fn fails(src: &str, want: &str) {
        let got = rules(src);
        assert!(got.iter().any(|r| r.contains(want)), "want {want:?} in {got:#?}");
    }

    #[test]
    fn base_is_valid() {
        assert_eq!(rules(BASE), Vec::<String>::new());
    }

    #[test]
    fn corpus_cases() {
        fails(&BASE.replace("{part.text}", "{request.api_key}"), "text.parts.text.data.text: unknown placeholder {request.api_key}");
        fails(&BASE.replace("{part.text}", "{a+b}"), "expressions are not allowed");
        fails(&BASE.replace("/v1/models/{model*}", "/v1/{model*}/x"), "must be in the last path segment");
        fails(&BASE.replace("[text.parts.text]", "[text.parts.textx]"), "unknown part kind");
        fails(&BASE.replace("content_filter = \"content_filter\"\n", ""), "finish map is not total: no unique style reason for IR content_filter");
        fails(&BASE.replace("message = \"{error.message}\"", "message = \"x\""), "must place {error.message}");
        fails(&BASE.replace("503 = \"api_error\" ", ""), "type_map must cover 503");
        fails(&BASE.replace("scheme = \"bearer\"", "scheme = \"basic\""), "unknown key scheme \"basic\"");
        fails(&BASE.replace("header = \"authorization\", scheme = \"bearer\"", "query = \"key\", scheme = \"bearer\""), "scheme must be \"raw\"");
        fails(&BASE.replace("sse_data_done", "xml"), "unknown framing");
        fails(&format!("{BASE}\n[session]\ncarriers = [{{ extractor = \"guess\" }}]\n"), "unknown session extractor");
        fails(&BASE.replace("type = \"text\"\nmodel = { body", "type = \"image\"\nmodel = { body"), "route type image has no [image] codec section");
        fails(&format!("{BASE}\n[forwarding]\nx = 1\n"), "a style file may not declare forwarding");
        fails(
            &BASE.replace(
                "data = { choices = [{ finish_reason = \"{finish}\" }] }",
                "data = { choices = [{ delta = { content = \"{delta.text}\" } }] }",
            ),
            "ambiguous-stream-rules: decodes the same frames as events[0]",
        );
        fails(&BASE.replace("\"{delta.text}\"", "\"{delta.text}{finish}\""), "adjacent placeholders");
    }

    #[test]
    fn route_collisions_across_files() {
        let a = validate_style(BASE, "a.toml").unwrap();
        let b = validate_style(&BASE.replace("id = \"mini\"", "id = \"other\""), "b.toml").unwrap();
        let errs = check_route_collisions(&[("a.toml", BASE, &a), ("b.toml", BASE, &b)]);
        assert!(errs.iter().any(|e| e.to_string().contains("route collision on POST")), "{errs:#?}");

        let disc = BASE.replace(
            "stream = { body = \"stream\" }\n",
            "stream = { body = \"stream\" }\ndiscriminator = { header_present = \"anthropic-version\" }\n",
        );
        let b = validate_style(&disc, "b.toml").unwrap();
        let only_get = |errs: Vec<ValidationError>| errs.iter().all(|e| e.to_string().contains("on GET"));
        assert!(only_get(check_route_collisions(&[("a.toml", BASE, &a), ("b.toml", &disc, &b)])));
    }

    #[test]
    fn path_templates() {
        assert_eq!(path_params("/v1beta/models/{model}:generateContent").unwrap(), vec!["model"]);
        assert_eq!(path_params("/v1/videos/{id}/content").unwrap(), vec!["id"]);
        assert!(path_params("v1/x").is_err());
        assert!(path_params("/v1/{a}{b}").is_err());
        assert!(path_params("/v1/{a b}").is_err());
        assert_eq!(shape("/v1/{id}"), shape("/v1/{other}"));
    }
}
