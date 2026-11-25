mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{get, migrated_pool, post, postgres};
use tokio::net::TcpListener;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn envelope(repository: &str, points: &[&str], commit: &str, statuses: &[&str]) -> Value {
    let results: Vec<Value> = statuses
        .iter()
        .enumerate()
        .map(|(index, status)| json!({"id": format!("rule-{index}"), "kind": "check", "status": status}))
        .collect();
    json!({
        "sender": "ci",
        "attestation": {
            "schema_version": 1,
            "repository": repository,
            "commit": commit,
            "execution_points": points,
            "overall": "PASS",
            "results": results
        }
    })
}

fn commit(digit: char) -> String {
    digit.to_string().repeat(40)
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn ingest(address: SocketAddr, envelope: Value) -> String {
    let body = envelope.to_string();
    let (status, body) =
        tokio::task::spawn_blocking(move || post(address, "/v0/attestations", &body))
            .await
            .expect("the request finishes");
    assert_eq!(status, 201);
    let body: Value = serde_json::from_str(&body).expect("a JSON body");
    body["id"].as_str().expect("the id").to_string()
}

async fn read(address: SocketAddr, path: &'static str) -> (u16, String) {
    tokio::task::spawn_blocking(move || get(address, path))
        .await
        .expect("the request finishes")
}

async fn read_json(address: SocketAddr, path: String) -> (u16, Value) {
    let (status, body) = tokio::task::spawn_blocking(move || get(address, &path))
        .await
        .expect("the request finishes");
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

async fn latest(address: SocketAddr) -> Vec<Value> {
    let (status, body) = read_json(address, "/v0/attestations/latest".to_string()).await;
    assert_eq!(status, 200);
    body.as_array().expect("an array").clone()
}

const SET_SOURCE: &str = "github.com/example/sets";
const SET_COMPLIANCE: &str =
    "/v0/rule-sets/compliance?source=github.com%2Fexample%2Fsets&path=.&id=baseline";
const COMPLETED_SETS: &str = "/v0/repositories/completed-rule-sets";
const COMMIT_EVIDENCE: &str = "/v0/repositories/commit-evidence";

async fn commit_evidence(address: SocketAddr, repository: &str, commit: &str) -> Value {
    let (status, body) = read_json(
        address,
        format!("{COMMIT_EVIDENCE}?repository={repository}&commit={commit}"),
    )
    .await;
    assert_eq!(status, 200);
    assert!(body.get("evidence").is_some(), "{body}");
    body["evidence"].clone()
}

async fn register_for_compliance(
    address: SocketAddr,
    route: &'static str,
    request: Value,
) -> Value {
    let (status, receipt) =
        tokio::task::spawn_blocking(move || post(address, route, &request.to_string()))
            .await
            .expect("registration finishes");
    assert_eq!(status, 200, "{receipt}");
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    let outcomes = receipt["outcomes"].as_array().unwrap();
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome["outcome"] == "created")
    );
    json!(outcomes)
}

fn set_binding(id: &str, version: &str, points: &[&str]) -> Value {
    json!({
        "source": SET_SOURCE, "path": ".", "id": id, "version": version,
        "execution_points": points
    })
}

fn compliance_evidence(
    repository: &str,
    points: &[&str],
    bindings: &[Value],
    rules: &[Value],
    statuses: &[&str],
) -> Value {
    let mut report = envelope(repository, points, COMMIT, statuses);
    report["attestation"]["rule_sets"] = json!(bindings);
    let results = report["attestation"]["results"].as_array_mut().unwrap();
    assert_eq!(results.len(), rules.len());
    for (result, rule) in results.iter_mut().zip(rules) {
        result["rule"] = json!({
            "source": rule["source"], "path": rule["path"], "id": rule["id"]
        });
    }
    report
}

