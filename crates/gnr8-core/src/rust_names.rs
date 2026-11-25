//! Rust identifier spelling shared by emitters.

use crate::graph::Field;
use crate::sdk::emit_common::split_words;
use std::collections::BTreeSet;

/// Strict and reserved Rust keywords (2021 edition, plus the 2024 `gen`).
const KEYWORDS: [&str; 52] = [
    "Self", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if",
    "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Keywords a raw identifier cannot spell.
const NOT_RAW: [&str; 4] = ["Self", "crate", "self", "super"];

/// A schema name as a Rust type name: kept when it already is an upper-camel-case identifier, and
/// otherwise rebuilt from its words. `None` when no word starts it with a letter.
pub(crate) fn type_ident(name: &str) -> Option<String> {
    let kept = name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    let ident = if kept {
        name.to_string()
    } else {
        upper_camel(name)
    };
    ident
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        .then_some(ident)
}

/// The words of `value`, each capitalized, joined.
fn upper_camel(value: &str) -> String {
    split_words(value)
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                let mut out = first.to_ascii_uppercase().to_string();
                out.push_str(&chars.as_str().to_ascii_lowercase());
                out
            })
        })
        .collect()
}

/// `value` as a lowercase snake-case identifier body (not yet keyword-escaped).
pub(crate) fn snake(value: &str) -> String {
    let mut out = split_words(value)
        .iter()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    if out.is_empty() {
        out.push_str("value");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "value_");
    }
    out
}

/// A keyword spelled as an identifier: a raw identifier, or `suffix` appended where Rust has none.
pub(crate) fn escape_keyword(ident: &str, suffix: &str) -> String {
    if NOT_RAW.contains(&ident) {
        format!("{ident}{suffix}")
    } else if KEYWORDS.contains(&ident) {
        format!("r#{ident}")
    } else {
        ident.to_string()
    }
}

/// One Rust field name per field, in field order. Two wire names that spell the same identifier
/// keep it for the first and number the rest (`file_id`, `file_id_2`), the fields being sorted by
/// wire name.
pub(crate) fn field_idents(fields: &[Field]) -> Vec<String> {
    let bases: Vec<String> = fields.iter().map(|field| snake(&field.json_name)).collect();
    disambiguate(&bases, |base, n| format!("{base}_{n}"))
        .into_iter()
        .map(|name| escape_keyword(&name, "_"))
        .collect()
}

/// One Rust variant name per enum member, numbered on a collision like [`field_idents`].
pub(crate) fn variant_idents(members: &[String]) -> Vec<String> {
    let bases: Vec<String> = members
        .iter()
        .map(|member| {
            let camel = upper_camel(member);
            if camel.is_empty() {
                "Value".to_string()
            } else if camel.starts_with(|c: char| c.is_ascii_digit()) {
                format!("Value{camel}")
            } else {
                camel
            }
        })
        .collect();
    disambiguate(&bases, |base, n| format!("{base}{n}"))
        .into_iter()
        .map(|name| escape_keyword(&name, "Value"))
        .collect()
}

/// `bases`, with every repeat of an earlier name numbered from 2 until it is unique.
fn disambiguate(bases: &[String], numbered: impl Fn(&str, usize) -> String) -> Vec<String> {
    let taken: BTreeSet<&str> = bases.iter().map(String::as_str).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(bases.len());
    for base in bases {
        let mut name = base.clone();
        let mut n = 2;
        while used.contains(&name) || (name != *base && taken.contains(name.as_str())) {
            name = numbered(base, n);
            n += 1;
        }
        used.insert(name.clone());
        out.push(name);
    }
    out
}
