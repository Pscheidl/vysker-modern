//! Veřejný web obce Vyskeř v Leptosu, databáze a doménový model.
#![recursion_limit = "256"]

#[cfg(all(feature = "demo", any(feature = "ssr", feature = "hydrate")))]
compile_error!("Režim demo sestavujte samostatně s --no-default-features --features demo.");

pub mod app;
#[cfg(feature = "ssr")]
pub mod backend;
pub mod catalog;
#[cfg(feature = "ssr")]
pub mod config;
pub mod content;
#[cfg(feature = "ssr")]
pub mod db;
#[cfg(feature = "ssr")]
pub mod model;
pub mod privacy;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);
}

pub mod notice_policy;

pub mod search;
