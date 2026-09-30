#![cfg(feature = "ssr")]
mod common;

#[tokio::test]
async fn postgres_schema_is_repeatable_and_preserves_demo_records() {
    let pool = common::test_pool().await;
    sqlx::migrate!().run(&pool).await.unwrap();
    obecni_web::db::seed_demo(&pool).await.unwrap();
    obecni_web::db::seed_demo(&pool).await.unwrap();
    let notices: i64 = sqlx::query_scalar("SELECT count(*) FROM notices")
        .fetch_one(&pool)
        .await
        .unwrap();
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((notices, files), (6, 8));
    assert!(
        sqlx::query("DELETE FROM notices")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("TRUNCATE notices CASCADE")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM attachments")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("INSERT INTO attachments(notice_id,name,content_type,size_bytes) VALUES (999999,'missing','application/pdf',0)").execute(&pool).await.is_err());
    let columns: Vec<(String, String)> = sqlx::query_as("SELECT column_name,data_type FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='notices'").fetch_all(&pool).await.unwrap();
    assert!(columns.contains(&("published_on".into(), "date".into())));
    assert!(columns.contains(&("retain_attachments".into(), "boolean".into())));
}
