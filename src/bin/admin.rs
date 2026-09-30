//! Vytvoření správce bez výchozího nebo veřejně známého hesla.
use obecni_web::{
    backend::{Backend, accounts, auth},
    config::Config,
    db,
};
use std::io::Read;
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next().ok_or_else(|| {
        anyhow::anyhow!("Usage: obec-admin [create | reset-password] EMAIL [--password-stdin]")
    })?;
    let reset = first == "reset-password";
    let email = if reset || first == "create" {
        args.next()
            .ok_or_else(|| anyhow::anyhow!("Email is required"))?
    } else {
        first
    };
    let mode = args.next();
    anyhow::ensure!(
        args.next().is_none() && matches!(mode.as_deref(), None | Some("--password-stdin")),
        "Neplatné argumenty"
    );
    let config = Config::from_env()?;
    let password = if mode.is_some() {
        let mut input = String::new();
        std::io::stdin().take(1026).read_to_string(&mut input)?;
        input.trim_end_matches(['\r', '\n']).to_owned()
    } else {
        rpassword::prompt_password(format!(
            "Heslo správce (alespoň {} znaků): ",
            config.minimum_password_length
        ))?
    };
    let pool = db::connect(&config).await?;
    let state = Backend::new(pool, config);
    let result = if reset {
        accounts::reset_password(&state, &email, password).await
    } else {
        auth::create_admin(&state, &email, password).await
    };
    let id = result.map_err(|e| anyhow::anyhow!(e.1))?;
    println!(
        "Administrator {id}: {}",
        if reset {
            "password reset, all sessions revoked"
        } else {
            "created"
        }
    );
    state.pool.close().await;
    Ok(())
}
