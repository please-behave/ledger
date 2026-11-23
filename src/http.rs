use crate::attestation::{Envelope, valid_rule_identity, valid_version};
use crate::database;
use crate::registration::{JsonObject, Outcome, RegistrationRequest};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, serve};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::signal;
use uuid::Uuid;

const DEFAULT_PAGE_SIZE: i64 = 100;
const MAX_PAGE_SIZE: i64 = 200;
const MAX_REGISTRATION_BODY_BYTES: usize = 2 * 1024 * 1024;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v0/attestations", post(ingest).get(history))
        .route("/v0/attestations/latest", get(latest))
        .route("/v0/attestations/{id}", get(read))
        .route("/v0/rules", get(rules))
        .route("/v0/rules/detail", get(rule_detail))
        .route(
            "/v0/rule-registrations",
            post(register).layer(DefaultBodyLimit::max(MAX_REGISTRATION_BODY_BYTES)),
        )
        .with_state(pool)
}

pub async fn run(listener: TcpListener, pool: PgPool) -> std::io::Result<()> {
    serve(listener, router(pool))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn readyz(State(pool): State<PgPool>) -> StatusCode {
    if database::schema_ready(&pool).await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn ingest(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let document: Value = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let envelope = Envelope::deserialize(&document).map_err(|_| StatusCode::BAD_REQUEST)?;
    match database::append_attestation(&pool, &envelope, &document).await {
        Ok(id) => Ok((StatusCode::CREATED, Json(json!({ "id": id.to_string() })))),
        Err(error) => Err(storage_failure(&error)),
    }
}

#[derive(serde::Serialize)]
struct RegistrationReceipt {
    outcomes: Vec<Outcome>,
}

async fn register(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<Json<RegistrationReceipt>, StatusCode> {
    let (request, digests) =
        RegistrationRequest::parse(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut outcomes = Vec::with_capacity(digests.len());
    for (rule, digest) in request.rules.iter().zip(digests) {
        let outcome =
            database::register_rule(&pool, &request.sender, &request.commit, rule, &digest)
                .await
                .map_err(|error| {
                    tracing::error!(%error, "cannot store the rule registration");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        outcomes.push(Outcome { outcome, digest });
    }
    Ok(Json(RegistrationReceipt { outcomes }))
}

async fn latest(State(pool): State<PgPool>) -> Result<Json<Vec<database::Summary>>, StatusCode> {
    database::latest_attestations(&pool)
        .await
        .map(Json)
        .map_err(|error| read_failure(&error))
}

struct Page {
    limit: i64,
    before: Option<Uuid>,
}

impl Page {
    fn parse(query: Option<&str>) -> Option<Self> {
        let (mut limit, mut before) = (None, None);
        for pair in query
            .unwrap_or_default()
            .split('&')
            .filter(|pair| !pair.is_empty())
        {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            match name {
                "limit" if limit.is_none() => limit = Some(value.parse::<i64>().ok()?),
                "before" if before.is_none() => before = Some(value.parse::<Uuid>().ok()?),
                _ => return None,
            }
        }
        Some(Self {
            limit: page_limit(limit).ok()?,
            before,
        })
    }
}

async fn history(
    State(pool): State<PgPool>,
    RawQuery(query): RawQuery,
) -> Result<Json<Vec<database::Summary>>, StatusCode> {
    let page = Page::parse(query.as_deref()).ok_or(StatusCode::BAD_REQUEST)?;
    database::attestation_history(&pool, page.before, page.limit)
        .await
        .map(Json)
        .map_err(|error| read_failure(&error))
}

async fn read(
    State(pool): State<PgPool>,
    Path(id): Path<String>,
) -> Result<Json<database::StoredAttestation>, StatusCode> {
    let id: Uuid = id.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    match database::attestation(&pool, id).await {
        Ok(Some(attestation)) => Ok(Json(attestation)),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(error) => Err(read_failure(&error)),
    }
}

fn read_failure(error: &sqlx::Error) -> StatusCode {
    tracing::error!(%error, "cannot read from the ledger");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn page_limit(limit: Option<i64>) -> Result<i64, StatusCode> {
    let limit = limit.unwrap_or(DEFAULT_PAGE_SIZE);
    (1..=MAX_PAGE_SIZE)
        .contains(&limit)
        .then_some(limit)
        .ok_or(StatusCode::BAD_REQUEST)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RulePage {
    limit: Option<i64>,
    before: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleIdentity {
    source: String,
    path: String,
    id: String,
}

impl RuleIdentity {
    fn parts(&self) -> Result<(&str, &str, &str), StatusCode> {
        valid_rule_identity(&self.source, &self.path, &self.id)
            .then_some((self.source.as_str(), self.path.as_str(), self.id.as_str()))
            .ok_or(StatusCode::BAD_REQUEST)
    }
}

async fn rules(
    State(pool): State<PgPool>,
    Query(page): Query<RulePage>,
) -> Result<Json<Vec<database::ListedRule>>, StatusCode> {
    let limit = page_limit(page.limit)?;
    let before = page
        .before
        .as_deref()
        .map(serde_json::from_str::<JsonObject<database::RuleCursor>>)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    let before = before.as_ref().map(|JsonObject(cursor)| cursor);
    if before.is_some_and(|cursor| {
        !valid_rule_identity(&cursor.source, &cursor.path, &cursor.id)
            || !valid_version(&cursor.version)
            || cursor.scan.registrations < 1
    }) {
        return Err(StatusCode::BAD_REQUEST);
    }
    database::registered_rules(&pool, before, limit)
        .await
        .map_err(|error| read_failure(&error))?
        .map(Json)
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn rule_detail(
    State(pool): State<PgPool>,
    Query(identity): Query<RuleIdentity>,
) -> Result<Json<Vec<database::RuleVersion>>, StatusCode> {
    let (source, path, id) = identity.parts()?;
    let versions = database::registered_rule_versions(&pool, source, path, id)
        .await
        .map_err(|error| read_failure(&error))?;
    if versions.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(versions))
}

fn storage_failure(error: &sqlx::Error) -> StatusCode {
    let violates_constraint = error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code.starts_with("23"));
    if violates_constraint {
        StatusCode::BAD_REQUEST
    } else {
        tracing::error!(%error, "cannot store the attestation");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn shutdown_signal() {
    let interrupt = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut stream) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            stream.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
}
