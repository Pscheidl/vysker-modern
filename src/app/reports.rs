//! Public reports, optional photographs and a map with explicit location access.
use super::{
    components::{Icon, PageHeading},
    pages::NotFound,
    site_url,
};
use crate::reports::{
    Report, ReportMap, load_report, load_report_map, load_reports, report_excerpt, status_label,
};
use leptos::prelude::*;
use leptos_meta::{Stylesheet, Title};
use leptos_router::{
    components::A,
    hooks::{use_params_map, use_query_map},
};

const PAGE_SIZE: i64 = 20;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/public/reports.js")]
extern "C" {
    fn mount_report_form(root: &web_sys::HtmlElement);
    fn mount_report_map(root: &web_sys::HtmlElement);
    fn mount_report_overview(root: &web_sys::HtmlElement, points: &str);
    fn unmount_report(root: &web_sys::HtmlElement);
}

#[component]
fn ReportLoading() -> impl IntoView {
    view! {<p role="status" class="field-note">"Načítám hlášení…"</p>}
}

#[component]
fn ReportFailure(retry: Callback<()>) -> impl IntoView {
    view! {<div class="empty-state" role="alert"><h2>"Hlášení se nepodařilo načíst"</h2><button class="button secondary" type="button" on:click=move |_|retry.run(())>"Zkusit znovu"</button></div>}
}

#[component]
fn ReportForm() -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_report_form(&element);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_report(&element);
            }
        });
    }
    view! {<div node_ref=root class="report-compose" id="nove-hlaseni">
        <p class="field-note">"Rozbitá lavička, výmol nebo nefunkční osvětlení? Popište závadu a uveďte místo. Fotografie a jméno můžete přidat dobrovolně."</p>
        <p id="report-draft-status" class="info-banner" role="status" hidden>"Rozpracované hlášení bylo obnoveno."</p>
        <form id="report-form" method="post" action="/api/v1/reports" enctype="multipart/form-data">
            <fieldset disabled id="report-fields">
                <label class="report-field" for="report-description"><span>"Popis problému"</span><textarea id="report-description" name="description" required maxlength="5000" rows="4" placeholder="Co je rozbité a kde přesně to najdeme?"></textarea></label>
                <div class="report-address-search">
                    <label class="report-field" for="report-address"><span>"Adresa nebo upřesnění místa"</span><input id="report-address" name="address" type="text" maxlength="300" placeholder="Vyskeř 50, 512 64 Vyskeř" autocomplete="off" role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="report-address-results" aria-describedby="report-location-help report-address-privacy"/></label>
                    <ul id="report-address-results" class="report-address-results" role="listbox" aria-label="Nalezené adresy" hidden></ul>
                </div>
                <p id="report-address-status" class="field-note" role="status" aria-live="polite"></p>
                <p id="report-location-help" class="field-note">"Zadejte adresu, GPS souřadnice nebo vlastní popis místa. Výběr adresy z nabídky umístí špendlík do mapy. Při jeho posunu se do pole zapíší přesné souřadnice. GPS zadávejte jako zeměpisnou šířku a délku, například 50.529509, 15.158555."</p>
                <p id="report-address-privacy" class="report-search-credit">"Našeptávání zajišťuje "<a href="https://photon.komoot.io/" target="_blank" rel="noopener noreferrer">"Photon"</a>" s daty "<a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener noreferrer">"OpenStreetMap"</a>". Zadávaná adresa se odesílá vyhledávací službě."</p>
                <div class="report-map" id="report-map" role="region" aria-label="Mapa pro výběr místa závady" aria-describedby="report-location-help"></div>
                <input id="report-latitude" name="latitude" type="hidden"/>
                <input id="report-longitude" name="longitude" type="hidden"/>
                <div class="report-map-actions">
                    <button class="button secondary report-mobile-location" type="button" id="report-geolocate" hidden>"Použít moji polohu"</button>
                    <button class="button secondary" type="button" id="report-map-center" disabled>"Umístit špendlík do středu mapy"</button>
                    <button class="text-link" type="button" id="report-map-clear" hidden>"Odebrat špendlík"</button>
                </div>
                <p id="report-location-status" class="field-note" role="status" aria-live="polite">"Místo zatím není označené."</p>
                <label class="report-field" for="report-photo"><span>"Fotografie (nepovinné)"</span><input id="report-photo" name="photo" type="file" accept="image/jpeg,image/png,image/webp" multiple aria-describedby="report-photo-help report-photo-status report-photo-error"/></label>
                <p id="report-photo-help" class="field-note">"JPG, PNG nebo WebP, nejvýše 15 MB na fotografii a 50 MB celkem. Fotografie můžete přidávat postupně. Na telefonu lze podle možností prohlížeče vybrat fotky nebo pořídit nové."</p>
                <p id="report-photo-error" class="report-error" role="alert" hidden></p>
                <p id="report-photo-status" class="field-note" role="status" aria-live="polite"></p>
                <div id="report-photo-preview" class="report-photo-previews" hidden></div>
                <label class="report-field" for="report-reporter"><span>"Kdo nahlásil (nepovinné)"</span><input id="report-reporter" name="reporter_name" type="text" maxlength="120" autocomplete="name" placeholder="Můžete nechat prázdné"/></label>
                <p class="report-privacy">"Hlášení včetně místa, fotografií a vyplněného jména bude veřejné. Neuvádějte citlivé údaje a fotografujte samotnou závadu. IP adresu požadavku zaznamenáme pro ochranu před zneužitím, veřejně se nezobrazuje."</p>
                <p id="report-submit-error" class="report-error" role="alert" tabindex="-1" hidden></p>
                <button class="button primary" id="report-submit" type="submit">"Odeslat hlášení"<Icon/></button>
            </fieldset>
        </form>
        <noscript><p class="info-banner">"Pro odeslání hlášení a práci s mapou zapněte JavaScript. Zveřejněná hlášení můžete prohlížet i bez něj."</p></noscript>
    </div>}
}

