#![cfg(feature = "ssr")]
mod common;

use obecni_web::backend::attachment_index::{
    EXTRACTOR_VERSION, RunOutcome, WorkerOptions, run_once,
};
use sqlx::PgPool;
use std::time::Duration;

async fn attachment(pool: &PgPool, name: &str, data: &[u8]) -> i64 {
    let document: i64 = sqlx::query_scalar("INSERT INTO documents(title,created_at) VALUES('Zápis pro indexaci','2026-11-01') RETURNING id")
        .fetch_one(pool).await.unwrap();
    sqlx::query_scalar("INSERT INTO attachments(document_id,name,content_type,size_bytes,data) VALUES($1,$2,'text/plain',$3,$4) RETURNING id")
        .bind(document).bind(name).bind(data.len() as i64).bind(data).fetch_one(pool).await.unwrap()
}

fn extractor(script: &str) -> (tempfile::TempDir, WorkerOptions) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("extract.py");
    std::fs::write(&path, script).unwrap();
    let options = WorkerOptions {
        extractor: path,
        timeout_seconds: 5,
        ..Default::default()
    };
    (dir, options)
}

const TEXT_EXTRACTOR: &str = r#"
import json, pathlib, sys
path = sys.argv[sys.argv.index('--input') + 1]
text = pathlib.Path(path).read_text()
print(json.dumps({'status':'ready','method':'text','warnings':[], 'pages':[
    {'page':1,'label':'Dokument','text':text}, {'page':2,'label':'Příloha','text':'Žluťoučký kůň'}]}))
"#;
const SLOW_EXTRACTOR: &str = r#"
import json, pathlib, sys, time
path = sys.argv[sys.argv.index('--input') + 1]
text = pathlib.Path(path).read_text()
time.sleep(0.7)
print(json.dumps({'status':'ready','method':'text','pages':[{'page':1,'text':text}]}))
"#;

async fn processing(pool: &PgPool, id: i64) {
    for _ in 0..150 {
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM attachment_extractions WHERE attachment_id=$1")
                .bind(id)
                .fetch_optional(pool)
                .await
                .unwrap();
        if status.as_deref() == Some("processing") {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("Worker never claimed attachment {id}");
}

#[tokio::test]
async fn existing_and_changed_attachments_are_indexed_once_with_normalized_pages() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "zapis.txt", "Pronájem hospody".as_bytes()).await;
    let (_dir, options) = extractor(TEXT_EXTRACTOR);
    assert_eq!(
        run_once(&pool, &options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: id,
            pages: 2
        }
    );
    let text: String = sqlx::query_scalar(
        "SELECT search_text FROM attachment_search_pages WHERE attachment_id=$1 AND page=2",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(text, "zlutoucky kun");
    assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Idle);
    sqlx::query("UPDATE attachments SET data=$2 WHERE id=$1")
        .bind(id)
        .bind("Jiný obsah".as_bytes())
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM attachment_search_pages WHERE attachment_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "Source mutation invalidates old text in the same transaction"
    );
    assert_eq!(
        run_once(&pool, &options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: id,
            pages: 2
        }
    );
    let text: String = sqlx::query_scalar(
        "SELECT content FROM attachment_search_pages WHERE attachment_id=$1 AND page=1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(text, "Jiný obsah");
}

