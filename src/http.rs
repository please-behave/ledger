use crate::attestation::{Envelope, valid_commit, valid_rule_identity, valid_version};
use crate::config::OverviewConfig;
use crate::database;
use crate::deployment_fact::{DeploymentFact, DeploymentOutcome};
use crate::platform_facts::PlatformSync;
use crate::registration::{JsonObject, Outcome, OutcomeKind, RegistrationRequest};
use crate::release_fact::{ReleaseFact, valid_release_key_part};
use crate::repository_overview::{self, RepositoryOverview};
use crate::rule_set_registration::{RuleSetRequest, SetOutcome};
use crate::target_registration::{TargetOutcome, TargetRequest};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, RawQuery, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Extension, Json, Router, serve};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::signal;
use uuid::Uuid;

const DEFAULT_PAGE_SIZE: i64 = 100;
const MAX_PAGE_SIZE: i64 = 200;
const MAX_INGEST_BODY_BYTES: usize = 2 * 1024 * 1024;
const MAX_PLATFORM_SYNC_BYTES: usize = 16 * 1024 * 1024;

pub fn router(pool: PgPool) -> Router {
    router_with_overview_config(pool, OverviewConfig::default())
}

fn router_with_overview_config(pool: PgPool, config: OverviewConfig) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v0/attestations", post(ingest).get(history))
        .route("/v0/attestations/latest", get(latest))
        .route("/v0/attestations/{id}", get(read))
        .route("/v0/repositories/commit-evidence", get(commit_evidence))
        .route(
            "/v0/repositories/overview",
            get(repository_overview).layer(Extension(config)),
        )
        .route("/v0/releases", get(release_history))
        .route("/v0/releases/latest", get(latest_releases))
        .route("/v0/releases/detail", get(release_detail))
        .route("/v0/deployments", get(deployments))
        .route("/v0/rules", get(rules))
        .route("/v0/rules/detail", get(rule_detail))
        .route("/v0/rule-sets", get(rule_sets))
        .route("/v0/rule-sets/detail", get(rule_set_detail))
        .route("/v0/rule-sets/compliance", get(rule_set_compliance))
        .route(
            "/v0/repositories/completed-rule-sets",
            get(completed_rule_sets),
        )
        .route(
            "/v0/rule-registrations",
            post(register).layer(DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES)),
        )
        .route(
            "/v0/rule-set-registrations",
            post(register_sets).layer(DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES)),
        )
        .route(
            "/v0/target-registrations",
            post(register_targets).layer(DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES)),
        )
        .route(
            "/v0/facts/releases",
            post(record_release).layer(DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES)),
        )
        .route(
            "/v0/facts/deployments",
            post(record_deployment).layer(DefaultBodyLimit::max(MAX_INGEST_BODY_BYTES)),
        )
        .route(
            "/v0/facts/platform-syncs",
            post(record_platform_sync).layer(DefaultBodyLimit::max(MAX_PLATFORM_SYNC_BYTES)),
        )
        .with_state(pool)
}

pub async fn run(listener: TcpListener, pool: PgPool) -> std::io::Result<()> {
    run_with_overview_config(listener, pool, OverviewConfig::default()).await
}

