# OrviScript

OrviScript is an open-source, automation-first, general-purpose programming language powered by Rust.

Version 0.10.0 is an early but runnable language milestone. It includes a reusable lexer and parser, enforced type annotations, semantic checks, a bytecode-first VM for synchronous functions and collections with a compatible tree-walk fallback, explicit module exports, automation APIs, native task workflows, project manifests, local path packages with a deterministic lockfile, project-wide test discovery, async functions and I/O, structured concurrency on a current-thread executor, isolated CPU-parallel workers, standalone single-file executables, a Rust language server, and a Visual Studio Code extension with `.orvi` file recognition and icons, syntax and semantic highlighting, live diagnostics, contextual foreign-syntax suggestions and Quick Fix actions, standard library completion and hover help, document symbols, cross-file go-to-definition, references, rename, and formatting. The language remains incomplete and does not yet include whole-program type inference, a public package registry or module-graph embedding in standalone bundles.

> Write automation like a script. Run it like a system tool.

## Why OrviScript

- **Automation is part of the language:** `task`, `needs`, project-wide tests and a standard library for files, processes, HTTP, JSON and time.
- **Concurrency has explicit boundaries:** non-blocking I/O uses structured async tasks, while CPU work uses isolated workers on a bounded native thread pool.
- **Failures and cleanup are visible:** `Result`, `Option`, `?`, `defer`, `using`, cancellation and timeouts avoid hidden control flow in production scripts.
- **Tools share one Rust frontend:** the CLI, runtime, formatter and LSP agree on syntax, types, standard-library APIs and diagnostics.
- **Programs can become system tools:** the initial standalone format packages a single-file program and runtime into one integrity-checked executable.

OrviScript is not trying to match Python's library count or Rust's low-level control. Its intended niche is readable automation that remains predictable when it grows into a concurrent production tool.

## Roadmap

The current `0.10.0` work completes the first standalone format. The next planned steps are full module-graph embedding, capability-based execution controls and secret-safe diagnostics, then SSH/SFTP/fetch and a fast package manager. Native AOT compilation remains an experiment to pursue only when profiling demonstrates a practical benefit.

The repository documents implemented behavior separately from future goals; roadmap items must not be treated as available features.

## Building

Install the stable Rust toolchain, clone the repository, and run:

```sh
cargo build --workspace
cargo test --workspace
```

The executable is named `orviscript`:

```sh
cargo run --bin orviscript -- --help
cargo run --bin orviscript -- run examples/hello.orvi
cargo run --bin orviscript -- fmt path/to/program.orvi
cargo run --bin orviscript -- fmt --check path/to/program.orvi
cargo run --bin orviscript -- build path/to/program.orvi --output program
cargo run --bin orviscript -- inspect program
cargo run --bin orviscript -- test examples/v0_3/automation.orvi
cargo run --bin orviscript -- task link project.test_artifact examples/v0_3/automation.orvi
cargo run --bin orviscript -- task run project.test_artifact
```

Create a project in a new directory and run its entry file:

```sh
orviscript init hello-orvi
orviscript run hello-orvi/src/main.orvi
```

`init` creates `orvi.toml`, `src/main.orvi`, and a project `.gitignore`. It refuses to overwrite any of these files if they already exist.

Build a single-file program into a standalone executable containing the OrviScript runtime and a SHA-256-verified source payload:

```sh
orviscript build src/main.orvi --output my-tool
./my-tool
orviscript inspect my-tool
```

The initial standalone format supports programs without file-based imports, including imports from `std:`. Local and package imports are rejected with an explicit build error until full module-graph embedding is available. Existing output is preserved unless `--force` is passed.

`build` packages source and the runtime; it is not AOT compilation. A standalone program still uses the same parser, semantic analysis and VM as `run`, so bundling is a deployment feature rather than a promise of faster execution.

Inside a project, the manifest entry is used when the file argument is omitted:

```sh
orviscript run
orviscript check
orviscript test
```

The commands search the current directory and its parents for `orvi.toml`. Project entries must be relative `.orvi` paths contained in the project. `orviscript test` discovers `test_` tasks in all project source files and supports `--filter` and `--list`. The shared project index also powers cross-file diagnostics, completion, hover, go-to-definition, references, and rename in the VS Code extension.

Add and inspect a local path dependency:

```sh
orviscript add math --path ../math
orviscript install
orviscript deps
```

This writes a `[dependencies]` entry to `orvi.toml` and a deterministic `orvi.lock`. Import the dependency entry with `import { name } from "pkg:math"` or another contained source file with `pkg:math/src/file.orvi`. Package names must match their manifests; missing packages, conflicting names, traversal attempts and dependency cycles are rejected. Version 0.10.0 intentionally supports path packages only and reserves remote registry support for a later release.

