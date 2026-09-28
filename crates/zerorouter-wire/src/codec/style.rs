//! A style file with its templates and selectors parsed once (the gate already proved
//! they parse and reverse).

use std::collections::BTreeMap;

use zerorouter_registry::schema::{
    BlockModel, FinishReason, Framing, InputSemantics, MediaCodec, ModelType, PartKind, Repair, RouteOp, StreamOn,
    StyleFile, TextLayout, ToolArgumentsMode, UsageDecl,
};
use zerorouter_registry::template::{FieldPath, Template};

use super::CodecError;

#[derive(Debug, Clone)]
pub struct Style {
    pub id: String,
    pub text: Option<TextStyle>,
    pub error_body: Template,
    pub error_types: BTreeMap<u16, String>,
    pub error_event: EventTpl,
    pub keepalive: Option<EventTpl>,
}

#[derive(Debug, Clone)]
pub struct TextStyle {
    pub layout: TextLayout,
    pub messages: FieldPath,
    pub message: Option<Template>,
    pub parts: BTreeMap<PartKind, PartTpl>,
    pub tools: Option<ToolsTpl>,
    pub params: Vec<ParamTpl>,
    pub finish_in: BTreeMap<String, FinishReason>,
    pub finish_out: BTreeMap<FinishReason, String>,
    pub usage: UsageSel,
    pub id_prefix: String,
    pub response: Template,
    pub framing: Framing,
    pub blocks: BlockModel,
    pub tool_arguments: ToolArgumentsMode,
    pub events: Vec<EventTpl>,
    pub count_response: Option<Template>,
    pub repairs: Vec<Repair>,
    /// Where the generate route reads the model and the stream flag, when in the body.
    pub model_path: Option<FieldPath>,
    pub stream_path: Option<FieldPath>,
}

#[derive(Debug, Clone)]
pub struct PartTpl {
    pub data: Template,
    pub matcher: Template,
    pub roles: BTreeMap<String, Template>,
    pub codec: Option<MediaCodec>,
}

impl PartTpl {
    /// The encode template for a message of `role` (a style role name).
    pub fn for_role(&self, role: &str) -> &Template {
        self.roles.get(role).unwrap_or(&self.data)
    }

    /// Every template a decoder should try.
    pub fn matchers(&self) -> impl Iterator<Item = &Template> {
        std::iter::once(&self.matcher).chain(self.roles.values())
    }
}

#[derive(Debug, Clone)]
pub struct ToolsTpl {
    pub path: FieldPath,
    pub data: Template,
    pub wrap: Option<Template>,
    pub choice: Option<ChoiceTpl>,
}

#[derive(Debug, Clone)]
pub struct ChoiceTpl {
    pub path: FieldPath,
    pub auto: Template,
    pub required: Template,
    pub none: Option<Template>,
    pub named: Template,
}

#[derive(Debug, Clone)]
pub struct ParamTpl {
    pub name: String,
    pub path: FieldPath,
    pub form: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UsageSel {
    pub input: Option<FieldPath>,
    pub output: Option<FieldPath>,
    pub cache_read: Option<FieldPath>,
    pub cache_write: Option<FieldPath>,
    pub reasoning: Option<FieldPath>,
    pub semantics: InputSemantics,
}

#[derive(Debug, Clone)]
pub struct EventTpl {
    pub on: Option<StreamOn>,
    pub event: Option<String>,
    pub data: Template,
    pub matcher: Template,
    pub when_request: Option<FieldPath>,
}

fn tpl(v: &toml::Value, at: &str) -> Result<Template, CodecError> {
    Template::parse(v).map_err(|e| CodecError::decode(at, e.to_string()))
}

fn path(s: &str) -> Result<FieldPath, CodecError> {
    FieldPath::parse(s).map_err(|e| CodecError::decode(s, e))
}

impl Style {
    pub fn compile(f: &StyleFile) -> Result<Self, CodecError> {
        let text = f.text.as_ref().map(|t| TextStyle::compile(f, t)).transpose()?;
        let error_types = f.errors.type_map.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.clone()))).collect();
        let event = |e: &zerorouter_registry::schema::StreamTemplate, at: &str| -> Result<EventTpl, CodecError> {
            let data = tpl(&e.data, at)?;
            Ok(EventTpl { on: None, event: e.event.clone(), matcher: data.clone(), data, when_request: None })
        };
        Ok(Self {
            id: f.id.clone(),
            text,
            error_body: tpl(&f.errors.body, "errors.body")?,
            error_types,
            error_event: event(&f.errors.stream_event, "errors.stream_event")?,
            keepalive: f.errors.keepalive.as_ref().map(|k| event(k, "errors.keepalive")).transpose()?,
        })
    }

    pub fn text(&self) -> Result<&TextStyle, CodecError> {
        self.text.as_ref().ok_or_else(|| CodecError::Missing { style: self.id.clone(), what: "text codec" })
    }

    /// The style's error type for an HTTP status: exact, else the class's `x00`, else 500's.
    pub fn error_type(&self, status: u16) -> &str {
        let class = status / 100 * 100;
        self.error_types
            .get(&status)
            .or_else(|| self.error_types.get(&class))
            .or_else(|| self.error_types.get(&500))
            .map_or("api_error", String::as_str)
    }
}

