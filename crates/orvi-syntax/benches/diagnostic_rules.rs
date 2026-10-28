use std::hint::black_box;
use std::time::{Duration, Instant};

use orvi_syntax::{lex, parse, rule_diagnostics_for_tokens};

const ITERATIONS: usize = 200;
const SYNTHETIC_RULE_COUNT: usize = 1_000;
const SOURCE_LINES: usize = 10_000;

include!(concat!(env!("OUT_DIR"), "/diagnostic_rules_benchmark.rs"));

fn main() {
    let source = (0..SOURCE_LINES)
        .map(|number| format!("let value_{number} = {number};\n"))
        .collect::<String>();
    let lexed = lex(&source);
    assert!(lexed.errors.is_empty());
    let tokens = lexed
        .tokens
        .into_iter()
        .filter(|token| !token.kind.is_trivia())
        .collect::<Vec<_>>();
    assert!(rule_diagnostics_for_tokens(&source, &tokens).is_empty());
    assert!(!parse(&source).has_errors());

    let missing = median(ITERATIONS, || {
        for _ in 0..tokens.len() {
            black_box(synthetic_candidates(black_box("not_in_catalog")));
        }
    });
    let matching = median(ITERATIONS, || {
        for _ in 0..tokens.len() {
            black_box(synthetic_candidates(black_box("foreign_token_999")));
        }
    });
    let production_scan = median(ITERATIONS, || {
        black_box(rule_diagnostics_for_tokens(
            black_box(&source),
            black_box(&tokens),
        ));
    });
    let full_parse = median(50, || {
        black_box(parse(black_box(&source)));
    });
    let scan_share = production_scan.as_secs_f64() / full_parse.as_secs_f64() * 100.0;

    println!("diagnostic rules benchmark");
    println!("synthetic catalog: {SYNTHETIC_RULE_COUNT} rules");
    println!("source: {SOURCE_LINES} lines, {} tokens", tokens.len());
    println!(
        "indexed missing lookup: {:.2} ns/token",
        ns_per_token(missing, tokens.len())
    );
    println!(
        "indexed matching lookup: {:.2} ns/token",
        ns_per_token(matching, tokens.len())
    );
    println!(
        "production rule scan: {:.2} ns/token",
        ns_per_token(production_scan, tokens.len())
    );
    println!(
        "full parse: {:.2} ns/token",
        ns_per_token(full_parse, tokens.len())
    );
    println!("rule scan upper-bound share of parse: {scan_share:.2}%");
}

fn median(iterations: usize, mut operation: impl FnMut()) -> Duration {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        operation();
        samples.push(started.elapsed());
    }
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn ns_per_token(duration: Duration, tokens: usize) -> f64 {
    duration.as_nanos() as f64 / tokens as f64
}
