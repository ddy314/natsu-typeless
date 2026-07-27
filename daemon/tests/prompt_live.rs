use natsu_typeless::{
    openai_compat::OpenAiCompatClient,
    prompt::polished_output_rejection,
    protocol::{DEFAULT_CLOUD_BASE_URL, DEFAULT_CLOUD_MODEL, SessionOptions},
    secrets,
    vocabulary::resolve_contextual_entities_in,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    name: String,
    input: String,
    #[serde(default)]
    vocabulary: Vec<String>,
    must_contain: Vec<String>,
    must_not_contain: Vec<String>,
}

#[tokio::test]
#[ignore = "uses the configured cloud key and consumes API quota"]
async fn openai_compatible_prompt_regression() {
    let key = secrets::get_cloud_api_key().expect("cloud API key");
    let base_url = std::env::var("NATSU_TYPELESS_TEST_API_BASE")
        .unwrap_or_else(|_| DEFAULT_CLOUD_BASE_URL.into());
    let model =
        std::env::var("NATSU_TYPELESS_TEST_MODEL").unwrap_or_else(|_| DEFAULT_CLOUD_MODEL.into());
    let client = OpenAiCompatClient::new().unwrap();
    let cases: Vec<Case> = serde_json::from_str(include_str!("prompt_cases.json")).unwrap();
    let case_filter = std::env::var("NATSU_TYPELESS_TEST_CASE").ok();
    for case in cases.into_iter().filter(|case| {
        case_filter
            .as_ref()
            .is_none_or(|filter| case.name == *filter)
    }) {
        let vocabulary_directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/vocabulary");
        let normalized_input = resolve_contextual_entities_in(&case.input, &vocabulary_directory);
        let options = SessionOptions {
            vocabulary: case.vocabulary.clone(),
            ..SessionOptions::default()
        };
        let output = client
            .polish(
                Some(&key),
                &base_url,
                &model,
                4_000,
                &normalized_input,
                &options,
            )
            .await
            .unwrap_or_else(|error| panic!("{}: {error}", case.name));
        let rejection = polished_output_rejection(&normalized_input, &output, &case.vocabulary);
        println!("{}: output={output:?}, rejection={rejection:?}", case.name);
        assert!(
            rejection.is_none(),
            "{}: runtime validation rejected {output:?}: {rejection:?}",
            case.name
        );
        for required in case.must_contain {
            assert!(
                output.to_lowercase().contains(&required.to_lowercase()),
                "{}: output {output:?} missed {required:?}",
                case.name
            );
        }
        for forbidden in case.must_not_contain {
            assert!(
                !output.to_lowercase().contains(&forbidden.to_lowercase()),
                "{}: output {output:?} contained {forbidden:?}",
                case.name
            );
        }
    }
}
