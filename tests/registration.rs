mod support;

use ledger::http;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{get, migrated_pool, post, postgres};
use tokio::net::TcpListener;

const ROUTE: &str = "/v0/rule-registrations";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const RETRY_COMMIT: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

#[derive(Deserialize)]
struct Vector {
    input: Box<RawValue>,
    digest: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("../why/contracts/rule-digest/vectors.json")).unwrap()
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn submit(address: SocketAddr, body: String) -> (u16, String) {
    tokio::task::spawn_blocking(move || post(address, ROUTE, &body))
        .await
        .expect("request finishes")
}

async fn outcomes(address: SocketAddr, body: String) -> Value {
    let (status, receipt) = submit(address, body).await;
    assert_eq!(status, 200, "{receipt}");
    serde_json::from_str::<Value>(&receipt).expect("JSON receipt")["outcomes"].clone()
}

async fn read_registry(address: SocketAddr, path: &str) -> (u16, String) {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || get(address, &path))
        .await
        .expect("request finishes")
}

fn rule_cursor(summary: &Value) -> Value {
    json!({
        "source": summary["source"],
        "path": summary["path"],
        "id": summary["id"],
        "version": summary["version"],
        "scan": summary["scan"],
    })
}

fn rules_after(cursor: &Value, limit: usize) -> String {
    let encoded: String = cursor
        .to_string()
        .bytes()
        .map(|byte| format!("%{byte:02X}"))
        .collect();
    format!("/v0/rules?limit={limit}&before={encoded}")
}

async fn rules_page(address: SocketAddr, path: &str) -> Vec<Value> {
    let (status, body) = read_registry(address, path).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).unwrap()
}

// REQ-0029, REQ-0030
#[tokio::test(flavor = "multi_thread")]
async fn registry_pages_summaries_and_reads_all_versions() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let source = "github.com/team/rules";
    let path = "checks/quality & style+";
    let id = "summary";
    let older_owner = "github.com/team/Rule-Registry";
    let greatest_owner = "  Release Key  ";
    let huge_version = "99999999999999999999999999999999999999999999999999999999999999.0";
    let review = json!({
        "source": source, "path": path, "id": id, "version": huge_version,
        "kind": "review", "name": "<b>Summary</b>", "description": "  One & two\n",
        "blocking": false, "prompt": " Keep <script> & \"quotes\".\r\n",
        "knowledge": {"extra": ["<img src=x>", {"kept": true}], "text": "  line\r\n"},
    });
    let older: Vec<_> = ["2.10", "10.0", "2.2"]
        .into_iter()
        .map(|version| {
            json!({
                "source": source, "path": path, "id": id, "version": version, "kind": "check",
            })
        })
        .collect();
    let greatest_receipt = outcomes(
        address,
        json!({"sender": greatest_owner, "commit": COMMIT, "rules": [
            review,
            {"source": source, "path": path, "id": "other", "version": "1.0", "kind": "check"},
            {"source": source, "path": "checks/other", "id": id, "version": "1.0", "kind": "check"},
            {"source": "a.test/team/rules", "path": path, "id": id, "version": "1.0", "kind": "check"},
        ]}).to_string(),
    )
    .await;
    assert_eq!(greatest_receipt[0]["outcome"], "created");
    let older_receipt = outcomes(
        address,
        json!({"sender": older_owner, "commit": COMMIT, "rules": older}).to_string(),
    )
    .await;
    let retry = outcomes(
        address,
        json!({"sender": "another-ci", "commit": RETRY_COMMIT, "rules": [review]}).to_string(),
    )
    .await;
    assert_eq!(retry[0]["outcome"], "unchanged");
    assert_eq!(retry[0]["digest"], greatest_receipt[0]["digest"]);

    let first_page = rules_page(address, "/v0/rules?limit=1").await;
    let scan = &first_page[0]["scan"];
    assert_eq!(scan["registrations"], 7);
    assert_eq!(
        first_page,
        vec![json!({
            "source": source, "path": path, "id": id, "version": huge_version,
            "kind": "review", "name": review["name"], "description": review["description"],
            "owner": greatest_owner, "scan": scan,
        })]
    );

    let next_page = rules_page(address, &rules_after(&rule_cursor(&first_page[0]), 200)).await;
    assert_eq!(
        next_page,
        vec![
            json!({"source": source, "path": path, "id": "other", "version": "1.0",
                "kind": "check", "owner": greatest_owner, "scan": scan}),
            json!({"source": "a.test/team/rules", "path": path, "id": id, "version": "1.0",
                "kind": "check", "owner": greatest_owner, "scan": scan}),
            json!({"source": source, "path": "checks/other", "id": id, "version": "1.0",
                "kind": "check", "owner": greatest_owner, "scan": scan}),
        ]
    );

    let (status, body) = read_registry(
        address, "/v0/rules/detail?source=github.com%2Fteam%2Frules&path=checks%2Fquality+%26+style%2B&id=summary",
    )
    .await;
    assert_eq!(status, 200);
    let versions: Vec<Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(versions.len(), 4);
    for (version, index) in versions.iter().zip([2, 0, 1]) {
        let mut expected = older[index].clone();
        expected["owner"] = json!(older_owner);
        expected["digest"] = older_receipt[index]["digest"].clone();
        expected["commit"] = json!(COMMIT);
        expected["registered_at"] = version["registered_at"].clone();
        assert_eq!(*version, expected);
        assert!(version["registered_at"].as_str().unwrap().ends_with('Z'));
    }
    let mut expected_review = review;
    expected_review["owner"] = json!(greatest_owner);
    expected_review["digest"] = greatest_receipt[0]["digest"].clone();
    expected_review["commit"] = json!(COMMIT);
    expected_review["registered_at"] =
        json!(sqlx::query_scalar::<_, String>(
        "select to_char(registered_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') \
         from rule_registrations where source = $1 and path = $2 and id = $3 and version = $4",
    )
    .bind(source)
    .bind(path)
    .bind(id)
    .bind(huge_version)
    .fetch_one(&pool)
    .await
    .unwrap());
    assert_eq!(versions[3], expected_review);

    let (status, body) = read_registry(
        address,
        "/v0/rules/detail?source=github.com%2Fteam%2Frules&path=checks%2Fother&id=summary",
    )
    .await;
    assert_eq!(status, 200);
    let only_version: Vec<Value> = serde_json::from_str(&body).unwrap();
    assert_eq!(only_version.len(), 1);
    assert_eq!(only_version[0]["owner"], greatest_owner);
}

