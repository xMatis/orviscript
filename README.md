# OrviScript

OrviScript is an open-source, automation-first, general-purpose programming language powered by Rust.

Version 0.3.0 is an early but runnable language milestone. It includes a reusable lexer and parser, enforced type annotations, semantic checks, a tree-walk interpreter, explicit module exports, automation APIs, native task workflows, a Rust language server, and a Visual Studio Code extension with `.orvi` file recognition and icons, syntax and semantic highlighting, live diagnostics, completion, hover help, document symbols, go-to-definition, references, rename, and formatting. The language remains incomplete and does not yet include whole-program type inference, async I/O or packages.

## Goals

The planned direction includes:

- An interpreter-focused runtime implemented in Rust.
- A standard library for reliable I/O and automation.
- Async I/O, parallel execution, and isolated workers.
- Standalone executable bundling.
- Experimental native compilation at a later stage.

The repository documents implemented behavior separately from future goals; later roadmap items must not be treated as available features.

## Roadmap

Development will begin with the language core and interpreter. Standard library APIs, tooling, task automation, packaging, standalone bundling, and experimental native compilation will follow in later phases.

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

The 0.3.0 subset supports mutable `let` bindings, immutable `const` bindings, optional type annotations on bindings and functions, distinct `void` and `null` types, numbers, strings, booleans, mutable reference lists and maps, typed structs with fields and defaults, structural interfaces, `Option<T>`, `Result<T, E>`, exhaustive `match` expressions, error propagation with `?`, LIFO cleanup through `defer` and `using`, explicit exports, local and standard module imports, JSON, filesystem, path, environment, process and terminal APIs, explicit execution context paths, native `task` declarations, a sequential dependency scheduler, a test command and a global task registry. Semantic checks report undefined and duplicate names, immutable assignments, invalid top-level returns, incorrect argument counts, unknown types, non-exhaustive matches, task dependency errors and statically visible type mismatches. The runtime also enforces declared binding, parameter, return, collection, struct field, interface and task result types. A tracing collector reclaims unreachable collection, map, struct and closure-environment cycles.

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

