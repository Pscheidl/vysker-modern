mod common;

use obecni_web::{config::DatabasePoolConfig, db};
use std::time::Duration;

#[tokio::test]
async fn configured_pool_enforces_capacity_and_recovers_after_timeout() {
    let app = common::App::new().await;
    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(&app.state.pool)
        .await
        .unwrap();
    let mut config = (*app.state.config).clone();
    let mut url = url::Url::parse(&config.database_url).unwrap();
    url.query_pairs_mut()
        .append_pair("options[search_path]", &schema);
    config.database_url = url.to_string();
    config.database_pool = DatabasePoolConfig {
        max_connections: 2,
        min_connections: 1,
        acquire_timeout_seconds: 1,
        idle_timeout_seconds: 7,
        max_lifetime_seconds: 13,
    };
    app.state.pool.close().await;

    // Exercise the application's connection factory against this test's isolated schema.
    let pool = db::connect(&config).await.unwrap();
    let options = pool.options();
    assert_eq!(options.get_max_connections(), 2);
    assert_eq!(options.get_min_connections(), 1);
    assert_eq!(options.get_acquire_timeout(), Duration::from_secs(1));
    assert_eq!(options.get_idle_timeout(), Some(Duration::from_secs(7)));
    assert_eq!(options.get_max_lifetime(), Some(Duration::from_secs(13)));

    let mut first = pool.acquire().await.unwrap();
    let second = pool.acquire().await.unwrap();
    assert_eq!(pool.size(), 2);
    let error = tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("acquiring above the configured capacity must time out")
        .unwrap_err();
    assert!(matches!(error, sqlx::Error::PoolTimedOut));
    assert_eq!(pool.size(), 2);

    let session: (String, String, String) = sqlx::query_as(
        "SELECT current_schema(), current_setting('TimeZone'), current_setting('lock_timeout')",
    )
    .fetch_one(&mut *first)
    .await
    .unwrap();
    assert_eq!(session, (schema, "UTC".into(), "10s".into()));
    drop(first);
    let mut recovered = pool.acquire().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&mut *recovered)
            .await
            .unwrap(),
        1
    );
    drop(recovered);
    drop(second);
    pool.close().await;

    config.database_pool.min_connections = 0;
    config.database_pool.idle_timeout_seconds = 0;
    config.database_pool.max_lifetime_seconds = 0;
    let pool = db::connect(&config).await.unwrap();
    assert_eq!(pool.options().get_min_connections(), 0);
    assert_eq!(pool.options().get_idle_timeout(), None);
    assert_eq!(pool.options().get_max_lifetime(), None);
    pool.close().await;
}
