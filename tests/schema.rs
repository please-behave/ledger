mod support;

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use support::{migrated_pool, postgres};

const ATTESTATION_ID: &str = "'0192f0a0-0000-7000-8000-000000000001'::uuid";
const OTHER_ID: &str = "'0192f0a0-0000-7000-8000-000000000002'::uuid";
const SHA1: &str = "'0123456789abcdef0123456789abcdef01234567'";

const ATTESTATION: [(&str, &str); 8] = [
    ("id", ATTESTATION_ID),
    ("sender", "'ci'"),
    ("repository", "'github.com/owner/name'"),
    ("commit_sha", SHA1),
    ("execution_points", "array['pre-push']"),
    ("overall", "'PASS'"),
    ("schema_version", "1"),
    ("document", "'{}'::jsonb"),
];

const RESULT: [(&str, &str); 5] = [
    ("attestation_id", ATTESTATION_ID),
    ("position", "0"),
    ("rule_id", "'format'"),
    ("kind", "'check'"),
    ("status", "'PASS'"),
];

async fn insert(
    pool: &PgPool,
    table: &str,
    row: &[(&str, &str)],
    overrides: &[(&str, &str)],
) -> Result<(), sqlx::Error> {
    let value = |column: &str, default: &str| {
        overrides
            .iter()
            .find(|(name, _)| *name == column)
            .map_or(default.to_string(), |(_, value)| (*value).to_string())
    };
    let columns: Vec<&str> = row.iter().map(|(column, _)| *column).collect();
    let values: Vec<String> = row
        .iter()
        .map(|(column, default)| value(column, default))
        .collect();
    let sql = format!(
        "insert into {table} ({}) values ({})",
        columns.join(", "),
        values.join(", ")
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .execute(pool)
        .await
        .map(|_| ())
}

async fn insert_attestation(pool: &PgPool, overrides: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    insert(pool, "attestations", &ATTESTATION, overrides).await
}

async fn insert_result(pool: &PgPool, overrides: &[(&str, &str)]) -> Result<(), sqlx::Error> {
    insert(pool, "attestation_results", &RESULT, overrides).await
}

async fn exists(pool: &PgPool, relation: &str) -> bool {
    sqlx::query_scalar("select to_regclass($1) is not null")
        .bind(relation)
        .fetch_one(pool)
        .await
        .expect("query the catalog")
}

async fn migrations_applied(pool: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from _sqlx_migrations where success")
        .fetch_one(pool)
        .await
        .expect("count the migrations")
}

// REQ-0002: migrate creates the complete schema and succeeds again without change.
#[tokio::test]
async fn migrate_creates_the_schema_and_is_idempotent() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let applied = migrations_applied(&pool).await;

    ledger::database::migrate(&pool)
        .await
        .expect("a second migrate");

    assert!(exists(&pool, "attestations").await);
    assert!(exists(&pool, "attestation_results").await);
    assert!(exists(&pool, "rule_registrations").await);
    for index in [
        "attestations_latest_idx",
        "attestations_repository_commit_idx",
        "attestation_results_rule_id_idx",
        "attestation_results_rule_identity_version_idx",
    ] {
        assert!(exists(&pool, index).await, "{index} exists");
    }
    assert_eq!(migrations_applied(&pool).await, applied);
}

