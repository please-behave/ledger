mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{attestation_envelope as envelope, get, migrated_pool, post, postgres};
use tokio::net::TcpListener;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

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

async fn record_release(address: SocketAddr, repository: &str, version: &str, commit: &str) {
    let body = json!({
        "repository": repository, "version": version, "commit": commit,
        "sender": "release-ci", "auth_method": "api_key"
    })
    .to_string();
    let (status, receipt) =
        tokio::task::spawn_blocking(move || post(address, "/v0/facts/releases", &body))
            .await
            .expect("release submission finishes");
    assert_eq!(status, 201, "{receipt}");
}

const DEPLOYMENTS: &str = "/v0/deployments";

async fn register_target(address: SocketAddr, sender: &str, target: &str) {
    let request = json!({"sender": sender, "targets": [{"name": target}]}).to_string();
    let (status, receipt) =
        tokio::task::spawn_blocking(move || post(address, "/v0/target-registrations", &request))
            .await
            .expect("target registration finishes");
    assert_eq!(status, 200, "{receipt}");
}

async fn record_deployment(address: SocketAddr, request: Value) {
    let (status, receipt) = tokio::task::spawn_blocking(move || {
        post(address, "/v0/facts/deployments", &request.to_string())
    })
    .await
    .expect("deployment submission finishes");
    assert_eq!(status, 201, "{receipt}");
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

// REQ-0040, REQ-0041, REQ-0046, REQ-0048, REQ-0055: the reads share check evidence.
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
    register_target(address, "ci", "production").await;
    for repository in ["complete", "incomplete", "unknown"] {
        record_release(address, repository, "v1", COMMIT).await;
        record_deployment(
            address,
            json!({
                "target": "production", "repository": repository, "version": "v1",
                "sender": "ci", "auth_method": "api_key"
            }),
        )
        .await;
        let (_, releases) =
            read_json(address, format!("/v0/releases?repository={repository}")).await;
        let (_, release) = read_json(
            address,
            format!("/v0/releases/detail?repository={repository}&version=v1"),
        )
        .await;
        assert_eq!(
            release["evidence"],
            commit_evidence(address, repository, COMMIT).await
        );
        let completed = if repository == "complete" {
            json!([
                {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "1.0", "name": "Baseline"},
                {"source": SET_SOURCE, "path": ".", "id": "secondary", "version": "1.0", "name": "secondary"}
            ])
        } else {
            json!([])
        };
        assert_eq!(
            releases[0]["evidence"],
            json!({"overall": release["evidence"]["overall"], "completed_rule_sets": completed})
        );
        let (status, deployments) =
            read_json(address, format!("{DEPLOYMENTS}?repository={repository}")).await;
        assert_eq!(status, 200);
        assert_eq!(deployments[0]["state"], "evidence");
        assert_eq!(deployments[0]["evidence"], releases[0]["evidence"]);
    }
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

// REQ-0047, REQ-0048: release reads page by receipt and observe late evidence.
#[tokio::test(flavor = "multi_thread")]
async fn release_reads_page_by_receipt_and_show_late_evidence() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    let newest_commit = commit('1');
    let repository = "github.com/example/project";
    let history = "/v0/releases?repository=github.com%2Fexample%2Fproject";
    let latest = "/v0/releases/latest";
    let detail = "/v0/releases/detail?repository=github.com%2Fexample%2Fproject&version=v1%2F%2B";
    let cursor = "%7B%22repository%22%3A%22github.com%2Fexample%2Fproject%22%2C%22version%22%3A%22v1%2F%2B%22%7D";

    record_release(address, repository, "v99", COMMIT).await;
    record_release(address, repository, "v0", COMMIT).await;
    record_release(address, repository, "v1/+", &newest_commit).await;
    record_release(address, "another", "v2", COMMIT).await;
    sqlx::query(
        "update release_facts set received_at = case when version = 'v99'
         then '2026-11-24T00:00:00Z'::timestamptz else '2026-11-25T00:00:00Z'::timestamptz end",
    )
    .execute(&pool)
    .await
    .expect("fix receipt times to exercise both ordering keys");

    ingest(
        address,
        envelope("unreleased", &["check"], &newest_commit, &[]),
    )
    .await;
    ingest(address, envelope(repository, &["check"], COMMIT, &[])).await;
    ingest(address, envelope(repository, &["ci"], &newest_commit, &[])).await;
    let fact = json!({
        "repository": repository, "version": "v1/+", "commit": newest_commit,
        "sender": "release-ci", "auth_method": "api_key",
        "received_at": "2026-11-25T00:00:00.000000Z", "evidence": null
    });
    assert_eq!(
        read(
            address,
            "/v0/releases/detail?repository=another&version=unknown"
        )
        .await
        .0,
        404
    );
    assert_eq!(read_json(address, detail.into()).await, (200, fact.clone()));
    for path in [latest, history] {
        assert_eq!(
            read_json(
                address,
                format!(
                    "{path}{}limit=1",
                    if path.contains('?') { '&' } else { '?' }
                )
            )
            .await,
            (200, json!([fact]))
        );
    }

    let (status, next) = read_json(address, format!("{latest}?before={cursor}")).await;
    assert_eq!(status, 200);
    assert_eq!(next.as_array().unwrap().len(), 1);
    assert_eq!(next[0]["repository"], "another");
    assert!(next[0].get("evidence").unwrap().is_null());

    let (status, next) = read_json(address, format!("{history}&before={cursor}")).await;
    assert_eq!(status, 200);
    let versions: Vec<_> = next
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["version"].as_str().unwrap())
        .collect();
    assert_eq!(versions, ["v0", "v99"]);
    assert_eq!(
        next[0]["evidence"],
        json!({"overall": "PASS", "completed_rule_sets": []})
    );

    let mut report = envelope(repository, &["check"], &newest_commit, &["FAIL"]);
    report["attestation"]["overall"] = json!("FAIL");
    let evidence_id = ingest(address, report).await;
    let (status, mut release) = read_json(address, detail.into()).await;
    assert_eq!(status, 200);
    assert_eq!(release["evidence"]["id"], evidence_id);
    assert_eq!(
        release["evidence"],
        commit_evidence(address, "github.com%2Fexample%2Fproject", &newest_commit).await
    );
    release["evidence"] = Value::Null;
    assert_eq!(release, fact);
    for path in [latest, history] {
        let (status, list) = read_json(address, path.into()).await;
        assert_eq!(status, 200);
        assert_eq!(
            list[0]["evidence"],
            json!({"overall": "FAIL", "completed_rule_sets": []})
        );
    }

    assert_eq!(
        read_json(address, "/v0/releases?repository=missing".into()).await,
        (200, json!([]))
    );
    for path in [
        "/v0/releases".into(),
        format!("{history}&limit=0"),
        format!("{history}&limit=201"),
        format!("{history}&limit=1&limit=2"),
        format!("{latest}?repository=another"),
        format!("{latest}?before=invalid"),
        format!("{latest}?before={}", cursor.replace("v1%2F%2B", "unknown")),
        format!("/v0/releases?repository=another&before={cursor}"),
        "/v0/releases/detail?repository=another&version=%00".into(),
        "/v0/releases/detail?repository=another&version=v2&extra=1".into(),
    ] {
        assert_eq!(read_json(address, path.clone()).await.0, 400, "{path}");
    }
}