// REQ-0029
#[tokio::test(flavor = "multi_thread")]
async fn registry_orders_names_across_pages_even_after_new_registrations() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let expected = [
        ("b.test", "checks", "empty", Some("")),
        ("a.test", "checks", "second", Some("ALPHA")),
        ("b.test", "a", "third", Some("alpha")),
        ("b.test", "checks", "fifth", Some("ALPHA")),
        ("b.test", "checks", "fourth", Some("Alpha")),
        ("b.test", "checks", "Banana", None),
        ("b.test", "checks", "last-ascii", Some("zeta")),
        ("b.test", "checks", "accent-first", Some("éclair")),
        ("b.test", "checks", "accent-second", Some("Éclair")),
        ("b.test", "checks", "greek", Some("Ωmega")),
    ];
    let rules: Vec<_> = expected
        .iter()
        .rev()
        .map(|(source, path, id, name)| {
            let mut rule = json!({"source": source, "path": path, "id": id,
                "version": "1.0", "kind": "check"});
            if let Some(name) = name {
                rule["name"] = json!(name);
            }
            rule
        })
        .collect();
    outcomes(
        address,
        json!({"sender": "ci", "commit": COMMIT, "rules": rules}).to_string(),
    )
    .await;

    let mut summaries = rules_page(address, "/v0/rules?limit=2").await;
    let first_cursor = rule_cursor(summaries.last().unwrap());
    outcomes(
        address,
        json!({"sender": "ci", "commit": COMMIT, "rules": [
            {"source": "a.test", "path": "checks", "id": "second",
                "version": "2.0", "kind": "check", "name": "鼐"},
            {"source": "b.test", "path": "checks", "id": "new",
                "version": "1.0", "kind": "check", "name": "Z"},
        ]})
        .to_string(),
    )
    .await;

    loop {
        let cursor = rule_cursor(summaries.last().unwrap());
        let page = rules_page(address, &rules_after(&cursor, 2)).await;
        if page.is_empty() {
            break;
        }
        assert_eq!(page.len(), 2);
        summaries.extend(page);
        assert!(summaries.len() <= expected.len());
    }
    assert_eq!(summaries.len(), expected.len());
    for (summary, (source, path, id, name)) in summaries.iter().zip(expected) {
        assert_eq!(summary["source"], source);
        assert_eq!(summary["path"], path);
        assert_eq!(summary["id"], id);
        assert_eq!(summary["version"], "1.0");
        assert_eq!(summary.get("name").and_then(Value::as_str), name);
        assert_eq!(summary["scan"], first_cursor["scan"]);
    }

    let fresh = rules_page(address, "/v0/rules").await;
    assert_eq!(fresh.len(), expected.len() + 1);
    assert_eq!(fresh.last().unwrap()["id"], "second");
    assert_eq!(fresh.last().unwrap()["version"], "2.0");

    let mut old_version = rule_cursor(fresh.last().unwrap());
    old_version["version"] = json!("1.0");
    for cursor in [
        old_version,
        json!({"source": "a.test", "path": "checks", "id": "second"}),
        json!({"scan": first_cursor["scan"], "source": "a.test", "path": "checks",
            "id": "second", "version": "01.0"}),
    ] {
        assert_eq!(
            read_registry(address, &rules_after(&cursor, 2)).await.0,
            400
        );
    }

    // A transaction can become visible after the first page with an earlier
    // registration timestamp. Reject this cursor rather than changing its scan.
    sqlx::query(
        "insert into rule_registrations
         (source, path, id, version, kind, digest, sender, commit_sha, registered_at)
         values ('b.test', 'checks', 'late', '1.0', 'check', 'digest', 'ci', $1, '2020-01-01T00:00:00Z')",
    )
    .bind(COMMIT)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        read_registry(address, &rules_after(&first_cursor, 2))
            .await
            .0,
        400
    );
}