#[component]
fn ReportRow(report: Report) -> impl IntoView {
    let title = report_excerpt(&report.description);
    let href = site_url(&format!("/hlaseni-zavad/{}", report.id));
    let date = crate::events::date_label(&report.created_at);
    let status = status_label(&report.status);
    view! {<article class="report-card">
        {report.has_photo.then(||view!{<A href=href.clone() attr:class="report-card-image" attr:aria-label=format!("Fotografie hlášení č. {}", report.id)><img src=format!("/api/v1/reports/{}/photo", report.id) alt="Fotografie nahlášené závady" loading="lazy"/></A>})}
        <div class="report-card-content"><div class="report-card-meta"><time datetime=report.created_at>{date}</time><span class=format!("report-status report-status-{}",report.status)>{status}</span></div>
            <h2><A href=href.clone()>{title}</A></h2>
            <p class="field-note">{report.address.unwrap_or_else(||"Místo označeno na mapě".into())}</p>
            <p class="field-note">"Nahlásil: "{report.reporter_name.unwrap_or_else(||"Anonymně".into())}</p>
            <A href attr:class="text-link">"Zobrazit detail"<Icon/></A>
        </div>
    </article>}
}

#[component]
fn ReportOverviewMap(data: ReportMap) -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    let displayed = data.points.len();
    #[cfg(feature = "hydrate")]
    {
        let points = serde_json::to_string(&data.points).unwrap_or_else(|_| "[]".into());
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_report_overview(&element, &points);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_report(&element);
            }
        });
    }
    let summary = if data.truncated {
        format!(
            "Na mapě je posledních {displayed} z {} hlášení s polohou. Hledáním můžete výběr zúžit.",
            data.located_count
        )
    } else if data.located_count == 0 {
        "V tomto výběru nejsou žádná hlášení s vyznačenou polohou.".into()
    } else {
        format!("Hlášení na mapě: {}.", data.located_count)
    };
    view! {
        <div node_ref=root id="reports-overview-map" class="report-map" role="region" aria-label="Mapa nahlášených závad" aria-describedby="reports-map-summary"></div>
        <p id="reports-map-summary" class="field-note">{summary}{(data.unlocated_count>0).then(||format!(" Bez vyznačené polohy: {}. Tato hlášení najdete v seznamu.", data.unlocated_count))}</p>
        <noscript><p class="field-note">"Pro zobrazení mapy zapněte JavaScript. Hlášení si můžete prohlédnout v seznamu pod mapou."</p></noscript>
    }
}

