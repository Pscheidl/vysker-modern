//! Standalone attachment indexing service. It does not start web, mail or import jobs.
use anyhow::{Context, bail};
use obecni_web::backend::attachment_index::{RunOutcome, WorkerOptions, run_once};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::{str::FromStr, time::Duration};

#[tokio::main(flavor = "multi_thread", worker_threads = 1)]
async fn main() -> anyhow::Result<()> {
    let mut options = WorkerOptions::default();
    let mut once = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--once" => once = true,
            "--loop" => once = false,
            "--extractor" => {
                options.extractor = args.next().context("Missing extractor path")?.into()
            }
            "--memory-mb" => {
                options.memory_mb = args.next().context("Missing memory limit")?.parse()?
            }
            "--timeout-seconds" => {
                options.timeout_seconds = args.next().context("Missing deadline")?.parse()?
            }
            "--max-pages" => {
                options.max_pages = args.next().context("Missing page limit")?.parse()?
            }
            "--help" => {
                println!(
                    "attachment-index [--once|--loop] [--extractor PATH] [--memory-mb 384] [--timeout-seconds 3600] [--max-pages 200]\n--once drains currently eligible work and exits. Otherwise polls every 15 seconds.\nDatabase: OBEC_DATABAZE or OBEC_DATABAZE_FILE. Run in a container with a 512 MiB memory limit."
                );
                return Ok(());
            }
            _ => bail!("Unknown argument: {arg}"),
        }
    }
    options.validate()?;
    let database_url = match (
        std::env::var("OBEC_DATABAZE").ok(),
        std::env::var("OBEC_DATABAZE_FILE").ok(),
    ) {
        (Some(url), None) => url,
        (None, Some(path)) => std::fs::read_to_string(path)?.trim().to_owned(),
        (Some(_), Some(_)) => bail!("Set only one of OBEC_DATABAZE and OBEC_DATABAZE_FILE"),
        (None, None) => bail!("Set OBEC_DATABAZE or OBEC_DATABAZE_FILE"),
    };
    let connect = PgConnectOptions::from_str(&database_url)
        .context("Invalid PostgreSQL connection configuration")?
        .application_name("vysker-attachment-index");
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(15))
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
        .connect_with(connect)
        .await
        .context("Attachment index database connection failed")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("Attachment index schema initialization failed")?;
    loop {
        let result = run_once(&pool, &options).await?;
        match result {
            RunOutcome::Idle | RunOutcome::Busy if once => break,
            RunOutcome::Idle | RunOutcome::Busy => {
                tokio::time::sleep(Duration::from_secs(15)).await
            }
            _ => println!("{result:?}"),
        }
    }
    pool.close().await;
    Ok(())
}
