use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
};

const MAX_VOCABULARY_FILES: usize = 32;
const MAX_FILE_BYTES: u64 = 256 * 1024;

pub fn load_domain_vocabulary() -> Vec<String> {
    let mut entries = Vec::new();
    for directory in vocabulary_directories() {
        entries.extend(load_domain_vocabulary_from(&directory));
    }
    deduplicate(entries)
}

fn load_domain_vocabulary_from(directory: &Path) -> Vec<String> {
    let mut entries = Vec::new();
    let Ok(read_dir) = fs::read_dir(directory) else {
        return entries;
    };
    let mut paths = read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "txt" || extension == "tsv")
        })
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths.into_iter().take(MAX_VOCABULARY_FILES) {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if path.extension().is_some_and(|extension| extension == "txt") {
            entries.extend(parse_vocabulary(&content));
        } else {
            entries.extend(
                parse_entity_rules(&content)
                    .into_iter()
                    .map(|rule| format!("@{}", rule.canonical)),
            );
        }
    }
    entries
}

pub fn resolve_contextual_entities(value: &str) -> String {
    vocabulary_directories()
        .into_iter()
        .fold(value.to_owned(), |current, directory| {
            resolve_contextual_entities_in(&current, &directory)
        })
}

pub fn resolve_contextual_entities_in(value: &str, directory: &Path) -> String {
    let Ok(read_dir) = fs::read_dir(directory) else {
        return value.to_owned();
    };
    let mut paths = read_dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "tsv"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .take(MAX_VOCABULARY_FILES)
        .fold(value.to_owned(), |current, path| {
            let content = fs::metadata(&path)
                .ok()
                .filter(|metadata| metadata.is_file() && metadata.len() <= MAX_FILE_BYTES)
                .and_then(|_| fs::read_to_string(path).ok());
            content.map_or(current.clone(), |content| {
                resolve_with_rules(&current, &parse_entity_rules(&content))
            })
        })
}

pub fn merge_vocabulary(primary: Vec<String>, domain: Vec<String>) -> Vec<String> {
    deduplicate(primary.into_iter().chain(domain).collect())
}

fn vocabulary_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(path) = env::var_os("NATSU_TYPELESS_VOCABULARY_DIR") {
        directories.push(PathBuf::from(path));
    }
    directories.push(data_home().join("natsu-typeless").join("vocabulary.d"));
    directories.push(Path::new("/usr/share/natsu-typeless/vocabulary.d").to_owned());
    directories
}

fn data_home() -> PathBuf {
    env::var_os("XDG_DATA_HOME").map_or_else(
        || {
            env::var_os("HOME").map_or_else(
                || PathBuf::from(".local/share"),
                |home| PathBuf::from(home).join(".local/share"),
            )
        },
        PathBuf::from,
    )
}

fn parse_vocabulary(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.chars().take(80).collect())
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct EntityRule {
    canonical: String,
    aliases: Vec<String>,
    entity_context: Vec<String>,
    ordinary_context: Vec<String>,
}

fn parse_entity_rules(content: &str) -> Vec<EntityRule> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let canonical = fields.next()?.trim();
            let aliases = split_terms(fields.next()?);
            let entity_context = split_terms(fields.next()?);
            let ordinary_context = split_terms(fields.next()?);
            (!canonical.is_empty() && !aliases.is_empty()).then(|| EntityRule {
                canonical: canonical.chars().take(80).collect(),
                aliases,
                entity_context,
                ordinary_context,
            })
        })
        .collect()
}

fn split_terms(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(|term| term.chars().take(80).collect())
        .collect()
}

fn resolve_with_rules(value: &str, rules: &[EntityRule]) -> String {
    let mut resolved = value.to_owned();
    for rule in rules {
        let normalized = resolved.to_ascii_lowercase();
        let entity_score = rule
            .entity_context
            .iter()
            .filter(|cue| normalized.contains(&cue.to_ascii_lowercase()))
            .count();
        let ordinary_score = rule
            .ordinary_context
            .iter()
            .filter(|cue| normalized.contains(&cue.to_ascii_lowercase()))
            .count();
        if entity_score > ordinary_score {
            for alias in &rule.aliases {
                resolved = replace_ascii_word(&resolved, alias, &rule.canonical);
            }
        } else if ordinary_score > entity_score {
            resolved = replace_ascii_word(&resolved, &rule.canonical, &rule.aliases[0]);
        }
    }
    resolved
}

fn replace_ascii_word(value: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() || !needle.is_ascii() {
        return value.to_owned();
    }
    let normalized = value.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    for (start, _) in normalized.match_indices(&needle) {
        let end = start + needle.len();
        let before_is_word = start > 0 && normalized.as_bytes()[start - 1].is_ascii_alphanumeric();
        let after_is_word =
            end < normalized.len() && normalized.as_bytes()[end].is_ascii_alphanumeric();
        if start < cursor || before_is_word || after_is_word {
            continue;
        }
        output.push_str(&value[cursor..start]);
        output.push_str(replacement);
        cursor = end;
    }
    output.push_str(&value[cursor..]);
    output
}

fn deduplicate(entries: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .filter(|entry| seen.insert(entry.trim().to_ascii_lowercase()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_comments_and_keeps_correction_rules() {
        assert_eq!(
            parse_vocabulary("# entities\nAcmeAI\n\nacme => AcmeAI\n"),
            vec!["AcmeAI", "acme => AcmeAI"]
        );
    }

    #[test]
    fn user_entries_win_case_insensitive_deduplication() {
        assert_eq!(
            merge_vocabulary(
                vec!["acmeai".into(), "WrongName => CanonicalName".into()],
                vec!["AcmeAI".into(), "OtherModel".into()]
            ),
            vec!["acmeai", "WrongName => CanonicalName", "OtherModel"]
        );
    }

    #[test]
    fn entity_rules_disambiguate_both_directions_from_context() {
        let rules =
            parse_entity_rules("AcmeAI\tacme\tAI,模型,订阅,Max\tserver,storage,部署,区域\n");
        assert_eq!(
            resolve_with_rules("acme Max 订阅", &rules),
            "AcmeAI Max 订阅"
        );
        assert_eq!(
            resolve_with_rules("部署 AcmeAI server 到测试区域", &rules),
            "部署 acme server 到测试区域"
        );
        assert_eq!(
            resolve_with_rules("同步 acme 文件", &rules),
            "同步 acme 文件"
        );
        assert_eq!(
            replace_ascii_word("acmeplus acme", "acme", "AcmeAI"),
            "acmeplus AcmeAI"
        );
    }

    #[test]
    fn ambiguous_entity_candidates_are_marked_for_asr_only() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/vocabulary");
        let vocabulary = load_domain_vocabulary_from(&directory);
        assert!(vocabulary.iter().any(|term| term == "@Claude"));
        assert!(!vocabulary.iter().any(|term| term == "Claude"));
    }
}
