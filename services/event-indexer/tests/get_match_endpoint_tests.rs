//! Tests for the GET /matches/:match_id endpoint.
//!
//! Tests include:
//! - Successful match lookup (200 OK)
//! - Match not found (404 Not Found) with descriptive error message
//! - Full match object structure validation

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Request, StatusCode};
use tokio::sync::RwLock;
use tower::ServiceExt;

use event_indexer::api::{build_router, ApiResponse};
use event_indexer::api_cache::ApiCache;
use event_indexer::cache::EventCache;
use event_indexer::db::Database;
use event_indexer::models::MatchInfo;
use event_indexer::rpc::SorobanRpcClient;
use sqlx::Row;

const UNREACHABLE_DSN: &str = "postgres://nobody:nobody@127.0.0.1:1/nowhere";

fn app() -> axum::Router {
    let db = Arc::new(
        Database::from_dsns(UNREACHABLE_DSN, UNREACHABLE_DSN, 1, 1).expect("DSN must parse"),
    );
    let cache = Arc::new(RwLock::new(EventCache::new(16)));
    let rpc = Arc::new(SorobanRpcClient::new("http://127.0.0.1:1").unwrap());
    build_router(db, cache, rpc, Arc::new(ApiCache::disabled()), None)
}

/// Issue a GET request and return the status and response body.
async fn get_match(match_id: u64) -> (StatusCode, String) {
    let response = app()
        .oneshot(
            Request::builder()
                .uri(format!("/matches/{}", match_id))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8(body.to_vec()).unwrap();

    (status, body_str)
}

#[tokio::test]
async fn get_match_invalid_match_id_returns_404() {
    // A genuine "not found" is Ok(None) from build_match_info, which
    // requires a query that actually completes -- the UNREACHABLE_DSN
    // fixture the other tests in this file use can only ever produce a
    // connection error (500), never a real empty result. Needs a reachable
    // (but unseeded) database, same as
    // integration_tests.rs::api_match_info_not_found_returns_404.
    if std::env::var("DATABASE_URL").is_err() {
        println!("Skipping get_match_invalid_match_id_returns_404: DATABASE_URL not set");
        return;
    }

    let db_url = std::env::var("DATABASE_URL").unwrap();
    let db = Arc::new(Database::from_dsns(&db_url, &db_url, 2, 2).expect("db"));
    db.init_schema().await.expect("schema");
    let cache = Arc::new(RwLock::new(EventCache::new(16)));
    let rpc = Arc::new(SorobanRpcClient::new("http://127.0.0.1:1").unwrap());
    let app = build_router(db, cache, rpc, Arc::new(ApiCache::disabled()), None);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/matches/99999")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body_bytes.to_vec()).unwrap();

    // Parse the response to ensure it's proper JSON
    let response: ApiResponse<Option<MatchInfo>> =
        serde_json::from_str(&body).expect("Response should be valid JSON");

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "Expected 404 for non-existent match"
    );
    assert!(!response.success, "Response success should be false");
    assert!(
        response.data.is_none(),
        "Response data should be None for 404"
    );
    assert!(
        response.error.is_some(),
        "Response should have an error message"
    );

    let error_msg = response.error.unwrap();
    assert!(
        error_msg.contains("99999"),
        "Error message should mention the match ID: {}",
        error_msg
    );
    assert!(
        error_msg.contains("not found"),
        "Error message should say 'not found': {}",
        error_msg
    );
}