// REQ-0019, REQ-0054, REQ-0055: current deployment pages reflect late releases and evidence.
#[tokio::test(flavor = "multi_thread")]
async fn deployment_read_pages_current_events_and_resolves_references_at_read_time() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    assert_eq!(
        read_json(address, DEPLOYMENTS.into()).await,
        (200, json!([]))
    );

    let repository = "github.com/example/software";
    let filtered = format!("{DEPLOYMENTS}?repository=github.com%2Fexample%2Fsoftware");
    register_target(address, "ci-a", "production").await;
    register_target(address, "ci-b", "production").await;
    register_target(address, "ci-a", "staging").await;
    for (owner, target, subject, key, reference) in [
        ("ci-a", "production", repository, "version", "v2"),
        ("ci-a", "production", repository, "version", "v1"),
        ("ci-b", "production", repository, "commit", COMMIT),
        ("ci-a", "staging", repository, "commit", COMMIT),
        ("ci-a", "production", "other", "version", "v1"),
    ] {
        let mut report = json!({
            "target": target, "repository": subject, "sender": owner, "auth_method": "api_key"
        });
        report[key] = json!(reference);
        record_deployment(address, report).await;
    }
    sqlx::query("update deployment_events set received_at = '2026-11-25T00:00:00Z'")
        .execute(&pool)
        .await
        .expect("tie receipt times");

    // A release of the same version from another repository cannot resolve this one.
    record_release(address, "other", "v1", COMMIT).await;
    let mut report = envelope(repository, &["check"], COMMIT, &["FAIL"]);
    report["attestation"]["overall"] = json!("FAIL");
    ingest(address, report).await;
    ingest(address, envelope(repository, &["ci"], COMMIT, &[])).await;
    let evidence = json!({"overall": "FAIL", "completed_rule_sets": []});

    let (status, list) = read_json(address, filtered.clone()).await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 3);
    assert_eq!(
        list[0]["target"],
        json!({"owner": "ci-a", "name": "staging"})
    );
    assert_eq!(
        list[1]["target"],
        json!({"owner": "ci-b", "name": "production"})
    );
    assert_eq!(list[1]["reported_commit"], COMMIT);
    assert_eq!(list[1]["version"], Value::Null);
    assert_eq!(list[1]["commit"], COMMIT);
    assert_eq!(list[1]["state"], "evidence");
    assert_eq!(list[1]["evidence"], evidence);
    let pending = &list[2];
    assert_eq!(
        *pending,
        json!({
            "id": pending["id"], "target": {"owner": "ci-a", "name": "production"},
            "repository": repository, "version": "v1", "reported_commit": null, "commit": null,
            "sender": "ci-a", "auth_method": "api_key", "received_at": "2026-11-25T00:00:00.000000Z",
            "state": "release_not_recorded", "evidence": null
        })
    );

    let (status, first) = read_json(address, format!("{DEPLOYMENTS}?limit=1")).await;
    assert_eq!(status, 200);
    assert_eq!(first.as_array().unwrap().len(), 1);
    assert_eq!(first[0]["repository"], "other");
    let other_cursor = first[0]["id"].as_str().unwrap();
    let cursor = list[0]["id"].as_str().unwrap();
    assert_eq!(
        read_json(address, format!("{DEPLOYMENTS}?before={other_cursor}")).await,
        (200, list.clone())
    );
    assert_eq!(
        read_json(address, format!("{filtered}&before={cursor}&limit=2")).await,
        (200, json!([list[1], pending]))
    );
    assert_eq!(
        read_json(
            address,
            format!("{filtered}&before={}", pending["id"].as_str().unwrap())
        )
        .await,
        (200, json!([]))
    );

    for query in [
        "limit=0",
        "limit=201",
        "limit=1&limit=2",
        "repository=",
        "repository=%00",
        "before=0",
        "before=invalid",
        "before=9999",
        "extra=1",
    ] {
        assert_eq!(
            read_json(address, format!("{DEPLOYMENTS}?{query}")).await.0,
            400,
            "{query}"
        );
    }
    assert_eq!(
        read_json(address, format!("{filtered}&before={other_cursor}"))
            .await
            .0,
        400
    );

    let released_commit = commit('a');
    ingest(
        address,
        envelope("other", &["check"], &released_commit, &[]),
    )
    .await;
    record_release(address, repository, "v1", &released_commit).await;
    let mut resolved = pending.clone();
    resolved["commit"] = json!(released_commit);
    resolved["state"] = json!("no_evidence");
    assert_eq!(
        read_json(
            address,
            format!("{filtered}&before={}", list[1]["id"].as_str().unwrap())
        )
        .await,
        (200, json!([resolved]))
    );
    ingest(
        address,
        envelope(repository, &["check"], &released_commit, &[]),
    )
    .await;
    let (_, updated) = read_json(address, filtered).await;
    resolved["state"] = json!("evidence");
    resolved["evidence"] = json!({"overall": "PASS", "completed_rule_sets": []});
    assert_eq!(updated[2], resolved);

    record_deployment(
        address,
        json!({
            "target": "staging", "repository": repository, "commit": released_commit,
            "sender": "ci-a", "auth_method": "api_key"
        }),
    )
    .await;
    assert_eq!(
        read_json(address, format!("{DEPLOYMENTS}?before={cursor}")).await,
        (200, json!([list[1], resolved]))
    );
    assert_eq!(
        read_json(address, format!("{DEPLOYMENTS}?before={other_cursor}")).await,
        (200, json!([list[1], resolved]))
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("select count(*) from deployment_events")
            .fetch_one(&pool)
            .await
            .expect("count deployment events"),
        6
    );
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
// REQ-0061: only matching Rule Set versions contribute their stored descriptions.
#[tokio::test(flavor = "multi_thread")]
async fn attestation_detail_returns_the_stored_rule_set_bindings() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let points = ["check", "pre-push", "ci"];
    let baseline = set_binding("baseline", "2.0", &["pre-push", "check"]);
    let older = set_binding("baseline", "1.0", &["ci"]);
    let unregistered = set_binding("baseline", "3.0", &["ci"]);
    let mut other_path = baseline.clone();
    other_path["path"] = json!("bundles/baseline");
    let bindings = json!([baseline, older, unregistered, other_path, baseline]);
    let description = "Résumé <baseline> & checks\n".repeat(100);
    register_for_compliance(
        address,
        "/v0/rule-set-registrations",
        json!({"sender": "ci", "commit": COMMIT, "sets": [
            {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "1.0", "members": []},
            {"source": SET_SOURCE, "path": ".", "id": "baseline", "version": "2.0", "members": [],
             "description": description}
        ]}),
    )
    .await;
    let mut bound = envelope("bound", &points, COMMIT, &["PASS"]);
    bound["attestation"]["rule_sets"] = bindings.clone();
    let mut enriched = bindings;
    enriched[0]["description"] = json!(description);
    enriched[4]["description"] = json!(description);
    let unbound = envelope("unbound", &points, COMMIT, &["PASS"]);
    let schema: Value = serde_json::from_str(include_str!(
        "../why/contracts/read/attestation.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::draft202012::new(&schema).unwrap();

    for (report, expected) in [(bound, enriched), (unbound, json!([]))] {
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
// REQ-0061: the same version match supplies an optional stored description.
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
        assert!(result.get("description").is_none());
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
    before["results"][0]["description"] = named["description"].clone();
    before["results"][1]["verification"] = json!({"status": "unverified", "reason": "mismatch"});
    before["results"][1]["name"] = named["name"].clone();
    before["results"][1]["description"] = named["description"].clone();
    before["results"][8]["verification"] = json!({"status": "verified"});
    assert_eq!(after, before);
    let schema: Value = serde_json::from_str(include_str!(
        "../why/contracts/read/attestation.schema.json"
    ))
    .unwrap();
    jsonschema::draft202012::new(&schema)
        .unwrap()
        .validate(&after)
        .unwrap();
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

// REQ-0049: filter by exact identity before taking each page.
#[tokio::test(flavor = "multi_thread")]
async fn history_pages_only_through_the_requested_repository() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let repository = "github.com/owner/project";
    let oldest = ingest(address, envelope(repository, &["ci"], COMMIT, &[])).await;
    ingest(
        address,
        envelope("github.com/owner/project-tools", &["ci"], COMMIT, &[]),
    )
    .await;
    let middle = ingest(address, envelope(repository, &["ci"], COMMIT, &[])).await;
    let newest = ingest(address, envelope(repository, &["ci"], COMMIT, &[])).await;
    ingest(
        address,
        envelope("github.com/other/project", &["ci"], COMMIT, &[]),
    )
    .await;

    let query = "?repository=github.com%2Fowner%2Fproject&limit=2";
    let first = history_ids(address, query).await;
    assert_eq!(first, [newest, middle]);

    let next = history_ids(address, &format!("{query}&before={}", first[1])).await;
    assert_eq!(next, [oldest]);
    assert!(
        history_ids(address, "?repository=github.com/owner/Project")
            .await
            .is_empty()
    );
    assert!(
        history_ids(address, "?repository=github.com/owner/missing")
            .await
            .is_empty()
    );
}

// REQ-0022, REQ-0049: every malformed or out-of-range parameter is 400.
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
        "?repository",
        "?repository=",
        "?repository=github.com/owner/project&repository=github.com/owner/project",
        "?repository=github.com%2Fowner%2Fproject%00",
        "?offset=1",
    ] {
        let path = format!("/v0/attestations{query}");
        let (status, _) = tokio::task::spawn_blocking(move || get(address, &path))
            .await
            .expect("the request finishes");
        assert_eq!(status, 400, "{query}");
    }
}

