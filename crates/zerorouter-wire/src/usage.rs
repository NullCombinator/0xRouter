//! Per-style usage extraction (research R13).
//!
//! IR `input` counts uncached input tokens only. A style whose reported input includes
//! cached tokens (`input_semantics = "includes_cache"`) is converted both ways.

use serde_json::Value;
use zerorouter_registry::schema::InputSemantics;

use crate::codec::style::UsageSel;
use crate::ir::Usage;
use crate::template::{Bindings, select_one};

/// Usage from `usage.*` template bindings, plus the style's declared paths in `body` when
/// given. Raw counts are merged before the cache conversion, so a binding without the
/// cache count can't undo it.
pub fn read(sel: &UsageSel, body: Option<&Value>, b: &Bindings) -> Usage {
    let mut u = body.map_or_else(Usage::default, |v| {
        let get = |p: &Option<zerorouter_registry::template::FieldPath>| p.as_ref().and_then(|p| select_one(p, v)?.as_u64());
        Usage {
            input: get(&sel.input),
            output: get(&sel.output),
            cache_read: get(&sel.cache_read),
            cache_write: get(&sel.cache_write),
            reasoning: get(&sel.reasoning),
        }
    });
    u.merge(Usage {
        input: b.u64("usage.input"),
        output: b.u64("usage.output"),
        cache_read: b.u64("usage.cache_read"),
        cache_write: b.u64("usage.cache_write"),
        reasoning: b.u64("usage.reasoning"),
    });
    normalise(sel.semantics, u)
}

fn normalise(semantics: InputSemantics, mut u: Usage) -> Usage {
    if semantics == InputSemantics::IncludesCache
        && let Some(i) = &mut u.input
    {
        *i = i.saturating_sub(u.cache_read.unwrap_or(0)).saturating_sub(u.cache_write.unwrap_or(0));
    }
    u
}

/// `usage.*` bindings in a style's semantics. `usage.total` is input (as the style reports
/// it) plus output.
pub fn bind(semantics: InputSemantics, u: &Usage, b: &mut Bindings) {
    let cached = u.cache_read.unwrap_or(0) + u.cache_write.unwrap_or(0);
    let input = u.input.map(|i| if semantics == InputSemantics::IncludesCache { i + cached } else { i });
    let fields = [
        ("usage.input", input),
        ("usage.output", u.output),
        ("usage.cache_read", u.cache_read),
        ("usage.cache_write", u.cache_write),
        ("usage.reasoning", u.reasoning),
        ("usage.total", (input.is_some() || u.output.is_some()).then(|| input.unwrap_or(0) + u.output.unwrap_or(0))),
    ];
    for (k, v) in fields {
        if let Some(v) = v {
            b.set(k, v);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn includes_cache_round_trips() {
        let b = Bindings::new().with("usage.input", 100).with("usage.cache_read", 30).with("usage.output", 5);
        let sel = |semantics| UsageSel { input: None, output: None, cache_read: None, cache_write: None, reasoning: None, semantics };
        let u = read(&sel(InputSemantics::IncludesCache), None, &b);
        assert_eq!(u, read(&sel(InputSemantics::IncludesCache), Some(&serde_json::json!({})), &b));
        assert_eq!((u.input, u.cache_read), (Some(70), Some(30)));
        let mut out = Bindings::new();
        bind(InputSemantics::IncludesCache, &u, &mut out);
        assert_eq!((out.u64("usage.input"), out.u64("usage.total")), (Some(100), Some(105)));
        let mut out = Bindings::new();
        bind(InputSemantics::ExcludesCache, &u, &mut out);
        assert_eq!(out.u64("usage.input"), Some(70));
    }
}