/// The `/matches/:id` handler must serve the materialized `matches` row
/// directly, without replaying the event log. We seed only the materialized
/// table (no events at all) and assert the endpoint returns exactly that state.
#[tokio::test]
async fn get_match_served_from_materialized_table() {
    if std::env::var("DATABASE_URL").is_err() {
        println!("Skipping get_match_served_from_materialized_table: DATABASE_URL not set");
        return;
    }

    let db_url = std::env::var("DATABASE_URL").unwrap();
    let db = Arc::new(Database::from_dsns(&db_url, &db_url, 2, 2).expect("db"));
    db.init_schema().await.expect("schema");

    let match_id: u64 = 424_242;
    let player1 = "GPLAYER1AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let player2 = "GPLAYER2BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";

    // Seed ONLY the materialized table; no events exist for this match.
    sqlx::query(
        "INSERT INTO matches (match_id, player1, player2, state, winner, updated_at) \
         VALUES ($1, $2, $3, $4, $5, NOW()) \
         ON CONFLICT (match_id) DO UPDATE SET \
             player1 = EXCLUDED.player1, \
             player2 = EXCLUDED.player2, \
             state = EXCLUDED.state, \
             winner = EXCLUDED.winner, \
             updated_at = NOW()",
    )
    .bind(match_id as i64)
    .bind(player1)
    .bind(player2)
    .bind("Completed")
    .bind(Some(player1))
    .execute(db.pool())
    .await
    .expect("seed materialized match row");

    let cache = Arc::new(RwLock::new(EventCache::new(16)));
    let rpc = Arc::new(SorobanRpcClient::new("http://127.0.0.1:1").unwrap());
    let app = build_router(db.clone(), cache, rpc, Arc::new(ApiCache::disabled()), None);

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/matches/{}", match_id))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body = String::from_utf8(body_bytes.to_vec()).unwrap();

    assert_eq!(
        status,
        StatusCode::OK,
        "Expected 200 for materialized match, body: {}",
        body
    );

    let parsed: ApiResponse<Option<MatchInfo>> =
        serde_json::from_str(&body).expect("Response should be valid JSON ApiResponse");
    assert!(parsed.success, "Response success should be true");
    let info = parsed
        .data
        .expect("Response data should contain the materialized match");

    assert_eq!(info.match_id, match_id, "match_id must match seeded row");
    assert_eq!(info.player1, player1, "player1 must come from matches table");
    assert_eq!(info.player2, player2, "player2 must come from matches table");
    assert_eq!(info.state, "Completed", "state must come from matches table");
    assert_eq!(
        info.winner.as_deref(),
        Some(player1),
        "winner must come from matches table"
    );

    // Confirm the materialized row is what was read: no events were ever
    // inserted for this match, so a replay-based implementation would have
    // returned 404 (or an empty/default state) instead.
    let event_count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM events WHERE match_id = $1")
        .bind(match_id as i64)
        .fetch_one(db.pool())
        .await
        .expect("count events")
        .get("count");
    assert_eq!(
        event_count, 0,
        "Test invariant: no events should exist for the seeded match"
    );

    // Clean up the seeded row so repeated runs stay deterministic.
    let _ = sqlx::query("DELETE FROM matches WHERE match_id = $1")
        .bind(match_id as i64)
        .execute(db.pool())
        .await;
}

#[tokio::test]
async fn get_match_endpoint_returns_valid_response_structure() {
    let (status, body) = get_match(1).await;

    // Even though the database is unreachable, the response structure should be valid
    let _response: ApiResponse<Option<MatchInfo>> =
        serde_json::from_str(&body).expect("Response should always be valid JSON ApiResponse");

    // The request will fail with 500 due to unreachable DB, not 400 (validation),
    // which means the endpoint routing and parameter parsing works correctly
    assert_ne!(
        status,
        StatusCode::BAD_REQUEST,
        "Numeric match_id should pass validation"
    );
}

#[tokio::test]
async fn get_match_with_non_numeric_id_returns_validation_error() {
    let response = app()
        .oneshot(
            Request::builder()
                .uri("/matches/not-a-number")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // The route won't even match if the parameter can't be parsed as u64
    // This should result in a 404 from the router (no matching route)
    let status = response.status();
    // Either 404 (route not matched) or 400 (validation) are acceptable
    assert!(
        status == StatusCode::NOT_FOUND || status == StatusCode::BAD_REQUEST,
        "Non-numeric match_id should be rejected, got: {}",
        status
    );
}
