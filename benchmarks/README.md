# OrviScript benchmarks

The benchmark suite measures the optimized CLI end to end, including process startup, parsing, semantic analysis and execution.

With the repository's Rust 1.95 toolchain, install a compatible Hyperfine release:

```powershell
cargo install hyperfine --version 1.20.0 --locked
```

Run the standard suite from the repository root:

```powershell
./scripts/benchmarks/Run-OrviBenchmarks.ps1
```

Compare the same one-million-iteration workload in OrviScript, JavaScript and Python:

```powershell
./scripts/benchmarks/Compare-Languages.ps1
```

The comparison script verifies that all three programs print the same checksum before measuring them. It uses the installed `orviscript` command when available and otherwise falls back to `target/release/orviscript`. Pass `-OrviExecutable` to select a downloaded release explicitly.

Measure the parser, semantic analyzer, runtime startup and interpreter separately inside one optimized Rust process:

```powershell
cargo bench -p orvi-runtime --bench pipeline_breakdown
```

Measure compiled diagnostic lookup with a synthetic 1,000-rule catalog and a 10,000-line source file:

```powershell
cargo bench -p orvi-syntax --bench diagnostic_rules
```

The benchmark reports indexed missing/matching lookup and the production token scan separately. The production engine uses generated token buckets, so unrelated catalog entries are not scanned for each source token.

Compare sequential CPU work, individual worker handles and `parallel.map` for 1, 2, 4 and 8 jobs:

```powershell
cargo bench -p orvi-runtime --bench worker_scaling
```

Use the `profiling` Cargo profile when collecting CPU profiles. It keeps debug symbols while retaining release optimizations:

```powershell
cargo build --profile profiling --locked --bin orviscript
```

Use `-Warmup` and `-Runs` to control sample counts. Reports are written to the ignored `artifacts/benchmarks/` directory as JSON and Markdown. Compare results only on the same machine, power plan and background-load conditions.

Workloads:

- `startup.orvi`: process and minimal-language startup cost;
- `sync_loop.orvi`: 100,000 interpreted loop iterations;
- `for_loop.orvi`: one million iterations through the optimized integer-range `for` path;
- `loop_control.orvi`: one million range iterations with conditional `continue` and final `break`;
- `async_10000_tasks.orvi`: the maximum 10,000 lightweight tasks in one scope;
- `async_timers.orvi`: 1,000 concurrent timers;
- `comparison.orvi`, `comparison.js`, `comparison.py`: the same one-million-iteration integer summation, including process startup and program execution.
