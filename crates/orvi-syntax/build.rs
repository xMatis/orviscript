use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

mod diagnostic_build_support;
use diagnostic_build_support::{Catalog, Fragment, Rule, sort_rules, validate_rules};

fn main() {
    if let Err(error) = generate() {
        panic!("invalid OrviScript diagnostic rules: {error}");
    }
}

fn generate() -> Result<(), String> {
    let crate_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|error| error.to_string())?);
    let diagnostics_dir = crate_dir.join("../../diagnostics");
    let manifest_path = diagnostics_dir.join("diagnostic-rules.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!(
        "cargo:rerun-if-changed={}",
        diagnostics_dir
            .join("diagnostic-rules.schema.json")
            .display()
    );

    let mut catalog: Catalog = read_json(&manifest_path)?;
    if catalog.schema_version != 1 {
        return Err(format!(
            "unsupported schemaVersion {}, expected 1",
            catalog.schema_version
        ));
    }
    let mut included = HashSet::new();
    for include in catalog.includes.clone() {
        if !included.insert(include.clone()) {
            return Err(format!("duplicate include '{include}'"));
        }
        let relative = Path::new(&include);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(format!("include '{include}' must stay inside diagnostics/"));
        }
        let path = diagnostics_dir.join(relative);
        println!("cargo:rerun-if-changed={}", path.display());
        let fragment: Fragment = read_json(&path)?;
        catalog.rules.extend(fragment.rules);
    }

    validate_rules(&catalog.rules)?;
    sort_rules(&mut catalog.rules);
    catalog.includes.clear();

    let canonical = serde_json::to_string(&catalog).map_err(|error| error.to_string())?;
    let hash = fnv1a64(canonical.as_bytes());
    let mut compiled = serde_json::to_value(&catalog).map_err(|error| error.to_string())?;
    compiled
        .as_object_mut()
        .ok_or_else(|| "compiled diagnostic catalog must be an object".to_owned())?
        .insert(
            "catalogHash".into(),
            serde_json::Value::String(format!("{hash:016x}")),
        );
    let normalized = serde_json::to_string_pretty(&compiled).map_err(|error| error.to_string())?;
    let output_dir = PathBuf::from(env::var("OUT_DIR").map_err(|error| error.to_string())?);
    fs::write(
        output_dir.join("diagnostic-rules.compiled.json"),
        format!("{normalized}\n"),
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        output_dir.join("diagnostic_rules.rs"),
        rust_source(&catalog.rules, hash),
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        output_dir.join("diagnostic_rules_benchmark.rs"),
        benchmark_source(1_000),
    )
    .map_err(|error| error.to_string())?;
    println!("cargo:rustc-env=ORVI_DIAGNOSTIC_RULES_HASH={hash:016x}");
    Ok(())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("could not read '{}': {error}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|error| format!("could not parse '{}': {error}", path.display()))
}

fn rust_source(rules: &[Rule], hash: u64) -> String {
    let uses_non_identifiers = rules
        .iter()
        .any(|rule| !is_identifier_token(&rule.matcher.token));
    let mut source = format!(
        "pub const DIAGNOSTIC_RULES_HASH: &str = \"{hash:016x}\";\n\
         const GENERATED_USES_NON_IDENTIFIERS: bool = {uses_non_identifiers};\n\
         static GENERATED_RULES: &[GeneratedRule] = &[\n"
    );
    for rule in rules {
        let languages = rule
            .source_languages
            .iter()
            .map(|language| format!("{:?}", language))
            .collect::<Vec<_>>()
            .join(", ");
        source.push_str(&format!(
            "    GeneratedRule {{ id: {:?}, category: {:?}, token: {:?}, context: DiagnosticContext::{}, message: {:?}, replacement: {:?}, severity: RuleSeverity::{}, quick_fix: {}, priority: {}, source_languages: &[{}] }},\n",
            rule.id,
            rule.category,
            rule.matcher.token,
            context_variant(&rule.matcher.context),
            rule.message,
            rule.replacement,
            severity_variant(&rule.severity),
            rule.quick_fix,
            rule.priority,
            languages,
        ));
    }
    source.push_str("];\n");
    let mut groups = Vec::new();
    let mut start = 0;
    while start < rules.len() {
        let token = &rules[start].matcher.token;
        let mut end = start + 1;
        while end < rules.len() && rules[end].matcher.token == *token {
            end += 1;
        }
        groups.push((token.clone(), start, end));
        start = end;
    }
    let first_bytes = first_byte_filter(&groups);
    let buckets = hash_buckets(groups);
    source.push_str(&format!(
        "static GENERATED_FIRST_BYTES: [u64; 4] = {first_bytes:?};\n"
    ));
    source.push_str("static GENERATED_BUCKETS: &[Option<GeneratedBucket>] = &[\n");
    for bucket in &buckets {
        match bucket {
            Some((token, start, end)) => source.push_str(&format!(
                "    Some(GeneratedBucket {{ token: {token:?}, start: {start}, end: {end} }}),\n"
            )),
            None => source.push_str("    None,\n"),
        }
    }
    source.push_str(
        "];\n\nfn generated_candidates(token: &str) -> &'static [GeneratedRule] {\n\
         \x20   let Some(first) = token.bytes().next() else { return &[]; };\n\
         \x20   if GENERATED_FIRST_BYTES[usize::from(first) / 64] & (1_u64 << (first % 64)) == 0 { return &[]; }\n\
         \x20   let mut slot = diagnostic_token_hash(token) & (GENERATED_BUCKETS.len() - 1);\n\
         \x20   for _ in 0..GENERATED_BUCKETS.len() {\n\
         \x20       match &GENERATED_BUCKETS[slot] {\n\
         \x20           Some(bucket) if bucket.token == token => {\n\
         \x20               return &GENERATED_RULES[bucket.start..bucket.end];\n\
         \x20           }\n\
         \x20           Some(_) => slot = (slot + 1) & (GENERATED_BUCKETS.len() - 1),\n\
         \x20           None => return &[],\n\
         \x20       }\n\
         \x20   }\n\
         \x20   &[]\n\
         }\n",
    );
    source
}