const OVERVIEW: &str = "/v0/repositories/overview";

async fn overview(address: SocketAddr) -> Value {
    let (status, body) = read(address, OVERVIEW).await;
    assert_eq!(status, 200, "{body}");
    let response: Value = serde_json::from_str(&body).expect("overview JSON");
    let schema = serde_json::from_str(include_str!(
        "../why/contracts/read/repository-overview.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap();
    assert!(validator.is_valid(&response), "{response}");
    response
}

fn overview_entry<'a>(response: &'a Value, repository: &str) -> &'a Value {
    response["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["repository"] == repository)
        .unwrap_or_else(|| panic!("missing {repository} from {response}"))
}

async fn sync_at(
    address: SocketAddr,
    scope: &str,
    completed_at: chrono::DateTime<chrono::Utc>,
    inventory: &[(&str, bool)],
    heads: &[Value],
) {
    let inventory: Vec<_> = inventory
        .iter()
        .map(|(name, archived)| {
            json!({
                "name": name, "default_branch": "main", "archived": archived, "fork": false,
                "last_activity_at": null
            })
        })
        .collect();
    let request = json!({
        "sender": "github-collector", "platform": "github", "host": "github.com", "scope": scope,
        "completed_at": completed_at, "inventory": inventory, "heads": heads
    });
    let (status, body) = tokio::task::spawn_blocking(move || {
        post(address, "/v0/facts/platform-syncs", &request.to_string())
    })
    .await
    .unwrap();
    assert_eq!(status, 204, "{body}");
}

fn observed(name: &str, commit: &str, at: chrono::DateTime<chrono::Utc>) -> Value {
    json!({
        "name": name, "branch": "main", "commit_sha": commit,
        "committed_at": at, "observed_at": at
    })
}

fn assert_state(entry: &Value, state: &str, token: &str) {
    assert_eq!(entry["state"], state, "{entry}");
    assert_eq!(entry["status_token"], token, "{entry}");
}

async fn database_now(pool: &PgPool) -> chrono::DateTime<chrono::Utc> {
    sqlx::query_scalar("select date_trunc('second', statement_timestamp())")
        .fetch_one(pool)
        .await
        .unwrap()
}

// REQ-0059, REQ-0060: one read includes only applicable repositories and scopes,
// and selects the newest reports independently of the state of observed-head evidence.
#[tokio::test]
async fn repository_overview_selects_reports_and_uses_covering_heartbeats() {
    use chrono::TimeDelta;

    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    assert_eq!(
        overview(address).await,
        json!({"heartbeat": {"completed_at": null, "stale": true}, "repositories": []})
    );

    let now = database_now(&pool).await;
    let stale = now - TimeDelta::hours(2);
    let owner_wide = now - TimeDelta::minutes(15);
    let specific = now - TimeDelta::minutes(5);
    let current = "github.com/owner/current";
    let unobserved = "github.com/owner/unobserved";
    let archived = "github.com/owner/archived";
    let no_check = "github.com/owner/no-check";
    let unmanaged = "github.com/owner/unmanaged";
    let stale_repository = "github.com/stale/empty";
    let head = observed("current", COMMIT, owner_wide);
    let mut check = envelope(current, &["check", "ci"], COMMIT, &["FAIL"]);
    check["attestation"]["overall"] = json!("FAIL");
    let before = ingest(address, check).await;
    ingest(address, envelope(archived, &["check"], COMMIT, &[])).await;
    ingest(address, envelope(unobserved, &["check"], COMMIT, &[])).await;
    ingest(address, envelope(no_check, &["ci"], COMMIT, &[])).await;
    ingest(address, envelope(unmanaged, &["check"], COMMIT, &[])).await;

    sync_at(address, "stale/*", stale, &[("empty", false)], &[]).await;
    assert_state(
        overview_entry(&overview(address).await, stale_repository),
        "unknown",
        "status-not-run",
    );
    sync_at(
        address,
        "owner/*",
        owner_wide,
        &[
            ("current", false),
            ("unobserved", false),
            ("no-check", false),
            ("never", false),
            ("archived", true),
        ],
        &[head],
    )
    .await;
    sync_at(
        address,
        "owner/current",
        specific,
        &[("current", false)],
        &[],
    )
    .await;
    sync_at(address, "owner/archived", stale, &[("archived", true)], &[]).await;
    sync_at(address, "unrelated/*", now, &[], &[]).await;

    record_release(address, current, "z", COMMIT).await;
    record_release(address, current, "a", &commit('2')).await;
    register_target(address, "deployer", "staging").await;
    register_target(address, "infra", "production").await;
    for (sender, target, repository) in [
        ("deployer", "staging", current),
        ("infra", "production", current),
        ("deployer", "staging", "github.com/owner/fact-only"),
    ] {
        record_deployment(
            address,
            json!({
                "sender": sender, "auth_method": "api_key", "target": target,
                "repository": repository, "commit": COMMIT
            }),
        )
        .await;
    }
    sqlx::query("update release_facts set received_at = $1")
        .bind(owner_wide)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("update deployment_events set received_at = $1")
        .bind(specific)
        .execute(&pool)
        .await
        .unwrap();

    let response = overview(address).await;
    assert_eq!(
        response["heartbeat"],
        json!({"completed_at": stale, "stale": true})
    );
    let names: Vec<_> = response["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["repository"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            current,
            "github.com/owner/never",
            no_check,
            unmanaged,
            unobserved,
            stale_repository
        ]
    );

    let entry = overview_entry(&response, current);
    assert_state(entry, "current", "status-passed");
    assert_eq!(entry["heartbeat_completed_at"], json!(specific));
    assert_eq!(entry["head"]["direct_evidence"], true);
    assert_eq!(entry["latest_check"]["id"], before);
    assert_eq!(entry["latest_check"]["overall"], "FAIL");
    assert_eq!(entry["observed_heads_since_evidence"], 0);
    assert_eq!(
        entry["last_evidence_at"],
        entry["latest_check"]["received_at"]
    );
    assert_eq!(
        entry["latest_release"],
        json!({"version": "z", "commit": COMMIT, "received_at": owner_wide})
    );
    assert_eq!(
        entry["latest_deployment"],
        json!({"owner": "infra", "name": "production", "received_at": specific})
    );

    let entry = overview_entry(&response, no_check);
    assert_state(entry, "never_integrated", "status-failed");
    assert_eq!(entry["heartbeat_completed_at"], json!(owner_wide));
    for field in [
        "head",
        "latest_check",
        "last_evidence_at",
        "latest_release",
        "latest_deployment",
    ] {
        assert_eq!(entry[field], Value::Null, "{field}");
    }
    assert_state(
        overview_entry(&response, "github.com/owner/never"),
        "never_integrated",
        "status-failed",
    );
    assert_state(
        overview_entry(&response, unobserved),
        "unknown",
        "status-not-run",
    );
    let entry = overview_entry(&response, unmanaged);
    assert_state(entry, "unknown", "status-not-run");
    assert_eq!(entry["managed"], false);
    assert_eq!(entry["platform"], Value::Null);
    assert_eq!(entry["heartbeat_completed_at"], Value::Null);
    assert_eq!(entry["latest_check"]["commit"], COMMIT);
    let entry = overview_entry(&response, stale_repository);
    assert_state(entry, "unknown", "status-not-run");
    assert_eq!(entry["heartbeat_completed_at"], json!(stale));

    let stored: (bool, i64) = sqlx::query_as(
        "select archived, (select count(*) from attestations a where a.repository = i.repository)
         from repository_inventory i where i.repository = $1",
    )
    .bind(archived)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, (true, 1));
}

