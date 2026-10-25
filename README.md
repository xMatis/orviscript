# OrviScript

OrviScript is an open-source, automation-first, general-purpose programming language powered by Rust.

The project is at an early but runnable stage. It includes a reusable lexer and parser, semantic checks, a tree-walk interpreter, `run` and `check` CLI commands, a Rust language server, and a Visual Studio Code extension with `.orvi` file recognition, syntax and semantic highlighting, live diagnostics, initial completion, hover help, go-to-definition, references, and rename. The language remains incomplete and does not yet include modules, static types, async I/O, packages, or a standard library beyond `print`.

## Goals

The planned direction includes:

- An interpreter-focused runtime implemented in Rust.
- A standard library for reliable I/O and automation.
- Async I/O, parallel execution, and isolated workers.
- Standalone executable bundling.
- Experimental native compilation at a later stage.

Except for the basic interpreter described above, these capabilities are goals, not implemented features.

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
```

The current language subset supports mutable `let` bindings, immutable `const` bindings, optional type annotations on bindings and functions, numbers, strings, booleans, `null`, mutable reference lists with indexing, typed structs with fields and defaults, `Option<T>`, `Result<T, E>`, exhaustive `match` expressions, error propagation with `?`, LIFO cleanup through `defer` and `using`, arithmetic and comparison operators, `if`/`else`, `while`, functions, closures, `return`, assignment, and `print`. Semantic checks report undefined and duplicate names, immutable assignments, invalid top-level returns, incorrect argument counts, unknown types, non-exhaustive matches, and statically visible type mismatches. The runtime also enforces declared binding, parameter, return, collection, and struct field types. A tracing collector reclaims unreachable reference-list, struct, and closure-environment cycles.

The VS Code extension lives in `editors/vscode`. It provides document formatting through the same formatter used by the CLI. Build the language server, then launch a development window with the extension enabled:

```sh
cargo build --bin orvi-lsp
code --new-window --extensionDevelopmentPath=editors/vscode examples/hello.orvi
```

The extension finds the workspace build automatically. For another installation, set `orviscript.server.path` to the `orvi-lsp` executable or place it on `PATH`.

Project website: [orviscript.com](https://orviscript.com)

## License

OrviScript is licensed under the [Apache License 2.0](LICENSE).