// REQ-0029, REQ-0030
#[tokio::test(flavor = "multi_thread")]
async fn registry_distinguishes_empty_reads_from_bad_requests() {
    let database = postgres().await;
    let address = start_service(migrated_pool(&database).await).await;
    assert_eq!(
        read_registry(address, "/v0/rules").await,
        (200, "[]".into())
    );
    assert_eq!(
        read_registry(
            address,
            "/v0/rules/detail?source=github.com/team/rules&path=checks/quality&id=absent"
        )
        .await
        .0,
        404
    );
    for path in [
        "/v0/rules?limit=0",
        "/v0/rules?limit=201",
        "/v0/rules?limit=bad",
        "/v0/rules?limit=1&limit=2",
        "/v0/rules?before=bad",
        "/v0/rules?before=%7B%7D&before=%7B%7D",
        "/v0/rules?before=%7B%7D",
        "/v0/rules?before=%5B%22repo%22%2C%22rules%22%2C%22one%22%5D",
        "/v0/rules?extra=1",
        "/v0/rules/detail?source=repo&path=rules",
        "/v0/rules/detail?source=repo&path=rules&id=",
        "/v0/rules/detail?source=repo&path=rules&id=one&id=two",
        "/v0/rules/detail?source=repo&path=rules&id=one&extra=1",
    ] {
        assert_eq!(read_registry(address, path).await.0, 400, "{path}");
    }
}

fn batch(sender: &str, commit: &str, rules: &str) -> String {
    format!(r#"{{"sender":"{sender}","commit":"{commit}","rules":[{rules}]}}"#)
}

async fn stored_review(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "select jsonb_build_object( \
          'name', name, 'description', description, 'blocking', blocking, 'prompt', prompt, \
          'knowledge', knowledge::text, 'digest', digest, 'sender', sender, \
          'commit', commit_sha, 'registered_at', registered_at) \
         from rule_registrations where id = 'summary' and version = '2.10'",
    )
    .fetch_one(pool)
    .await
    .expect("the original review registration")
}

// REQ-0023, REQ-0026
#[tokio::test(flavor = "multi_thread")]
async fn registrations_match_vectors_and_preserve_the_original() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let vectors = vectors();
    let rules = vectors
        .iter()
        .map(|vector| vector.input.get())
        .collect::<Vec<_>>()
        .join(",");
    let first = outcomes(address, batch("ci", COMMIT, &rules)).await;
    assert_eq!(
        first,
        json!([
            {"outcome": "created", "digest": vectors[0].digest},
            {"outcome": "created", "digest": vectors[1].digest},
            {"outcome": "unchanged", "digest": vectors[2].digest},
        ])
    );

    #[derive(Deserialize)]
    struct ReviewInput {
        name: String,
        description: String,
        prompt: String,
        knowledge: Box<RawValue>,
    }
    let review: ReviewInput = serde_json::from_str(vectors[1].input.get()).unwrap();
    let original = stored_review(&pool).await;
    assert_eq!(original["name"], review.name);
    assert_eq!(original["description"], review.description);
    assert_eq!(original["prompt"], review.prompt);
    assert_eq!(original["blocking"], false);
    assert_eq!(original["sender"], "ci");
    assert_eq!(original["commit"], COMMIT);
    assert_eq!(original["digest"], vectors[1].digest);
    assert_eq!(original["knowledge"], review.knowledge.get());
    assert!(original["registered_at"].is_string());

    let retry = outcomes(address, batch("another-ci", RETRY_COMMIT, &rules)).await;
    assert_eq!(
        retry,
        json!([
            {"outcome": "unchanged", "digest": vectors[0].digest},
            {"outcome": "unchanged", "digest": vectors[1].digest},
            {"outcome": "unchanged", "digest": vectors[2].digest},
        ])
    );

    let mut changed: Value = serde_json::from_str(vectors[1].input.get()).unwrap();
    changed["name"] = json!("New display name");
    let mut new_version = changed.clone();
    new_version["version"] = json!("2.11");
    let mixed = outcomes(
        address,
        batch(
            "another-ci",
            RETRY_COMMIT,
            &format!("{changed},{new_version}"),
        ),
    )
    .await;
    assert_eq!(mixed[0]["outcome"], "conflict");
    assert!(
        mixed[0]["digest"]
            .as_str()
            .unwrap()
            .starts_with("v1:sha256:")
    );
    assert_ne!(mixed[0]["digest"], vectors[1].digest);
    assert_eq!(mixed[1]["outcome"], "created");
    assert_eq!(stored_review(&pool).await, original);

    let stored_digest: String = sqlx::query_scalar(
        "select digest from rule_registrations where id = 'summary' and version = '2.11'",
    )
    .fetch_one(&pool)
    .await
    .expect("the next version exists alongside the original");
    assert_eq!(mixed[1]["digest"], stored_digest);
}