// REQ-0040, REQ-0041, REQ-0046: the reads share completeness for check evidence.
#[tokio::test(flavor = "multi_thread")]
async fn rule_set_compliance_uses_only_the_newest_check_evidence() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let rules: Vec<Value> = (0..5)
        .map(|index| {
            json!({
                "source": "github.com/example/rules", "path": ".please/checks",
                "id": format!("member-{index}"), "version": "1.0", "kind": "check"
            })
        })
        .collect();
    let outcomes = register_for_compliance(
        address,
        "/v0/rule-registrations",
        json!({"sender": "ci", "commit": COMMIT, "rules": rules}),
    )
    .await;
    let members: Vec<Value> = rules.iter().enumerate().map(|(index, rule)| {
        json!({
            "source": rule["source"], "path": rule["path"], "id": rule["id"],
            "version": rule["version"], "digest": outcomes[index]["digest"], "enabled": index != 2
        })
    }).collect();
    let baseline = set_binding("baseline", "1.0", &["check"]);
    let secondary = set_binding("secondary", "1.0", &["check"]);
    let unregistered = set_binding("baseline", "9.0", &["check"]);
    let mut registered_sets = Vec::new();
    for (id, version, name, members) in [
        ("baseline", "1.0", Some("Baseline"), &members[..]),
        ("baseline", "2.0", Some("New baseline"), &members[..1]),
        ("secondary", "1.0", None, &members[..1]),
    ] {
        let mut set = json!({
            "source": SET_SOURCE, "path": ".", "id": id, "version": version, "members": members
        });
        if let Some(name) = name {
            set["name"] = json!(name);
        }
        registered_sets.push(set);
    }
    register_for_compliance(
        address,
        "/v0/rule-set-registrations",
        json!({"sender": "ci", "commit": COMMIT, "sets": registered_sets}),
    )
    .await;

    for (repository, points, bindings) in [
        (
            "complete",
            &["check", "ci"][..],
            vec![baseline.clone(), baseline.clone(), secondary],
        ),
        ("ignored-run", &["ci"][..], vec![baseline.clone()]),
        (
            "ignored-binding",
            &["check"][..],
            vec![set_binding("baseline", "1.0", &["ci"])],
        ),
        ("unknown", &["check"][..], vec![unregistered]),
        ("no-longer-bound", &["check"][..], vec![baseline.clone()]),
        ("incomplete", &["check"][..], vec![baseline.clone()]),
    ] {
        ingest(
            address,
            compliance_evidence(repository, points, &bindings, &rules, &["PASS"; 5]),
        )
        .await;
    }
    ingest(
        address,
        envelope("no-longer-bound", &["check"], COMMIT, &["PASS"]),
    )
    .await;
    ingest(
        address,
        envelope("complete", &["pre-push"], COMMIT, &["FAIL"]),
    )
    .await;

    let mut failing_rules = rules.clone();
    failing_rules[4]["path"] = json!(".please/checks/unrelated");
    let mut failure = compliance_evidence(
        "incomplete",
        &["ci", "check"],
        &[baseline],
        &failing_rules,
        &["FAIL", "ERROR", "WARN", "NOT_RUN", "PASS"],
    );
    failure["attestation"]["results"][4]["id"] = rules[4]["id"].clone();
    ingest(address, failure).await;

    let failures: Vec<Value> = rules
        .iter()
        .zip(["FAIL", "ERROR", "WARN", "NOT_RUN", "NOT_RUN"])
        .map(|(rule, status)| {
            json!({
                "source": rule["source"], "path": rule["path"], "id": rule["id"],
                "version": rule["version"], "status": status
            })
        })
        .collect();
    assert_eq!(
        read_json(address, SET_COMPLIANCE.to_string()).await,
        (
            200,
            json!([
                {"repository": "complete", "version": "1.0", "registered": true, "complete": true, "non_passing_members": []},
                {"repository": "incomplete", "version": "1.0", "registered": true, "complete": false, "non_passing_members": failures},
                {"repository": "unknown", "version": "9.0", "registered": false, "complete": false, "non_passing_members": []}
            ])
        )
    );
    assert_eq!(
        read_json(address, COMPLETED_SETS.to_string()).await,
        (
            200,
            json!([{"repository": "complete", "sets": [
                {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "1.0", "name": "Baseline"},
                {"source": SET_SOURCE, "path": ".", "id": "secondary", "version": "1.0", "name": "secondary"}
            ]}])
        )
    );
    assert_eq!(
        commit_evidence(address, "complete", COMMIT).await["rule_sets"],
        json!([
            {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "1.0", "name": "Baseline",
             "registered": true, "complete": true, "non_passing_members": []},
            {"source": SET_SOURCE, "path": ".", "id": "secondary", "version": "1.0", "name": "secondary",
             "registered": true, "complete": true, "non_passing_members": []}
        ])
    );
    assert_eq!(
        commit_evidence(address, "incomplete", COMMIT).await["rule_sets"],
        json!([
            {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "1.0", "name": "Baseline",
             "registered": true, "complete": false, "non_passing_members": failures}
        ])
    );
    assert_eq!(
        commit_evidence(address, "unknown", COMMIT).await["rule_sets"],
        json!([
            {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "9.0", "name": "baseline",
             "registered": false, "complete": false, "non_passing_members": []}
        ])
    );
    for repository in ["no-longer-bound", "ignored-binding"] {
        assert_eq!(
            commit_evidence(address, repository, COMMIT).await["rule_sets"],
            json!([])
        );
    }
    assert_eq!(
        commit_evidence(address, "ignored-run", COMMIT).await,
        Value::Null
    );
    assert_eq!(
        read_json(address, SET_COMPLIANCE.replace("baseline", "absent")).await,
        (200, json!([]))
    );
    assert_eq!(
        read_json(address, format!("{SET_COMPLIANCE}&id=duplicate"))
            .await
            .0,
        400
    );
}

