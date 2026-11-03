#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::Response,
};
use common::{App, body_json};
use obecni_web::backend::{self, Backend, attachment_index::EXTRACTOR_VERSION, subscriptions};
use serde_json::{Value, json};
use std::{collections::BTreeSet, net::SocketAddr};
use time::OffsetDateTime;
use tower::ServiceExt;

const MODERN: &str = "2026-07-28";
const LEGACY: &str = "2025-11-25";
const TOOLS: [&str; 9] = [
    "search",
    "list_documents",
    "get_document",
    "get_page",
    "read_attachment_text",
    "get_content_coverage",
    "list_events",
    "list_rental_assets",
    "get_rental_availability",
];

fn rpc_request(version: &str, method: &str, params: Value) -> Request<Body> {
    rpc_request_with_id(version, method, params, json!(1))
}

fn rpc_request_with_id(version: &str, method: &str, mut params: Value, id: Value) -> Request<Body> {
    if version == MODERN {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": MODERN,
            "io.modelcontextprotocol/clientInfo": {"name":"municipal-integration-test","version":"1"},
            "io.modelcontextprotocol/clientCapabilities": {}
        });
    }
    let mut request = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost:3000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", version)
        .header("mcp-method", method);
    if let Some(name) = params.get("name").and_then(Value::as_str) {
        request = request.header("mcp-name", name);
    }
    request
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string(),
        ))
        .unwrap()
}

#[tokio::test]
async fn overlong_rpc_ids_are_rejected_without_echoing_them() {
    let app = App::new().await;
    for version in [MODERN, LEGACY] {
        // This is below the request-body bound. The ID itself must be rejected
        // before the SDK can copy it into a potentially much larger response.
        let request = rpc_request_with_id(
            version,
            "tools/list",
            json!({}),
            json!("i".repeat(120 * 1024)),
        );
        let response = app.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 2 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["jsonrpc"], "2.0");
        assert!(value["id"].is_null());
        assert_eq!(value["error"]["code"], -32600);
        assert_eq!(value["error"]["data"]["code"], "request_id_too_large");

        let id = json!("i".repeat(1000));
        let response = app
            .router
            .clone()
            .oneshot(rpc_request_with_id(
                version,
                "tools/list",
                json!({}),
                id.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), backend::mcp::MAX_RESPONSE_BYTES)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["id"], id);
        assert!(value["result"]["tools"].is_array());
    }
}

#[tokio::test]
async fn response_bound_includes_the_sdk_envelope_and_correlation_id() {
    let app = App::new().await;
    let page = app.call("POST", "/api/v1/admin/pages", Some(json!({
        "slug":"mcp-boundary-page", "title":"Response boundary", "content":"Boundary fixture", "published":true
    })), true).await;
    assert_eq!(page.status(), StatusCode::CREATED);
    let arguments = json!({"slug":"mcp-boundary-page","max_chars":32000});
    let mut value = backend::agent_content::call(&app.state, "get_page", arguments.clone())
        .await
        .unwrap();

    // Size a real page result just below the payload bound. Escaped source text
    // makes the fixture large without exceeding the public 32000-character cap.
    // The ordinary JSON-RPC envelope and a valid correlation ID then push the
    // final SDK response over the wire bound.
    let target = backend::mcp::MAX_RESPONSE_BYTES - 512;
    let mut lower = 0;
    let mut upper = 32000;
    while lower < upper {
        let count = (lower + upper + 1) / 2;
        value["content"] = json!("\u{1}".repeat(count));
        value["total_characters"] = json!(count);
        let bytes =
            serde_json::to_vec(&rmcp::model::CallToolResult::structured(value.clone())).unwrap();
        if bytes.len() <= target {
            lower = count;
        } else {
            upper = count - 1;
        }
    }
    let content = "\u{1}".repeat(lower);
    sqlx::query("UPDATE pages SET content=$1 WHERE slug='mcp-boundary-page'")
        .bind(content)
        .execute(&app.state.pool)
        .await
        .unwrap();
    let value = backend::agent_content::call(&app.state, "get_page", arguments.clone())
        .await
        .unwrap();
    let bytes = serde_json::to_vec(&rmcp::model::CallToolResult::structured(value)).unwrap();
    // RFC3339 timestamp fractions can differ slightly between the two reads.
    assert!(bytes.len() < backend::mcp::MAX_RESPONSE_BYTES && bytes.len() > target - 64);

    let id = json!("i".repeat(1000));
    let request = rpc_request_with_id(
        MODERN,
        "tools/call",
        json!({"name":"get_page","arguments":arguments}),
        id.clone(),
    );
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = axum::body::to_bytes(response.into_body(), backend::mcp::MAX_RESPONSE_BYTES)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["id"], id);
    assert_eq!(value["error"]["data"]["code"], "response_too_large");
    assert_eq!(value["error"]["data"]["retryable"], false);
    assert!(value.get("result").is_none());
}