fn is_identifier_token(token: &str) -> bool {
    let mut characters = token.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric())
}

fn context_variant(context: &str) -> &'static str {
    match context {
        "function_declaration" => "FunctionDeclaration",
        "variable_declaration" => "VariableDeclaration",
        "condition" => "Condition",
        "loop" => "Loop",
        "type_position" => "TypePosition",
        "import" => "Import",
        "return" => "Return",
        "expression" => "Expression",
        _ => unreachable!("validated context"),
    }
}

fn severity_variant(severity: &str) -> &'static str {
    match severity {
        "error" => "Error",
        "warning" => "Warning",
        "information" => "Information",
        "hint" => "Hint",
        _ => unreachable!("validated severity"),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn benchmark_source(rule_count: usize) -> String {
    let values = (0..rule_count)
        .map(|number| number.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let groups = (0..rule_count)
        .map(|number| (format!("foreign_token_{number}"), number, number + 1))
        .collect();
    let buckets = hash_buckets(groups);
    let mut source = format!(
        "static SYNTHETIC_RULE_VALUES: [usize; {rule_count}] = [{values}];\n\
         struct SyntheticBucket {{ token: &'static str, start: usize, end: usize }}\n\
         static SYNTHETIC_BUCKETS: &[Option<SyntheticBucket>] = &[\n"
    );
    for bucket in &buckets {
        match bucket {
            Some((token, start, end)) => source.push_str(&format!(
                "    Some(SyntheticBucket {{ token: {token:?}, start: {start}, end: {end} }}),\n"
            )),
            None => source.push_str("    None,\n"),
        }
    }
    source.push_str(
        "];\n\
         fn synthetic_hash(token: &str) -> usize {\n\
         \x20   let mut hash = 0xcbf29ce484222325_u64;\n\
         \x20   for byte in token.bytes() {\n\
         \x20       hash ^= u64::from(byte);\n\
         \x20       hash = hash.wrapping_mul(0x100000001b3);\n\
         \x20   }\n\
         \x20   hash as usize\n\
         }\n\
         fn synthetic_candidates(token: &str) -> &'static [usize] {\n\
         \x20   let mut slot = synthetic_hash(token) & (SYNTHETIC_BUCKETS.len() - 1);\n\
         \x20   for _ in 0..SYNTHETIC_BUCKETS.len() {\n\
         \x20       match &SYNTHETIC_BUCKETS[slot] {\n\
         \x20           Some(bucket) if bucket.token == token => {\n\
         \x20               return &SYNTHETIC_RULE_VALUES[bucket.start..bucket.end];\n\
         \x20           }\n\
         \x20           Some(_) => slot = (slot + 1) & (SYNTHETIC_BUCKETS.len() - 1),\n\
         \x20           None => return &[],\n\
         \x20       }\n\
         \x20   }\n\
         \x20   &[]\n\
         }\n",
    );
    source
}

fn hash_buckets(groups: Vec<(String, usize, usize)>) -> Vec<Option<(String, usize, usize)>> {
    let capacity = (groups.len().saturating_mul(2)).next_power_of_two().max(1);
    let mut buckets = vec![None; capacity];
    for group in groups {
        let mut slot = fnv1a64(group.0.as_bytes()) as usize & (capacity - 1);
        while buckets[slot].is_some() {
            slot = (slot + 1) & (capacity - 1);
        }
        buckets[slot] = Some(group);
    }
    buckets
}

fn first_byte_filter(groups: &[(String, usize, usize)]) -> [u64; 4] {
    let mut filter = [0_u64; 4];
    for (token, _, _) in groups {
        if let Some(first) = token.bytes().next() {
            filter[usize::from(first) / 64] |= 1_u64 << (first % 64);
        }
    }
    filter
}