// REQ-0060: pending starts at the first uncovered observation, overdue advances
// with elapsed time, and a late check changes evidence for its exact observed head.
#[tokio::test]
async fn repository_overview_derives_pending_and_overdue_from_observation_time() {
    use chrono::TimeDelta;

    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    let now = database_now(&pool).await;
    let first = now - TimeDelta::days(4);
    let covered = now - TimeDelta::minutes(30);
    let pending = now - TimeDelta::minutes(10);
    let one = commit('1');
    let two = commit('2');
    let three = commit('3');
    let repositories = [("late", false), ("old", false)];
    let late = "github.com/owner/late";
    let old = "github.com/owner/old";
    ingest(address, envelope(old, &["check"], COMMIT, &[])).await;
    sync_at(
        address,
        "owner/*",
        first,
        &repositories,
        &[observed("old", &one, first), observed("late", &one, first)],
    )
    .await;
    sync_at(
        address,
        "owner/*",
        covered,
        &repositories,
        &[observed("late", &two, covered)],
    )
    .await;
    sync_at(
        address,
        "owner/*",
        pending,
        &repositories,
        &[observed("late", &three, pending)],
    )
    .await;

    // Late evidence on an older observed head moves the start of the gap forward.
    ingest(address, envelope(late, &["check"], &two, &[])).await;
    ingest(address, envelope(late, &["ci"], &three, &[])).await;
    let response = overview(address).await;
    assert_eq!(
        response["heartbeat"],
        json!({"completed_at": pending, "stale": false})
    );
    let entry = overview_entry(&response, late);
    assert_state(entry, "pending", "status-allowed");
    assert_eq!(entry["pending_since"], json!(pending));
    assert_eq!(entry["head"]["direct_evidence"], false);
    assert_eq!(entry["observed_heads_since_evidence"], 1);
    assert_eq!(entry["overdue_steps"], Value::Null);
    assert!(entry["pending_age_seconds"].as_i64().unwrap() >= 600);
    let entry = overview_entry(&response, old);
    assert_state(entry, "overdue", "status-warning");
    assert_eq!(entry["pending_since"], json!(first));
    assert_eq!(entry["observed_heads_since_evidence"], 1);
    assert_eq!(entry["overdue_step"], 3);
    assert_eq!(entry["overdue_steps"], 4);

    let exact = ingest(address, envelope(late, &["check"], &three, &[])).await;
    let newest = ingest(address, envelope(late, &["check"], COMMIT, &[])).await;
    let response = overview(address).await;
    let entry = overview_entry(&response, late);
    assert_state(entry, "current", "status-passed");
    assert_eq!(entry["head"]["direct_evidence"], true);
    assert_eq!(entry["observed_heads_since_evidence"], 0);
    assert_eq!(entry["pending_since"], Value::Null);
    assert_ne!(exact, newest);
    assert_eq!(entry["latest_check"]["id"], newest);
    assert_eq!(entry["latest_check"]["commit"], COMMIT);
}
