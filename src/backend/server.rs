use super::{Backend, cez, mail, notices, power_outages, privacy};
use std::net::SocketAddr;
use time::OffsetDateTime;
use tokio::sync::watch;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn logging() {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "obecni_web=info,tower_http=info,sqlx=warn".into()),
        )
        .with(tracing_subscriber::fmt::layer().with_target(false))
        .init();
}
pub async fn initialize() -> anyhow::Result<Backend> {
    let config = crate::config::Config::from_env()?;
    let pool = crate::db::connect(&config).await?;
    if config.seed_demo {
        crate::db::seed_demo(&pool).await?;
    }
    let state = Backend::new(pool, config);
    let policy = privacy::policy(&state)
        .await
        .map_err(|error| anyhow::anyhow!(error.1))?;
    if state.config.production {
        let policy = policy.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "Produkce vyžaduje informace o soukromí v administraci nebo OBEC_PRIVACY_CONFIG"
            )
        })?;
        privacy::validate_policy(policy, true)?;
    }
    check_launch_content(&state).await?;
    state
        .config
        .email_from
        .parse::<lettre::message::Mailbox>()?;
    notices::maintenance(&state, OffsetDateTime::now_utc())
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    privacy::maintenance(&state, OffsetDateTime::now_utc())
        .await
        .map_err(|e| anyhow::anyhow!(e.1))?;
    state.last_maintenance.store(
        OffsetDateTime::now_utc().unix_timestamp(),
        std::sync::atomic::Ordering::Relaxed,
    );
    Ok(state)
}
pub async fn serve(state: Backend, app: axum::Router) -> anyhow::Result<()> {
    let mut sender = mail::Worker::new(&state)?;
    let outage_client = if state.config.cez_outages.enabled {
        Some(cez::Client::new(&state.config.public_url)?)
    } else {
        None
    };
    let listener = tokio::net::TcpListener::bind(state.config.bind_address).await?;
    let (stop, mut stopped) = watch::channel(false);
    let mut mail_stopped = stopped.clone();
    let mut outage_stopped = stopped.clone();
    let worker_state = state.clone();
    let worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=stopped.changed()=>break,
                _=interval.tick()=>{
                    let mut maintenance_ok=true;
                    if let Err(error)=notices::maintenance(&worker_state,OffsetDateTime::now_utc()).await { maintenance_ok=false; tracing::error!(?error,"údržba dokumentů selhala"); }
                    if let Err(error)=privacy::maintenance(&worker_state,OffsetDateTime::now_utc()).await { maintenance_ok=false; tracing::error!(?error,"úklid osobních údajů selhal"); }
                    if maintenance_ok {worker_state.last_maintenance.store(OffsetDateTime::now_utc().unix_timestamp(),std::sync::atomic::Ordering::Relaxed);}

                }
            }
        }
    });
    // Slow SMTP must not delay scheduled publication, retention or readiness.
    let mail_state = state.clone();
    let mail_worker = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=mail_stopped.changed()=>break,
                _=interval.tick()=>{
                    for _ in 0..20 {
                        if *mail_stopped.borrow() { break; }
                        match sender.deliver_one(&mail_state,OffsetDateTime::now_utc()).await {
                            Ok(true)=>{}, Ok(false)=>break,
                            Err(error)=>{tracing::error!(%error,"zpracování fronty selhalo");break;}
                        }
                    }
                }
            }
        }
    });
    // External service calls must not delay document maintenance or mail delivery.
    let outage_state = state.clone();
    let outage_worker = tokio::spawn(async move {
        let Some(client) = outage_client else { return };
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = outage_stopped.changed() => break,
                _ = interval.tick() => {
                    tokio::select! {
                        _ = outage_stopped.changed() => break,
                        result = power_outages::sync_if_due(&outage_state, &client, OffsetDateTime::now_utc()) => {
                            match result {
                                Ok(Some(report)) => tracing::info!(?report, "kontrola odstávek ČEZ dokončena"),
                                Ok(None) => {},
                                Err(error) => tracing::warn!(%error, "kontrola odstávek ČEZ selhala, kalendář zůstal beze změny"),
                            }
                        }
                    }
                }
            }
        }
    });
    tracing::info!("server běží na http://{}", listener.local_addr()?);
    let result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown().await;
        let _ = stop.send(true);
    })
    .await;
    worker.await?;
    mail_worker.await?;
    outage_worker.await?;
    state.pool.close().await;
    result?;
    Ok(())
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! { _=tokio::signal::ctrl_c()=>{}, _=terminate.recv()=>{} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("dokončuji rozdělané požadavky");
}

pub const REQUIRED_PAGES: &[&str] = &[
    "kontakt",
    "obec",
    "kalendar",
    "pristupnost",
    "povinne-informace",
];
pub async fn check_launch_content(state: &Backend) -> anyhow::Result<()> {
    if !state.config.production {
        return Ok(());
    }
    let preview_notices: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM legacy_notice_imports WHERE preview_published=TRUE)",
    )
    .fetch_one(&state.pool)
    .await?;
    anyhow::ensure!(
        !preview_notices,
        "Databáze obsahuje místní náhled převzaté úřední desky. Pro produkci proveďte samostatný import a kontrolu vyvěšení."
    );
    for slug in REQUIRED_PAGES {
        let content: Option<String> =
            sqlx::query_scalar("SELECT content FROM pages WHERE slug=$1 AND published=TRUE")
                .bind(slug)
                .fetch_optional(&state.pool)
                .await?;
        anyhow::ensure!(
            content.is_some_and(|s| !s.trim().is_empty() && !s.contains("DOPLNIT")),
            "Před spuštěním zveřejněte schválenou obsahovou stránku: {slug}"
        );
    }
    Ok(())
}