// REQ-0028: legacy results read as unregistered after the schema is expanded.
#[tokio::test]
async fn rule_identity_migrations_preserve_existing_rows_and_legacy_inserts() {
    const MIGRATIONS: Migrator = sqlx::migrate!();
    const ORIGINAL_SCHEMA: i64 = 20261121000000;
    let database = postgres().await;
    let pool = ledger::database::connect(&database.url)
        .await
        .expect("connect");
    MIGRATIONS
        .run_to(ORIGINAL_SCHEMA, &pool)
        .await
        .expect("apply the original schema");
    insert_attestation(&pool, &[])
        .await
        .expect("an attestation");
    insert_result(&pool, &[]).await.expect("an existing result");

    ledger::database::migrate(&pool)
        .await
        .expect("expand the schema");
    insert_result(&pool, &[("position", "1")])
        .await
        .expect("a result inserted by an older build");

    let results: Vec<(String, bool)> = sqlx::query_as(
        "select rule_id, rule_source is null and rule_path is null and rule_local_id is null \
         and rule_version is null and rule_digest is null \
         from attestation_results order by position",
    )
    .fetch_all(&pool)
    .await
    .expect("read both legacy results");
    assert_eq!(
        results,
        [("format".to_owned(), true), ("format".to_owned(), true)]
    );

    let id = sqlx::query_scalar("select id from attestations")
        .fetch_one(&pool)
        .await
        .expect("the legacy attestation ID");
    let attestation = ledger::database::attestation(&pool, id)
        .await
        .expect("read the legacy attestation")
        .expect("the legacy attestation exists");
    let response = serde_json::to_value(attestation).expect("serialize the response");
    let unregistered = serde_json::json!({"status": "unverified", "reason": "unregistered"});
    assert_eq!(response["results"][0]["verification"], unregistered);
    assert_eq!(response["results"][1]["verification"], unregistered);

    let (valid, definition): (bool, String) = sqlx::query_as(
        "select indisvalid, pg_get_indexdef(indexrelid) from pg_index \
         where indexrelid = 'attestation_results_rule_identity_version_idx'::regclass",
    )
    .fetch_one(&pool)
    .await
    .expect("read the lookup index");
    assert!(valid, "the concurrent index build completed");
    assert!(definition.contains("(rule_source, rule_path, rule_local_id, rule_version)"));
}

#[tokio::test]
async fn rule_registry_migration_preserves_attestations_and_keys_versions() {
    const MIGRATIONS: Migrator = sqlx::migrate!();
    const PRE_REGISTRY_SCHEMA: i64 = 20261122000001;
    let database = postgres().await;
    let pool = ledger::database::connect(&database.url)
        .await
        .expect("connect");
    MIGRATIONS
        .run_to(PRE_REGISTRY_SCHEMA, &pool)
        .await
        .expect("apply the preceding schema");
    insert_attestation(&pool, &[])
        .await
        .expect("an existing attestation");
    insert_result(&pool, &[]).await.expect("an existing result");
    assert!(!exists(&pool, "rule_registrations").await);

    ledger::database::migrate(&pool)
        .await
        .expect("add the rule registry");
    insert_attestation(&pool, &[("id", OTHER_ID)])
        .await
        .expect("an attestation inserted by an older build");
    insert_result(&pool, &[("attestation_id", OTHER_ID)])
        .await
        .expect("a result inserted by an older build");
    let positions: Vec<i32> = sqlx::query_scalar("select position from attestation_results")
        .fetch_all(&pool)
        .await
        .expect("both attestations retain their results");
    assert_eq!(positions, [0, 0]);

    let insert_version = |version: &'static str| {
        sqlx::query_scalar::<_, bool>(
            "insert into rule_registrations \
             (source, path, id, version, kind, digest, sender, commit_sha) \
             values ($1, $2, $3, $4, 'check', $5, $6, $7) \
             returning registered_at is not null",
        )
        .bind("github.com/owner/name")
        .bind("rules/format")
        .bind("format")
        .bind(version)
        .bind("v1:sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .bind("ci")
        .bind("0123456789abcdef0123456789abcdef01234567")
        .fetch_one(&pool)
    };
    assert!(insert_version("1.0").await.expect("the first version"));
    assert!(insert_version("1.1").await.expect("a second version"));
    let duplicate = insert_version("1.0").await.expect_err("the key is unique");
    assert!(matches!(duplicate, sqlx::Error::Database(error) if error.is_unique_violation()));
}

#[tokio::test]
async fn valid_rows_are_accepted() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;

    insert_attestation(&pool, &[])
        .await
        .expect("a valid attestation");
    insert_attestation(
        &pool,
        &[
            ("id", OTHER_ID),
            ("commit_sha", &format!("'{}'", "a".repeat(64))),
        ],
    )
    .await
    .expect("a 64 character commit");
    insert_result(&pool, &[]).await.expect("a valid result");

    let received: bool =
        sqlx::query_scalar("select received_at is not null from attestations limit 1")
            .fetch_one(&pool)
            .await
            .expect("query");
    assert!(received, "the database sets the receipt time");
}

