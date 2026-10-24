//! PostgreSQL connection, schema and transaction boundaries.
use crate::config::Config;
use anyhow::Context;
use sqlx::{
    PgPool, Postgres, Transaction,
    migrate::Migrator,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{str::FromStr, time::Duration};

static MIGRATIONS: Migrator = sqlx::migrate!();

pub async fn connect(config: &Config) -> anyhow::Result<PgPool> {
    let settings = &config.database_pool;
    settings.validate()?;
    // Never include the connection URL in errors, as it can contain a password.
    let options = PgConnectOptions::from_str(&config.database_url)
        .context("OBEC_DATABAZE must be a PostgreSQL connection URL")?
        .application_name("vysker-web");
    let pool = PgPoolOptions::new()
        .max_connections(settings.max_connections)
        .min_connections(settings.min_connections)
        .acquire_timeout(Duration::from_secs(settings.acquire_timeout_seconds.into()))
        .idle_timeout(optional_timeout(settings.idle_timeout_seconds))
        .max_lifetime(optional_timeout(settings.max_lifetime_seconds))
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET TIME ZONE 'UTC'")
                    .execute(&mut *conn)
                    .await?;
                sqlx::query("SET lock_timeout = '10s'")
                    .execute(&mut *conn)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await
        .context("PostgreSQL connection failed")?;
    MIGRATIONS
        .run(&pool)
        .await
        .context("PostgreSQL schema initialization failed")?;
    tracing::info!(
        max_connections = settings.max_connections,
        min_connections = settings.min_connections,
        acquire_timeout_seconds = settings.acquire_timeout_seconds,
        idle_timeout_seconds = settings.idle_timeout_seconds,
        max_lifetime_seconds = settings.max_lifetime_seconds,
        "PostgreSQL connection pool initialized"
    );
    Ok(pool)
}

fn optional_timeout(seconds: u32) -> Option<Duration> {
    (seconds > 0).then(|| Duration::from_secs(seconds.into()))
}

/// Serialize multi-step business writes across processes, without blocking readers.
/// The lock is scoped to this application's schema and released on rollback or commit.
/// Password checks and SMTP network calls run outside this transaction.
pub async fn begin_write(pool: &PgPool) -> sqlx::Result<Transaction<'_, Postgres>> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended(current_schema() || ':vysker-write', 0))",
    )
    .execute(&mut *tx)
    .await?;
    Ok(tx)
}

pub async fn seed_demo(pool: &PgPool) -> anyhow::Result<()> {
    let mut tx = begin_write(pool).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notices")
        .fetch_one(&mut *tx)
        .await?;
    if count == 0 {
        sqlx::raw_sql(include_str!("../seed-demo.sql"))
            .execute(&mut *tx)
            .await
            .context("Demo data insertion failed")?;
    }
    tx.commit().await?;
    Ok(())
}
