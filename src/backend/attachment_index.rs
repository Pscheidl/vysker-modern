//! Sequential, bounded extraction of attachment text for the public search index.
//!
//! Run extraction in the separate `attachment-index` process/container, never in an HTTP request.
//! An advisory lock serializes workers across processes. Source row locks are held only
//! while claiming/publishing a result, never during document parsing or OCR.
use anyhow::{Context, bail, ensure};
use axum::{Json, extract::State};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, PgPool};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub const EXTRACTOR_VERSION: i32 = 1;
pub const MAX_INPUT_BYTES: i64 = 10 * 1024 * 1024;
const MAX_RESULT_BYTES: u64 = 12 * 1024 * 1024;
const MAX_ATTEMPTS: i32 = 3;

/// A consistent queue snapshot for administrators, including eligible draft files.
/// Source mutation triggers invalidate stale bytes, so polling need not read or hash them.
pub async fn status(
    State(state): State<super::Backend>,
    _: super::auth::Admin,
) -> super::Result<Json<serde_json::Value>> {
    let snapshot = sqlx::query_scalar(
        "WITH eligible AS MATERIALIZED (
            SELECT a.id AS attachment_id,a.document_id,a.name,e.attempts,e.started_at,e.finished_at,
                   left(e.error,1000) AS error,
                   CASE WHEN e.extractor_version=$1 AND e.source_name=a.name
                        THEN e.status ELSE 'pending' END AS state
            FROM attachments a LEFT JOIN attachment_extractions e ON e.attachment_id=a.id
            WHERE a.data IS NOT NULL AND a.removed_at IS NULL AND a.is_public
              AND lower(a.name) ~ '\\.(pdf|docx|odt|xlsx|ods|csv|txt|json|xml)$'
        )
        SELECT jsonb_build_object(
            'total',count(*),
            'ready',count(*) FILTER (WHERE state='ready'),
            'pending',count(*) FILTER (WHERE state='pending'),
            'processing',count(*) FILTER (WHERE state='processing'),
            'failed',count(*) FILTER (WHERE state='failed'),
            'retrying',count(*) FILTER (WHERE state='failed' AND attempts<$2),
            'current',(SELECT jsonb_build_object('attachment_id',attachment_id,'document_id',document_id,'name',name)
                       FROM eligible WHERE state='processing' ORDER BY started_at,attachment_id LIMIT 1),
            'failed_items',coalesce((SELECT jsonb_agg(item ORDER BY finished_at DESC NULLS LAST,attachment_id DESC)
                FROM (SELECT attachment_id,finished_at,
                      jsonb_build_object('attachment_id',attachment_id,'document_id',document_id,'name',name,
                        'error',coalesce(error,'Zpracování souboru se nezdařilo.')) AS item
                      FROM eligible WHERE state='failed'
                      ORDER BY finished_at DESC NULLS LAST,attachment_id DESC LIMIT 5) failures),'[]'::jsonb)
        ) FROM eligible",
    )
    .bind(EXTRACTOR_VERSION)
    .bind(MAX_ATTEMPTS)
    .fetch_one(&state.pool)
    .await?;
    Ok(Json(snapshot))
}

#[derive(Clone, Debug)]
pub struct WorkerOptions {
    pub extractor: PathBuf,
    pub memory_mb: u32,
    pub timeout_seconds: u32,
    pub max_pages: u32,
    pub max_output_chars: usize,
}
impl Default for WorkerOptions {
    fn default() -> Self {
        Self {
            extractor: std::env::var_os("OBEC_ATTACHMENT_EXTRACTOR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("scripts/extract_attachment.py")),
            memory_mb: 384,
            timeout_seconds: 3600,
            max_pages: 200,
            max_output_chars: 2_000_000,
        }
    }
}
impl WorkerOptions {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            (128..=4096).contains(&self.memory_mb),
            "Extraction memory must be 128..4096 MiB"
        );
        ensure!(
            (5..=3600).contains(&self.timeout_seconds),
            "Extraction deadline must be 5..3600 seconds"
        );
        ensure!(
            (1..=1000).contains(&self.max_pages),
            "Extraction page limit must be 1..1000"
        );
        ensure!(
            (1..=2_000_000).contains(&self.max_output_chars),
            "Extraction text limit must be 1..2000000 characters"
        );
        ensure!(
            self.extractor.is_file(),
            "Attachment extractor script is missing"
        );
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Busy,
    Idle,
    Indexed { attachment_id: i64, pages: usize },
    Failed { attachment_id: i64 },
    Discarded { attachment_id: i64 },
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: i64,
    name: String,
    data: Option<Vec<u8>>,
    source_sha256: String,
}
#[derive(Debug, Deserialize)]
pub struct ExtractedPage {
    pub page: i32,
    #[serde(default)]
    pub label: String,
    pub text: String,
}
#[derive(Debug, Deserialize)]
struct Extraction {
    status: String,
    #[serde(default)]
    pages: Vec<ExtractedPage>,
    #[serde(default)]
    method: String,
    #[serde(default)]
    warnings: Vec<String>,
    error: Option<String>,
}

