mod admin;
mod catalog;
mod components;
mod events;
mod pages;
mod reports;
mod search;

use crate::content::{Board, load_board};
use components::{Footer, Header, SkipLink};
use leptos::prelude::*;
#[cfg(not(feature = "demo"))]
use leptos_meta::{HashedStylesheet, MetaTags};
use leptos_meta::{Meta, Title, provide_meta_context};
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};
use pages::*;

pub type SiteResource = Resource<Result<crate::catalog::SiteInfo, ServerFnError>>;
pub type BoardResource = Resource<Result<Board, ServerFnError>>;

/// Trunk vloží základní cestu hostingu do <base>, včetně názvu repozitáře.
pub fn base_path() -> String {
    #[cfg(feature = "demo")]
    return document()
        .query_selector("base")
        .ok()
        .flatten()
        .and_then(|element| element.get_attribute("href"))
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_owned();

    #[cfg(not(feature = "demo"))]
    String::new()
}

pub fn site_url(path: &str) -> String {
    format!("{}{path}", base_path())
}

#[cfg(not(feature = "demo"))]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="cs">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="color-scheme" content="light dark"/>
                <link rel="icon" type="image/png" sizes="32x32" href="/images/vysker-znak-mat-favicon-32.png"/>
                <link rel="icon" type="image/png" sizes="128x128" href="/images/vysker-znak-mat-favicon-128.png"/>
                <link rel="apple-touch-icon" href="/images/vysker-znak-mat-apple-touch-icon.png"/>
                <link rel="manifest" href="/manifest.webmanifest"/>
                <meta name="theme-color" content="#fdfdfd"/>
                <script src="/install.js" defer></script>
                <link rel="preload" href="/fonts/geist.woff2" r#as="font" type="font/woff2" crossorigin="anonymous"/>
                <script src="/theme.js"></script>
                <script src="/relief-loader.js" defer></script>
                <script src="/chapel-seasons.js" defer></script>
                <AutoReload options=options.clone()/>
                <HashedStylesheet id="leptos" options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body><App/></body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let board = Resource::new(|| (), |_| load_board());
    provide_context(board);
    // Navigation is part of the initial HTML, including when JavaScript is disabled.
    provide_context(Resource::new_blocking(
        || (),
        |_| crate::catalog::load_site_info(),
    ));
    // InOrder keeps configured navigation available without JavaScript.
    // leptos_axum 0.8.10 ignores replace_blocks for PartiallyBlocked rendering.
    view! {
        <Title text="Vyskeř · Obec v Českém ráji"/>
        <Meta name="description" content="Úřední deska, dokumenty a informace obce Vyskeř. Vše důležité na jednom místě."/>
        <SiteRobots/>
        <Router base=base_path()>
            <SkipLink/>
            <PublicHeader/>
            <main id="obsah" tabindex="-1">
                <Routes fallback=NotFound>
                    <Route path=path!("admin") view=admin::Administration/>
                    <Route path=path!("admin/*path") view=admin::Administration/>
                    <Route path=path!("") view=Home ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("instalace") view=InstallApp ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("uredni-deska") view=NoticeBoard ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("uredni-deska/:id") view=NoticeDetail ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("dokumenty") view=Documents ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("dokumenty/:id") view=catalog::DocumentDetail ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("stranky/:slug") view=catalog::ContentPage ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("hledat") view=SearchResults ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("relief") view=Relief ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("obec") view=Contact ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("povinne-informace") view=RequiredInformation ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("stranky") view=PagesIndex ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("kontakt") view=ContactRedirect ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("odber") view=Subscribe ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("kalendar") view=Calendar ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("kalendar/:id") view=events::EventDetail ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("hlaseni-zavad") view=reports::Reports ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("hlaseni-zavad/nove") view=reports::NewReport ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("hlaseni-zavad/:id") view=reports::ReportDetail ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("pristupnost") view=Accessibility ssr=leptos_router::SsrMode::InOrder/>
                    <Route path=path!("ochrana-udaju") view=Privacy ssr=leptos_router::SsrMode::InOrder/>
                </Routes>
            </main>
            <PublicFooter/>
        </Router>
    }
}

#[component]
fn PublicHeader() -> impl IntoView {
    let location = leptos_router::hooks::use_location();
    view! {<Show when=move || !is_admin_path(&location.pathname.get())><Header/></Show>}
}
#[component]
fn PublicFooter() -> impl IntoView {
    let location = leptos_router::hooks::use_location();
    view! {<Show when=move || !is_admin_path(&location.pathname.get())><Footer/></Show>}
}
fn is_admin_path(path: &str) -> bool {
    let prefix = site_url("/admin");
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

#[component]
fn SiteRobots() -> impl IntoView {
    let info = expect_context::<SiteResource>();
    view! {<Suspense fallback=||view!{<Meta name="robots" content="noindex, nofollow"/>}>{move||info.get().map(|r|view!{<Meta name="robots" content=if r.is_ok_and(|i|i.production){"index, follow"}else{"noindex, nofollow"}/>})}</Suspense>}
}
