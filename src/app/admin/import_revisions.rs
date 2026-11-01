use super::{AdminContext, api, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Deserialize)]
struct Revision {
    id: i64,
    source_key: String,
    document_id: Option<i64>,
    fingerprint: String,
    proposal: Value,
    first_seen_at: String,
    state: String,
    accepted_document_id: Option<i64>,
    #[serde(default)]
    original: Value,
    #[serde(default)]
    files: Vec<RevisionFile>,
}
#[derive(Clone, Deserialize)]
struct RevisionFile {
    position: usize,
    name: String,
    sha256: String,
}

#[component]
pub fn ImportRevisions() -> impl IntoView {
    let offset = RwSignal::new(0usize);
    let state = RwSignal::new("pending".to_string());
    let reload = RwSignal::new(0u64);
    let entries = LocalResource::new(move || {
        let _ = reload.get();
        let url = format!(
            "/api/v1/admin/import-revisions?limit=21&offset={}&state={}",
            offset.get(),
            state.get()
        );
        async move { api::get::<Vec<Revision>>(&url).await }
    });
    view! {<Title text="Změny zjištěné u zdroje · Správa Vyskře"/><Heading title="Změny zjištěné u zdroje" description="Neveřejné revize převzatých dokumentů. Přijetí vždy vytvoří nový propojený koncept."/>
        <p class="info-banner">"Změna zjištěná importérem není sama o sobě doloženou opravou vydanou původcem. Původní uzamčený dokument se nepřepisuje."</p>
        <label class="admin-field" for="revision-state"><span>"Stav kontroly"</span><select id="revision-state" prop:value=move ||state.get() on:change=move |ev|{state.set(event_target_value(&ev));offset.set(0);}><option value="pending">"Čeká na kontrolu"</option><option value="accepted">"Přijato jako nový záznam"</option><option value="rejected">"Zamítnuto"</option></select></label>
        <Suspense fallback=Pending>{move ||entries.get().map(|result|match result {
            Err(error)=>view!{<FailureView error/>}.into_any(),
            Ok(items)=>{let count=items.len();view!{
                {(count==0).then(||view!{<p>"V tomto přehledu nejsou žádné revize."</p>})}
                {items.into_iter().take(20).map(|revision|view!{<RevisionReview revision reload/>}).collect_view()}
                <Pager offset count/>
            }.into_any()},
        })}</Suspense>
    }
}

#[component]
fn RevisionReview(revision: Revision, reload: RwSignal<u64>) -> impl IntoView {
    let id = revision.id;
    let ctx = expect_context::<AdminContext>();
    let reason = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let outcome = RwSignal::new(None::<i64>);
    let decide = Action::new_local(move |action: &String| {
        let payload = json!({"action":action,"reason":reason.get_untracked()});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/import-revisions/{id}/decision"),
                Some(payload),
            )
            .await
            {
                Ok(value) => {
                    outcome.set(value["id"].as_i64());
                    ctx.changed(if value["id"].is_i64(){"Revize byla přijata jako nový neveřejný koncept. Původní dokument se nezměnil."}else{"Revize byla zamítnuta. Stejná podoba se při dalším importu znovu neotevře."});
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let disabled = Signal::derive(move || decide.pending().get() || reason.get().trim().is_empty());
    let original_title = value_text(&revision.original, "title");
    let original_text = value_text(&revision.original, "description");
    let proposed_title = value_text(&revision.proposal, "title");
    let proposed_text = revision.proposal["content"]
        .as_str()
        .or(revision.proposal["description"].as_str())
        .or(revision.proposal["evidence"].as_str())
        .unwrap_or_default()
        .to_owned();
    let pending = revision.state == "pending";
    let original_files = revision.original["attachments"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    view! {<article class="admin-panel admin-form-section"><h2>{format!("Revize #{id}: {proposed_title}")}</h2>
        <p>{format!("Zdrojová identita: {} · zjištěno {}",revision.source_key,crate::catalog::document_event_date(Some(&revision.first_seen_at), None))}</p>
        {revision.document_id.map(|document_id|view!{<AdminLink href=format!("/admin/dokumenty/{document_id}") label="Otevřít původní záznam"/>})}
        {revision.accepted_document_id.map(|document_id|view!{<AdminLink href=format!("/admin/dokumenty/{document_id}") label="Otevřít přijatý navazující záznam"/>})}
        <dl class="revision-diff"><div><dt>"Přijatý název"</dt><dd>{original_title}</dd></div><div><dt>"Nově zjištěný název"</dt><dd>{proposed_title}</dd></div><div><dt>"Přijatý text"</dt><dd class="revision-text">{original_text}</dd></div><div><dt>"Nově zjištěný text"</dt><dd class="revision-text">{proposed_text}</dd></div></dl>
        <details><summary>"Porovnat přílohy a zdrojové údaje"</summary><h3>"Přílohy přijaté podoby"</h3>
            {if original_files.is_empty(){view!{<p>"Přílohy přijaté podoby ověřte v původním záznamu."</p>}.into_any()}else{view!{<ul>{original_files.into_iter().map(|file|view!{<li>{value_text(&file,"name")}<code class="file-fingerprint">{value_text(&file,"sha256")}</code></li>}).collect_view()}</ul>}.into_any()}}
            <h3>"Nově zjištěné přílohy"</h3><ul>{revision.files.into_iter().map(|file|view!{<li><a href=format!("/api/v1/admin/import-revisions/{id}/files/{}",file.position) download>{file.name}</a><code class="file-fingerprint">{file.sha256}</code></li>}).collect_view()}</ul>
            <p>"Prázdný seznam může znamenat revizi metadat. Rozsah změny je uveden v následujících zdrojových údajích."</p>
            <pre class="revision-source">{serde_json::to_string_pretty(&revision.proposal).unwrap_or_default()}</pre><small>"Otisk pozorované revize: "<code class="file-fingerprint">{revision.fingerprint}</code></small>
        </details>
        {pending.then(move ||view!{<ErrorMessage error/><label class="admin-field" for=format!("revision-reason-{id}")><span>"Interní důvod rozhodnutí *"</span><textarea id=format!("revision-reason-{id}") maxlength="2000" required prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))></textarea></label>
            <div class="admin-actions"><ConfirmButton label="Přijmout jako nový koncept" title="Přijmout změnu jako nový záznam?" description="Vznikne nový neveřejný koncept s novým ID a vazbou na původní dokument. Vyvěšení ani nové zákonné lhůty se tím nezakládají." disabled on_confirm=Callback::new(move |()|{decide.dispatch("accept".into());})/>
            <ConfirmButton label="Zamítnout revizi" title="Zamítnout tuto revizi?" description="Rozhodnutí zůstane v historii. Stejná zdrojová podoba se při dalším importu neobjeví jako nový konflikt." danger=true disabled on_confirm=Callback::new(move |()|{decide.dispatch("reject".into());})/></div>
        })}
        {move ||outcome.get().map(|document_id|view!{<AdminLink href=format!("/admin/dokumenty/{document_id}") label="Otevřít nový koncept"/>})}
    </article>}
}

fn value_text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