// REQ-0045, REQ-0046: receipt ordering and sender provenance belong to the exact commit.
#[tokio::test(flavor = "multi_thread")]
async fn commit_evidence_selects_the_newest_check_for_the_exact_pair() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    let repository = "github.com/example/project";
    let encoded_repository = "github.com%2Fexample%2Fproject";
    let other_commit = "A".repeat(64);
    let first = ingest(address, envelope(repository, &["check"], COMMIT, &[])).await;
    let mut report = envelope(repository, &["ci", "check"], COMMIT, &["FAIL"]);
    report["sender"] = json!(repository);
    report["auth_method"] = json!("github_oidc");
    report["attestation"]["overall"] = json!("FAIL");
    let selected = ingest(address, report.clone()).await;
    let later_id = ingest(address, envelope(repository, &["check"], COMMIT, &[])).await;
    assert!(first < selected && selected < later_id);
    sqlx::query(
        "update attestations set received_at = case when id = $3::uuid
         then '2026-11-24T00:00:00Z'::timestamptz else '2026-11-25T00:00:00Z'::timestamptz end
         where id in ($1::uuid, $2::uuid, $3::uuid)",
    )
    .bind(&first)
    .bind(&selected)
    .bind(&later_id)
    .execute(&pool)
    .await
    .expect("fix receipt times to exercise both ordering keys");

    ingest(address, envelope(repository, &["pre-push"], COMMIT, &[])).await;
    let other = ingest(
        address,
        envelope(repository, &["check"], &other_commit, &[]),
    )
    .await;
    ingest(
        address,
        envelope("other-repository", &["check"], COMMIT, &[]),
    )
    .await;
    assert_eq!(
        commit_evidence(address, encoded_repository, COMMIT).await,
        json!({
            "id": selected, "overall": "FAIL", "received_at": "2026-11-25T00:00:00.000000Z",
            "sender": repository, "auth_method": "github_oidc", "rule_sets": []
        })
    );
    let legacy = commit_evidence(address, encoded_repository, &other_commit).await;
    assert_eq!(legacy["id"], other);
    assert_eq!(legacy.get("auth_method"), Some(&Value::Null));
    assert_eq!(
        commit_evidence(address, encoded_repository, &commit('f')).await,
        Value::Null
    );
    assert_eq!(
        commit_evidence(address, "other-repository", &other_commit).await,
        Value::Null
    );

    for query in [
        format!("repository={encoded_repository}"),
        format!("repository={encoded_repository}&commit=short"),
        format!("repository={encoded_repository}&commit={COMMIT}&commit={COMMIT}"),
        format!("repository={encoded_repository}&commit={COMMIT}&extra=1"),
        format!("repository=%00&commit={COMMIT}"),
    ] {
        assert_eq!(
            read_json(address, format!("{COMMIT_EVIDENCE}?{query}"))
                .await
                .0,
            400
        );
    }
    for unsupported in [json!("anonymous"), Value::Null] {
        report["auth_method"] = unsupported;
        let body = report.to_string();
        let (status, _) =
            tokio::task::spawn_blocking(move || post(address, "/v0/attestations", &body))
                .await
                .expect("ingest finishes");
        assert_eq!(status, 400);
    }
}

