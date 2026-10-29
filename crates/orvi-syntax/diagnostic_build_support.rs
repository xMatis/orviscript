use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Catalog {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub schema_version: u32,
    #[serde(default)]
    pub includes: Vec<String>,
    pub rules: Vec<Rule>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Fragment {
    pub rules: Vec<Rule>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub category: String,
    #[serde(rename = "match")]
    pub matcher: RuleMatch,
    pub message: String,
    pub replacement: String,
    pub severity: String,
    pub quick_fix: bool,
    pub priority: i32,
    #[serde(default)]
    pub source_languages: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RuleMatch {
    pub token: String,
    pub context: String,
}

pub fn validate_rules(rules: &[Rule]) -> Result<(), String> {
    let contexts = [
        "function_declaration",
        "variable_declaration",
        "condition",
        "loop",
        "type_position",
        "import",
        "return",
        "expression",
    ];
    let severities = ["error", "warning", "information", "hint"];
    let mut ids = HashSet::new();
    let mut triggers = HashMap::new();
    for rule in rules {
        if rule.id.len() != 8
            || !rule.id.starts_with("ORVI")
            || !rule.id[4..]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            return Err(format!("rule id '{}' must have the ORVI0000 form", rule.id));
        }
        if !ids.insert(rule.id.clone()) {
            return Err(format!("duplicate rule id '{}'", rule.id));
        }
        if rule.category.trim().is_empty()
            || rule.matcher.token.is_empty()
            || rule.message.trim().is_empty()
            || rule.replacement.is_empty()
        {
            return Err(format!(
                "rule '{}' contains an empty required field",
                rule.id
            ));
        }
        if !(-10_000..=10_000).contains(&rule.priority) {
            return Err(format!(
                "rule '{}' priority is outside -10000..10000",
                rule.id
            ));
        }
        if !valid_replacement(&rule.replacement) {
            return Err(format!(
                "rule '{}' replacement must be exactly one identifier, keyword or operator token",
                rule.id
            ));
        }
        let mut languages = HashSet::new();
        if rule
            .source_languages
            .iter()
            .any(|language| language.trim().is_empty() || !languages.insert(language))
        {
            return Err(format!(
                "rule '{}' has an empty or duplicate source language",
                rule.id
            ));
        }
        if !contexts.contains(&rule.matcher.context.as_str()) {
            return Err(format!(
                "rule '{}' uses unknown context '{}'",
                rule.id, rule.matcher.context
            ));
        }
        if !severities.contains(&rule.severity.as_str()) {
            return Err(format!(
                "rule '{}' uses unknown severity '{}'",
                rule.id, rule.severity
            ));
        }
        let trigger = (
            rule.matcher.token.clone(),
            rule.matcher.context.clone(),
            rule.priority,
        );
        if let Some(previous) = triggers.insert(trigger, rule.id.clone()) {
            return Err(format!(
                "rules '{previous}' and '{}' conflict at the same token, context and priority",
                rule.id
            ));
        }
    }
    Ok(())
}

pub fn sort_rules(rules: &mut [Rule]) {
    rules.sort_by(|left, right| {
        left.matcher
            .token
            .cmp(&right.matcher.token)
            .then_with(|| left.matcher.context.cmp(&right.matcher.context))
            .then_with(|| right.priority.cmp(&left.priority))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn valid_replacement(replacement: &str) -> bool {
    let mut characters = replacement.chars();
    let identifier = characters
        .next()
        .is_some_and(|character| character == '_' || character.is_alphabetic())
        && characters.all(|character| character == '_' || character.is_alphanumeric());
    identifier
        || matches!(
            replacement,
            "+" | "-"
                | "*"
                | "/"
                | "%"
                | "=="
                | "!="
                | "<"
                | "<="
                | ">"
                | ">="
                | "&&"
                | "||"
                | "="
                | "=>"
                | "->"
                | "!"
                | "?"
        )
}