impl TextStyle {
    fn compile(f: &StyleFile, t: &zerorouter_registry::schema::TextCodec) -> Result<Self, CodecError> {
        let mut parts = BTreeMap::new();
        for (kind, d) in &t.parts {
            let at = format!("text.parts.{kind}");
            let data = tpl(&d.data, &at)?;
            let matcher = d.match_.as_ref().map(|m| tpl(m, &at)).transpose()?.unwrap_or_else(|| data.clone());
            let roles = d.roles.iter().map(|(r, v)| Ok((r.clone(), tpl(v, &at)?))).collect::<Result<_, CodecError>>()?;
            parts.insert(*kind, PartTpl { data, matcher, roles, codec: d.codec });
        }
        let tools = match &t.tools {
            Some(td) => Some(ToolsTpl {
                path: path(&td.path)?,
                data: tpl(&td.data, "text.tools.data")?,
                wrap: td.wrap.as_ref().map(|w| tpl(w, "text.tools.wrap")).transpose()?,
                choice: match &td.choice {
                    Some(c) => Some(ChoiceTpl {
                        path: path(&c.path)?,
                        auto: tpl(&c.auto, "choice.auto")?,
                        required: tpl(&c.required, "choice.required")?,
                        none: c.none.as_ref().map(|n| tpl(n, "choice.none")).transpose()?,
                        named: tpl(&c.named, "choice.named")?,
                    }),
                    None => None,
                },
            }),
            None => None,
        };
        let params = t
            .params
            .iter()
            .map(|(name, d)| {
                let form = match d {
                    zerorouter_registry::schema::ParamDecl::Form { form, .. } => Some(form.clone()),
                    zerorouter_registry::schema::ParamDecl::Path(_) => None,
                };
                Ok(ParamTpl { name: name.clone(), path: path(d.path())?, form })
            })
            .collect::<Result<_, CodecError>>()?;
        let mut finish_out: BTreeMap<FinishReason, String> = t.finish_out.clone();
        for (style_reason, ir) in &t.finish {
            let unique = t.finish.values().filter(|r| *r == ir).count() == 1;
            if unique {
                finish_out.entry(*ir).or_insert_with(|| style_reason.clone());
            }
        }
        let events = t
            .stream
            .events
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let at = format!("text.stream.events[{i}]");
                let data = tpl(&e.data, &at)?;
                let matcher = e.match_.as_ref().map(|m| tpl(m, &at)).transpose()?.unwrap_or_else(|| data.clone());
                let when_request = e.when_request.as_deref().map(path).transpose()?;
                Ok(EventTpl { on: Some(e.on), event: e.event.clone(), data, matcher, when_request })
            })
            .collect::<Result<_, CodecError>>()?;
        let generate = f.routes.iter().find(|r| r.op == RouteOp::Generate && r.model_type == ModelType::Text);
        let model_path = generate.and_then(|r| r.model.as_ref()?.body.as_deref()).map(path).transpose()?;
        let stream_path = generate.and_then(|r| r.stream.as_ref()?.body.as_deref()).map(path).transpose()?;
        Ok(Self {
            layout: t.layout.clone(),
            messages: path(&t.layout.messages)?,
            message: t.layout.message.as_ref().map(|m| tpl(m, "text.layout.message")).transpose()?,
            parts,
            tools,
            params,
            finish_in: t.finish.clone(),
            finish_out,
            usage: UsageSel::compile(&t.usage)?,
            id_prefix: t.response.id_prefix.clone(),
            response: tpl(&t.response.body, "text.response.body")?,
            framing: t.stream.framing,
            blocks: t.stream.blocks,
            tool_arguments: t.stream.tool_arguments,
            events,
            count_response: t.count_tokens.as_ref().map(|c| tpl(&c.response, "text.count_tokens")).transpose()?,
            repairs: t.repairs.clone(),
            model_path,
            stream_path,
        })
    }

    /// IR role → this style's role name.
    pub fn role_out(&self, ir: &str) -> String {
        self.layout.roles.get(ir).cloned().unwrap_or_else(|| ir.to_owned())
    }

    /// This style's role name → IR role name.
    pub fn role_in<'a>(&'a self, style: &'a str) -> &'a str {
        self.layout.roles.iter().find(|(_, v)| *v == style).map_or(style, |(k, _)| k.as_str())
    }

    pub fn finish_to_ir(&self, style_reason: &str) -> FinishReason {
        self.finish_in.get(style_reason).copied().unwrap_or(FinishReason::Stop)
    }

    pub fn finish_from_ir(&self, r: FinishReason) -> &str {
        self.finish_out.get(&r).map_or(r.as_str(), String::as_str)
    }
}

impl UsageSel {
    pub fn compile(u: &UsageDecl) -> Result<Self, CodecError> {
        let p = |s: &Option<String>| s.as_deref().map(path).transpose();
        Ok(Self {
            input: p(&u.input)?,
            output: p(&u.output)?,
            cache_read: p(&u.cache_read)?,
            cache_write: p(&u.cache_write)?,
            reasoning: p(&u.reasoning)?,
            semantics: u.input_semantics,
        })
    }
}