// REQ-0015: the newest attestation of every repository and execution points, in a stable order.
#[tokio::test(flavor = "multi_thread")]
async fn latest_selects_the_newest_attestation_of_every_combination_in_order() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool).await;
    let (b, a) = ("github.com/o/b", "github.com/o/a");
    ingest(address, envelope(b, &["ci"], &commit('1'), &[])).await;
    ingest(address, envelope(a, &["pre-push", "ci"], &commit('2'), &[])).await;
    ingest(address, envelope(a, &["pre-push"], &commit('3'), &[])).await;
    ingest(address, envelope(a, &["ci"], &commit('4'), &[])).await;
    let newest_b = ingest(address, envelope(b, &["ci"], &commit('5'), &[])).await;
    let newest_a = ingest(address, envelope(a, &["pre-push"], &commit('6'), &[])).await;

    let summaries = latest(address).await;

    let order: Vec<(&str, Vec<&str>, &str)> = summaries
        .iter()
        .map(|summary| {
            (
                summary["repository"].as_str().expect("repository"),
                summary["execution_points"]
                    .as_array()
                    .expect("points")
                    .iter()
                    .map(|point| point.as_str().expect("point"))
                    .collect(),
                summary["commit"].as_str().expect("commit"),
            )
        })
        .collect();
    let (commit4, commit2, commit6, commit5) = (commit('4'), commit('2'), commit('6'), commit('5'));
    assert_eq!(
        order,
        [
            (a, vec!["ci"], commit4.as_str()),
            (a, vec!["pre-push"], commit6.as_str()),
            (a, vec!["pre-push", "ci"], commit2.as_str()),
            (b, vec!["ci"], commit5.as_str()),
        ]
    );
    assert_eq!(summaries[1]["id"], newest_a);
    assert_eq!(summaries[3]["id"], newest_b);
}

// REQ-0015: nothing stored is an empty list.
#[tokio::test(flavor = "multi_thread")]
async fn latest_is_empty_without_attestations() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;

    assert!(latest(address).await.is_empty());
}

// REQ-0016: the summary counts WARN and NOT_RUN results.
// REQ-0020: the summary counts all results.
// REQ-0017: the attestation has the summary fields and its results in stored order.
#[tokio::test(flavor = "multi_thread")]
async fn counts_and_results_follow_the_stored_attestation() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let statuses = ["WARN", "PASS", "NOT_RUN", "WARN", "FAIL", "NOT_RUN", "WARN"];
    let id = ingest(address, envelope("r", &["ci"], COMMIT, &statuses)).await;

    let summaries = latest(address).await;
    let (status, attestation) = read_json(address, format!("/v0/attestations/{id}")).await;

    assert_eq!(summaries[0]["warn_count"], 3);
    assert_eq!(summaries[0]["not_run_count"], 2);
    assert_eq!(summaries[0]["result_count"], 7);
    assert_eq!(status, 200);
    assert_eq!(attestation["id"], id);
    assert_eq!(attestation["sender"], "ci");
    assert_eq!(attestation["repository"], "r");
    assert_eq!(attestation["commit"], COMMIT);
    assert_eq!(attestation["execution_points"], json!(["ci"]));
    assert_eq!(attestation["overall"], "PASS");
    assert_eq!(attestation["warn_count"], 3);
    assert_eq!(attestation["not_run_count"], 2);
    assert_eq!(attestation["result_count"], 7);
    assert!(
        attestation["received_at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );
    let results: Vec<(&str, &str)> = attestation["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|result| {
            (
                result["id"].as_str().expect("id"),
                result["status"].as_str().expect("status"),
            )
        })
        .collect();
    let expected: Vec<(String, &str)> = statuses
        .iter()
        .enumerate()
        .map(|(index, status)| (format!("rule-{index}"), *status))
        .collect();
    assert_eq!(
        results,
        expected
            .iter()
            .map(|(id, status)| (id.as_str(), *status))
            .collect::<Vec<_>>()
    );
    assert_eq!(attestation["results"][0]["kind"], "check");
}

