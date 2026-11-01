use super::{api, ui::AdminLink};
use crate::app::components::Icon;
use leptos::prelude::*;
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct FileProgress {
    attachment_id: i64,
    document_id: i64,
    name: String,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct FailedFile {
    attachment_id: i64,
    document_id: i64,
    name: String,
}
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct IndexStatus {
    total: i64,
    ready: i64,
    pending: i64,
    processing: i64,
    failed: i64,
    retrying: i64,
    current: Option<FileProgress>,
    failed_items: Vec<FailedFile>,
}

fn refresh(status: RwSignal<Option<IndexStatus>>, busy: RwSignal<bool>, failed: RwSignal<bool>) {
    // A slow request cannot overlap another poll or a manual refresh. Disposed
    // signals also stop requests after the administrator leaves this screen.
    if busy.try_get_untracked() != Some(false) {
        return;
    }
    busy.set(true);
    leptos::task::spawn_local(async move {
        match api::get::<IndexStatus>("/api/v1/admin/attachment-index/status").await {
            Ok(next) => {
                if status.try_get_untracked().flatten().as_ref() != Some(&next) {
                    let _ = status.try_set(Some(next));
                }
                let _ = failed.try_set(false);
            }
            Err(_) => {
                // Keep the last successful counts visible during a temporary outage.
                let _ = failed.try_set(true);
            }
        }
        let _ = busy.try_set(false);
    });
}

#[component]
pub fn AttachmentIndexProgress() -> impl IntoView {
    let status = RwSignal::new(None::<IndexStatus>);
    let busy = RwSignal::new(false);
    let failed = RwSignal::new(false);
    Effect::new(move |_| refresh(status, busy, failed));
    #[cfg(feature = "hydrate")]
    {
        if let Ok(timer) = leptos::leptos_dom::helpers::set_interval_with_handle(
            move || refresh(status, busy, failed),
            std::time::Duration::from_secs(3),
        ) {
            on_cleanup(move || timer.clear());
        }
    }
    view! {<section class="admin-panel attachment-index-progress" aria-labelledby="attachment-index-title">
        <div class="admin-panel-heading"><div><h2 id="attachment-index-title">"Vyhledávání v přílohách"</h2><p class="field-note">"Příprava obsahu nahraných příloh pro vyhledávání na webu."</p></div>
            <button type="button" class="button secondary" disabled=move ||busy.get() on:click=move |_|refresh(status,busy,failed)><Icon name="refresh"/>"Obnovit stav"</button>
        </div>
        {move ||status.get().map(|current|view!{<ProgressState status=current/>})}
        {move ||(status.get().is_none() && !failed.get()).then(||view!{<p class="field-note" role="status">"Načítám stav příloh…"</p>})}
        {move ||failed.get().then(||view!{<p class="admin-error" role="status">{if status.get().is_some(){"Stav se nepodařilo obnovit. Zobrazen je poslední známý stav."}else{"Stav příloh se nepodařilo načíst. Zkuste obnovení."}}</p>})}
    </section>}
}

#[component]
fn ProgressState(status: IndexStatus) -> impl IntoView {
    let percent = if status.total > 0 {
        status.ready.saturating_mul(100) / status.total
    } else {
        0
    };
    let has_files = status.total > 0;
    let more_failures = status.failed > status.failed_items.len() as i64;
    let failure_count = status.failed_items.len();
    view! {
        <p class="attachment-index-summary" role="status">{if has_files {format!("Připraveno {} z {} příloh · {} %",status.ready,status.total,percent)}else{"Zatím nejsou nahrané přílohy ke zpracování.".into()}}</p>
        <progress max=status.total.max(1) value=status.ready aria-label="Přílohy připravené pro vyhledávání">{format!("{percent} %")}</progress>
        <dl class="attachment-index-counts">
            <div><dt>"Připraveno"</dt><dd data-state="ready">{status.ready}</dd></div>
            <div><dt>"Čeká"</dt><dd data-state="pending">{status.pending}</dd></div>
            <div><dt>"Zpracovává se"</dt><dd data-state="processing">{status.processing}</dd></div>
            <div><dt>"Nepodařilo se"</dt><dd data-state="failed">{status.failed}</dd></div>
        </dl>
        {status.current.map(|file|view!{<p class="attachment-index-current">"Právě se zpracovává: "<AdminLink href=format!("/admin/dokumenty/{}",file.document_id) label=file.name/></p>})}
        {has_files.then(||view!{<p class="field-note">"Stav se obnovuje automaticky. Postup přibývá po dokončení celé přílohy."</p>})}
        {(status.retrying>0).then(||view!{<p class="field-note">{format!("Přílohy čekající na další pokus: {}.",status.retrying)}</p>})}
        {(!status.failed_items.is_empty()).then(||view!{<div class="attachment-index-failures"><h3>"Přílohy, které se nepodařilo zpracovat"</h3><ul>{status.failed_items.into_iter().map(|file|view!{<li><AdminLink href=format!("/admin/dokumenty/{}",file.document_id) label=file.name/></li>}).collect_view()}</ul>
            {more_failures.then(||view!{<p class="field-note">{format!("Zobrazeno posledních {failure_count} příloh s chybou.")}</p>})}
        </div>})}
    }
}