#[tokio::test]
async fn originals_removed_or_made_private_erase_derived_text_immediately() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "zapis.txt", b"Public text").await;
    let (_dir, options) = extractor(TEXT_EXTRACTOR);
    run_once(&pool, &options).await.unwrap();
    sqlx::query("UPDATE attachments SET is_public=FALSE WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Idle);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attachment_search_pages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE attachments SET is_public=TRUE WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    run_once(&pool, &options).await.unwrap();
    sqlx::query("UPDATE attachments SET data=NULL,removed_at='2026-11-01T12:00:00Z' WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attachment_extractions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Idle);
}

#[tokio::test]
async fn failed_extractions_back_off_and_stop_after_three_attempts() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "broken.pdf", b"bad PDF").await;
    let (_dir, options) = extractor(
        "import json,sys\nprint(json.dumps({'status':'failed','error':'Cannot read document'}))\nsys.exit(1)\n",
    );
    for attempt in 1..=3 {
        assert_eq!(
            run_once(&pool, &options).await.unwrap(),
            RunOutcome::Failed { attachment_id: id }
        );
        let record: (i32, bool) = sqlx::query_as("SELECT attempts,next_attempt_at>now() FROM attachment_extractions WHERE attachment_id=$1")
            .bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(record, (attempt, true));
        assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Idle);
        sqlx::query("UPDATE attachment_extractions SET next_attempt_at=now()-interval '1 second' WHERE attachment_id=$1")
            .bind(id).execute(&pool).await.unwrap();
    }
    assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Idle);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attachment_search_pages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn competing_workers_cannot_extract_two_documents_at_once() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "one.txt", b"one").await;
    attachment(&pool, "two.txt", b"two").await;
    let (_dir, options) = extractor(SLOW_EXTRACTOR);
    let task_pool = pool.clone();
    let task_options = options.clone();
    let task = tokio::spawn(async move { run_once(&task_pool, &task_options).await.unwrap() });
    processing(&pool, id).await;
    assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Busy);
    assert_eq!(
        task.await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: id,
            pages: 1
        }
    );
}

#[tokio::test]
async fn changed_source_during_extraction_never_publishes_stale_text() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "zapis.txt", b"obsolete content").await;
    let (_dir, options) = extractor(SLOW_EXTRACTOR);
    let task_pool = pool.clone();
    let task_options = options.clone();
    let task = tokio::spawn(async move { run_once(&task_pool, &task_options).await.unwrap() });
    processing(&pool, id).await;
    sqlx::query("UPDATE attachments SET data=$2 WHERE id=$1")
        .bind(id)
        .bind(b"new content".as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        task.await.unwrap(),
        RunOutcome::Discarded { attachment_id: id }
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attachment_search_pages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        run_once(&pool, &options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: id,
            pages: 1
        }
    );
}

#[tokio::test]
async fn old_extractor_version_is_rebuilt_and_abandoned_claim_is_recovered() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "zapis.txt", b"content").await;
    let (_dir, options) = extractor(TEXT_EXTRACTOR);
    run_once(&pool, &options).await.unwrap();
    sqlx::query("UPDATE attachment_extractions SET extractor_version=$2 WHERE attachment_id=$1")
        .bind(id)
        .bind(EXTRACTOR_VERSION - 1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        run_once(&pool, &options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: id,
            pages: 2
        }
    );
    sqlx::query("UPDATE attachment_extractions SET status='processing',started_at=now()-interval '2 hours' WHERE attachment_id=$1")
        .bind(id).execute(&pool).await.unwrap();
    assert_eq!(
        run_once(&pool, &options).await.unwrap(),
        RunOutcome::Indexed {
            attachment_id: id,
            pages: 2
        }
    );
    sqlx::query("UPDATE attachment_extractions SET status='processing',attempts=3,started_at=now() WHERE attachment_id=$1")
        .bind(id).execute(&pool).await.unwrap();
    assert_eq!(run_once(&pool, &options).await.unwrap(), RunOutcome::Idle);
    let status: String =
        sqlx::query_scalar("SELECT status FROM attachment_extractions WHERE attachment_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "failed",
        "Interrupted jobs also obey the attempt limit"
    );
}

#[tokio::test]
async fn invalid_extractor_results_are_not_published_as_partial_success() {
    let pool = common::test_pool().await;
    let id = attachment(&pool, "zapis.txt", b"content").await;
    let (_dir, options) = extractor(
        "import json\nprint(json.dumps({'status':'ready','method':'text','pages':[{'page':1,'text':'ok'},{'page':1,'text':'duplicate'}]}))\n",
    );
    assert_eq!(
        run_once(&pool, &options).await.unwrap(),
        RunOutcome::Failed { attachment_id: id }
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM attachment_search_pages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
