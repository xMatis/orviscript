# OrviScript for Visual Studio Code

Official language support for `.orvi` files: syntax and semantic highlighting, bracket matching, context-aware standard library completion, hover information, diagnostics, document symbols, navigation, rename, and formatting.

The extension starts the Rust `orvi-lsp` server. It detects repository builds and standard OrviScript installations automatically, then falls back to `PATH`. Set `orviscript.server.path` only to use a custom executable.