#[component]
pub fn Reports() -> impl IntoView {
    let params = use_query_map();
    let page = Memo::new(move |_| {
        params
            .get()
            .get("strana")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| (1..=5001).contains(v))
            .unwrap_or(1)
    });
    let query = Memo::new(move |_| {
        params
            .get()
            .get("q")
            .unwrap_or_default()
            .trim()
            .chars()
            .take(200)
            .collect::<String>()
    });
    let search_input = NodeRef::<leptos::html::Input>::new();
    #[cfg(feature = "hydrate")]
    Effect::new(move |previous: Option<String>| {
        let current = query.get();
        // Preserve text entered into the server-rendered field before hydration.
        // Only a later URL query change (reset, history or another search) replaces it.
        if previous.as_ref().is_some_and(|value| value != &current) {
            if let Some(input) = search_input.get() {
                input.set_value(&current);
            }
        }
        current
    });
    let reports = Resource::new(
        move || (page.get(), query.get()),
        |(page, query)| async move {
            load_reports(PAGE_SIZE + 1, (page - 1) * PAGE_SIZE, query.clone())
                .await
                .map(|items| (page, query, items))
        },
    );
    let map = Resource::new(move || query.get(), load_report_map);
    view! {<Title text="Nahlášené závady · Vyskeř"/>
        <Stylesheet href="/vendor/leaflet-1.9.4/leaflet.css"/>
        <Stylesheet href="/vendor/leaflet-markercluster-1.5.3/MarkerCluster.css"/>
        <div class="page-width interior reports-page"><PageHeading title="Nahlášené závady" description="Podívejte se, co lidé nahlásili a jak pokračuje řešení." eyebrow="PODNĚTY Z OBCE"/>
            <div class="reports-toolbar"><p class="field-note">"Nejnovější hlášení jsou nahoře."</p><A href=site_url("/hlaseni-zavad/nove") attr:class="button primary">"Nahlásit závadu"<Icon/></A></div>
            <form class="reports-search" method="get" action=site_url("/hlaseni-zavad") role="search" aria-label="Vyhledávání hlášení">
                <label class="report-field" for="reports-query"><span>"Hledat v hlášeních"</span><input node_ref=search_input id="reports-query" type="search" name="q" maxlength="200" placeholder="Popis závady nebo adresa" value=move ||query.get()/></label>
                <button class="button secondary" type="submit">"Hledat"</button>
                {move ||(!query.get().is_empty()).then(||view!{<A href=site_url("/hlaseni-zavad") attr:class="text-link">"Zrušit hledání"</A>})}
            </form>
            <section class="reports-overview" aria-labelledby="reports-map-title">
                <h2 id="reports-map-title">"Závady na mapě"</h2>
                <p class="field-note">"Kliknutím na špendlík zobrazíte náhled hlášení. Číslo označuje více závad blízko sebe, kliknutím je přiblížíte nebo rozbalíte."</p>
                <Suspense fallback=move ||view!{<div class="reports-map-loading" role="status">"Načítám mapu hlášení…"</div>}>
                    {move ||map.get().map(|result|match result {
                        Ok(data)=>view!{<ReportOverviewMap data/>}.into_any(),
                        Err(_)=>view!{<div class="reports-map-loading"><p role="alert">"Mapu hlášení se nepodařilo načíst. Hlášení najdete v seznamu níže."</p><button class="button secondary" type="button" on:click=move |_|map.refetch()>"Zkusit mapu znovu"</button></div>}.into_any(),
                    })}
                </Suspense>
            </section>
                <section class="reports-feed" aria-label="Seznam nahlášených závad">
                    <Suspense fallback=ReportLoading>{move ||reports.get().map(|result|match result {
                        Err(_)=>view!{<ReportFailure retry=Callback::new(move |_|reports.refetch())/>}.into_any(),
                        Ok((page,query,items))=>{
                            let more=items.len()>PAGE_SIZE as usize;
                            let encoded_query = percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC).to_string();
                            view!{
                                {if items.is_empty(){view!{<p class="empty-state">{if query.is_empty(){"Zatím tu nejsou žádná hlášení."}else{"Hledání neodpovídá žádné hlášení. Zkuste jiný popis nebo adresu."}}</p>}.into_any()}else{items.into_iter().take(PAGE_SIZE as usize).map(|report|view!{<ReportRow report/>}).collect_view().into_any()}}
                                <nav class="public-pager" aria-label="Stránkování hlášení"><span>{format!("Strana {page}")}</span><div>
                                    {(page>1).then(||view!{<A href=site_url(&format!("/hlaseni-zavad?strana={}&q={}",page-1,encoded_query)) attr:class="button secondary">"Předchozí"</A>})}
                                    {(more&&page<5001).then(||view!{<A href=site_url(&format!("/hlaseni-zavad?strana={}&q={}",page+1,encoded_query)) attr:class="button secondary">"Další"</A>})}
                                </div></nav>
                            }.into_any()
                        }
                    })}</Suspense>
                </section>
        </div>
    }
}

#[component]
pub fn NewReport() -> impl IntoView {
    view! {<Title text="Nahlásit závadu · Vyskeř"/><Stylesheet href="/vendor/leaflet-1.9.4/leaflet.css"/>
        <div class="page-width interior report-new-page">
            <A href=site_url("/hlaseni-zavad") attr:class="back-link"><Icon name="back"/>"Nahlášené závady"</A>
            <PageHeading title="Nahlásit závadu" description="Dejte nám vědět, co je v obci potřeba opravit." eyebrow="PODNĚTY Z OBCE"/>
            <ReportForm/>
        </div>
    }
}