async fn rpc(app: &App, version: &str, method: &str, params: Value) -> Response {
    app.router
        .clone()
        .oneshot(rpc_request(version, method, params))
        .await
        .unwrap()
}

async fn success(response: Response) -> Value {
    let status = response.status();
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("application/json"),
        "Simple MCP reads must have a bounded JSON response"
    );
    assert!(response.headers().get("mcp-session-id").is_none());
    let value = body_json(response).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(value["jsonrpc"], "2.0");
    assert_eq!(value["id"], 1);
    assert!(value.get("error").is_none(), "{value}");
    value["result"].clone()
}

fn names(result: &Value) -> BTreeSet<String> {
    result["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn modern_and_legacy_clients_share_the_public_read_only_catalog() {
    let app = App::new().await;
    let discovery = success(rpc(&app, MODERN, "server/discover", json!({})).await).await;
    let versions = discovery["supportedVersions"].as_array().unwrap();
    assert!(versions.contains(&json!(MODERN)));
    assert!(versions.contains(&json!(LEGACY)));
    assert!(discovery["capabilities"]["tools"].is_object());

    // The modern client may list tools immediately, without initialize or a session.
    let modern = success(rpc(&app, MODERN, "tools/list", json!({})).await).await;
    let expected = TOOLS.into_iter().map(str::to_owned).collect();
    assert_eq!(names(&modern), expected);
    for tool in modern["tools"].as_array().unwrap() {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert!(tool["description"].as_str().is_some_and(|s| !s.is_empty()));
    }

    let initialize = success(
        rpc(
            &app,
            LEGACY,
            "initialize",
            json!({"protocolVersion":LEGACY,"capabilities":{},
                "clientInfo":{"name":"legacy-municipal-test","version":"1"}}),
        )
        .await,
    )
    .await;
    assert_eq!(initialize["protocolVersion"], LEGACY);
    let initialized = Request::builder()
        .method("POST")
        .uri("/mcp")
        .header("host", "localhost:3000")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", LEGACY)
        .header("mcp-method", "notifications/initialized")
        .body(Body::from(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.router
            .clone()
            .oneshot(initialized)
            .await
            .unwrap()
            .status(),
        StatusCode::ACCEPTED
    );
    let legacy = success(rpc(&app, LEGACY, "tools/list", json!({})).await).await;
    assert_eq!(names(&legacy), names(&modern));
    assert_eq!(legacy["tools"], modern["tools"]);

    for version in [MODERN, LEGACY] {
        let read = success(
            rpc(
                &app,
                version,
                "tools/call",
                json!({"name":"get_content_coverage","arguments":{}}),
            )
            .await,
        )
        .await;
        assert_ne!(read["isError"], true, "{read}");
        assert!(read["structuredContent"].is_object());
        assert!(
            read["content"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        );
        if version == MODERN {
            assert_eq!(read["resultType"], "complete");
        } else {
            assert!(read.get("resultType").is_none());
        }
    }
}

#[tokio::test]
async fn server_card_matches_runtime_identity_and_versions() {
    let app = App::new().await;
    let card = body_json(app.call("GET", "/mcp/server-card", None, false).await).await;
    let discovery = success(rpc(&app, MODERN, "server/discover", json!({})).await).await;
    assert_eq!(
        card["name"],
        discovery["_meta"]["io.modelcontextprotocol/serverInfo"]["name"]
    );
    assert_eq!(
        card["version"],
        discovery["_meta"]["io.modelcontextprotocol/serverInfo"]["version"]
    );
    assert_eq!(card["remotes"][0]["url"], "http://localhost:3000/mcp");
    assert_eq!(card["remotes"][0]["type"], "streamable-http");
    assert_eq!(
        card["remotes"][0]["supportedProtocolVersions"],
        discovery["supportedVersions"]
    );
    assert!(card.get("tools").is_none());
}

#[tokio::test]
async fn transport_rejects_wrong_origins_invalid_headers_and_oversized_requests() {
    let app = App::new().await;
    for origin in ["https://untrusted.example", "null", "http://localhost:3999"] {
        let mut request = rpc_request(MODERN, "tools/list", json!({}));
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        let response = app.router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin}");
    }
    // An explicit matching browser origin and a normal server request both work.
    let mut request = rpc_request(MODERN, "tools/list", json!({}));
    request
        .headers_mut()
        .insert("origin", "http://localhost:3000".parse().unwrap());
    success(app.router.clone().oneshot(request).await.unwrap()).await;

    let mut request = rpc_request(MODERN, "tools/list", json!({}));
    request
        .headers_mut()
        .insert("host", "attacker.example".parse().unwrap());
    assert_eq!(
        app.router.clone().oneshot(request).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    let mut request = rpc_request(
        MODERN,
        "tools/call",
        json!({"name":"get_content_coverage","arguments":{}}),
    );
    request
        .headers_mut()
        .insert("mcp-method", "tools/list".parse().unwrap());
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"]["code"], -32020);

    let mut request = rpc_request(
        MODERN,
        "tools/call",
        json!({"name":"get_content_coverage","arguments":{}}),
    );
    request
        .headers_mut()
        .insert("mcp-name", "search".parse().unwrap());
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(response).await["error"]["code"], -32020);

    let response = rpc(
        &app,
        MODERN,
        "tools/call",
        json!({"name":"search","arguments":{"query":"x".repeat(129*1024)}}),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn https_default_ports_do_not_become_origin_or_host_wildcards() {
    let app = App::new().await;
    let mut config = app.state.config.as_ref().clone();
    config.public_url = "https://municipal.example".into();
    let router = backend::router(Backend::new(app.state.pool.clone(), config));

    for (host, origin) in [
        ("municipal.example", "https://municipal.example"),
        ("municipal.example:443", "https://municipal.example:443"),
    ] {
        let mut request = rpc_request(MODERN, "tools/list", json!({}));
        request.headers_mut().insert("host", host.parse().unwrap());
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        success(router.clone().oneshot(request).await.unwrap()).await;
    }
    for (host, origin) in [
        ("municipal.example", "https://municipal.example:444"),
        ("municipal.example:444", "https://municipal.example"),
        ("municipal.example", "http://municipal.example"),
    ] {
        let mut request = rpc_request(MODERN, "tools/list", json!({}));
        request.headers_mut().insert("host", host.parse().unwrap());
        request
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        assert_eq!(
            router.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "Unexpectedly accepted Host {host} with Origin {origin}"
        );
    }
}

#[tokio::test]
async fn rate_limit_rejects_the_sixty_first_request_without_trusting_forwarded_ips() {
    let app = App::new().await;
    let peer: SocketAddr = "192.0.2.10:12345".parse().unwrap();
    for index in 0..60 {
        let mut request = rpc_request(MODERN, "tools/list", json!({}));
        request.extensions_mut().insert(ConnectInfo(peer));
        // Changing an untrusted forwarded address must not create a new bucket.
        request
            .headers_mut()
            .insert("x-real-ip", format!("198.51.100.{index}").parse().unwrap());
        request.headers_mut().insert(
            "x-forwarded-for",
            format!("203.0.113.{index}").parse().unwrap(),
        );
        success(app.router.clone().oneshot(request).await.unwrap()).await;
    }
    let mut request = rpc_request(MODERN, "tools/list", json!({}));
    request.extensions_mut().insert(ConnectInfo(peer));
    request
        .headers_mut()
        .insert("x-real-ip", "198.51.100.250".parse().unwrap());
    let response = app.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()["retry-after"], "60");
    assert!(body_json(response).await["error"].is_string());

    let mut request = rpc_request(MODERN, "tools/list", json!({}));
    request.extensions_mut().insert(ConnectInfo(
        "192.0.2.11:12345".parse::<SocketAddr>().unwrap(),
    ));
    success(app.router.clone().oneshot(request).await.unwrap()).await;
}

#[tokio::test]
async fn unavailable_tools_and_methods_are_not_successful_operations() {
    let app = App::new().await;
    for (method, params) in [
        (
            "tools/call",
            json!({"name":"create_reservation","arguments":{}}),
        ),
        (
            "municipal/execute_sql",
            json!({"query":"DELETE FROM documents"}),
        ),
    ] {
        let response = rpc(&app, MODERN, method, params).await;
        let value = body_json(response).await;
        assert_eq!(value["id"], 1);
        assert!(
            matches!(value["error"]["code"].as_i64(), Some(-32601 | -32602)),
            "Unknown operation must be a JSON-RPC error: {value}"
        );
        assert!(value.get("result").is_none());
    }
}

#[tokio::test]
async fn administrator_cookies_do_not_expand_the_public_mcp_scope() {
    let app = App::new().await;
    let document = app.notice(json!({"title":"Neveřejný podklad"})).await;
    let uploaded = app
        .upload(
            &format!("/api/v1/admin/documents/{document}/attachments"),
            "nepublikovany.csv",
            b"internal,do not expose\n",
        )
        .await;
    assert_eq!(uploaded.status(), StatusCode::CREATED);
    let attachment = body_json(uploaded).await["id"].as_i64().unwrap();

    for (name, arguments) in [
        ("get_document", json!({"id":document})),
        ("read_attachment_text", json!({"attachment_id":attachment})),
    ] {
        for authenticated in [false, true] {
            let mut request = rpc_request(
                MODERN,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            );
            if authenticated {
                request
                    .headers_mut()
                    .insert("cookie", app.cookie.parse().unwrap());
                request
                    .headers_mut()
                    .insert("x-csrf-token", app.csrf.parse().unwrap());
            }
            let result = success(app.router.clone().oneshot(request).await.unwrap()).await;
            assert_eq!(result["isError"], true, "{result}");
            assert_eq!(result["structuredContent"]["error"]["status"], 404);
            assert_eq!(result["structuredContent"]["retryable"], false);
            assert!(!result.to_string().contains("Neveřejný podklad"));
            assert!(!result.to_string().contains("do not expose"));
        }
    }
}

#[tokio::test]
async fn a_disabled_public_feature_is_a_structured_tool_error() {
    let app = App::with_rentals(false).await;
    let result = success(
        rpc(
            &app,
            MODERN,
            "tools/call",
            json!({"name":"list_rental_assets","arguments":{}}),
        )
        .await,
    )
    .await;
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["error"]["status"], 503);
    assert!(result["structuredContent"]["error"]["code"].is_string());
    assert!(result["structuredContent"]["error"]["message"].is_string());
    assert!(result["structuredContent"]["retryable"].is_boolean());
}

async fn read_side_effect_state(app: &App) -> Value {
    let mut snapshot = serde_json::Map::new();
    // Each test has its own schema and no background workers. Snapshot the actual
    // persisted sources, publication history, OCR state and both mail queues.
    for table in [
        "documents",
        "attachments",
        "pages",
        "events",
        "document_events",
        "notice_events",
        "document_postings",
        "document_announcements",
        "attachment_extractions",
        "attachment_search_pages",
        "publication_outbox",
        "mail_queue",
        "mail_publication_items",
        "subscribers",
        "audit_log",
        "rental_assets",
        "rentals",
        "rental_blocks",
        "rental_mail_queue",
    ] {
        let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]'::jsonb) FROM ",
        );
        // Identifiers come only from the fixed table whitelist above.
        query.push(table).push(" t");
        let value: Value = query
            .build_query_scalar()
            .fetch_one(&app.state.pool)
            .await
            .unwrap();
        snapshot.insert(table.to_owned(), value);
    }
    Value::Object(snapshot)
}

