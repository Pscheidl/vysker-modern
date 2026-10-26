#![cfg(feature = "ssr")]

use obecni_web::backend::auth;
use sqlx::{PgConnection, postgres::PgPoolOptions};

async fn snapshot(conn: &mut PgConnection, table: &'static str, added_column: &str) -> Vec<String> {
    // Table names come only from the fixed table list below, never from input.
    sqlx::QueryBuilder::<sqlx::Postgres>::new("SELECT (to_jsonb(t)-")
        .push_bind(added_column)
        .push("::text)::text FROM ")
        .push(table)
        .push(" t ORDER BY 1")
        .build_query_scalar()
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn hourly_digest_upgrade_preserves_subscriptions_and_existing_delivery_queues() {
    let url =
        std::env::var("TEST_DATABASE_URL").expect("Run scripts/test.sh or set TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    // Keep the historical schema and upgrade inside one transaction so the
    // isolated schema is removed on rollback, including after a failed assertion.
    let mut tx = pool.begin().await.unwrap();
    let schema = format!("digest_upgrade_{}", &auth::token()[..32]);
    sqlx::QueryBuilder::<sqlx::Postgres>::new("CREATE SCHEMA ")
        .push(&schema)
        .build()
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SELECT set_config('search_path',$1,true)")
        .bind(&schema)
        .execute(&mut *tx)
        .await
        .unwrap();

    let migrations = sqlx::migrate!();
    for migration in migrations
        .iter()
        .filter(|migration| migration.version <= 20)
    {
        sqlx::raw_sql(migration.sql.clone())
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    // These rows are inserted before migration 21 exists. They intentionally
    // include customized preferences and an SMTP lease already in progress.
    sqlx::raw_sql(
        "INSERT INTO consent_notices(fingerprint,version,consent_text,privacy_json)
            VALUES ('legacy-consent','2026-10','Original confirmed wording','{}');
         INSERT INTO subscribers(id,email,verified_at,retention_started_at,
             all_notice_categories,notice_category_ids,uncategorized_notices,documents)
            VALUES (41,'existing@example.test',1100,1000,FALSE,ARRAY[1,3],FALSE,TRUE);
         INSERT INTO subscription_consents(id,subscriber_id,notice_fingerprint,requested_at,
             confirmed_at,all_notice_categories,notice_category_ids,uncategorized_notices,documents)
            VALUES (51,41,'legacy-consent',1000,1100,TRUE,ARRAY[]::BIGINT[],TRUE,TRUE);
         INSERT INTO notices(id,title,published_on,status,published_at)
            VALUES (101,'Existing notice','2026-10-26','published','2026-10-26T09:00:00Z');
         INSERT INTO documents(id,title,status,created_at,published_at)
            VALUES (201,'Sent document','published','2026-10-26T08:00:00Z','2026-10-26T09:00:00Z'),
                   (202,'Waiting publication','published','2026-10-26T08:00:00Z','2026-10-26T09:10:00Z');
         INSERT INTO mail_queue(id,subscriber_id,consent_id,purpose,deduplication_key,subject,body,
             attempts,next_attempt_at,locked_until,lock_token,sent_at,cancelled,created_at)
            VALUES (81,41,51,'document','notice:101:41','Pending notice','Original body and link',
                    3,1700,2000,'original-smtp-lease',NULL,FALSE,1200),
                   (82,41,51,'document','document:201:41','Sent document',NULL,
                    2,1300,0,NULL,1400,FALSE,1201),
                   (83,41,51,'document','document:202:41','Cancelled document',NULL,
                    1,1300,0,NULL,NULL,TRUE,1202);
         INSERT INTO publication_outbox(id,document_id,subscriber_id,consent_id,created_at)
            VALUES (91,202,41,51,1500);
         INSERT INTO administrators(id,email,password_hash)
            VALUES (61,'admin@example.test','unchanged-administrator-hash');
         INSERT INTO password_resets(hash,administrator_id,expires_at)
            VALUES ('existing-reset-hash',61,3000);
         INSERT INTO recovery_mail(id,administrator_id,token_hash,body,created_at,next_attempt_at,
             attempts,lock_token,locked_until)
            VALUES (301,61,'existing-reset-hash','Original password recovery link',1300,1600,
                    2,'original-recovery-lease',1900);
         INSERT INTO issue_reports(id,description,address,request_ip,created_at)
            VALUES (71,'Existing defect','Vyskeř 1','192.0.2.1','2026-10-26T09:20:00Z');
         INSERT INTO report_mail_queue(id,report_id,recipient,subject,body,attempts,
             next_attempt_at,locked_until,lock_token,created_at)
            VALUES (302,71,'admin@example.test','Existing report','Original report notification',
                    4,1800,2100,'original-report-lease',1300);",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO subscription_tokens(hash,subscriber_id,purpose,expires_at,consent_id)
         VALUES ($1,41,'unsubscribe',$2,51)",
    )
    .bind(auth::hash("existing-unsubscribe-secret"))
    .bind(i64::MAX)
    .execute(&mut *tx)
    .await
    .unwrap();

    let tables = [
        ("subscribers", "next_digest_at"),
        ("consent_notices", ""),
        ("subscription_consents", ""),
        ("subscription_tokens", ""),
        ("mail_queue", "digest_cutoff"),
        ("publication_outbox", ""),
        ("notices", ""),
        ("documents", ""),
        ("administrators", ""),
        ("password_resets", ""),
        ("recovery_mail", ""),
        ("issue_reports", ""),
        ("report_mail_queue", ""),
    ];
    let mut before = Vec::new();
    for (table, added_column) in tables {
        before.push(snapshot(&mut tx, table, added_column).await);
    }

    let upgrade = migrations
        .iter()
        .find(|migration| migration.version == 21)
        .expect("The hourly digest migration must be present");
    sqlx::raw_sql(upgrade.sql.clone())
        .execute(&mut *tx)
        .await
        .unwrap();

    for ((table, added_column), expected) in tables.into_iter().zip(before) {
        assert_eq!(
            snapshot(&mut tx, table, added_column).await,
            expected,
            "Migration changed existing {table} data"
        );
    }
    let next_digest: i64 = sqlx::query_scalar("SELECT next_digest_at FROM subscribers WHERE id=41")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(next_digest, 0);
    let legacy_cutoffs: Vec<Option<i64>> =
        sqlx::query_scalar("SELECT digest_cutoff FROM mail_queue ORDER BY id")
            .fetch_all(&mut *tx)
            .await
            .unwrap();
    assert_eq!(legacy_cutoffs, vec![None, None, None]);
    let digest_members: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_publication_items")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(digest_members, 0);
    tx.rollback().await.unwrap();
    pool.close().await;
}