// REQ-0005: a missing field is rejected.
#[tokio::test]
async fn missing_fields_are_rejected() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    insert_attestation(&pool, &[])
        .await
        .expect("a valid attestation");

    for (column, _) in ATTESTATION {
        let result = insert_attestation(&pool, &[(column, "null"), ("id", OTHER_ID)]).await;
        assert!(result.is_err(), "{column} is required");
    }
    for (column, _) in RESULT {
        let result = insert_result(&pool, &[(column, "null")]).await;
        assert!(result.is_err(), "{column} is required");
    }
}

// REQ-0006: a commit must be 40 or 64 hexadecimal characters.
#[tokio::test]
async fn invalid_commits_are_rejected() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;

    for commit in [
        "''".to_string(),
        "'abc123'".to_string(),
        format!("'{}'", "a".repeat(39)),
        format!("'{}'", "a".repeat(41)),
        format!("'{}'", "a".repeat(63)),
        format!("'{}'", "a".repeat(65)),
        format!("'{}'", "g".repeat(40)),
        format!("'{}'", "z".repeat(64)),
    ] {
        let result = insert_attestation(&pool, &[("commit_sha", &commit)]).await;
        assert!(result.is_err(), "{commit} is rejected");
    }
}

// REQ-0007: the overall status is PASS, FAIL or ERROR.
#[tokio::test]
async fn invalid_overall_statuses_are_rejected() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;

    for (index, overall) in ["PASS", "FAIL", "ERROR"].into_iter().enumerate() {
        let id = format!("'0192f0a0-0000-7000-8000-00000000010{index}'::uuid");
        insert_attestation(&pool, &[("id", &id), ("overall", &format!("'{overall}'"))])
            .await
            .expect("an allowed status");
    }
    for overall in ["", "pass", "NOT_RUN", "WARN", "OK"] {
        let result = insert_attestation(&pool, &[("overall", &format!("'{overall}'"))]).await;
        assert!(result.is_err(), "{overall:?} is rejected");
    }
}

// REQ-0008: a rule result has a known kind and status.
#[tokio::test]
async fn invalid_rule_result_values_are_rejected() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    insert_attestation(&pool, &[])
        .await
        .expect("a valid attestation");

    for (position, kind) in ["check", "review", "integrity"].into_iter().enumerate() {
        insert_result(
            &pool,
            &[
                ("position", &position.to_string()),
                ("kind", &format!("'{kind}'")),
            ],
        )
        .await
        .expect("an allowed kind");
    }
    for (offset, status) in ["PASS", "FAIL", "ERROR", "NOT_RUN", "WARN"]
        .into_iter()
        .enumerate()
    {
        let position = (10 + offset).to_string();
        insert_result(
            &pool,
            &[("position", &position), ("status", &format!("'{status}'"))],
        )
        .await
        .expect("an allowed status");
    }
    for kind in ["", "CHECK", "lint"] {
        let result =
            insert_result(&pool, &[("position", "99"), ("kind", &format!("'{kind}'"))]).await;
        assert!(result.is_err(), "kind {kind:?} is rejected");
    }
    for status in ["", "pass", "SKIPPED"] {
        let result = insert_result(
            &pool,
            &[("position", "99"), ("status", &format!("'{status}'"))],
        )
        .await;
        assert!(result.is_err(), "status {status:?} is rejected");
    }
}

// REQ-0009: a rule result belongs to a stored attestation and has a unique position.
#[tokio::test]
async fn rule_results_keep_their_integrity() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    insert_attestation(&pool, &[])
        .await
        .expect("a valid attestation");
    insert_result(&pool, &[]).await.expect("a valid result");

    let orphan = insert_result(&pool, &[("attestation_id", OTHER_ID)]).await;
    let duplicate = insert_result(&pool, &[("rule_id", "'other'")]).await;

    assert!(
        orphan.is_err(),
        "a result without an attestation is rejected"
    );
    assert!(
        duplicate.is_err(),
        "a second result at one position is rejected"
    );
}