pub async fn run_with_overview_config(
    listener: TcpListener,
    pool: PgPool,
    config: OverviewConfig,
) -> std::io::Result<()> {
    serve(listener, router_with_overview_config(pool, config))
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

async fn record_release(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let fact = ReleaseFact::parse(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let outcome = database::record_release(&pool, &fact)
        .await
        .map_err(|error| {
            tracing::error!(%error, "cannot store the release fact");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let status = match outcome {
        OutcomeKind::Created => StatusCode::CREATED,
        OutcomeKind::Unchanged => StatusCode::OK,
        OutcomeKind::Conflict => StatusCode::CONFLICT,
    };
    Ok((status, Json(json!({ "outcome": outcome }))))
}

async fn record_deployment(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let fact = DeploymentFact::parse(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let outcome = database::record_deployment(&pool, &fact)
        .await
        .map_err(|error| {
            tracing::error!(%error, "cannot store the deployment fact");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::BAD_REQUEST)?;
    let status = match outcome {
        DeploymentOutcome::Created => StatusCode::CREATED,
        DeploymentOutcome::Unchanged => StatusCode::OK,
    };
    Ok((status, Json(json!({ "outcome": outcome }))))
}

async fn record_platform_sync(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let sync = PlatformSync::parse(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    sync.store(&pool).await.map_err(|error| {
        tracing::error!(%error, "cannot store the platform sync");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Serialize)]
struct RegistrationReceipt<T> {
    outcomes: Vec<T>,
}

async fn register(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<Json<RegistrationReceipt<Outcome>>, StatusCode> {
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

async fn register_sets(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<Json<RegistrationReceipt<SetOutcome>>, StatusCode> {
    let request = RuleSetRequest::parse(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mut outcomes = Vec::with_capacity(request.sets.len());
    for set in &request.sets {
        let outcome = database::register_set(&pool, &request.sender, &request.commit, set)
            .await
            .map_err(|error| {
                tracing::error!(%error, "cannot store the Rule Set registration");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        outcomes.push(outcome);
    }
    Ok(Json(RegistrationReceipt { outcomes }))
}

async fn register_targets(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<Json<RegistrationReceipt<TargetOutcome>>, StatusCode> {
    let request = TargetRequest::parse(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let outcomes = database::register_targets(&pool, request)
        .await
        .map_err(|error| {
            tracing::error!(%error, "cannot store the deployment target registrations");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(RegistrationReceipt { outcomes }))
}

async fn latest(State(pool): State<PgPool>) -> Result<Json<Vec<database::Summary>>, StatusCode> {
    database::latest_attestations(&pool)
        .await
        .map(Json)
        .map_err(|error| read_failure(&error))
}

struct HistoryPage {
    limit: i64,
    before: Option<Uuid>,
    repository: Option<String>,
}

impl HistoryPage {
    fn parse(query: Option<&str>) -> Option<Self> {
        let (mut limit, mut before, mut repository) = (None, None, None);
        for pair in query
            .unwrap_or_default()
            .split('&')
            .filter(|pair| !pair.is_empty())
        {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            match name {
                "limit" if limit.is_none() => limit = Some(value.parse::<i64>().ok()?),
                "before" if before.is_none() => before = Some(value.parse::<Uuid>().ok()?),
                "repository" if repository.is_none() => {
                    let (_, identity) = form_urlencoded::parse(pair.as_bytes()).next()?;
                    if identity.is_empty() || identity.contains('\0') {
                        return None;
                    }
                    repository = Some(identity.into_owned());
                }
                _ => return None,
            }
        }
        Some(Self {
            limit: page_limit(limit).ok()?,
            before,
            repository,
        })
    }
}

async fn history(
    State(pool): State<PgPool>,
    RawQuery(query): RawQuery,
) -> Result<Json<Vec<database::Summary>>, StatusCode> {
    let page = HistoryPage::parse(query.as_deref()).ok_or(StatusCode::BAD_REQUEST)?;
    database::attestation_history(&pool, page.before, page.limit, page.repository.as_deref())
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

async fn repository_overview(
    State(pool): State<PgPool>,
    Extension(config): Extension<OverviewConfig>,
) -> Result<Json<RepositoryOverview>, StatusCode> {
    repository_overview::read(&pool, &config)
        .await
        .map(Json)
        .map_err(|error| read_failure(&error))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitEvidenceQuery {
    repository: String,
    commit: String,
}

async fn commit_evidence(
    State(pool): State<PgPool>,
    Query(query): Query<CommitEvidenceQuery>,
) -> Result<Json<Value>, StatusCode> {
    if query.repository.contains('\0') || !valid_commit(&query.commit) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let evidence = database::commit_evidence(&pool, &query.repository, &query.commit)
        .await
        .map_err(|error| read_failure(&error))?;
    Ok(Json(json!({ "evidence": evidence })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseKey {
    repository: String,
    version: String,
}

impl ReleaseKey {
    fn parts(&self) -> Result<(&str, &str), StatusCode> {
        if !valid_release_key_part(&self.repository) || !valid_release_key_part(&self.version) {
            return Err(StatusCode::BAD_REQUEST);
        }
        Ok((&self.repository, &self.version))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasePage {
    repository: Option<String>,
    limit: Option<i64>,
    before: Option<String>,
}

async fn release_history(
    State(pool): State<PgPool>,
    Query(page): Query<ReleasePage>,
) -> Result<Json<Vec<database::ReleaseSummary>>, StatusCode> {
    if !page
        .repository
        .as_deref()
        .is_some_and(valid_release_key_part)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    release_list(&pool, page).await
}

async fn latest_releases(
    State(pool): State<PgPool>,
    Query(page): Query<ReleasePage>,
) -> Result<Json<Vec<database::ReleaseSummary>>, StatusCode> {
    if page.repository.is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    release_list(&pool, page).await
}

async fn release_list(
    pool: &PgPool,
    page: ReleasePage,
) -> Result<Json<Vec<database::ReleaseSummary>>, StatusCode> {
    let limit = page_limit(page.limit)?;
    let cursor = page
        .before
        .as_deref()
        .map(serde_json::from_str::<JsonObject<ReleaseKey>>)
        .transpose()
        .map_err(|_| StatusCode::BAD_REQUEST)?
        .map(|JsonObject(key)| key);
    let before = cursor.as_ref().map(ReleaseKey::parts).transpose()?;
    let repository = page.repository.as_deref();
    if let Some((cursor_repository, _)) = before
        && repository.is_some_and(|repository| repository != cursor_repository)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    paged_response(database::release_page(pool, repository, before, limit).await)
}

async fn release_detail(
    State(pool): State<PgPool>,
    Query(key): Query<ReleaseKey>,
) -> Result<Json<database::ReleaseDetail>, StatusCode> {
    let (repository, version) = key.parts()?;
    database::release_detail(&pool, repository, version)
        .await
        .map_err(|error| read_failure(&error))?
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeploymentPage {
    repository: Option<String>,
    limit: Option<i64>,
    before: Option<i64>,
}

async fn deployments(
    State(pool): State<PgPool>,
    Query(page): Query<DeploymentPage>,
) -> Result<Json<Vec<database::DeploymentSummary>>, StatusCode> {
    let repository = page.repository.as_deref();
    if repository.is_some_and(|value| !valid_release_key_part(value))
        || page.before.is_some_and(|id| id <= 0)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let limit = page_limit(page.limit)?;
    paged_response(database::deployment_page(&pool, repository, page.before, limit).await)
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
struct RegistryPage {
    limit: Option<i64>,
    before: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryIdentity {
    source: String,
    path: String,
    id: String,
}

impl RegistryIdentity {
    fn parts(&self) -> Result<(&str, &str, &str), StatusCode> {
        valid_rule_identity(&self.source, &self.path, &self.id)
            .then_some((self.source.as_str(), self.path.as_str(), self.id.as_str()))
            .ok_or(StatusCode::BAD_REQUEST)
    }
}

impl RegistryPage {
    fn parameters(self) -> Result<(Option<database::RegistryCursor>, i64), StatusCode> {
        let limit = page_limit(self.limit)?;
        let before = self
            .before
            .as_deref()
            .map(serde_json::from_str::<JsonObject<database::RegistryCursor>>)
            .transpose()
            .map_err(|_| StatusCode::BAD_REQUEST)?
            .map(|JsonObject(cursor)| cursor);
        if before.as_ref().is_some_and(|cursor| {
            !valid_rule_identity(&cursor.source, &cursor.path, &cursor.id)
                || !valid_version(&cursor.version)
                || cursor.scan.registrations < 1
        }) {
            return Err(StatusCode::BAD_REQUEST);
        }
        Ok((before, limit))
    }
}

async fn rules(
    State(pool): State<PgPool>,
    Query(page): Query<RegistryPage>,
) -> Result<Json<Vec<database::RegistryEntry<database::RuleSummary>>>, StatusCode> {
    let (before, limit) = page.parameters()?;
    paged_response(database::registered_rules(&pool, before.as_ref(), limit).await)
}

async fn rule_sets(
    State(pool): State<PgPool>,
    Query(page): Query<RegistryPage>,
) -> Result<Json<Vec<database::RegistryEntry<database::RuleSetSummary>>>, StatusCode> {
    let (before, limit) = page.parameters()?;
    paged_response(database::registered_sets(&pool, before.as_ref(), limit).await)
}

fn paged_response<T>(
    page: Result<Option<Vec<T>>, sqlx::Error>,
) -> Result<Json<Vec<T>>, StatusCode> {
    page.map_err(|error| read_failure(&error))?
        .map(Json)
        .ok_or(StatusCode::BAD_REQUEST)
}

async fn rule_detail(
    State(pool): State<PgPool>,
    Query(identity): Query<RegistryIdentity>,
) -> Result<Json<Vec<database::RuleVersion>>, StatusCode> {
    let (source, path, id) = identity.parts()?;
    version_history(database::registered_rule_versions(&pool, source, path, id).await)
}

async fn rule_set_detail(
    State(pool): State<PgPool>,
    Query(identity): Query<RegistryIdentity>,
) -> Result<Json<Vec<database::RuleSetVersion>>, StatusCode> {
    let (source, path, id) = identity.parts()?;
    version_history(database::registered_set_versions(&pool, source, path, id).await)
}

async fn rule_set_compliance(
    State(pool): State<PgPool>,
    Query(identity): Query<RegistryIdentity>,
) -> Result<Json<Vec<database::RuleSetCompliance>>, StatusCode> {
    let (source, path, id) = identity.parts()?;
    database::rule_set_compliance(&pool, source, path, id)
        .await
        .map(Json)
        .map_err(|error| read_failure(&error))
}

async fn completed_rule_sets(
    State(pool): State<PgPool>,
) -> Result<Json<Vec<database::CompletedRuleSets>>, StatusCode> {
    database::completed_rule_sets(&pool)
        .await
        .map(Json)
        .map_err(|error| read_failure(&error))
}

fn version_history<T>(versions: Result<Vec<T>, sqlx::Error>) -> Result<Json<Vec<T>>, StatusCode> {
    let versions = versions.map_err(|error| read_failure(&error))?;
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
