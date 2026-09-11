use std::collections::BTreeMap;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use sqlx::{Row, SqlitePool};

use crate::DaemonError;
use crate::DaemonServerResponse;
use crate::DaemonState;
use crate::RuntimeProjectConfig;
use crate::ServerTokenRefreshResponse;

const SERVER_PROXY_MAX_PATH_BYTES: usize = 8 * 1024;

const SERVER_RESPONSE_CACHE_MAX_ENTRIES: i64 = 512;
const SERVER_RESPONSE_CACHE_MAX_LOGICAL_BYTES: i64 = 64 * 1024 * 1024;
enum TokenRefreshOutcome {
    Refreshed(RuntimeProjectConfig),
    Superseded,
}

pub(crate) async fn post_server_json<T, R>(
    state: &DaemonState,
    path: &str,
    request: &T,
) -> Result<R, DaemonError>
where
    T: Serialize + ?Sized,
    R: DeserializeOwned,
{
    let mut headers = BTreeMap::new();
    headers.insert("content-type".to_owned(), "application/json".to_owned());
    let response = execute_authenticated_server_request(
        state,
        reqwest::Method::POST,
        path,
        &headers,
        Some(serde_json::to_vec(request)?),
    )
    .await?;
    decode_server_json(response).await
}

pub(crate) async fn get_server_json<R>(state: &DaemonState, path: &str) -> Result<R, DaemonError>
where
    R: DeserializeOwned,
{
    let response = execute_authenticated_server_request(
        state,
        reqwest::Method::GET,
        path,
        &BTreeMap::new(),
        None,
    )
    .await?;
    decode_server_json(response).await
}

pub(crate) async fn delete_server_json(
    state: &DaemonState,
    path: &str,
    expected_version: i64,
) -> Result<(), DaemonError> {
    let mut headers = BTreeMap::new();
    headers.insert("if-match".to_owned(), expected_version.to_string());
    let response =
        execute_authenticated_server_request(state, reqwest::Method::DELETE, path, &headers, None)
            .await?;
    ensure_server_success(response).await?;
    Ok(())
}

