#![cfg(feature = "ssr")]
mod common;

#[tokio::test]
async fn postgres_schema_is_repeatable_and_preserves_demo_records() {
    let pool = common::test_pool().await;
    sqlx::migrate!().run(&pool).await.unwrap();
    let feature_versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations WHERE version>=30 ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(feature_versions, vec![30]);
    let rental_defaults: (bool, i64) =
        sqlx::query_as("SELECT enabled,version FROM rental_settings WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rental_defaults, (false, 1));
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

// Exercise the complete feature migration against the actual pre-feature
// schema. The transaction also removes the temporary schema on rollback.
async fn before_rentals(pool: &sqlx::PgPool) -> sqlx::Transaction<'_, sqlx::Postgres> {
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("rental_upgrade_{}", obecni_web::backend::auth::token());
    // The identifier is generated locally, never supplied by a test caller.
    assert!(
        schema
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    );
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path',$1,TRUE)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();
    let migrations = sqlx::migrate!();
    for migration in migrations.iter().filter(|migration| migration.version < 30) {
        sqlx::raw_sql(migration.sql.clone())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx
}

#[tokio::test]
async fn rental_migration_preserves_existing_accounts_sessions_and_custom_navigation() {
    let pool = common::test_pool().await;
    let mut tx = before_rentals(&pool).await;
    sqlx::query("INSERT INTO administrators(id,email,password_hash) VALUES (1,'first@example.test','existing-first-hash'),(2,'second@example.test','existing-second-hash')")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO sessions(token_hash,csrf_hash,administrator_id,expires_at) VALUES ('existing-session','existing-csrf',1,9999999999)")
        .execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO navigation_items(label,path,sort_order,visible,version) VALUES ('Pronájem sálu','/pronajmy',60,FALSE,9),('Rezervace','/kontakt',70,TRUE,4)")
        .execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0030_rental_system.sql"))
        .execute(&mut *tx)
        .await
        .unwrap();
    let rights: Vec<(i64, bool, bool, bool, bool, bool)> = sqlx::query_as("SELECT id,is_primary,can_view_identity,can_manage_permissions,can_create_accounts,can_manage_mail_settings FROM administrators ORDER BY id")
        .fetch_all(&mut *tx).await.unwrap();
    assert_eq!(
        rights,
        vec![
            (1, true, true, true, true, true),
            (2, false, false, false, false, false),
        ]
    );
    let credentials: Vec<(String, String)> =
        sqlx::query_as("SELECT email,password_hash FROM administrators ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        credentials,
        vec![
            ("first@example.test".into(), "existing-first-hash".into()),
            ("second@example.test".into(), "existing-second-hash".into()),
        ]
    );
    let sessions: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT token_hash,csrf_hash,administrator_id FROM sessions")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(
        sessions,
        vec![("existing-session".into(), "existing-csrf".into(), 1)]
    );
    let links: Vec<(String, bool, i64)> =
        sqlx::query_as("SELECT label,visible,version FROM navigation_items WHERE path='/pronajmy'")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(links, vec![("Pronájem sálu".into(), false, 9)]);
    let other: (bool, i64) = sqlx::query_as(
        "SELECT visible,version FROM navigation_items WHERE path='/kontakt' AND label='Rezervace'",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(other, (true, 4));
    let enabled: bool = sqlx::query_scalar("SELECT enabled FROM rental_settings WHERE id=1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(!enabled);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn rental_migration_rejects_an_inactive_original_administrator() {
    let pool = common::test_pool().await;
    let mut tx = before_rentals(&pool).await;
    sqlx::query("INSERT INTO administrators(id,email,password_hash,active) VALUES (1,'first@example.test','existing-first-hash',FALSE),(2,'second@example.test','existing-second-hash',TRUE)")
        .execute(&mut *tx).await.unwrap();
    let error = sqlx::raw_sql(include_str!("../migrations/0030_rental_system.sql"))
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("První správce je neaktivní"));
    tx.rollback().await.unwrap();
}