// REQ-0042: the detail read returns reported bindings, or an empty array without them.
#[tokio::test(flavor = "multi_thread")]
async fn attestation_detail_returns_the_stored_rule_set_bindings() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let points = ["check", "pre-push", "ci"];
    let baseline = set_binding("baseline", "2.0", &["pre-push", "check"]);
    let mut older = set_binding("baseline", "1.0", &["ci"]);
    older["path"] = json!("bundles/baseline");
    let bindings = json!([baseline, older, baseline]);
    let mut bound = envelope("bound", &points, COMMIT, &["PASS"]);
    bound["attestation"]["rule_sets"] = bindings.clone();
    let unbound = envelope("unbound", &points, COMMIT, &["PASS"]);
    let schema: Value = serde_json::from_str(include_str!(
        "../why/contracts/read/attestation.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::draft202012::new(&schema).unwrap();

    for (report, expected) in [(bound, bindings), (unbound, json!([]))] {
        let id = ingest(address, report).await;
        let (status, attestation) = read_json(address, format!("/v0/attestations/{id}")).await;
        assert_eq!(status, 200);
        assert_eq!(attestation["rule_sets"], expected);
        validator.validate(&attestation).unwrap();
    }

    assert!(
        latest(address)
            .await
            .iter()
            .all(|summary| summary.get("rule_sets").is_none())
    );
}

// REQ-0032: the detail read returns stored identities with optional versions.
#[tokio::test(flavor = "multi_thread")]
async fn results_return_the_stored_rule_identity_when_present() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let mut report = envelope("r", &["ci"], COMMIT, &["PASS"; 3]);
    let identity =
        json!({"source": "example.org/Org/Rules", "path": "rules/é & #", "id": "Check<1>"});
    let mut versioned_identity = identity.clone();
    versioned_identity["version"] = json!("0.12");
    report["attestation"]["results"][0]["rule"] = versioned_identity.clone();
    report["attestation"]["results"][0]["rule"]["digest"] = json!("reported-digest");
    report["attestation"]["results"][1]["rule"] = identity.clone();
    let id = ingest(address, report).await;

    let (status, attestation) = read_json(address, format!("/v0/attestations/{id}")).await;

    assert_eq!(status, 200);
    let results = attestation["results"].as_array().expect("results");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0]["id"], "rule-0");
    assert_eq!(results[0]["rule"], versioned_identity);
    assert_eq!(results[1]["rule"], identity);
    assert!(results[2].get("rule").is_none());
}

// REQ-0028: a later registration resolves results against the full identity and version.
// REQ-0031: the registered name is optional and independent of verification.
#[tokio::test(flavor = "multi_thread")]
async fn verification_reflects_a_registration_that_arrives_after_ingest() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let vectors: Value =
        serde_json::from_str(include_str!("../why/contracts/rule-digest/vectors.json")).unwrap();
    let unnamed = &vectors[0]["input"];
    let named = &vectors[1]["input"];
    let matching = json!({
        "source": named["source"],
        "path": named["path"],
        "id": named["id"],
        "version": named["version"],
        "digest": vectors[1]["digest"],
    });
    let variants = [
        ("digest", "different-digest"),
        ("source", "github.com/example/other"),
        ("path", ".please/checks/other"),
        ("id", "other"),
        ("version", "1.1"),
    ];
    let mut report = envelope("github.com/example/consumer", &["ci"], COMMIT, &["PASS"; 9]);
    let results = report["attestation"]["results"].as_array_mut().unwrap();
    results[0]["rule"] = matching.clone();
    for (result, (field, value)) in results[1..].iter_mut().zip(variants) {
        result["rule"] = matching.clone();
        result["rule"][field] = json!(value);
    }
    results[6]["rule"] = json!({
        "source": named["source"],
        "path": named["path"],
        "id": named["id"],
    });
    results[8]["rule"] = json!({
        "source": unnamed["source"],
        "path": unnamed["path"],
        "id": unnamed["id"],
        "version": unnamed["version"],
        "digest": vectors[0]["digest"],
    });
    let id = ingest(address, report).await;

    let (status, mut before) = read_json(address, format!("/v0/attestations/{id}")).await;
    assert_eq!(status, 200);
    let unregistered = json!({"status": "unverified", "reason": "unregistered"});
    for result in before["results"].as_array().unwrap() {
        assert_eq!(result["verification"], unregistered);
        assert!(result.get("name").is_none());
    }

    let request =
        json!({"sender": "catalog-ci", "commit": COMMIT, "rules": [unnamed, named]}).to_string();
    let (status, receipt) =
        tokio::task::spawn_blocking(move || post(address, "/v0/rule-registrations", &request))
            .await
            .expect("the request finishes");
    assert_eq!(status, 200, "{receipt}");
    let receipt: Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(
        receipt["outcomes"],
        json!([
            {"outcome": "created", "digest": vectors[0]["digest"]},
            {"outcome": "created", "digest": vectors[1]["digest"]},
        ])
    );

    let (status, after) = read_json(address, format!("/v0/attestations/{id}")).await;
    assert_eq!(status, 200);
    before["results"][0]["verification"] = json!({"status": "verified"});
    before["results"][0]["name"] = named["name"].clone();
    before["results"][1]["verification"] = json!({"status": "unverified", "reason": "mismatch"});
    before["results"][1]["name"] = named["name"].clone();
    before["results"][8]["verification"] = json!({"status": "verified"});
    assert_eq!(after, before);
}