pub(crate) async fn execute_authenticated_server_request(
    state: &DaemonState,
    method: reqwest::Method,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<reqwest::Response, DaemonError> {
    validate_server_proxy_path(path)?;
    let snapshot = state.project_config_with_credentials_snapshot().await;
    let request_session_revision = snapshot.session_revision;
    let config = snapshot.config;
    let access_token = config.access_token.clone().ok_or_else(|| {
        DaemonError::InvalidConfig("access_token is required for Server requests".to_owned())
    })?;
    let response = send_server_request(
        state,
        &config.server_url,
        &access_token,
        method.clone(),
        path,
        headers,
        body.clone(),
    )
    .await?;
    if response.status() != reqwest::StatusCode::UNAUTHORIZED || config.refresh_token.is_none() {
        return Ok(response);
    }

    let TokenRefreshOutcome::Refreshed(refreshed) =
        refresh_server_tokens(state, &config, request_session_revision).await?
    else {
        return Ok(response);
    };
    send_server_request(
        state,
        &refreshed.server_url,
        refreshed
            .access_token
            .as_deref()
            .expect("a refreshed session must contain an access token"),
        method,
        path,
        headers,
        body,
    )
    .await
}

async fn send_server_request(
    state: &DaemonState,
    server_url: &str,
    access_token: &str,
    method: reqwest::Method,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<reqwest::Response, DaemonError> {
    let url = format!(
        "{}/{}",
        server_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    );
    let mut builder = state.inner.http.request(method.clone(), url);
    if !access_token.is_empty() {
        builder = builder.bearer_auth(access_token);
    }
    for (name, value) in headers {
        builder = builder.header(name, value);
    }
    let request_id = crate::diagnostics::request_id();
    let body_bytes = body.as_ref().map_or(0, Vec::len);
    let draft_count = (path.split('?').next() == Some("/api/v1/reviews"))
        .then_some(body.as_ref())
        .flatten()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
        .and_then(|v| {
            v.get("drafts")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
        });
    if let Some(body) = body {
        builder = builder.body(body);
    }
    let started = std::time::Instant::now();
    let route = crate::diagnostics::route(path);
    tracing::info!(event = "http_started", %request_id, %method, %route, body_bytes, draft_count);
    let response = builder
        .header("x-clumsies-request-id", &request_id)
        .header("x-request-id", &request_id)
        .send()
        .await
        .map_err(|error| crate::diagnostics::http_error(error, "send"))?;
    let server_request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .and_then(crate::diagnostics::validated_request_id);
    tracing::info!(event = "http_headers", %request_id, server_request_id, %method, %route,
        status = response.status().as_u16(), elapsed_ms = started.elapsed().as_millis() as u64);
    Ok(response)
}

async fn refresh_server_tokens(
    state: &DaemonState,
    expected: &RuntimeProjectConfig,
    expected_session_revision: u64,
) -> Result<TokenRefreshOutcome, DaemonError> {
    let _refresh_guard = state.inner.token_refresh.lock().await;
    let snapshot = state.project_config_snapshot();
    if snapshot.session_revision != expected_session_revision
        || snapshot.config.server_url != expected.server_url
    {
        return Ok(TokenRefreshOutcome::Superseded);
    }
    let stale_access_token = expected
        .access_token
        .as_deref()
        .expect("the rejected authenticated request must retain its access token");
    if snapshot.config.access_token.as_deref() != Some(stale_access_token) {
        if snapshot.config.access_token.is_some() {
            return Ok(TokenRefreshOutcome::Refreshed(snapshot.config));
        }
        return Ok(TokenRefreshOutcome::Superseded);
    }
    let config = snapshot.config;
    let session_revision = snapshot.session_revision;
    let refresh_token = config.refresh_token.clone().ok_or_else(|| {
        DaemonError::InvalidConfig("refresh_token is required to refresh the session".to_owned())
    })?;
    let response = send_server_request(
        state,
        &config.server_url,
        "",
        reqwest::Method::POST,
        "/api/v1/auth/token",
        &BTreeMap::from([("content-type".to_owned(), "application/json".to_owned())]),
        Some(serde_json::to_vec(
            &json!({"grant_type": "refresh_token", "refresh_token": refresh_token}),
        )?),
    )
    .await?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .map_err(|error| crate::diagnostics::http_error(error, "body"))?;
        if status == reqwest::StatusCode::BAD_REQUEST
            || status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            state
                .clear_server_tokens_if_current(
                    session_revision,
                    &config.server_url,
                    stale_access_token,
                )
                .await?;
        }
        return Err(DaemonError::ServerResponse {
            status: status.as_u16(),
            body,
        });
    }
    let tokens: ServerTokenRefreshResponse = decode_server_json(response).await?;

    let _mutation = state.inner.project_config_mutation.lock().await;
    let current = state.project_config_snapshot();
    if current.session_revision != session_revision
        || current.config.server_url != config.server_url
        || current.config.access_token.as_deref() != Some(stale_access_token)
    {
        return Ok(TokenRefreshOutcome::Superseded);
    }
    let mut refreshed = current.config;
    refreshed.access_token = Some(tokens.access_token);
    refreshed.refresh_token = Some(tokens.refresh_token);
    crate::replace_server_credentials(
        state.inner.credential_store.clone(),
        refreshed.credentials(),
    )
    .await?;
    state.publish_project_config(refreshed.clone());
    Ok(TokenRefreshOutcome::Refreshed(refreshed))
}

pub(crate) async fn decode_server_json<R>(response: reqwest::Response) -> Result<R, DaemonError>
where
    R: DeserializeOwned,
{
    let response = ensure_server_success(response).await?;
    response
        .json::<R>()
        .await
        .map_err(|error| crate::diagnostics::http_error(error, "decode"))
}

pub(crate) async fn ensure_server_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, DaemonError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response
        .text()
        .await
        .map_err(|error| crate::diagnostics::http_error(error, "body"))?;
    Err(DaemonError::ServerResponse {
        status: status.as_u16(),
        body,
    })
}

