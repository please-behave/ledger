// REQ-0023
use anyhow::Result;
use ledger::rule_digest::{RuleDigestInput, RuleKind, rule_digest};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
struct Vector {
    name: String,
    input: Value,
    digest: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("../why/contracts/rule-digest/vectors.json")).unwrap()
}

fn digest(input: &Value) -> Result<String> {
    let kind = match input["kind"].as_str().unwrap() {
        "check" => RuleKind::Check,
        "review" => RuleKind::Review {
            blocking: input["blocking"].as_bool().unwrap(),
            prompt: input["prompt"].as_str().unwrap(),
        },
        kind => panic!("unknown fixture kind {kind}"),
    };

    rule_digest(&RuleDigestInput {
        source: input["source"].as_str().unwrap(),
        path: input["path"].as_str().unwrap(),
        id: input["id"].as_str().unwrap(),
        version: input["version"].as_str().unwrap(),
        kind,
        name: input["name"].as_str(),
        description: input["description"].as_str(),
        knowledge: input
            .get("knowledge")
            .map(|value| value.as_object().unwrap()),
    })
}

#[test]
fn matches_the_normative_vectors() {
    for vector in vectors() {
        assert_eq!(
            digest(&vector.input).unwrap(),
            vector.digest,
            "{}",
            vector.name
        );
    }
}

#[test]
fn every_covered_field_affects_the_digest() {
    let input = &vectors()[1].input;
    let original = digest(input).unwrap();
    for (field, replacement) in [
        ("source", json!("github.com/example/another-catalog")),
        ("path", json!(".please/reviews/other")),
        ("id", json!("other")),
        ("version", json!("2.11")),
        ("name", json!("Other name")),
        ("description", json!("Other description")),
        ("blocking", json!(true)),
        ("prompt", json!("Read the report.")),
    ] {
        let mut changed = input.clone();
        changed[field] = replacement;
        assert_ne!(digest(&changed).unwrap(), original, "{field}");
    }

    let mut changed = input.clone();
    changed["knowledge"]["extension"]["é"][1] = json!(false);
    assert_ne!(digest(&changed).unwrap(), original);

    let mut changed = input.clone();
    changed["kind"] = json!("check");
    let check = changed.as_object_mut().unwrap();
    check.remove("blocking");
    check.remove("prompt");
    assert_ne!(digest(&changed).unwrap(), original);
}

#[test]
fn omitting_optional_fields_affects_the_digest() {
    let input = &vectors()[1].input;
    let original = digest(input).unwrap();
    for field in ["name", "description", "knowledge"] {
        let mut changed = input.clone();
        changed.as_object_mut().unwrap().remove(field);
        let omitted = digest(&changed).unwrap();
        assert_ne!(omitted, original, "omitting {field}");

        changed[field] = if field == "knowledge" {
            json!({})
        } else {
            json!("")
        };
        assert_ne!(digest(&changed).unwrap(), omitted, "empty {field}");
    }
}

#[test]
fn preserves_array_order() {
    let mut input = vectors().remove(1).input;
    let original = digest(&input).unwrap();
    input["knowledge"]["conditions"]
        .as_array_mut()
        .unwrap()
        .reverse();
    assert_ne!(digest(&input).unwrap(), original);
}

#[test]
fn rejects_nulls_numbers_and_normalized_key_collisions() {
    let mut input = vectors().remove(1).input;
    for invalid in [
        json!({"nested": [null]}),
        json!({"nested": {"count": 1}}),
        json!({"nested": {"\r\n": true, "\n": false}}),
    ] {
        input["knowledge"] = invalid;
        assert!(digest(&input).is_err());
    }
}