#[tokio::test]
async fn every_tool_leaves_sources_ocr_publication_and_mail_unchanged() {
    let app = App::new().await;
    let now = OffsetDateTime::now_utc();
    let email = "mcp-read-regression@example.test";
    common::request_subscription(&app.state, email, now)
        .await
        .unwrap();
    subscriptions::use_token(&app.state, &app.confirmation_token(email).await, true, now)
        .await
        .unwrap();
    let document = app.notice(json!({"title":"Pronájem obecní hospody"})).await;
    let mut attachments = Vec::new();
    for name in ["cekajici.csv", "neprevedeny.csv"] {
        let uploaded = app
            .upload(
                &format!("/api/v1/admin/documents/{document}/attachments"),
                name,
                b"rok,hlasovani\n2026,3\n",
            )
            .await;
        assert_eq!(uploaded.status(), StatusCode::CREATED);
        attachments.push(body_json(uploaded).await["id"].as_i64().unwrap());
    }
    assert_eq!(app.publish(document).await.status(), StatusCode::NO_CONTENT);
    sqlx::query(
        "INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,attempts,error) SELECT id,sha256,name,$2,'failed',3,'Fixture conversion failure' FROM attachments WHERE id=$1",
    )
    .bind(attachments[1])
    .bind(EXTRACTOR_VERSION)
    .execute(&app.state.pool)
    .await
    .unwrap();

    let page = app.call("POST", "/api/v1/admin/pages", Some(json!({
        "slug":"mcp-test-page", "title":"Informace o obci", "content":"# Vyskeř\nInformace pro obyvatele.", "published":true
    })), true).await;
    assert_eq!(page.status(), StatusCode::CREATED);
    let asset = app.call("POST", "/api/v1/admin/rental-assets", Some(json!({
        "name":"Obecní sál", "description":"Sál", "rates":{"nonprofit":{"hourly_price":0,"weekend_day_price":0},"standard":{"hourly_price":100,"weekend_day_price":150}},
        "notification_email":"office@example.test", "require_meter":false, "active":true
    })), true).await;
    assert_eq!(asset.status(), StatusCode::OK);
    let asset = body_json(asset).await["id"].as_i64().unwrap();
    let before = read_side_effect_state(&app).await;
    assert!(!before["publication_outbox"].as_array().unwrap().is_empty());
    assert!(!before["document_events"].as_array().unwrap().is_empty());

    for (name, arguments) in [
        ("search", json!({"query":"pronajem"})),
        ("list_documents", json!({})),
        ("get_document", json!({"id":document})),
        ("get_page", json!({"slug":"mcp-test-page"})),
        (
            "read_attachment_text",
            json!({"attachment_id":attachments[0]}),
        ),
        ("get_content_coverage", json!({})),
        ("list_events", json!({})),
        ("list_rental_assets", json!({})),
        ("get_rental_availability", json!({"asset_id":asset})),
    ] {
        let result = success(
            rpc(
                &app,
                MODERN,
                "tools/call",
                json!({"name":name,"arguments":arguments}),
            )
            .await,
        )
        .await;
        assert_ne!(result["isError"], true, "{name}: {result}");
        assert!(result["structuredContent"].is_object());
        assert_eq!(
            read_side_effect_state(&app).await,
            before,
            "{name} changed persistent state"
        );
    }
    for _ in 0..2 {
        for (attachment, status) in [(attachments[0], "pending"), (attachments[1], "failed")] {
            let result = success(
                rpc(
                    &app,
                    MODERN,
                    "tools/call",
                    json!({"name":"read_attachment_text","arguments":{"attachment_id":attachment}}),
                )
                .await,
            )
            .await;
            assert_eq!(result["structuredContent"]["status"], status);
            assert_eq!(result["structuredContent"]["complete"], false);
        }
    }
    assert_eq!(
        read_side_effect_state(&app).await,
        before,
        "Reading pending or failed text started OCR or changed publication/mail state"
    );
}
