use super::{
    components::{Icon, ImportNote, Loading},
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
            Ok(Some(doc))=>view!{<DocumentContent document=doc/>}.into_any(),
            Ok(None)=>view!{<NotFound/>}.into_any(),
            Err(_)=>view!{<p role="alert">"Dokument se nepodařilo načíst."</p>}.into_any()
        })}</Suspense>
    </div>}
}

#[component]
fn DocumentContent(document: crate::catalog::Document) -> impl IntoView {
    let has_postings = !document.postings.is_empty();
    let has_relations = !document.relations.is_empty();
    view! {
        <Title text=format!("{} · Vyskeř",document.title)/>
        <header class="page-heading"><p class="eyebrow">{document.topic.unwrap_or_else(||"DOKUMENT OBCE".into())}</p><h1>{document.title}</h1></header>
        {document.import_origin.map(|_|view!{<ImportNote/>})}
        <p class="document-description">{document.description}</p>
        <dl class="document-facts"><div><dt>"Datum vytvoření dokumentu"</dt><dd>{crate::catalog::document_date(document.document_date.as_deref().unwrap_or_default())}</dd></div>
            {document.source_published_on.map(|value|view!{<div><dt>"Původní zveřejnění"</dt><dd>{crate::catalog::document_date(&value)}</dd></div>})}
            {document.reference_number.map(|value|view!{<div><dt>"Číslo jednací"</dt><dd>{value}</dd></div>})}
        </dl>
        <section class="attachments" aria-labelledby="document-attachments"><h2 id="document-attachments">"Veřejné přílohy"</h2>
            {document.files.is_empty().then(||view!{<p>"Dokument nemá veřejně dostupnou přílohu."</p>})}
            {document.files.into_iter().map(|file|{
                let original_label=format!("Stáhnout původní přílohu {}",file.name);
                let name_label=format!("{} (stáhnout původní přílohu)",file.name);
                let preview_label=format!("Zobrazit přepis textu přílohy {} (otevře se v novém okně)",file.name);
                let download_label=format!("Stáhnout přepis textu přílohy {}",file.name);
                view!{<div class="attachment attachment-card">
                    <div class="attachment-original">
                        <Icon name="paper"/>
                        <div class="attachment-content">
                            <strong class="attachment-name">{match file.url.clone() {
                                Some(url)=>view!{<a href=url download aria-label=name_label>{file.name}</a>}.into_any(),
                                None=>file.name.into_any(),
                            }}</strong>
                            <p class="attachment-size">{file.size}</p>
                        </div>
                        {file.url.map(|url|view!{<a class="attachment-download" href=url download aria-label=original_label><Icon name="download"/><span>"Stáhnout původní přílohu"</span></a>})}
                    </div>
                    {file.text_url.map(|url|view!{<div class="attachment-transcript">
                        <h3>"Přepis textu"</h3>
                        <p class="attachment-text-note">"Automatický přepis pro čtení a kopírování. Může obsahovat chyby."</p>
                        <div class="attachment-text-links">
                            <a href=url.clone() target="_blank" rel="external noopener noreferrer" aria-label=preview_label><Icon name="external"/><span>"Zobrazit přepis textu"</span></a>
                            <a href=format!("{url}/download") download aria-label=download_label><Icon name="paper"/><span>"Stáhnout přepis textu"<span class="attachment-text-format">" (.txt)"</span></span></a>
                        </div>
                    </div>})}
                </div>}
            }).collect_view()}
        </section>
        {has_postings.then(||view!{<section class="document-postings" aria-labelledby="posting-history-title"><h2 id="posting-history-title">"Vyvěšení na úřední desce"</h2><ol class="posting-history">{document.postings.into_iter().map(|posting|{
            let posted=crate::catalog::document_event_date(posting.started_at.as_deref(),posting.started_on.as_deref());
            let removed=crate::catalog::document_event_date(posting.removed_at.as_deref(),posting.removed_on.as_deref());
            let origin=if posting.origin=="local" {"Místní vyvěšení"}else{"Údaje převzaté z původního webu"};
            view!{<li><h3>{origin}</h3><dl class="document-facts"><div><dt>"Vyvěšeno"</dt><dd>{posted}</dd></div><div><dt>"Plánované sejmutí"</dt><dd>{posting.planned_removal_on.as_deref().map(crate::catalog::document_date).unwrap_or_else(||"Není určeno".into())}</dd></div><div><dt>"Skutečné sejmutí"</dt><dd>{removed}</dd></div></dl>
                {posting.corrections.into_iter().map(|correction|view!{<div class="posting-correction"><h4>"Dodatková oprava evidenčního údaje"</h4><p>{match correction.field.as_str(){"started_on"=>"Datum vyvěšení","removed_on"=>"Datum skutečného sejmutí",_=>"Evidovaný údaj"}}</p><dl class="document-facts"><div><dt>"Původně uvedeno"</dt><dd>{correction.original.as_deref().map(crate::catalog::document_date).unwrap_or_else(||"Není doloženo".into())}</dd></div><div><dt>"Opravený údaj"</dt><dd>{correction.corrected.as_deref().map(crate::catalog::document_date).unwrap_or_else(||"Není doloženo".into())}</dd></div></dl><p class="field-note">"Oprava zapsána: "{crate::catalog::document_event_date(Some(&correction.recorded_at),None)}</p></div>}).collect_view()}
                {(posting.origin!="local").then(||view!{<p class="field-note">"Převzetí z původního webu není novým místním vyvěšením. Dochovaná kopie sama nepotvrzuje přesnou podobu původně vyvěšených bajtů."</p>})}
                {(posting.initial_state=="historical"&&posting.removed_at.is_none()&&posting.removed_on.is_none()).then(||view!{<p class="field-note">"Historické vyvěšení. Skutečné datum sejmutí není doloženo."</p>})}
                {(posting.initial_state=="uncertain" || posting.certainty=="uncertain" || posting.certainty=="review").then(||view!{<p class="field-note">"Historické údaje nejsou úplné a vyžadují ověření."</p>})}
            </li>}
        }).collect_view()}</ol></section>})}
        {has_relations.then(||view!{<section aria-labelledby="related-documents"><h2 id="related-documents">"Související dokumenty"</h2><ul class="document-relations">{document.relations.into_iter().map(|relation|{
            let label=if relation.direction=="original" {"Původní dokument"}else{match relation.kind.as_str(){"correction"=>"Oprava","supplement"=>"Doplnění","replacement"=>"Nahrazující dokument","anonymized"=>"Anonymizované znění","source_change"=>"Změna zjištěná u zdroje",_=>"Navazující dokument"}};
            view!{<li><span>{label}</span><A href=site_url(&format!("/dokumenty/{}",relation.id))>{relation.title}</A></li>}
        }).collect_view()}</ul></section>})}
    }
}

#[component]
pub fn ContentPage() -> impl IntoView {
    let params = use_params_map();
    let contact = move || params.get().get("slug").as_deref() == Some("kontakt");
    let page = Resource::new(
        move || params.get().get("slug").unwrap_or_default(),
        load_page,
    );
    view! {<Show when=contact fallback=move || view! {<div class="page-width interior prose"><Suspense fallback=Loading>{move ||page.get().map(|result|match result {
        Ok(Some(page))=>view!{<Title text=format!("{} · Vyskeř",page.title)/><header class="page-heading"><h1>{page.title}</h1></header><super::pages::PageBody content=page.content/>}.into_any(),
        Ok(None)=>view!{<NotFound/>}.into_any(),
        Err(_)=>view!{<p role="alert">"Stránku se nepodařilo načíst."</p>}.into_any()
    })}</Suspense></div>}><super::pages::ContactRedirect/></Show>}
}
