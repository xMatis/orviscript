use std::hint::black_box;
use std::time::{Duration, Instant};

use orvi_runtime::execute;
use orvi_sema::analyze_program;
use orvi_syntax::{Program, parse};

const SOURCE: &str = include_str!("../../../benchmarks/comparison.orvi");

#[derive(Clone, Copy)]
struct Measurement {
    median: Duration,
    mean: Duration,
    min: Duration,
    max: Duration,
}

fn measure<T>(samples: usize, warmups: usize, mut operation: impl FnMut() -> T) -> Measurement {
    for _ in 0..warmups {
        black_box(operation());
    }

    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let result = operation();
        durations.push(started.elapsed());
        black_box(result);
    }
    durations.sort_unstable();
    let total = durations.iter().copied().sum::<Duration>();

    Measurement {
        median: durations[durations.len() / 2],
        mean: total / samples as u32,
        min: durations[0],
        max: durations[durations.len() - 1],
    }
}

fn print_measurement(name: &str, measurement: Measurement, samples: usize) {
    println!(
        "{name:<24} median={:>12.3?} mean={:>12.3?} min={:>12.3?} max={:>12.3?} samples={samples}",
        measurement.median, measurement.mean, measurement.min, measurement.max
    );
}

fn main() {
    let parsed = parse(SOURCE);
    assert!(!parsed.has_errors(), "comparison.orvi must parse");
    let analysis = analyze_program(&parsed.program);
    assert!(
        analysis.errors.is_empty(),
        "comparison.orvi must pass semantic analysis"
    );
    let execution = execute(&parsed.program).expect("comparison.orvi must execute");
    assert_eq!(execution.output, ["499999500000"]);

    let parse_samples = 20_000;
    let parse_time = measure(parse_samples, 100, || parse(black_box(SOURCE)));

    let semantic_samples = 20_000;
    let semantic_time = measure(semantic_samples, 100, || {
        analyze_program(black_box(&parsed.program))
    });

    let empty_program = Program {
        statements: Vec::new(),
    };
    let startup_samples = 500;
    let runtime_startup = measure(startup_samples, 10, || {
        execute(black_box(&empty_program)).expect("empty program must execute")
    });

    let execution_samples = 10;
    let execution_time = measure(execution_samples, 2, || {
        execute(black_box(&parsed.program)).expect("comparison.orvi must execute")
    });

    let pipeline_samples = 10;
    let pipeline_time = measure(pipeline_samples, 2, || {
        let parsed = parse(black_box(SOURCE));
        assert!(!parsed.has_errors());
        let analysis = analyze_program(&parsed.program);
        assert!(analysis.errors.is_empty());
        execute(&parsed.program).expect("comparison.orvi must execute")
    });

    println!("OrviScript internal pipeline benchmark (optimized Rust benchmark profile)");
    print_measurement("parser", parse_time, parse_samples);
    print_measurement("semantic analysis", semantic_time, semantic_samples);
    print_measurement("empty runtime startup", runtime_startup, startup_samples);
    print_measurement("execute 1M loop", execution_time, execution_samples);
    print_measurement("complete in-process", pipeline_time, pipeline_samples);

    let execution_without_startup = execution_time.median.saturating_sub(runtime_startup.median);
    let accounted = parse_time.median + semantic_time.median + execution_time.median;
    println!(
        "estimated loop execution excluding runtime startup: {:.3?}",
        execution_without_startup
    );
    println!(
        "median stage share: parser={:.4}% semantic={:.4}% runtime+execution={:.4}%",
        parse_time.median.as_secs_f64() / accounted.as_secs_f64() * 100.0,
        semantic_time.median.as_secs_f64() / accounted.as_secs_f64() * 100.0,
        execution_time.median.as_secs_f64() / accounted.as_secs_f64() * 100.0
    );
}