#[component]
fn DetailMap(latitude: f64, longitude: f64) -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_report_map(&element);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_report(&element);
            }
        });
    }
    view! {<div node_ref=root class="report-map" id="report-detail-map" data-latitude=latitude.to_string() data-longitude=longitude.to_string() role="region" aria-label="Mapa místa závady"></div>
    <p class="field-note">{format!("Souřadnice: {latitude:.6}, {longitude:.6}. ")}<a href=format!("https://www.openstreetmap.org/?mlat={latitude}&mlon={longitude}#map=18/{latitude}/{longitude}") target="_blank" rel="noopener noreferrer">"Otevřít v OpenStreetMap"</a></p>}
}

#[component]
pub fn ReportDetail() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let submitted = Memo::new(move |_| {
        query
            .get()
            .get("odeslano")
            .is_some_and(|value| value == "1")
    });
    let report = Resource::new(
        move || {
            params
                .get()
                .get("id")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
        },
        load_report,
    );
    view! {<Stylesheet href="/vendor/leaflet-1.9.4/leaflet.css"/><div class="page-width interior report-detail">
        <A href=site_url("/hlaseni-zavad") attr:class="back-link"><Icon name="back"/>"Zpět na nahlášené závady"</A>
        <Suspense fallback=ReportLoading>{move ||report.get().map(|result|match result {
            Err(_)=>view!{<ReportFailure retry=Callback::new(move |_|report.refetch())/>}.into_any(),
            Ok(None)=>view!{<NotFound/>}.into_any(),
            Ok(Some(item))=>{
                let date=crate::events::date_label(&item.created_at);
                let photos=item.photo_urls();
                let resolution_photos=item.resolution_photo_urls();
                let resolved=item.status=="resolved";
                let has_resolution=resolved || item.resolution_comment.is_some() || !resolution_photos.is_empty();
                view!{<Title text=format!("Hlášení č. {} · Vyskeř",item.id)/>
                    {move ||submitted.get().then(||view!{<p id="report-submitted" class="info-banner" role="status">"Hlášení bylo odesláno. Děkujeme."</p>})}
                    <header class="page-heading"><p class="eyebrow">"HLÁŠENÍ ZÁVAD"</p><h1>{format!("Hlášení č. {}", item.id)}</h1></header>
                    <div class="report-card-meta"><time datetime=item.created_at>{date}</time><span class=format!("report-status report-status-{}",item.status)>{status_label(&item.status)}</span></div>
                    <p class="report-description">{item.description}</p>
                    <p><strong>"Nahlásil: "</strong>{item.reporter_name.unwrap_or_else(||"Anonymně".into())}</p>
                    {item.address.map(|address|view!{<p><strong>"Místo: "</strong>{address}</p>})}
                    {item.latitude.zip(item.longitude).map(|(latitude,longitude)|view!{<DetailMap latitude longitude/>})}
                    {(!photos.is_empty()).then(||view!{<section class="report-detail-photos" aria-label="Fotografie závady">{photos.into_iter().enumerate().map(|(index,url)|view!{<a href=url.clone() target="_blank" rel="noopener" class="report-detail-photo"><img src=url.clone() alt=format!("Fotografie nahlášené závady č. {}",index+1) loading="lazy"/></a>}).collect_view()}</section>})}
                    {has_resolution.then(||view!{<section class="report-resolution" aria-labelledby="report-resolution-title">
                        <h2 id="report-resolution-title">"Řešení závady"</h2>
                        {resolved.then(||view!{<p class="field-note">"Závada je označena jako vyřešená."</p>})}
                        {item.resolution_updated_at.map(|updated|{let label=crate::events::date_label(&updated);view!{<p class="field-note">"Aktualizováno "<time datetime=updated>{label}</time></p>}})}
                        {item.resolution_comment.map(|comment|view!{<p class="report-resolution-comment">{comment}</p>})}
                        {(!resolution_photos.is_empty()).then(||view!{<div class="report-detail-photos" aria-label="Fotografie řešení závady">{resolution_photos.into_iter().enumerate().map(|(index,url)|view!{<a href=url.clone() target="_blank" rel="noopener" class="report-detail-photo"><img src=url.clone() alt=format!("Fotografie řešení závady č. {}",index+1) loading="lazy"/></a>}).collect_view()}</div>})}
                    </section>})}
                }.into_any()
            }
        })}</Suspense>
    </div>}
}
