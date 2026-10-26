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

Use `-Warmup` and `-Runs` to control sample counts. Reports are written to the ignored `artifacts/benchmarks/` directory as JSON and Markdown. Compare results only on the same machine, power plan and background-load conditions.

Workloads:

- `startup.orvi`: process and minimal-language startup cost;
- `sync_loop.orvi`: 100,000 interpreted loop iterations;
- `async_10000_tasks.orvi`: the maximum 10,000 lightweight tasks in one scope;
- `async_timers.orvi`: 1,000 concurrent timers.
