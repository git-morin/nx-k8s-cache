use prometheus::{Encoder, IntCounterVec, Opts, Registry, TextEncoder};
use std::sync::LazyLock;

pub static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::new);

/// `nx_cache_ops_total{op="get|put", result="hit|miss|stored|conflict|forbidden|invalid|error"}`
pub static CACHE_OPS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let counter = IntCounterVec::new(
        Opts::new("nx_cache_ops_total", "Cache operations by type and result"),
        &["op", "result"],
    )
    .unwrap();
    REGISTRY.register(Box::new(counter.clone())).unwrap();
    counter
});

pub fn init() {
    LazyLock::force(&CACHE_OPS);
}

pub fn render() -> String {
    let encoder = TextEncoder::new();
    let mut buffer = Vec::new();
    encoder
        .encode(&REGISTRY.gather(), &mut buffer)
        .unwrap_or_default();
    String::from_utf8(buffer).unwrap_or_default()
}