// REQ-0026
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_repetitions_create_one_registration() {
    let database = postgres().await;
    let address = start_service(migrated_pool(&database).await).await;
    let body = batch("ci", COMMIT, vectors()[0].input.get());
    let (first, second) = tokio::join!(outcomes(address, body.clone()), outcomes(address, body),);
    let mut statuses = [
        first[0]["outcome"].as_str().unwrap(),
        second[0]["outcome"].as_str().unwrap(),
    ];
    statuses.sort_unstable();
    assert_eq!(statuses, ["created", "unchanged"]);
    assert_eq!(first[0]["digest"], second[0]["digest"]);
}

// REQ-0027
#[tokio::test(flavor = "multi_thread")]
async fn invalid_batches_leave_the_registry_empty() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let cases = vectors();
    let check = cases[0].input.get();
    let review = cases[1].input.get();
    let positional_rule = json!([
        "github.com/example/catalog",
        ".please/checks/format",
        "format",
        "1.0",
        "check"
    ]);
    let positional_envelope = format!(r#"["ci","{COMMIT}",[{check}]]"#);
    assert_eq!(submit(address, positional_envelope).await.0, 400);
    assert_eq!(
        submit(address, batch("ci", COMMIT, &positional_rule.to_string()))
            .await
            .0,
        400
    );
    for (label, invalid_rule) in [
        ("version syntax", check.replace("1.0", "01.0")),
        (
            "version length",
            check.replace("1.0", &format!("1.{}", "2".repeat(63))),
        ),
        (
            "identity length",
            check.replace("github.com/example/catalog", &"📝".repeat(257)),
        ),
        ("unknown kind", check.replace("\"check\"", "\"integrity\"")),
        (
            "review-only fields",
            check.replace("\"check\"", "\"check\", \"blocking\": false"),
        ),
        (
            "missing blocking",
            review.replace("\"blocking\": false,", ""),
        ),
        (
            "null optional field",
            check.replace("\"check\"", "\"check\", \"name\": null"),
        ),
        (
            "duplicate rule field",
            check.replace("\"check\"", "\"check\", \"kind\": \"check\""),
        ),
        (
            "duplicate nested knowledge field",
            check.replace(
                "\"check\"",
                r#""check", "knowledge": {"items":[{"a": true, "\u0061": false}]}"#,
            ),
        ),
        (
            "normalized knowledge key collision",
            check.replace(
                "\"check\"",
                r#""check", "knowledge": {"\r\n": true, "\n": false}"#,
            ),
        ),
        (
            "invalid knowledge value",
            check.replace("\"check\"", r#""check", "knowledge": {"count": 1}"#),
        ),
    ] {
        let (status, _) = submit(
            address,
            batch("ci", COMMIT, &format!("{check},{invalid_rule}")),
        )
        .await;
        assert_eq!(status, 400, "{label}");
    }
    assert_eq!(submit(address, batch("ci", "invalid", check)).await.0, 400);
    assert_eq!(submit(address, batch("", COMMIT, check)).await.0, 400);
    assert_eq!(submit(address, batch("ci", COMMIT, "")).await.0, 400);
    let too_many = std::iter::repeat_n(check, 101)
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(submit(address, batch("ci", COMMIT, &too_many)).await.0, 400);
    let mut too_large = batch("ci", COMMIT, check);
    too_large.push_str(&" ".repeat(2 * 1024 * 1024));
    assert_eq!(submit(address, too_large).await.0, 413);

    let rows: i64 = sqlx::query_scalar("select count(*) from rule_registrations")
        .fetch_one(&pool)
        .await
        .expect("count registrations");
    assert_eq!(rows, 0);
}
