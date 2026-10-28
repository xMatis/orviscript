# OrviScript diagnostic rules

`diagnostic-rules.json` is the central, build-time source for contextual syntax suggestions used by the parser, CLI and LSP. The JSON is validated and compiled into Rust lookup tables by `crates/orvi-syntax/build.rs`; it is never parsed while an OrviScript program runs.

## Adding a rule

Add an object to `rules` in `diagnostic-rules.json` and rebuild the workspace:

```json
{
  "id": "ORVI1001",
  "category": "foreign_syntax",
  "match": {
    "token": "function",
    "context": "function_declaration"
  },
  "message": "Unknown syntax. Did you mean 'fn'?",
  "replacement": "fn",
  "severity": "error",
  "quickFix": true,
  "priority": 100,
  "sourceLanguages": ["javascript", "typescript"]
}
```

The generated diagnostic reports an error but does not change source code. Editors may expose `replacement` as an explicit Quick Fix.

Fields:

- `id`: stable `ORVI` code followed by four digits; it must be unique.
- `category`: machine-readable grouping such as `foreign_syntax`.
- `match.token`: one exact token used for the generated lookup bucket.
- `match.context`: AST/token context in which the rule is allowed to match.
- `message`: diagnostic shown by the CLI and LSP.
- `replacement`: one OrviScript token suggested by the Quick Fix.
- `severity`: `error`, `warning`, `information` or `hint`.
- `quickFix`: whether the replacement is exposed as an editor action.
- `priority`: integer from `-10000` to `10000`; the highest matching rule wins.
- `sourceLanguages`: optional documentation metadata for the syntax being recognized.

Supported contexts are `function_declaration`, `variable_declaration`, `condition`, `loop`, `type_position`, `import`, `return` and `expression`.

## Splitting a large catalog

Keep `diagnostic-rules.json` as the entry point and add repository-relative fragment paths to `includes`:

```json
{
  "$schema": "./diagnostic-rules.schema.json",
  "schemaVersion": 1,
  "includes": ["rules/javascript.json", "rules/python.json"],
  "rules": []
}
```

Each fragment contains only a `rules` array. Absolute paths and `..` traversal are rejected. Duplicate includes, rule IDs and ambiguous token/context/priority combinations fail the Rust build.

## Validation and release artifact

Run `cargo test -p orvi-syntax` after editing the catalog. To inspect the exact flattened catalog embedded in a binary, run:

```text
orviscript diagnostics hash
orviscript diagnostics export diagnostic-rules.compiled.json
```

Release archives include `diagnostic-rules.compiled.json`. Its content and hash come from the executable, so it cannot silently diverge from the rules used by CLI and LSP.

The flattened artifact contains `schemaVersion` and `catalogHash`. The hash is calculated from the deterministic merged catalog before the metadata field is added and is also printed by `orviscript diagnostics hash`.
