pub const DICTATION_PROMPT_V1: &str = include_str!("../prompts/dictation-v1.txt");
pub const DICTATION_PROMPT_V2: &str = include_str!("../prompts/dictation-v2.txt");
pub const DICTATION_PROMPT: &str = DICTATION_PROMPT_V2;

pub fn validate_polished_output(raw: &str, polished: &str) -> bool {
    polished_output_rejection(raw, polished, &[]).is_none()
}

pub fn polished_output_rejection(
    raw: &str,
    polished: &str,
    preferred_vocabulary: &[String],
) -> Option<&'static str> {
    let trimmed = polished.trim();
    if trimmed.is_empty() {
        return Some("empty output");
    }
    let raw_chars = raw.chars().count();
    let output_chars = trimmed.chars().count();
    let maximum = (raw_chars + 128).max((raw_chars * 7).div_ceil(4));
    if output_chars > maximum {
        return Some("suspicious expansion");
    }

    let normalized_output = trimmed.to_ascii_lowercase();
    let normalized_raw = raw.to_ascii_lowercase();
    let missing_technical_tokens = technical_tokens(raw)
        .filter(|token| !normalized_output.contains(&token.to_ascii_lowercase()))
        .map(str::to_ascii_lowercase)
        .collect::<std::collections::HashSet<_>>();
    let authoritative_corrections = preferred_vocabulary
        .iter()
        .map(|term| canonical_vocabulary_term(term).to_ascii_lowercase())
        .filter(|term| {
            term.len() >= 2 && normalized_output.contains(term) && !normalized_raw.contains(term)
        })
        .collect::<std::collections::HashSet<_>>();
    let candidate_entity_removals = missing_technical_tokens
        .iter()
        .filter(|missing| {
            preferred_vocabulary.iter().any(|entry| {
                entry.trim().starts_with('@')
                    && entry
                        .trim()
                        .trim_start_matches('@')
                        .eq_ignore_ascii_case(missing)
            })
        })
        .count();
    if missing_technical_tokens.len() > authoritative_corrections.len() + candidate_entity_removals
    {
        return Some("technical token changed");
    }
    None
}

pub fn apply_vocabulary_corrections(value: &str, vocabulary: &[String]) -> String {
    let mut corrected = value.to_owned();
    for entry in vocabulary {
        let Some((heard, canonical)) = entry.split_once("=>").or_else(|| entry.split_once("->"))
        else {
            continue;
        };
        let heard = heard.trim();
        let canonical = canonical.trim();
        if heard.is_empty() || canonical.is_empty() || heard == canonical {
            continue;
        }
        corrected = corrected.replace(heard, canonical);
    }
    corrected
}

fn canonical_vocabulary_term(value: &str) -> &str {
    value
        .split_once("=>")
        .or_else(|| value.split_once("->"))
        .map_or(value, |(_, canonical)| canonical)
        .trim()
        .trim_start_matches('@')
}

fn technical_tokens(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '_' | '-' | '+' | '.' | '/')
        })
        .filter(|token| {
            token.len() >= 2
                && token.chars().any(|character| {
                    character.is_ascii_uppercase()
                        || character.is_ascii_digit()
                        || matches!(character, '_' | '-' | '+' | '.' | '/')
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_suspicious_expansion() {
        assert!(!validate_polished_output("hello", " "));
        assert!(!validate_polished_output("hi", &"x".repeat(131)));
    }

    #[test]
    fn accepts_normal_cleanup() {
        assert!(validate_polished_output(
            "嗯那个我们明天测试一下",
            "我们明天测试一下。"
        ));
    }

    #[test]
    fn preserves_technical_identifiers() {
        assert!(validate_polished_output(
            "把 Qwen3-ASR 接到 fcitx5",
            "把 Qwen3-ASR 接到 fcitx5。"
        ));
        assert!(!validate_polished_output(
            "把 Qwen3-ASR 接到 fcitx5",
            "把语音模型接到输入法。"
        ));
    }

    #[test]
    fn accepts_authoritative_vocabulary_corrections() {
        let vocabulary = vec![
            "WrongName => CanonicalName".to_string(),
            "WrongTerm => CanonicalTerm".to_string(),
        ];
        assert_eq!(
            apply_vocabulary_corrections("WrongName 和 WrongTerm", &vocabulary),
            "CanonicalName 和 CanonicalTerm"
        );
        assert_eq!(
            polished_output_rejection(
                "WrongName 和 WrongTerm",
                "CanonicalName 和 CanonicalTerm",
                &vocabulary
            ),
            None
        );
    }

    #[test]
    fn only_ambiguous_entity_candidates_may_be_removed() {
        assert_eq!(
            polished_output_rejection(
                "Fable 和 Claude",
                "Claude",
                &["Fable".to_string(), "@Claude".to_string()]
            ),
            Some("technical token changed")
        );
        assert_eq!(
            polished_output_rejection("Claude server", "cloud server", &["@Claude".to_string()]),
            None
        );
    }
}
