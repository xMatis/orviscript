use std::time::Instant;

use orvi_runtime::execute;
use orvi_syntax::parse;

const WORKER_BODY: &str = "let index = 0; let total = 0; while index < worker_input { total = total + index; index = index + 1; } total";

fn worker_program(count: usize, iterations: i64) -> String {
    let mut source = String::from("async fn main() -> Result<void, string> {");
    for index in 0..count {
        source.push_str(&format!(
            "let worker_{index} = worker.spawn(\"{WORKER_BODY}\", {iterations});"
        ));
    }
    for index in 0..count {
        source.push_str(&format!("(await worker.join(worker_{index}))?;"));
    }
    source.push_str("return Ok(); } await main();");
    source
}

fn sequential_program(count: usize, iterations: i64) -> String {
    format!(
        "let job = 0; while job < {count} {{ let index = 0; let total = 0; while index < {iterations} {{ total = total + index; index = index + 1; }} job = job + 1; }}"
    )
}

fn measure(label: &str, source: &str) {
    let parsed = parse(source);
    assert!(!parsed.has_errors(), "benchmark source must parse");
    let started = Instant::now();
    execute(&parsed.program).expect("benchmark source must execute");
    println!("{label:<16} {:?}", started.elapsed());
}

fn main() {
    let iterations = 1_000_000;
    for count in [1, 2, 4, 8] {
        measure(
            &format!("sequential-{count}"),
            &sequential_program(count, iterations),
        );
        measure(
            &format!("workers-{count}"),
            &worker_program(count, iterations),
        );
    }
}
