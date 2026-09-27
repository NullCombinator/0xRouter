//! R11 performance gate: full load < 50 ms, `resolve` p50 < 1 µs.
//!
//! `cargo bench -p zerorouter-registry --bench resolve -- --save-baseline slice-002`

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use zerorouter_registry::{OperatorHome, RegistryHandle};

const CONFIG: &str = r#"
schema = 1

[[unified_model]]
name = "sonnet"
members = [
  { provider = "kr", model = "claude-sonnet-4-5" },
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]

[[unified_model]]
name = "embed"
members = [{ provider = "openai", model = "text-embedding-3-large" }]
"#;

fn bench(c: &mut Criterion) {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), CONFIG).unwrap();

    c.bench_function("load", |b| b.iter(|| RegistryHandle::open(OperatorHome::new(black_box(home.path()))).unwrap()));

    let reg = RegistryHandle::open(OperatorHome::new(home.path())).unwrap().snapshot();
    let mut group = c.benchmark_group("resolve");
    for (name, target) in [
        ("direct", "kr/claude-sonnet-4-5"),
        ("direct_suffix", "kr/claude-sonnet-4-5(high)"),
        ("unified", "sonnet"),
        ("uncatalogued", "openai/brand-new-model"),
        ("not_found", "claude-sonnet-4.5"),
    ] {
        group.bench_function(name, |b| b.iter(|| drop(black_box(reg.resolve(black_box(target))))));
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