/// Attempts one document. A second caller returns `Busy`, including another process.
pub async fn run_once(pool: &PgPool, options: &WorkerOptions) -> anyhow::Result<RunOutcome> {
    options.validate()?;
    let mut conn = pool.acquire().await?;
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended(current_database() || ':' || current_schema() || ':attachment-index',0))")
        .fetch_one(&mut *conn).await?;
    if !locked {
        return Ok(RunOutcome::Busy);
    }
    // Close rather than return this connection if the task is cancelled before unlock.
    // PoolConnection::close_on_drop prevents leaked session advisory locks.
    conn.close_on_drop();
    let outcome = process_one(&mut conn, options).await;
    let unlock = sqlx::query("SELECT pg_advisory_unlock(hashtextextended(current_database() || ':' || current_schema() || ':attachment-index',0))")
        .execute(&mut *conn).await;
    unlock?;
    outcome
}

async fn process_one(
    conn: &mut PgConnection,
    options: &WorkerOptions,
) -> anyhow::Result<RunOutcome> {
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    // Holding the global lock proves no other worker owns an old claim. Recover
    // interrupted jobs immediately, while preserving their bounded attempt count.
    sqlx::query("UPDATE attachment_extractions SET status='failed',error='Předchozí zpracování bylo přerušeno.',finished_at=now(),next_attempt_at=now() WHERE status='processing'")
        .execute(&mut *tx).await?;
    let candidate: Option<Candidate> = sqlx::query_as(
        "SELECT a.id,a.name,CASE WHEN octet_length(a.data)<=$1 THEN a.data END AS data,
         encode(sha256(a.data),'hex') AS source_sha256
         FROM attachments a LEFT JOIN attachment_extractions e ON e.attachment_id=a.id
         WHERE a.data IS NOT NULL AND a.removed_at IS NULL AND a.is_public
         AND lower(a.name) ~ '\\.(pdf|docx|odt|xlsx|ods|csv|txt|json|xml)$'
         AND (e.attachment_id IS NULL OR e.extractor_version<>$2
              OR e.source_name<>a.name
              OR (e.status='failed' AND e.attempts<$3 AND e.next_attempt_at<=now()))
         ORDER BY a.id LIMIT 1 FOR UPDATE OF a",
    )
    .bind(MAX_INPUT_BYTES)
    .bind(EXTRACTOR_VERSION)
    .bind(MAX_ATTEMPTS)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(candidate) = candidate else {
        tx.commit().await?;
        return Ok(RunOutcome::Idle);
    };
    sqlx::query("DELETE FROM attachment_search_pages WHERE attachment_id=$1")
        .bind(candidate.id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO attachment_extractions(attachment_id,source_sha256,source_name,extractor_version,status,attempts)
         VALUES($1,$2,$3,$4,'processing',1)
         ON CONFLICT(attachment_id) DO UPDATE SET source_sha256=EXCLUDED.source_sha256,source_name=EXCLUDED.source_name,
           extractor_version=EXCLUDED.extractor_version,status='processing',method='',warnings='[]',error=NULL,
           attempts=CASE WHEN attachment_extractions.source_sha256=EXCLUDED.source_sha256 AND attachment_extractions.source_name=EXCLUDED.source_name AND attachment_extractions.extractor_version=EXCLUDED.extractor_version THEN attachment_extractions.attempts+1 ELSE 1 END,
           started_at=now(),finished_at=NULL",
    ).bind(candidate.id).bind(&candidate.source_sha256).bind(&candidate.name).bind(EXTRACTOR_VERSION)
        .execute(&mut *tx).await?;
    tx.commit().await?;

    let id = candidate.id;
    let source_name = candidate.name.clone();
    let source_sha256 = candidate.source_sha256.clone();
    let options = options.clone();
    let result = tokio::task::spawn_blocking(move || {
        let bytes = candidate
            .data
            .context("Soubor překračuje limit 10 MiB pro indexaci.")?;
        // Use the bytes as the source of truth even for older imports with no stored hash.
        ensure!(
            format!("{:x}", Sha256::digest(&bytes)) == candidate.source_sha256,
            "Attachment checksum mismatch"
        );
        extract(&bytes, &candidate.name, &options)
    })
    .await
    .context("Attachment extraction worker stopped")?;

    let mut tx = conn.begin().await?;
    let unchanged: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM attachments a JOIN attachment_extractions e ON e.attachment_id=a.id
         WHERE a.id=$1 AND a.data IS NOT NULL AND a.removed_at IS NULL AND a.is_public
         AND a.name=$2 AND encode(sha256(a.data),'hex')=$3
         AND e.source_sha256=$3 AND e.source_name=$2 AND e.extractor_version=$4 AND e.status='processing'
         FOR SHARE OF a)",
    ).bind(id).bind(source_name).bind(source_sha256).bind(EXTRACTOR_VERSION)
        .fetch_one(&mut *tx).await?;
    if !unchanged {
        tx.commit().await?;
        return Ok(RunOutcome::Discarded { attachment_id: id });
    }
    let outcome = match result {
        Ok(extraction) => {
            for page in &extraction.pages {
                sqlx::query("INSERT INTO attachment_search_pages(attachment_id,page,label,content) VALUES($1,$2,$3,$4)")
                    .bind(id).bind(page.page).bind(&page.label).bind(&page.text).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE attachment_extractions SET status='ready',method=$2,warnings=$3,error=NULL,finished_at=now() WHERE attachment_id=$1")
                .bind(id).bind(extraction.method).bind(serde_json::to_value(extraction.warnings)?).execute(&mut *tx).await?;
            RunOutcome::Indexed {
                attachment_id: id,
                pages: extraction.pages.len(),
            }
        }
        Err(error) => {
            // Store only bounded diagnostic text, never the attachment contents or a URL.
            let error: String = error.to_string().chars().take(1000).collect();
            tracing::warn!(attachment_id = id, "Attachment text extraction failed");
            sqlx::query("UPDATE attachment_extractions SET status='failed',error=$2,finished_at=now(),next_attempt_at=now()+CASE attempts WHEN 1 THEN interval '5 minutes' ELSE interval '30 minutes' END WHERE attachment_id=$1")
                .bind(id).bind(error).execute(&mut *tx).await?;
            RunOutcome::Failed { attachment_id: id }
        }
    };
    tx.commit().await?;
    Ok(outcome)
}