The 0.10.0 subset supports mutable `let` bindings, immutable `const` bindings, optional type annotations on bindings and functions, distinct `void` and `null` types, numbers, strings, booleans, mutable reference lists and maps, `while`, typed `for` loops over lists and integer ranges, typed structs with fields and defaults, structural interfaces, `Option<T>`, `Result<T, E>`, exhaustive `match` expressions, error propagation with `?`, LIFO cleanup through `defer` and `using`, explicit exports, local, package and standard module imports, JSON, filesystem, path, environment, process, terminal, HTTP, time and worker APIs, explicit execution context paths, native workflow declarations, a project-wide test command and a global task registry. `async fn`, `await`, `Future<T>`, `Task<T>`, async filesystem/process/HTTP/timer APIs and scoped `task.spawn`/`join`/`cancel`/`timeout` run on a single-thread executor. `Worker<T>` and `worker.spawn` use a bounded native thread pool for isolated CPU-parallel code, with serializable messages, structured joining, cooperative cancellation and deadlines. Project manifests define a contained entry file and local dependencies, while semantic checks carry exported function signatures across local and package modules. The runtime enforces declared types and uses tracing GC for reference cycles.

The VM introduced in 0.8.2 compiles synchronous top-level functions, recursion, explicit returns, lists, maps, indexing, field access, collection mutation, `Option`, `Result`, `?` and `match`. Function slots and operand stacks are recycled between calls, recursion is bounded to 128 frames, and VM-created collection cycles are handled by tracing cleanup. Safe single-expression leaf functions are inlined with preserved argument order and runtime type checks; other functions retain normal call frames. Known integer expressions use specialized bytecode, and common slot assignments fuse load, arithmetic and store into one instruction while preserving checked overflow and division-by-zero errors. Worker threads keep a bounded cache of 64 compiled source chunks. Programs using closures, structs, imports, cleanup or async features continue through the compatible tree-walk backend.

Contextual syntax suggestions are defined in `diagnostics/diagnostic-rules.json` and compiled into static Rust lookup tables. For example, JavaScript `function greet() {}` reports `ORVI1001` and offers an explicit `function` → `fn` Quick Fix without changing source during `run`, `check` or `fmt`. See `diagnostics/README.md` for the versioned schema, supported contexts and fragment format. Inspect the catalog embedded in a binary with:

```sh
orviscript diagnostics hash
orviscript diagnostics export diagnostic-rules.compiled.json
```

Run isolated CPU work by passing source and a transferable input value. A completed worker returns a map containing `value` and captured `output`:

```orvi
async fn main() -> Result<void, string> {
    let handle: Worker<map<any>> = worker.spawn(
        "print(worker_input); worker_input * worker_input",
        12
    );
    let result = (await worker.join(handle))?;
    print(result.value);
    print(result.output);
    return Ok();
}

await main();
```

Worker messages support `void`, `null`, booleans, finite numbers, strings, lists, maps, `Option` and `Result`. Functions, futures, tasks, workers, modules, structs and cyclic values cannot cross the isolation boundary. Worker source cannot create nested workers. Each lexical scope waits for its unfinished workers before it exits.

Use `worker.status(handle)` for a nonblocking state check and `worker.stats()` for pool capacity, current load and lifetime counters. Set `ORVISCRIPT_WORKER_THREADS` to a value from 1 through 32 before starting OrviScript to cap the pool. Invalid values fall back to the detected CPU parallelism. A panic inside an isolated worker is converted to an error and does not terminate its pool thread.

The VS Code extension lives in `editors/vscode`. It provides document formatting through the same formatter used by the CLI. Build the language server, then launch a development window with the extension enabled:

```sh
cargo build --bin orvi-lsp
code --new-window --extensionDevelopmentPath=editors/vscode examples/hello.orvi
```

The extension finds the workspace build automatically. For another installation, set `orviscript.server.path` to the `orvi-lsp` executable or place it on `PATH`.

## Distribution

The release workflow builds portable archives for Windows, Linux, and macOS, an installable VSIX, SHA-256 checksum files, and a per-user Windows installer. The installer upgrades the same installation across versions, adds the CLI and LSP to `PATH`, registers `.orvi` with the OrviScript file icon, and can install or update the VS Code extension. See [installer/README.md](installer/README.md) for the release contract and local build command.

Project website: [orviscript.com](https://orviscript.com)

## License

OrviScript is licensed under the [Apache License 2.0](LICENSE).

