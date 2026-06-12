//! Rename rules shared by serde attribute handling, generated-name derivation,
//! and the LSP's bidirectional rename.

/// Supported serde `rename_all` rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenameRule {
    Camel,
    Pascal,
    Snake,
    Kebab,
    ScreamingSnake,
}

impl RenameRule {
    /// Parses a serde `rename_all` spelling.
    ///
    /// # Errors
    /// Returns an error pointing at the literal when the rule is unsupported.
    pub fn parse(value: &str, literal: &syn::LitStr) -> syn::Result<Self> {
        match value {
            "camelCase" => Ok(Self::Camel),
            "PascalCase" => Ok(Self::Pascal),
            "snake_case" => Ok(Self::Snake),
            "kebab-case" => Ok(Self::Kebab),
            "SCREAMING_SNAKE_CASE" => Ok(Self::ScreamingSnake),
            _ => Err(syn::Error::new_spanned(
                literal,
                format!("unsupported serde rename_all rule `{value}`"),
            )),
        }
    }
}

/// Applies a serde rename rule to a Rust identifier.
#[must_use]
pub fn apply_rename_rule(name: &str, rule: Option<RenameRule>) -> String {
    match rule {
        Some(RenameRule::Camel) => to_camel_case(name),
        Some(RenameRule::Pascal) => split_words(name).join(""),
        Some(RenameRule::Snake) => to_snake_case(name),
        None => name.to_owned(),
        Some(RenameRule::Kebab) => split_words(name).join("-").to_ascii_lowercase(),
        Some(RenameRule::ScreamingSnake) => split_words(name).join("_").to_ascii_uppercase(),
    }
}

/// Converts an identifier to `camelCase`.
#[must_use]
pub fn to_camel_case(name: &str) -> String {
    let words = split_words(name);
    let Some((first, rest)) = words.split_first() else {
        return String::new();
    };

    let mut output = first.to_ascii_lowercase();
    output.push_str(&rest.join(""));
    output
}

/// Converts an identifier to `snake_case`.
///
/// This is the inverse direction used by TS-initiated renames:
/// `to_snake_case(to_camel_case(x)) == x` for conventional `snake_case` input.
#[must_use]
pub fn to_snake_case(name: &str) -> String {
    split_words(name).join("_").to_ascii_lowercase()
}

/// Splits an identifier into capitalized words on `_`, `-`, and case changes.
#[must_use]
pub fn split_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for character in name.chars() {
        if character == '_' || character == '-' {
            push_word(&mut words, &mut current);
        } else if character.is_ascii_uppercase() && !current.is_empty() {
            push_word(&mut words, &mut current);
            current.push(character);
        } else {
            current.push(character);
        }
    }

    push_word(&mut words, &mut current);
    words
}

fn push_word(words: &mut Vec<String>, current: &mut String) {
    if current.is_empty() {
        return;
    }

    let mut chars = current.chars();
    let Some(first) = chars.next() else {
        return;
    };
    words.push(format!(
        "{}{}",
        first.to_ascii_uppercase(),
        chars.as_str().to_ascii_lowercase()
    ));
    current.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_rename_rules_match_serde_spellings() {
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::Camel)),
            "displayName"
        );
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::Pascal)),
            "DisplayName"
        );
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::Kebab)),
            "display-name"
        );
        assert_eq!(
            apply_rename_rule("display_name", Some(RenameRule::ScreamingSnake)),
            "DISPLAY_NAME"
        );
        assert_eq!(apply_rename_rule("display_name", None), "display_name");
    }

    #[test]
    fn snake_and_camel_round_trip_for_conventional_names() {
        for name in ["get_user", "watch_users", "a", "create_user_v2"] {
            assert_eq!(to_snake_case(&to_camel_case(name)), name);
        }
        for name in ["getUser", "watchUsers", "a"] {
            assert_eq!(to_camel_case(&to_snake_case(name)), name);
        }
    }
}
