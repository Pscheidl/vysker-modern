//! Veřejný web obce Vyskeř, Leptos SSR a Axum.
#![recursion_limit = "256"]

use leptos::prelude::*;
use leptos_axum::{LeptosRoutes, generate_route_list};
use obecni_web::{
    app::{App, shell},
    backend::{self, server},
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    server::logging();
    let state = server::initialize().await?;
    let pool = state.pool.clone();

    let conf = get_configuration(Some("Cargo.toml"))?;
    let options = conf.leptos_options;
    let routes = generate_route_list(App);
    let context_pool = pool.clone();
    let fallback_pool = pool.clone();
    let context_backend = state.clone();
    let fallback_backend = state.clone();
    let app = axum::Router::new()
        .leptos_routes_with_context(
            &options,
            routes,
            move || {
                provide_context(context_pool.clone());
                provide_context(context_backend.clone());
            },
            {
                let options = options.clone();
                move || shell(options.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler_with_context(
            move || {
                provide_context(fallback_pool.clone());
                provide_context(fallback_backend.clone());
            },
            shell,
        ))
        .with_state(options)
        .merge(backend::router(state.clone()))
        .layer(axum::middleware::from_fn(backend::auth::admin_page_headers))
        .layer(axum::middleware::from_fn_with_state(
            state.config.clone(),
            backend::privacy::response_headers,
        ));
    server::serve(state, app).await
}
