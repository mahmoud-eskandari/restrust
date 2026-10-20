//! `{{var}}` template interpolation.
//!
//! Precedence is expressed by scope order: callers pass scopes as
//! `&[local, environment, collection, global]` — earlier scopes win. Only
//! single-pass expansion is performed (resolved values are not re-scanned).

use crate::model::KeyValue;

/// Replaces every `{{name}}` occurrence in `input` with the value of `name`
/// from the first scope (in `scopes` order) that has an *enabled* row with
/// that key. Unresolved occurrences are left verbatim, and any text without a
/// complete `{{...}}` pair is preserved as-is.
///
/// `scopes` must be ordered by precedence: request-local first, then
/// environment, then collection, then global variables.
pub fn interpolate(input: &str, scopes: &[&[KeyValue]]) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        match after_open.find("}}") {
            Some(end) => {
                let name = &after_open[..end];
                match lookup(name, scopes) {
                    Some(value) => out.push_str(&value),
                    None => {
                        out.push_str("{{");
                        out.push_str(name);
                        out.push_str("}}");
                    }
                }
                rest = &after_open[end + 2..];
            }
            None => {
                // Unmatched opening brace: emit the remainder verbatim.
                out.push_str("{{");
                out.push_str(after_open);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Lists the variable names referenced by `{{...}}` occurrences, in order of
/// first appearance, without duplicates. Empty placeholders (`{{}}`) are
/// ignored.
pub fn used_variables(input: &str) -> Vec<String> {
    let mut used: Vec<String> = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("{{") {
        let after_open = &rest[start + 2..];
        match after_open.find("}}") {
            Some(end) => {
                let name = &after_open[..end];
                if !name.is_empty() && !used.iter().any(|seen| seen == name) {
                    used.push(name.to_string());
                }
                rest = &after_open[end + 2..];
            }
            None => break,
        }
    }
    used
}

fn lookup(name: &str, scopes: &[&[KeyValue]]) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    for scope in scopes {
        for row in *scope {
            if row.enabled && row.key == name {
                return Some(row.value.clone());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolate_resolves_from_highest_precedence_scope() {
        let local = [KeyValue::new("token", "local-token")];
        let env = [
            KeyValue::new("token", "env-token"),
            KeyValue::new("host", "env.io"),
        ];
        let collection = [KeyValue::new("host", "collection.io")];
        let global = [
            KeyValue::new("host", "global.io"),
            KeyValue::new("only_global", "g"),
        ];

        let scopes: &[&[KeyValue]] = &[&local, &env, &collection, &global];
        assert_eq!(interpolate("{{token}}", scopes), "local-token");
        assert_eq!(interpolate("{{host}}", scopes), "env.io");
        assert_eq!(interpolate("{{only_global}}", scopes), "g");
    }

    #[test]
    fn interpolate_partial_scopes() {
        let env = [KeyValue::new("base_url", "https://api.test")];
        let global = [KeyValue::new("k", "v")];
        assert_eq!(
            interpolate("{{k}} {{base_url}}", &[&env, &global]),
            "v https://api.test"
        );
    }

    #[test]
    fn interpolate_url_and_path() {
        let env = [KeyValue::new("base_url", "https://api.test")];
        assert_eq!(
            interpolate("{{base_url}}/path?x=1", &[&env]),
            "https://api.test/path?x=1"
        );
    }

    #[test]
    fn interpolate_adjacent_placeholders() {
        let scopes = [
            &[KeyValue::new("a", "1")][..],
            &[KeyValue::new("b", "2")][..],
        ];
        assert_eq!(interpolate("{{a}}{{b}}", &scopes), "12");
        assert_eq!(interpolate("x{{a}}{{b}}y", &scopes), "x12y");
    }

    #[test]
    fn interpolate_unresolved_left_verbatim() {
        let env = [KeyValue::new("known", "v")];
        assert_eq!(interpolate("{{unknown}}", &[&env]), "{{unknown}}");
        assert_eq!(
            interpolate("{{known}} {{unknown}}", &[&env]),
            "v {{unknown}}"
        );
        // Adjacent resolved + unresolved
        assert_eq!(interpolate("{{known}}{{unknown}}", &[&env]), "v{{unknown}}");
        // Empty and degenerate placeholders stay verbatim
        assert_eq!(interpolate("{{}}", &[&env]), "{{}}");
        assert_eq!(
            interpolate("no placeholders here", &[&env]),
            "no placeholders here"
        );
        // Unmatched opening braces are preserved
        assert_eq!(interpolate("a {{ b", &[&env]), "a {{ b");
    }

    #[test]
    fn interpolate_skips_disabled_rows() {
        let mut row = KeyValue::new("disabled_var", "value");
        row.enabled = false;
        let enabled = [KeyValue::new("other", "x")];
        let scopes: &[&[KeyValue]] = &[&[row][..], &enabled];
        assert_eq!(interpolate("{{disabled_var}}", scopes), "{{disabled_var}}");
    }

    #[test]
    fn interpolate_values_are_not_recursively_expanded() {
        let scopes = [&[
            KeyValue::new("outer", "{{inner}}"),
            KeyValue::new("inner", "x"),
        ][..]];
        assert_eq!(interpolate("{{outer}}", &scopes), "{{inner}}");
    }

    #[test]
    fn used_variables_lists_unique_in_order() {
        let input = "{{base_url}}/a {{b}} {{base_url}} {{}} {{c";
        assert_eq!(used_variables(input), vec!["base_url", "b"]);
        assert!(used_variables("plain text").is_empty());
    }

    #[test]
    fn used_variables_adjacent() {
        assert_eq!(used_variables("{{a}}{{b}}{{a}}"), vec!["a", "b"]);
    }
}
