use super::{
    components::{Icon, Loading},
    pages::NotFound,
    site_url,
};
use crate::catalog::{load_document, load_page};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::{components::A, hooks::use_params_map};

#[component]
pub fn DocumentLibrary() -> impl IntoView {
    view! {<super::search::DocumentListing/>}
}

#[component]
pub fn DocumentDetail() -> impl IntoView {
    let params = use_params_map();
    let document = Resource::new(
        move || {
            params
                .get()
                .get("id")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0)
        },
        load_document,
    );
    view! {<div class="page-width interior prose"><A href=site_url("/dokumenty") attr:class="back-link"><Icon name="back"/>"Zpět na dokumenty"</A>
        <Suspense fallback=Loading>{move ||document.get().map(|result|match result {
            Ok(Some(doc))=>view!{<Title text=format!("{} · Vyskeř",doc.title)/><header class="page-heading"><h1>{doc.title}</h1><p class="page-description">{doc.description}</p></header><section class="attachments"><h2>"Přílohy"</h2>{doc.files.into_iter().map(|file|view!{<div class="attachment"><Icon name="paper"/><div><strong>{file.name}</strong><p>{file.size}</p></div>{file.url.map(|url|view!{<a class="text-link" href=url download>"Stáhnout"<Icon/></a>})}</div>}).collect_view()}</section>}.into_any(),
            Ok(None)=>view!{<NotFound/>}.into_any(),
            Err(_)=>view!{<p role="alert">"Dokument se nepodařilo načíst."</p>}.into_any()
        })}</Suspense>
    </div>}
}

#[component]
pub fn ContentPage() -> impl IntoView {
    let params = use_params_map();
    let page = Resource::new(
        move || params.get().get("slug").unwrap_or_default(),
        load_page,
    );
    view! {<div class="page-width interior prose"><Suspense fallback=Loading>{move ||page.get().map(|result|match result {
        Ok(Some(page))=>view!{<Title text=format!("{} · Vyskeř",page.title)/><header class="page-heading"><h1>{page.title}</h1></header><div class="markdown-content" inner_html=crate::markdown::render(&page.content)></div>}.into_any(),
        Ok(None)=>view!{<NotFound/>}.into_any(),
        Err(_)=>view!{<p role="alert">"Stránku se nepodařilo načíst."</p>}.into_any()
    })}</Suspense></div>}
}