// REQ-0016, REQ-0020: an attestation without results has zero counts and an empty result list.
// REQ-0017: its results are an empty list.
#[tokio::test(flavor = "multi_thread")]
async fn an_attestation_without_results_has_zero_counts() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let id = ingest(address, envelope("r", &["ci"], COMMIT, &[])).await;

    let summaries = latest(address).await;
    let (status, attestation) = read_json(address, format!("/v0/attestations/{id}")).await;

    assert_eq!(summaries[0]["warn_count"], 0);
    assert_eq!(summaries[0]["not_run_count"], 0);
    assert_eq!(summaries[0]["result_count"], 0);
    assert_eq!(status, 200);
    assert_eq!(attestation["result_count"], 0);
    assert_eq!(attestation["results"], json!([]));
}

// REQ-0018: an unknown identifier is 404 and a malformed one is 400.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_malformed_identifiers_are_told_apart() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    ingest(address, envelope("r", &["ci"], COMMIT, &[])).await;

    let (unknown, _) = read(
        address,
        "/v0/attestations/0190a8c2-0000-7000-8000-000000000000",
    )
    .await;
    let (malformed, _) = read(address, "/v0/attestations/not-a-uuid").await;

    assert_eq!(unknown, 404);
    assert_eq!(malformed, 400);
}

// REQ-0019: reading, the history included, leaves the stored data as it was.
#[tokio::test(flavor = "multi_thread")]
async fn reading_changes_nothing() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    let id = ingest(address, envelope("r", &["ci"], COMMIT, &["WARN"])).await;
    let snapshot = || async {
        let attestations: Vec<String> =
            sqlx::query_scalar("select attestations::text from attestations order by id")
                .fetch_all(&pool)
                .await
                .expect("read the attestations");
        let results: Vec<String> = sqlx::query_scalar(
            "select attestation_results::text from attestation_results order by attestation_id, position",
        )
        .fetch_all(&pool)
        .await
        .expect("read the results");
        (attestations, results)
    };
    let before = snapshot().await;

    latest(address).await;
    read_json(address, format!("/v0/attestations/{id}")).await;
    read(address, "/v0/attestations/not-a-uuid").await;
    read_json(address, "/v0/attestations".to_string()).await;
    read(address, "/v0/attestations?limit=0").await;

    assert_eq!(snapshot().await, before);
}

const SMALLEST: &str = "00000000-0000-0000-0000-000000000000";
const GREATEST: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

fn sequential_id(number: i64) -> String {
    format!("0190a8c2-0000-7000-8000-{number:012x}")
}

async fn insert_sequential(pool: &PgPool, count: i64) {
    sqlx::query(
        "insert into attestations \
         (id, received_at, sender, repository, commit_sha, execution_points, overall, schema_version, document) \
         select ('0190a8c2-0000-7000-8000-' || lpad(to_hex(n), 12, '0'))::uuid, \
                timestamptz '2026-01-01 00:00:00+00', 'ci', 'r', $1, array['ci'], 'PASS', 1, '{}'::jsonb \
         from generate_series(1, $2::bigint) n",
    )
    .bind(commit('a'))
    .bind(count)
    .execute(pool)
    .await
    .expect("insert the attestations");
}

