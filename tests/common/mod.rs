#![allow(dead_code)]
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use http_body_util::BodyExt;
use obecni_web::{
    backend::{self, Backend, auth},
    config::Config,
};
use serde_json::{Value, json};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::str::FromStr;
use tower::ServiceExt;

pub struct App {
    pub state: Backend,
    pub router: Router,
    pub cookie: String,
    pub csrf: String,
    pub admin_id: i64,
}
impl App {
    pub async fn new() -> Self {
        let config = Config {
            production: false,
            minimum_password_length: obecni_web::config::DEFAULT_MIN_PASSWORD_LENGTH,
            trusted_proxy: None,
            privacy: Some(
                serde_json::from_str(include_str!("../../config/privacy.example.json")).unwrap(),
            ),
            bind_address: "127.0.0.1:0".parse().unwrap(),
            database_url: std::env::var("TEST_DATABASE_URL")
                .expect("Run scripts/test.sh or set TEST_DATABASE_URL"),
            public_url: "http://localhost:3000".into(),
            seed_demo: false,
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1025,
            smtp_tls: "none".into(),
            smtp_username: None,
            smtp_password: None,
            mail_settings_key_file: std::env::temp_dir()
                .join(format!("obec-mail-settings-{}.key", auth::token())),
            email_from: "Vyskeř <noreply@vysker.test>".into(),
        };
        let pool = test_pool().await;
        let state = Backend::new(pool, config);
        let admin_id = auth::create_admin(
            &state,
            "admin@vysker.test",
            "Testovaci-dlouhe-heslo-123456!".into(),
        )
        .await
        .unwrap();
        let router = backend::router(state.clone());
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/login")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"email":"admin@vysker.test","password":"Testovaci-dlouhe-heslo-123456!"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let csrf = body_json(response).await["csrf_token"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            state,
            router,
            cookie,
            csrf,
            admin_id,
        }
    }
    pub async fn call(
        &self,
        method: &str,
        path: &str,
        mut value: Option<Value>,
        authenticated: bool,
    ) -> Response {
        let mut request = Request::builder().method(method).uri(path);
        if path == "/api/v1/subscriptions" {
            if let Some(value) = value.as_mut() {
                if value.get("consent").is_none() {
                    value["consent"] = serde_json::to_value(consent(&self.state)).unwrap();
                }
            }
        }
        if authenticated {
            request = request
                .header("cookie", &self.cookie)
                .header("x-csrf-token", &self.csrf);
        }
        let body = if let Some(value) = value {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
        } else {
            Body::empty()
        };
        self.router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap()
    }
    pub async fn notice(&self, extra: Value) -> i64 {
        let mut input = json!({"title":"Testovací vyhláška","description":"Informace pro občany"});
        input
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response = self
            .call("POST", "/api/v1/admin/notices", Some(input), true)
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        body_json(response).await["id"].as_i64().unwrap()
    }
    pub async fn publish(&self, id: i64) -> Response {
        self.call(
            "POST",
            &format!("/api/v1/admin/notices/{id}/publish"),
            None,
            true,
        )
        .await
    }
    pub async fn upload(&self, parent_path: &str, name: &str, data: &[u8]) -> Response {
        let mut body=format!("--vysker-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\r\n--vysker-boundary--\r\n");
        self.router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(parent_path)
                    .header("cookie", &self.cookie)
                    .header("x-csrf-token", &self.csrf)
                    .header(
                        "content-type",
                        "multipart/form-data; boundary=vysker-boundary",
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    pub async fn confirmation_token(&self, email: &str) -> String {
        let body:String=sqlx::query_scalar("SELECT p.body FROM mail_queue p JOIN subscribers o ON o.id=p.subscriber_id WHERE o.email=$1 AND p.purpose='verification' AND p.cancelled=FALSE ORDER BY p.id DESC LIMIT 1").bind(email).fetch_one(&self.state.pool).await.unwrap();
        token_from_body(&body)
    }
}
pub fn token_from_body(body: &str) -> String {
    body.split("token=")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}
pub async fn body_json(response: Response) -> Value {
    serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap()
}
pub const PDF: &[u8] = b"%PDF-1.7\nukazkovy soubor pro test\n%%EOF";

pub fn consent(state: &Backend) -> backend::subscriptions::ConsentAcceptance {
    backend::subscriptions::ConsentAcceptance {
        fingerprint: backend::privacy::notice(state).unwrap().fingerprint,
    }
}
pub async fn request_subscription(
    state: &Backend,
    email: &str,
    now: time::OffsetDateTime,
) -> backend::Result<()> {
    backend::subscriptions::request_subscription(state, email, &consent(state), now).await
}

/// Each test gets a separate schema inside the disposable database supplied by test.sh.
pub async fn test_pool() -> sqlx::PgPool {
    let url =
        std::env::var("TEST_DATABASE_URL").expect("Run scripts/test.sh or set TEST_DATABASE_URL");
    let options = PgConnectOptions::from_str(&url).expect("Valid test database URL");
    let control = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    let schema = format!("test_{}", auth::token());
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&control)
        .await
        .unwrap();
    control.close().await;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options.options([("search_path", schema.as_str()), ("timezone", "UTC")]))
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}
