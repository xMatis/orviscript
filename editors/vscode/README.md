# OrviScript for Visual Studio Code

Official language support for `.orvi` files: syntax and semantic highlighting, bracket matching, context-aware standard library completion, hover information, diagnostics, document symbols, navigation, rename, and formatting.

The extension starts the Rust `orvi-lsp` server. It detects repository builds and standard OrviScript installations automatically, then falls back to `PATH`. Set `orviscript.server.path` only to use a custom executable.

Contextual diagnostics such as JavaScript `function` used in place of OrviScript `fn` come from the shared Rust frontend. They provide standard squiggles and explicit Quick Fix actions. `orviscript.diagnostics.inlineMessages` controls the optional message at the end of the line, `orviscript.diagnostics.enabledCategories` filters compiled categories (an empty list enables all), and `orviscript.diagnostics.minimumSeverity` sets the lowest published rule severity.