async fn history_ids(address: SocketAddr, query: &str) -> Vec<String> {
    let (status, body) = read_json(address, format!("/v0/attestations{query}")).await;
    assert_eq!(status, 200);
    body.as_array()
        .expect("an array")
        .iter()
        .map(|summary| summary["id"].as_str().expect("id").to_string())
        .collect()
}

// REQ-0021: all attestations, newest first, even when they were received at the same instant.
#[tokio::test(flavor = "multi_thread")]
async fn history_orders_by_identifier_from_greatest_to_smallest() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    insert_sequential(&pool, 5).await;

    let ids = history_ids(address, "").await;

    let expected: Vec<String> = (1..=5).rev().map(sequential_id).collect();
    assert_eq!(ids, expected);
}

// REQ-0021: the summary is the one of the latest read.
#[tokio::test(flavor = "multi_thread")]
async fn history_returns_the_summaries_of_the_latest_read() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    ingest(address, envelope("r", &["ci"], COMMIT, &["WARN", "PASS"])).await;

    let (_, history) = read_json(address, "/v0/attestations".to_string()).await;

    assert_eq!(history, Value::Array(latest(address).await));
}

// REQ-0021: nothing stored is an empty list.
#[tokio::test(flavor = "multi_thread")]
async fn history_is_empty_without_attestations() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;

    assert!(history_ids(address, "").await.is_empty());
}

// REQ-0021: the default limit is 100 and the greatest limit is 200.
#[tokio::test(flavor = "multi_thread")]
async fn history_defaults_to_100_and_allows_at_most_200() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    insert_sequential(&pool, 250).await;

    assert_eq!(history_ids(address, "").await.len(), 100);
    assert_eq!(history_ids(address, "?limit=1").await.len(), 1);
    assert_eq!(history_ids(address, "?limit=200").await.len(), 200);
}

// REQ-0021: `before` excludes the given identifier and everything above it.
#[tokio::test(flavor = "multi_thread")]
async fn history_before_returns_only_smaller_identifiers() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    insert_sequential(&pool, 5).await;

    let stored = history_ids(address, &format!("?before={}", sequential_id(3))).await;
    let unknown = history_ids(address, "?before=0190a8c2-0000-7000-8000-0000000000ff").await;
    let smallest = history_ids(address, &format!("?before={SMALLEST}")).await;
    let greatest = history_ids(address, &format!("?before={GREATEST}")).await;

    assert_eq!(stored, [sequential_id(2), sequential_id(1)]);
    assert_eq!(unknown.len(), 5);
    assert!(smallest.is_empty());
    assert_eq!(greatest.len(), 5);
}

// REQ-0021: pages read in sequence return every attestation exactly once, even with arrivals between reads.
#[tokio::test(flavor = "multi_thread")]
async fn history_pages_return_every_attestation_once() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    insert_sequential(&pool, 7).await;

    let mut seen = Vec::new();
    let mut before = String::new();
    loop {
        let page = history_ids(address, &format!("?limit=3{before}")).await;
        seen.extend(page.clone());
        ingest(address, envelope("r", &["ci"], COMMIT, &[])).await;
        if page.len() < 3 {
            break;
        }
        before = format!("&before={}", page.last().expect("a last identifier"));
    }

    let expected: Vec<String> = (1..=7).rev().map(sequential_id).collect();
    assert_eq!(seen, expected);
}

// REQ-0022: every malformed or out-of-range parameter is 400.
#[tokio::test(flavor = "multi_thread")]
async fn history_rejects_invalid_paging_parameters() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;

    for query in [
        "?limit=0",
        "?limit=201",
        "?limit=-1",
        "?limit=abc",
        "?limit=",
        "?limit=1.5",
        "?limit=99999999999999999999",
        "?limit=1&limit=2",
        "?before=not-a-uuid",
        "?before=",
        "?before=0190a8c2-0000-7000-8000-00000000000g",
        "?offset=1",
    ] {
        let path = format!("/v0/attestations{query}");
        let (status, _) = tokio::task::spawn_blocking(move || get(address, &path))
            .await
            .expect("the request finishes");
        assert_eq!(status, 400, "{query}");
    }
}
