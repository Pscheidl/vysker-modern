#![cfg(feature = "ssr")]
mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use common::{App, body_json};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn body_text(response: axum::response::Response) -> String {
    String::from_utf8(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn discovery_is_canonical_and_supports_conditional_cross_origin_reading() {
    let app = App::new().await;
    for (path, mime) in [
        ("/llms.txt", "text/plain; charset=utf-8"),
        ("/for-agents.md", "text/markdown; charset=utf-8"),
        ("/openapi.json", "application/json"),
        (
            "/.well-known/ai-catalog.json",
            "application/ai-catalog+json",
        ),
        ("/mcp/server-card", "application/mcp-server-card+json"),
    ] {
        let response = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("host", "attacker.invalid")
                    .header("origin", "https://reader.example")
                    .header("x-forwarded-host", "attacker.invalid")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(response.headers()["content-type"], mime, "{path}");
        assert_eq!(response.headers()["access-control-allow-origin"], "*");
        assert_eq!(response.headers()["cache-control"], "public, max-age=3600");
        assert!(!response.headers().contains_key("set-cookie"));
        let etag = response.headers()["etag"].to_str().unwrap().to_owned();
        let body = body_text(response).await;
        assert!(body.contains("http://localhost:3000"), "{path}");
        assert!(!body.contains("attacker.invalid"), "{path}");
        let conditional = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header("if-none-match", format!("\"old\", W/{etag}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conditional.status(), StatusCode::NOT_MODIFIED, "{path}");
        assert_eq!(conditional.headers()["etag"], etag);
        assert!(body_text(conditional).await.is_empty());
        let preflight = app
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri(path)
                    .header("origin", "https://reader.example")
                    .header("access-control-request-method", "GET")
                    .header("access-control-request-headers", "if-none-match")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT, "{path}");
        assert_eq!(preflight.headers()["access-control-allow-origin"], "*");
        assert!(
            preflight.headers()["access-control-allow-headers"]
                .to_str()
                .unwrap()
                .contains("If-None-Match")
        );
    }
    let card = body_json(app.call("GET", "/mcp/server-card", None, false).await).await;
    let catalog = body_json(
        app.call("GET", "/.well-known/ai-catalog.json", None, false)
            .await,
    )
    .await;
    assert_eq!(
        catalog["entries"][0]["type"],
        "application/mcp-server-card+json"
    );
    assert_eq!(
        catalog["entries"][0]["url"],
        "http://localhost:3000/mcp/server-card"
    );
    // The experimental extension's schema.ts defines protocol versions per remote.
    assert_eq!(
        card["$schema"],
        "https://static.modelcontextprotocol.io/schemas/v1/server-card.schema.json"
    );
    assert!(card.get("supportedProtocolVersions").is_none());
    assert_eq!(
        card["remotes"][0]["supportedProtocolVersions"],
        json!(obecni_web::backend::mcp::SUPPORTED_PROTOCOL_VERSIONS)
    );
    assert_eq!(card["_meta"]["info.vysker/discoveryStatus"], "experimental");
    assert_eq!(card["_meta"]["info.vysker/operatingMode"], "preview");
}

#[tokio::test]
async fn page_markdown_has_the_same_publication_boundary_as_the_page_api() {
    let app = App::new().await;
    let input = json!({"slug":"agent-test-page","title":"Veřejný zápis","content":"## Usnesení\n\nSchváleno 5 hlasy.\n\n<script>alert(1)</script>","published":false});
    let created = app
        .call("POST", "/api/v1/admin/pages", Some(input.clone()), true)
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let id = body_json(created).await["id"].as_i64().unwrap();
    let path = "/api/v1/pages/agent-test-page/markdown";
    for authenticated in [false, true] {
        assert_eq!(
            app.call("GET", path, None, authenticated).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    let mut published = input;
    published["published"] = json!(true);
    published["expected_version"] = json!(1);
    assert_eq!(
        app.call(
            "PUT",
            &format!("/api/v1/admin/pages/{id}"),
            Some(published),
            true
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let response = app.call("GET", path, None, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "text/markdown; charset=utf-8"
    );
    assert_eq!(response.headers()["content-language"], "cs");
    let cache_directives = response
        .headers()
        .get_all("cache-control")
        .iter()
        .flat_map(|value| value.to_str().unwrap().split(','))
        .map(|directive| directive.trim().to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        cache_directives,
        std::collections::BTreeSet::from(["no-store".to_owned(), "private".to_owned()])
    );
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    let rendered = body_text(response).await;
    let public_page = body_json(
        app.call("GET", "/api/v1/pages/agent-test-page", None, false)
            .await,
    )
    .await;
    assert!(rendered.starts_with("# Veřejný zápis\n"));
    assert!(rendered.contains("Source: http://localhost:3000/stranky/agent-test-page"));
    assert!(rendered.ends_with(&format!("{}\n", public_page["content"].as_str().unwrap())));
    sqlx::query("UPDATE pages SET published=FALSE WHERE id=$1")
        .bind(id)
        .execute(&app.state.pool)
        .await
        .unwrap();
    assert_eq!(
        app.call("GET", path, None, true).await.status(),
        StatusCode::NOT_FOUND
    );
}

fn check_references(value: &Value, root: &Value) {
    match value {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                assert!(
                    root.pointer(reference.strip_prefix('#').unwrap()).is_some(),
                    "{reference}"
                );
            }
            for nested in object.values() {
                check_references(nested, root)
            }
        }
        Value::Array(array) => {
            for nested in array {
                check_references(nested, root)
            }
        }
        _ => {}
    }
}

#[tokio::test]
async fn openapi_describes_only_public_reads_and_the_actual_disabled_rental_response() {
    let app = App::with_rentals(false).await;
    let spec = body_json(app.call("GET", "/openapi.json", None, false).await).await;
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["servers"][0]["url"], "http://localhost:3000");
    check_references(&spec, &spec);
    for (path, operations) in spec["paths"].as_object().unwrap() {
        assert!(
            !path.contains("admin") && !path.contains("identity"),
            "{path}"
        );
        assert_eq!(
            operations.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["get"]
        );
    }
    assert!(
        spec["paths"]["/api/v1/pages/{slug}/markdown"]["get"]["responses"]["200"]["content"]
            .get("text/markdown")
            .is_some()
    );
    let rental_path = "/api/v1/rental-assets";
    assert!(
        spec["paths"][rental_path]["get"]["responses"]
            .get("503")
            .is_some()
    );
    assert_eq!(
        app.call("GET", rental_path, None, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let search = body_json(app.call("GET", "/api/v1/search?q=", None, false).await).await;
    for field in spec["components"]["schemas"]["SearchPage"]["required"]
        .as_array()
        .unwrap()
    {
        assert!(search.get(field.as_str().unwrap()).is_some());
    }
}
