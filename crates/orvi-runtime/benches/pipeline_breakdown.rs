use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use orvi_runtime::execute;
use orvi_sema::analyze_program;
use orvi_syntax::{Program, parse};

const SOURCE: &str = include_str!("../../../benchmarks/comparison.orvi");
const FUNCTION_SOURCE: &str = "fn add_one(value: int) -> int { return value + 1; } let index = 0; let total = 0; while index < 1000000 { total = add_one(total); index = index + 1; } total";
const COLLECTION_SOURCE: &str = "let values = [0, 0, 0, 0]; let record = { total: 0 }; let index = 0; while index < 250000 { let slot = index % 4; values[slot] = values[slot] + 1; record.total = record.total + values[slot]; index = index + 1; } record.total";
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static DEALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) }
    }
}

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

    ALLOCATIONS.store(0, Ordering::Relaxed);
    DEALLOCATIONS.store(0, Ordering::Relaxed);
    let allocation_probe = execute(&parsed.program).expect("comparison.orvi must execute");
    let execution_allocations = ALLOCATIONS.load(Ordering::Relaxed);
    let execution_deallocations = DEALLOCATIONS.load(Ordering::Relaxed);
    black_box(&allocation_probe);

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

    let functions = parse(FUNCTION_SOURCE);
    assert!(!functions.has_errors());
    let function_time = measure(execution_samples, 2, || {
        execute(black_box(&functions.program)).expect("function benchmark must execute")
    });

    let collections = parse(COLLECTION_SOURCE);
    assert!(!collections.has_errors());
    let collection_time = measure(execution_samples, 2, || {
        execute(black_box(&collections.program)).expect("collection benchmark must execute")
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
    print_measurement("execute 1M calls", function_time, execution_samples);
    print_measurement("collections 250K", collection_time, execution_samples);
    print_measurement("complete in-process", pipeline_time, pipeline_samples);
    println!(
        "execute 1M allocations: {execution_allocations} allocations, {execution_deallocations} deallocations before result drop"
    );

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
