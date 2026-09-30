//! Samostatné API pro Compose a lokální backendový vývoj.
use obecni_web::backend::{router, server};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::logging();
    let state = server::initialize().await?;
    server::serve(state.clone(), router(state)).await
}