pub(crate) fn validate_server_proxy_path(path: &str) -> Result<(), DaemonError> {
    if path.len() > SERVER_PROXY_MAX_PATH_BYTES
        || !path.starts_with("/api/v1/")
        || path.contains("\r")
        || path.contains("\n")
        || path.contains("#")
        || path.split('?').next().is_some_and(|path| {
            path.split('/')
                .any(|segment| segment == "." || segment == "..")
        })
    {
        return Err(DaemonError::InvalidRequest(
            "Server proxy path must be a normalized /api/v1 resource".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn filter_proxy_request_headers(
    headers: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    const ALLOWED: &[&str] = &[
        "accept",
        "content-type",
        "idempotency-key",
        "if-match",
        "if-none-match",
        "x-clumsies-request-id",
    ];
    headers
        .into_iter()
        .filter_map(|(name, value)| {
            let name = name.to_ascii_lowercase();
            ALLOWED.contains(&name.as_str()).then_some((name, value))
        })
        .collect()
}

pub(crate) fn filter_proxy_response_headers(
    headers: &reqwest::header::HeaderMap,
) -> BTreeMap<String, String> {
    const ALLOWED: &[&str] = &["content-type", "etag", "x-request-id"];
    headers
        .iter()
        .filter_map(|(name, value)| {
            let name = name.as_str();
            if !ALLOWED.contains(&name) {
                return None;
            }
            value
                .to_str()
                .ok()
                .map(|value| (name.to_owned(), value.to_owned()))
        })
        .collect()
}

pub(crate) fn is_retryable_http_status(status: u16) -> bool {
    status == 408 || status == 429 || status >= 500
}

pub(crate) async fn save_cached_server_response(
    pool: &SqlitePool,
    server_url: &str,
    path: &str,
    response: &DaemonServerResponse,
) -> Result<(), DaemonError> {
    let headers_json = serde_json::to_string(&response.headers)?;
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO server_response_cache (
            server_url, path, status, headers_json, body, updated_at
         )
         VALUES ($1, $2, $3, $4, $5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT(server_url, path) DO UPDATE SET
            status = excluded.status,
            headers_json = excluded.headers_json,
            body = excluded.body,
            updated_at = excluded.updated_at",
    )
    .bind(server_url)
    .bind(path)
    .bind(i64::from(response.status))
    .bind(headers_json)
    .bind(&response.body)
    .execute(&mut *transaction)
    .await?;
    prune_cached_server_responses(
        &mut transaction,
        SERVER_RESPONSE_CACHE_MAX_ENTRIES,
        SERVER_RESPONSE_CACHE_MAX_LOGICAL_BYTES,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

async fn prune_cached_server_responses(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    max_entries: i64,
    max_logical_bytes: i64,
) -> Result<(), DaemonError> {
    sqlx::query(
        "WITH ranked AS (
            SELECT
                rowid,
                ROW_NUMBER() OVER (
                    ORDER BY updated_at DESC, rowid DESC
                ) AS cache_rank,
                SUM(
                    length(CAST(server_url AS BLOB))
                    + length(CAST(path AS BLOB))
                    + length(CAST(headers_json AS BLOB))
                    + length(CAST(body AS BLOB))
                ) OVER (
                    ORDER BY updated_at DESC, rowid DESC
                    ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
                ) AS cumulative_bytes
            FROM server_response_cache
         )
         DELETE FROM server_response_cache
         WHERE rowid IN (
            SELECT rowid
            FROM ranked
            WHERE cache_rank > $1 OR cumulative_bytes > $2
         )",
    )
    .bind(max_entries)
    .bind(max_logical_bytes)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub(crate) async fn load_cached_server_response(
    pool: &SqlitePool,
    server_url: &str,
    path: &str,
) -> Result<Option<DaemonServerResponse>, DaemonError> {
    let Some(row) = sqlx::query(
        "SELECT status, headers_json, body
         FROM server_response_cache
         WHERE server_url = $1 AND path = $2",
    )
    .bind(server_url)
    .bind(path)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    let mut headers: BTreeMap<String, String> =
        serde_json::from_str(&row.try_get::<String, _>("headers_json")?)?;
    headers.insert("x-clumsies-cache".to_owned(), "stale".to_owned());
    Ok(Some(DaemonServerResponse {
        status: row.try_get::<i64, _>("status")?.try_into().map_err(|_| {
            DaemonError::Server("cached Server response has an invalid status".to_owned())
        })?,
        headers,
        body: row.try_get("body")?,
    }))
}

pub(crate) async fn clear_server_response_cache(pool: &SqlitePool) -> Result<(), DaemonError> {
    sqlx::query("DELETE FROM server_response_cache")
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_request_headers_preserve_contract_headers_and_filter_credentials() {
        let filtered = filter_proxy_request_headers(BTreeMap::from([
            ("Accept".to_owned(), "application/json".to_owned()),
            ("Content-Type".to_owned(), "application/json".to_owned()),
            ("IdEmPoTeNcY-Key".to_owned(), "project-create-1".to_owned()),
            ("If-Match".to_owned(), "17".to_owned()),
            ("If-None-Match".to_owned(), "\"etag\"".to_owned()),
            ("X-Clumsies-Request-ID".to_owned(), "request-1".to_owned()),
            ("Authorization".to_owned(), "Bearer caller-token".to_owned()),
            ("Cookie".to_owned(), "session=caller".to_owned()),
            ("X-Untrusted".to_owned(), "value".to_owned()),
        ]));

        assert_eq!(
            filtered,
            BTreeMap::from([
                ("accept".to_owned(), "application/json".to_owned()),
                ("content-type".to_owned(), "application/json".to_owned()),
                ("idempotency-key".to_owned(), "project-create-1".to_owned(),),
                ("if-match".to_owned(), "17".to_owned()),
                ("if-none-match".to_owned(), "\"etag\"".to_owned()),
                ("x-clumsies-request-id".to_owned(), "request-1".to_owned(),),
            ])
        );
    }

    #[test]
    fn proxy_path_has_a_bounded_cache_key() {
        let oversized = format!("/api/v1/{}", "a".repeat(SERVER_PROXY_MAX_PATH_BYTES));

        assert!(validate_server_proxy_path(&oversized).is_err());
        assert!(validate_server_proxy_path("/api/v1/reviews?limit=100").is_ok());
    }
    #[tokio::test]
    async fn response_cache_pruning_enforces_entry_and_utf8_byte_limits() {
        assert_eq!(SERVER_RESPONSE_CACHE_MAX_ENTRIES, 512);
        assert_eq!(SERVER_RESPONSE_CACHE_MAX_LOGICAL_BYTES, 64 * 1024 * 1024);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE server_response_cache (
                server_url TEXT NOT NULL,
                path TEXT NOT NULL,
                status BIGINT NOT NULL,
                headers_json TEXT NOT NULL,
                body TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (server_url, path)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();

        for index in 0..4 {
            sqlx::query(
                "INSERT INTO server_response_cache (
                    server_url, path, status, headers_json, body, updated_at
                 ) VALUES ($1, $2, 200, '{}', 'body', $3)",
            )
            .bind("s")
            .bind(format!("p{index}"))
            .bind(format!("2026-01-01T00:00:0{index}Z"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let mut transaction = pool.begin().await.unwrap();
        prune_cached_server_responses(&mut transaction, 2, i64::MAX)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM server_response_cache ORDER BY updated_at DESC, rowid DESC",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(paths, ["p3", "p2"]);

        sqlx::query("DELETE FROM server_response_cache")
            .execute(&pool)
            .await
            .unwrap();
        for index in 0..3 {
            sqlx::query(
                "INSERT INTO server_response_cache (
                    server_url, path, status, headers_json, body, updated_at
                 ) VALUES ($1, $2, 200, '{}', $3, $4)",
            )
            .bind("s")
            .bind(format!("p{index}"))
            .bind("你好")
            .bind(format!("2026-01-01T00:00:0{index}Z"))
            .execute(&pool)
            .await
            .unwrap();
        }
        let mut transaction = pool.begin().await.unwrap();
        // Each row is 11 logical bytes: server 1 + path 2 + headers 2 + UTF-8 body 6.
        // A character-count implementation would incorrectly retain two rows under 15.
        prune_cached_server_responses(&mut transaction, 10, 15)
            .await
            .unwrap();
        transaction.commit().await.unwrap();
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM server_response_cache ORDER BY updated_at DESC, rowid DESC",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(paths, ["p2"]);
    }
}
