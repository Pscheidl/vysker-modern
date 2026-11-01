use super::{
    AdminContext,
    api::{self, Entry, Kind},
    ui::*,
};
use crate::app::components::Icon;
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_navigate;
use serde_json::json;

#[component]
pub fn Editor(kind: Kind, id: i64) -> impl IntoView {
    let reload = RwSignal::new(0u64);
    let data = LocalResource::new(move || {
        let _ = reload.get();
        async move {
            if id > 0 {
                api::get::<Entry>(&format!("/api/v1/admin/{}/{id}", kind.api())).await
            } else {
                let stats: api::Overview = api::get("/api/v1/admin/overview").await?;
                Ok(Entry {
                    status: "draft".into(),
                    published_on: Some(stats.current_date),
                    ..Default::default()
                })
            }
        }
    });
    view! {<Suspense fallback=Pending>{move||data.get().map(|result|match result{
        Ok(entry)=>view!{<EditorForm kind entry reload/>}.into_any(),
        Err(error)=>view!{<FailureView error/><button class="button secondary" type="button" on:click=move |_|data.refetch()>"Načíst znovu"</button>}.into_any(),
    })}</Suspense>}
}

#[component]
fn EditorForm(kind: Kind, entry: Entry, reload: RwSignal<u64>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let site = use_context::<crate::app::SiteResource>();
    let id = entry.id;
    let version = entry.version;
    let content_version = entry.content_version;
    let operational_entry = entry.clone();
    let status = entry.status().to_string();
    let scheduled = entry.board_status == "scheduled";
    let immutable = entry.lock_state == "locked" || entry.lock_state == "review";
    let locked = kind != Kind::Page && (immutable || scheduled || (id > 0 && status != "draft"));
    let name = RwSignal::new(entry.title.clone());
    let description = RwSignal::new(entry.description.clone().unwrap_or_default());
    let document_date = RwSignal::new(entry.document_date.clone().unwrap_or_default());
    let reference = RwSignal::new(entry.reference_number.clone().unwrap_or_default());
    // Existing provenance stays intact although the ordinary editor no longer asks for an issuer.
    let issuer = StoredValue::new(entry.issuer.clone());
    let board_enabled = RwSignal::new(entry.board_enabled);
    let posted = RwSignal::new(entry.published_on.clone().unwrap_or_default());
    let duration = RwSignal::new(
        entry
            .duration_days
            .map(|days| days.to_string())
            .unwrap_or_default(),
    );
    let retention = RwSignal::new(entry.retain_attachments);
    let slug = RwSignal::new(entry.slug.clone());
    let content = RwSignal::new(entry.content.clone());
    let published = RwSignal::new(entry.published.unwrap_or(false));
    let file_busy = RwSignal::new(false);
    let error = RwSignal::new(None);
    let navigate = use_navigate();
    let save = Action::new_local(move |_: &()| {
        let navigate = navigate.clone();
        let enabled = board_enabled.get_untracked();
        let payload = match kind {
            Kind::Document => json!({
                "title":name.get_untracked(),"description":description.get_untracked(),
                "reference_number":optional(reference.get_untracked()),"issuer":issuer.get_value(),
                "document_date":optional(document_date.get_untracked()),"expected_version":content_version,
                "board_enabled":enabled,
                "published_on":if enabled{optional(posted.get_untracked())}else{None},
                "duration_days":if enabled{duration.get_untracked().parse::<i64>().ok()}else{None},
                "retain_attachments":if enabled{retention.get_untracked()}else{None}
            }),
            Kind::Page => {
                json!({"title":name.get_untracked(),"slug":slug.get_untracked(),"content":content.get_untracked(),"published":published.get_untracked(),"expected_version":version})
            }
        };
        async move {
            error.set(None);
            let path = if id == 0 {
                format!("/api/v1/admin/{}", kind.api())
            } else {
                format!("/api/v1/admin/{}/{id}", kind.api())
            };
            match api::save(if id == 0 { "POST" } else { "PUT" }, &path, Some(payload)).await {
                Ok(result) => {
                    ctx.dirty.set(false);
                    if let Some(site) = site {
                        site.refetch();
                    }
                    ctx.changed("Změny jsou uložené.");
                    if id == 0 {
                        if let Some(id) = result["id"].as_i64() {
                            navigate(&format!("{}/{id}", kind.path()), Default::default());
                        }
                    } else {
                        reload.update(|v| *v += 1);
                    }
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let busy = Signal::derive(move || save.pending().get() || file_busy.get());
    let heading = if id == 0 {
        kind.create_label().to_string()
    } else {
        entry.title.clone()
    };
    let public_url = match kind {
        Kind::Document => format!("/dokumenty/{id}"),
        Kind::Page => crate::catalog::page_path(&entry.slug),
    };
    let public_visible = if kind == Kind::Page {
        entry.published == Some(true)
    } else {
        entry
            .is_public_now
            .unwrap_or(entry.public_access && !entry.access_blocked)
    };
    let files_visible = entry.files_public_now.or(entry
        .attachments_public
        .map(|public| public && public_visible));
    let has_postings = !entry.postings.is_empty();
    let lock_label = match entry.lock_state.as_str() {
        "locked"
            if has_postings
                || matches!(
                    entry.board_status.as_str(),
                    "published" | "archived" | "withdrawn"
                ) =>
        {
            "Uzamčeno po vyvěšení"
        }
        "locked" => "Uzamčeno",
        "review" => "Obsah blokován k ověření",
        _ if status == "archived" => "Archivováno",
        _ if status == "published" => "Zveřejněno",
        _ => "Koncept",
    };
    let board_label = match entry.board_status.as_str() {
        "published" => "Aktuálně vyvěšeno",
        "scheduled" => "Vyvěšení naplánováno",
        "archived" | "withdrawn" => "Sejmuto",
        _ => "Bez aktuálního vyvěšení",
    };
    view! {
        <Title text=format!("{heading} · Správa Vyskře")/>
        <div class="admin-editor-back"><AdminLink href=kind.path() label=kind.title() icon="back"/><span>"/"</span><span>{if id==0{"Nový záznam".into()}else{format!("#{id:04}")}}</span></div>
        <div class="admin-editor-heading"><h1>{heading}</h1>{if kind==Kind::Page{view!{<Status value=status.clone()/>}.into_any()}else{view!{<span class="status-badge">{lock_label}</span>}.into_any()}}</div>
        {(kind==Kind::Document).then(||view!{<dl class="document-state-facts"><div><dt>"Obsah"</dt><dd>{lock_label}</dd></div><div><dt>"Veřejná dostupnost"</dt><dd>{if public_visible{"Veřejně dostupný"}else{"Neveřejný"}}</dd></div><div><dt>"Úřední deska"</dt><dd>{board_label}</dd></div>{files_visible.map(|files_public|view!{<div><dt>"Přílohy"</dt><dd>{if files_public{"Veřejně dostupné"}else{"Pouze pro administrátory"}}</dd></div>})}</dl>})}
        {entry.origin.clone().map(|origin|view!{<AdminOrigin origin/>})}
        <ErrorMessage error/>
        <div class="admin-editor-grid"><div class="admin-editor-content">
            <form id="content-editor" on:submit=move|ev|{ev.prevent_default();if !busy.get_untracked()&&!locked{save.dispatch(());}}>
                <fieldset disabled=move||busy.get()||locked>
                    <section class="admin-panel admin-form-section"><div class="admin-panel-heading"><h2>"Základní údaje"</h2><span class="admin-caption">"* Povinné údaje"</span></div>
                        <Field id="entry-name" label="Název" value=name required=true maxlength=if kind==Kind::Page{"200"}else{"300"}/>
                        {if kind==Kind::Page{view!{
                            <Field id="page-slug" label="Adresa stránky" value=slug required=true maxlength="80" help="Například spolky. Malá písmena bez diakritiky, číslice a pomlčky."/>
                            <p class="admin-url">{move||crate::catalog::page_path(&slug.get())}</p>
                            <super::markdown_editor::MarkdownEditor id title=name content busy working=file_busy/>
                        }.into_any()}else{view!{
                            <TextArea id="entry-description" label="Popis" value=description/>
                            <DocumentField id="document-reference" label="Číslo jednací (nepovinné)" value=reference maxlength="200" help="Vyplňte číslo uvedené na písemnosti. Pokud dokument číslo jednací nemá, ponechte pole prázdné. Web mu číslo jednací nepřiděluje."/>
                            <DocumentField id="document-date" label="Datum vytvoření dokumentu (nepovinné)" kind="date" value=document_date help="Datum, kdy dokument vznikl, například datum uvedené na jeho první straně. Nejde o datum nahrání na web ani vyvěšení. Není-li známé, ponechte pole prázdné."/>
                        }.into_any()}}
                    </section>
                    {(kind==Kind::Document&&!locked).then(move||view!{<section class="admin-panel admin-form-section"><div class="document-field-label"><label class="admin-check"><input id="document-board-enabled" type="checkbox" prop:checked=move||board_enabled.get() on:change=move|ev|{board_enabled.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span><strong>"Vyvěsit na úřední desce"</strong><small>"Po uložení přidáte přílohy a potvrdíte finální náhled. Samotné uložení konceptu dokument nevyvěsí."</small></span></label><super::document_actions::DocumentHelp label="Vyvěšení na úřední desce" text="Zaškrtnutím připravíte vyvěšení na elektronické desce tohoto webu. Proběhne až po potvrzení finálního náhledu. Záznam na webu sám nepotvrzuje vyvěšení na fyzické úřední desce ani splnění všech zákonných podmínek."/></div>
                        <Show when=move||board_enabled.get()><super::document_actions::BoardSettings published_on=posted duration_days=duration retention mark_dirty=true/></Show>
                    </section>})}
                    {(kind==Kind::Page).then(move||view!{<section class="admin-panel admin-form-section"><h2>"Dostupnost stránky"</h2><label class="admin-check"><input type="checkbox" prop:checked=move||published.get() on:change=move|ev|{published.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span><strong>"Zveřejnit na webu"</strong><small>"Změna se projeví po uložení. Bez zaškrtnutí zůstává stránka konceptem."</small></span></label></section>})}
                </fieldset>
            </form>
            {(kind==Kind::Page&&id>0).then(move||view!{<PageHistory id name content busy/>})}
            {(kind==Kind::Document&&has_postings).then(move||view!{<IncidentForm id/>})}
            {(kind==Kind::Document).then(move||view!{<Files kind id files=entry.attachments editable=!locked busy working=file_busy reload/>})}
            {(kind==Kind::Document&&id>0).then(move||view!{<super::document_actions::DocumentActions entry=operational_entry busy reload/>})}
        </div>
        <aside class="admin-editor-sidebar"><section class="admin-panel admin-save-panel"><h2>"Uložení a zveřejnění"</h2>
            {if locked{view!{<div class="admin-note"><Icon name="lock"/><p>{if scheduled{"Dokument čeká na plánované vyvěšení. Obsah této podoby nelze během čekání měnit."}else if immutable{"Obsah a přílohy nelze měnit. Opravu nebo doplnění založte jako nový propojený koncept."}else{"Zveřejněný dokument upravíte vytvořením nového navazujícího konceptu."}}</p></div>}.into_any()}else{view!{
                <p>{if id==0&&kind==Kind::Document{"Nejdříve uložte koncept. Potom přidejte přílohy a dokument zveřejněte."}else{"Uložte změny před dalšími kroky."}}</p>
                <button class="button primary" type="submit" form="content-editor" disabled=move||busy.get()>{move||if save.pending().get(){"Ukládám…"}else if id==0&&kind==Kind::Document{"Uložit koncept"}else{"Uložit změny"}}</button>
                <span class="admin-save-state" role="status">{move||if ctx.dirty.get(){"Máte neuložené změny"}else if id==0{"Nový koncept"}else{"Všechny změny jsou uložené"}}</span>
            }.into_any()}}
            {public_visible.then(move||view!{<a href=public_url target="_blank" rel="noopener" class="admin-public-link">"Otevřít na webu"<Icon name="external"/></a>})}
            {(kind==Kind::Document&&has_postings).then(move||view!{<a class="admin-public-link" href=format!("/api/v1/admin/notices/{id}/evidence") download="doklad-vyveseni.json">"Stáhnout doklad vyvěšení"<Icon name="paper"/></a>})}
            <p class="admin-caption"><Icon name="history"/>"Změny se zapisují do auditu."</p>
        </section></aside></div>
    }
}

#[component]
fn DocumentField(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    help: &'static str,
    #[prop(default = "text")] kind: &'static str,
    #[prop(default = "1000")] maxlength: &'static str,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    view! {<div class="admin-field"><div class="document-field-label"><label for=id>{label}</label><super::document_actions::DocumentHelp label text=help/></div><input id=id type=kind maxlength=maxlength prop:value=move||value.get() on:input=move|ev|{value.set(event_target_value(&ev));ctx.dirty.set(true);}/></div>}
}

#[component]
fn AdminOrigin(origin: serde_json::Value) -> impl IntoView {
    let source = origin["source_url"].as_str().unwrap_or_default().to_owned();
    let identity = origin["source_key"].as_str().unwrap_or_default().to_owned();
    let published = origin["source_published_on"]
        .as_str()
        .map(crate::catalog::document_date);
    let imported = origin["imported_at"]
        .as_str()
        .map(|value| crate::catalog::document_event_date(Some(value), None));
    let observed = origin["last_observed_at"]
        .as_str()
        .map(|value| crate::catalog::document_event_date(Some(value), None));
    view! {<aside class="info-banner" aria-label="Původ převzatého dokumentu"><div><strong>"Převzatý dokument"</strong><p>"Datum importu není datem původního ani místního vyvěšení."</p>
        <dl class="document-facts">{(!source.is_empty()).then(||view!{<div><dt>"Původní adresa"</dt><dd class="source-identity">{source}</dd></div>})}{(!identity.is_empty()).then(||view!{<div><dt>"Zdrojová identita"</dt><dd>{identity}</dd></div>})}
            {published.map(|value|view!{<div><dt>"Původní datum zveřejnění"</dt><dd>{value}</dd></div>})}
            {imported.map(|value|view!{<div><dt>"Místní import"</dt><dd>{value}</dd></div>})}
            {observed.map(|value|view!{<div><dt>"Poslední pozorování zdroje"</dt><dd>{value}</dd></div>})}
        </dl>
    </div></aside>}
}

fn optional(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.into())
}

#[component]
fn Files(
    kind: Kind,
    id: i64,
    files: Vec<api::File>,
    editable: bool,
    busy: Signal<bool>,
    working: RwSignal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let input = NodeRef::<leptos::html::Input>::new();
    let selected = RwSignal::new(false);
    let error = RwSignal::new(None);
    let upload = Action::new_local(move |file: &web_sys::File| {
        let file = file.clone();
        async move {
            working.set(true);
            error.set(None);
            let result = api::upload(
                &format!("/api/v1/admin/{}/{id}/attachments", kind.api()),
                file,
            )
            .await;
            working.set(false);
            match result {
                Ok(_) => {
                    ctx.changed("Příloha byla nahrána.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let remove = Action::new_local(move |id: &i64| {
        let id = *id;
        async move {
            working.set(true);
            error.set(None);
            let result =
                api::save("DELETE", &format!("/api/v1/admin/attachments/{id}"), None).await;
            working.set(false);
            match result {
                Ok(_) => {
                    ctx.changed("Obsah přílohy byl odstraněn.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let disabled = Signal::derive(move || busy.get() || ctx.dirty.get());
    view! {<section class="admin-panel admin-form-section"><div class="admin-panel-heading"><h2>"Přílohy"</h2><Icon name="paper"/></div><ErrorMessage error/>
        {if id==0{view!{<div class="admin-note"><Icon name="info"/><p>"Přílohy přidáte po prvním uložení konceptu."</p></div>}.into_any()}else{view!{
            <ul class="admin-files">{files.into_iter().map(|file|{let file_id=file.id;let removed=file.removed_at.is_some();view!{<li><Icon name="paper"/><div><strong>{file.name}</strong><small>{if removed{"Obsah odstraněn".into()}else if !file.available{"Bez uloženého souboru".into()}else{format!("{} kB",(file.size_bytes+1023)/1024)}}</small></div>{file.available.then(||view!{<a class="text-link" href=format!("/api/v1/admin/attachments/{file_id}") download>"Stáhnout"</a>})}{(editable&&!removed).then(move||view!{<ConfirmButton label="Odebrat" title="Odebrat přílohu?" description="Obsah souboru se odstraní. Jeho název zůstane v historii záznamu." danger=true disabled on_confirm=Callback::new(move|()|{remove.dispatch(file_id);})/>})}</li>}}).collect_view()}</ul>
            {editable.then(move||view!{<div class="admin-upload"><Icon name="upload"/><label for="upload-file"><strong>"Přidat přílohu"</strong><span>"PDF, obrázek, text nebo kancelářský dokument. Nejvýše 10 MiB."</span></label><input node_ref=input id="upload-file" type="file" accept=".pdf,.png,.jpg,.jpeg,.txt,.csv,.docx,.xlsx,.odt,.ods" disabled=move||disabled.get() on:change=move |_|{
                error.set(None);let file=input.get().and_then(|el|el.files()).and_then(|files|files.item(0));
                if file.as_ref().is_some_and(|file|file.size()>10.0*1024.0*1024.0){selected.set(false);error.set(Some(api::Failure{status:413,message:"Soubor je větší než 10 MiB.".into()}));}else{selected.set(file.is_some());}
            }/><button class="button secondary" type="button" disabled=move||disabled.get()||!selected.get() on:click=move |_|{if let Some(file)=input.get().and_then(|el|el.files()).and_then(|files|files.item(0)){upload.dispatch(file);}}>{move||if upload.pending().get(){"Nahrávám…"}else{"Nahrát soubor"}}</button>
                <Show when=move||ctx.dirty.get()><p class="admin-caption">"Nejdříve uložte změny ve formuláři."</p></Show>
            </div>})}
            {(!editable).then(||view!{<p class="admin-caption">"Přílohy zveřejněného nebo archivovaného záznamu nelze měnit."</p>})}
        }.into_any()}}
    </section>}
}

#[component]
fn IncidentForm(id: i64) -> impl IntoView {
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let reason = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let success = RwSignal::new(false);
    let action = Action::new_local(move |_: &()| {
        let payload = json!({"started_at":format!("{}:00Z",start.get_untracked()),"ended_at":format!("{}:00Z",end.get_untracked()),"reason":reason.get_untracked()});
        async move {
            error.set(None);
            success.set(false);
            match api::save(
                "POST",
                &format!("/api/v1/admin/notices/{id}/incidents"),
                Some(payload),
            )
            .await
            {
                Ok(_) => success.set(true),
                Err(e) => error.set(Some(e)),
            }
        }
    });
    view! {<details class="admin-panel admin-form-section"><summary>"Zaznamenat výpadek zveřejnění"</summary><p>"Časy zadejte v UTC. Zápis nemění lhůtu. Odpovědná osoba posoudí nové vyvěšení nebo jiné opatření."</p><form on:submit=move|ev|{ev.prevent_default();if !action.pending().get_untracked(){action.dispatch(());}}>
        <label class="admin-field" for="incident-start"><span>"Začátek v UTC"</span><input id="incident-start" type="datetime-local" required on:input=move|ev|start.set(event_target_value(&ev))/></label>
        <label class="admin-field" for="incident-end"><span>"Konec v UTC"</span><input id="incident-end" type="datetime-local" required on:input=move|ev|end.set(event_target_value(&ev))/></label>
        <label class="admin-field" for="incident-reason"><span>"Popis a přijaté opatření bez osobních údajů"</span><textarea id="incident-reason" maxlength="2000" required on:input=move|ev|reason.set(event_target_value(&ev))></textarea></label>
        <ErrorMessage error/><button class="button secondary" type="submit" disabled=move||action.pending().get()>"Zapsat do dokladu"</button><Show when=move||success.get()><p role="status">"Výpadek je zaznamenán v dokladu vyvěšení."</p></Show>
    </form></details>}
}

#[derive(Clone, serde::Deserialize)]
struct PageRevision {
    version: i64,
    title: String,
    content: String,
    saved_at: String,
}
#[component]
fn PageHistory(
    id: i64,
    name: RwSignal<String>,
    content: RwSignal<String>,
    busy: Signal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let offset = RwSignal::new(0usize);
    let revisions = LocalResource::new(move || async move {
        api::get::<Vec<PageRevision>>(&format!(
            "/api/v1/admin/pages/{id}/revisions?limit=20&offset={}",
            offset.get()
        ))
        .await
    });
    view! {<section class="admin-panel admin-form-section"><h2>"Historie stránky"</h2><p class="admin-caption">"Starší verzi načtete do editoru a před uložením zkontrolujete. Adresa a zveřejnění zůstávají podle aktuální stránky."</p>
        <Suspense fallback=Pending>{move||revisions.get().map(|r|match r{
            Err(error)=>view!{<FailureView error/>}.into_any(),
            Ok(items)=>{let more=items.len()==20;view!{<ul class="revision-list">{items.into_iter().map(|item|view!{<li><span>{format!("Verze {} · {} · {}",item.version,api::date(&item.saved_at),item.title)}</span><button class="button secondary" type="button" disabled=move||busy.get() on:click=move |_|{if !busy.get_untracked() && ctx.leave(){name.set(item.title.clone());content.set(item.content.clone());ctx.dirty.set(true);}}>"Načíst do editoru"</button></li>}).collect_view()}</ul><div class="admin-actions"><button class="button secondary" type="button" disabled=move||offset.get()==0 on:click=move |_|offset.update(|v|*v=v.saturating_sub(20))>"Novější verze"</button><button class="button secondary" type="button" disabled=!more on:click=move |_|offset.update(|v|*v+=20)>"Starší verze"</button></div>}.into_any()}
        })}</Suspense>
    </section>}
}
