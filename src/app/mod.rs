mod admin;
mod catalog;
mod components;
mod pages;
mod search;

use crate::content::{Board, load_board};
use components::{Footer, Header, SkipLink};
use leptos::prelude::*;
use leptos_meta::{Meta, Title, provide_meta_context};
#[cfg(not(feature = "demo"))]
use leptos_meta::{MetaTags, Stylesheet};
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
                <link rel="icon" type="image/svg+xml" href="/favicon.svg"/>
                <link rel="preload" href="/fonts/geist.woff2" r#as="font" type="font/woff2" crossorigin="anonymous"/>
                <script src="/theme.js"></script>
                <AutoReload options=options.clone()/>
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
    provide_context(Resource::new(|| (), |_| crate::catalog::load_site_info()));
    #[cfg(not(feature = "demo"))]
    let stylesheet = view! { <Stylesheet id="leptos" href="/pkg/obecni-web.css"/> };
    #[cfg(feature = "demo")]
    let stylesheet = ();
    view! {
        {stylesheet}
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
                    <Route path=path!("") view=Home/>
                    <Route path=path!("uredni-deska") view=NoticeBoard/>
                    <Route path=path!("uredni-deska/:id") view=NoticeDetail ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("dokumenty") view=Documents/>
                    <Route path=path!("dokumenty/:id") view=catalog::DocumentDetail ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("stranky/:slug") view=catalog::ContentPage ssr=leptos_router::SsrMode::Async/>
                    <Route path=path!("hledat") view=SearchResults/>
                    <Route path=path!("obec") view=Municipality/>
                    <Route path=path!("povinne-informace") view=RequiredInformation/>
                    <Route path=path!("stranky") view=PagesIndex/>
                    <Route path=path!("kontakt") view=Contact/>
                    <Route path=path!("odber") view=Subscribe/>
                    <Route path=path!("kalendar") view=Calendar/>
                    <Route path=path!("pristupnost") view=Accessibility/>
                    <Route path=path!("ochrana-udaju") view=Privacy/>
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
