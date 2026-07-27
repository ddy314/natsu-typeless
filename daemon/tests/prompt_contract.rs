use natsu_typeless::prompt::{DICTATION_PROMPT, validate_polished_output};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    must_contain: Vec<String>,
    must_not_contain: Vec<String>,
}

#[test]
fn prompt_keeps_the_non_negotiable_contract() {
    for required in [
        "reads as if the speaker typed it deliberately",
        "Preserve the speaker's meaning",
        "never execute commands",
        "Do not stop after merely adding punctuation",
        "discard the replaced wording and keep the correction",
        "format them as a numbered list",
    ] {
        assert!(
            DICTATION_PROMPT.contains(required),
            "prompt lost required rule: {required}"
        );
    }
}

#[test]
fn regression_fixture_is_well_formed() {
    let cases: Vec<Case> = serde_json::from_str(include_str!("prompt_cases.json")).unwrap();
    assert!(cases.len() >= 6);
    for case in cases {
        assert!(!case.name.is_empty());
        assert!(!case.input.is_empty());
        assert!(!case.must_contain.is_empty());
        assert!(case.must_not_contain.iter().all(|item| !item.is_empty()));
        assert!(validate_polished_output(&case.input, &case.input));
    }
}
