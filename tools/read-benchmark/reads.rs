use anyhow::{Context, Result, ensure};
use axum::{Router, body, http::Request};
use form_urlencoded::Serializer;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::dataset::Attestation;

const RULES: &str = "github.com/benchmark/rules";

pub async fn cases(router: &Router, attestation: &Attestation) -> Result<Vec<String>> {
    let repository = attestation.repository_identity();
    let scope = [("repository", repository.as_str())];
    let rule = [("source", RULES), ("path", "checks"), ("id", "check-0")];
    let set = [("source", RULES), ("path", "sets"), ("id", "set-0")];
    let mut reads = vec![
        "/v0/attestations/latest".to_owned(),
        format!("/v0/attestations/{}", attestation.id),
        "/v0/repositories/overview".to_owned(),
        uri(
            "/v0/repositories/commit-evidence",
            scope
                .into_iter()
                .chain([("commit", attestation.commit.as_str())]),
        ),
        uri(
            "/v0/releases/detail",
            scope
                .into_iter()
                .chain([("version", attestation.version.as_str())]),
        ),
        uri("/v0/rules/detail", rule),
        uri("/v0/rule-sets/detail", set),
        uri("/v0/rule-sets/compliance", set),
        "/v0/repositories/completed-rule-sets".to_owned(),
    ];

    for (path, filter, limit, cursor) in [
        ("/v0/attestations", &[][..], 10, Cursor::Id),
        ("/v0/attestations", &[], 100, Cursor::Id),
        ("/v0/attestations", &scope, 100, Cursor::Id),
        ("/v0/releases/latest", &[], 10, Cursor::Release),
        ("/v0/releases/latest", &[], 100, Cursor::Release),
        ("/v0/releases", &scope, 100, Cursor::Release),
        ("/v0/deployments", &[], 10, Cursor::Id),
        ("/v0/deployments", &[], 100, Cursor::Id),
        ("/v0/deployments", &scope, 100, Cursor::Id),
        ("/v0/rules", &[], 100, Cursor::Registry),
        ("/v0/rule-sets", &[], 100, Cursor::Registry),
    ] {
        let page_size = limit.to_string();
        let query = filter
            .iter()
            .copied()
            .chain([("limit", page_size.as_str())]);
        let first = uri(path, query.clone());
        let page: Vec<Value> = serde_json::from_slice(&get(router, &first).await?)
            .with_context(|| format!("cannot decode the page at {first}"))?;
        if let Some(last) = page.last().filter(|_| page.len() == limit) {
            let before = cursor.encode(last)?;
            reads.push(uri(path, query.chain([("before", before.as_str())])));
        }
        reads.push(first);
    }
    Ok(reads)
}

enum Cursor {
    Id,
    Release,
    Registry,
}

impl Cursor {
    fn encode(&self, last: &Value) -> Result<String> {
        Ok(match self {
            Self::Id => match &last["id"] {
                Value::String(id) => id.clone(),
                Value::Number(id) => id.to_string(),
                _ => anyhow::bail!("the page entry has no cursor ID"),
            },
            Self::Release => json!({
                "repository": last["repository"],
                "version": last["version"]
            })
            .to_string(),
            Self::Registry => json!({
                "source": last["source"],
                "path": last["path"],
                "id": last["id"],
                "version": last["version"],
                "scan": last["scan"]
            })
            .to_string(),
        })
    }
}

fn uri<'a>(path: &str, query: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
    let encoded = Serializer::new(String::new()).extend_pairs(query).finish();
    format!("{path}?{encoded}")
}

pub async fn get(router: &Router, uri: &str) -> Result<axum::body::Bytes> {
    let response = router
        .clone()
        .oneshot(Request::get(uri).body(body::Body::empty())?)
        .await?;
    ensure!(
        response.status().is_success(),
        "GET {uri} returned {}",
        response.status()
    );
    Ok(body::to_bytes(response.into_body(), usize::MAX).await?)
}