struct WorkDir(PathBuf);
impl WorkDir {
    fn create() -> anyhow::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;
        let path = std::env::temp_dir().join(format!("obec-extract-{}", super::auth::token()));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
    fn file(&self, name: &str) -> anyhow::Result<File> {
        use std::os::unix::fs::OpenOptionsExt;
        Ok(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.0.join(name))?)
    }
}
impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn extract(data: &[u8], name: &str, options: &WorkerOptions) -> anyhow::Result<Extraction> {
    let dir = WorkDir::create()?;
    dir.file("input")?.write_all(data)?;
    let status = Command::new("timeout")
        .arg("--signal=TERM")
        .arg("--kill-after=5s")
        .arg(format!("{}s", options.timeout_seconds + 10))
        .arg("python3")
        .arg(&options.extractor)
        .arg("--input")
        .arg(dir.0.join("input"))
        .arg("--name")
        .arg(name)
        .arg("--max-memory-mb")
        .arg(options.memory_mb.to_string())
        .arg("--timeout-seconds")
        .arg(options.timeout_seconds.to_string())
        .arg("--max-pages")
        .arg(options.max_pages.to_string())
        .arg("--max-output-chars")
        .arg(options.max_output_chars.to_string())
        .env("OMP_THREAD_LIMIT", "1")
        .env("OMP_NUM_THREADS", "1")
        .env("OPENBLAS_NUM_THREADS", "1")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(dir.file("result.json")?)
        .stderr(dir.file("stderr")?)
        .status()
        .context("Could not start attachment extractor")?;
    let output = read_bounded(&dir.0.join("result.json"), MAX_RESULT_BYTES)?;
    let extraction: Extraction = serde_json::from_slice(&output)
        .context("Extractor returned an invalid result or exceeded its limits")?;
    if !status.success() || extraction.status != "ready" {
        bail!(
            "{}",
            extraction
                .error
                .unwrap_or_else(|| "Extraktor nedokončil zpracování dokumentu.".into())
        );
    }
    ensure!(
        extraction.pages.len() <= options.max_pages as usize,
        "Extractor returned too many pages"
    );
    ensure!(
        extraction
            .pages
            .iter()
            .any(|page| !page.text.trim().is_empty()),
        "Extractor returned no readable text"
    );
    let mut numbers = HashSet::new();
    let mut chars = 0usize;
    for page in &extraction.pages {
        ensure!(
            page.page > 0 && numbers.insert(page.page),
            "Extractor returned duplicate or invalid page numbers"
        );
        ensure!(
            !page.text.contains('\0') && !page.label.contains('\0'),
            "Document contains invalid text characters"
        );
        ensure!(
            page.label.chars().count() <= 300,
            "Document section label is too long"
        );
        chars = chars.saturating_add(page.text.chars().count());
        ensure!(
            chars <= options.max_output_chars,
            "Extracted text is too large"
        );
    }
    ensure!(
        extraction.warnings.len() <= 200
            && extraction
                .warnings
                .iter()
                .all(|w| w.chars().count() <= 1000),
        "Too many extraction warnings"
    );
    ensure!(extraction.method.len() <= 30, "Invalid extraction method");
    Ok(extraction)
}
fn read_bounded(path: &Path, limit: u64) -> anyhow::Result<Vec<u8>> {
    let mut data = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 <= limit,
        "Extractor output exceeded limit"
    );
    Ok(data)
}
