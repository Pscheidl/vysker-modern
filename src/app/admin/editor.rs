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
            let entry = if id > 0 {
                api::get::<Entry>(&format!("/api/v1/admin/{}/{id}", kind.api())).await?
            } else {
                let stats: api::Overview = api::get("/api/v1/admin/overview").await?;
                Entry {
                    status: "draft".into(),
                    published_on: Some(stats.current_date),
                    ..Default::default()
                }
            };
            let categories = if kind == Kind::Notice {
                api::get::<Vec<api::Category>>("/api/v1/categories").await?
            } else {
                vec![]
            };
            Ok::<_, api::Failure>((entry, categories))
        }
    });
    view! {<Suspense fallback=Pending>{move||data.get().map(|result|match result{
        Ok((entry,categories))=>view!{<EditorForm kind entry categories reload/>}.into_any(),
        Err(error)=>view!{<FailureView error/><button class="button secondary" type="button" on:click=move |_|data.refetch()>"Načíst znovu"</button>}.into_any(),
    })}</Suspense>}
}

#[component]
fn EditorForm(
    kind: Kind,
    entry: Entry,
    categories: Vec<api::Category>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let site = use_context::<crate::app::SiteResource>();
    let id = entry.id;
    let version = entry.version;
    let status = entry.status().to_string();
    let draft = id == 0 || status == "draft";
    let archived = matches!(status.as_str(), "archived" | "withdrawn");
    let locked = kind != Kind::Page && !draft;
    let name = RwSignal::new(entry.title.clone());
    let description = RwSignal::new(entry.description.clone().unwrap_or_default());
    let reference = RwSignal::new(entry.reference_number.clone().unwrap_or_default());
    let issuer = RwSignal::new(entry.issuer.clone().unwrap_or_default());
    let category = RwSignal::new(
        entry
            .category_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    );
    let posted = RwSignal::new(entry.published_on.clone().unwrap_or_default());
    let until = RwSignal::new(entry.withdraw_on.clone().unwrap_or_default());
    let duration = RwSignal::new("15".to_string());
    let mode = RwSignal::new(
        if id == 0 {
            "days"
        } else if entry.withdraw_on.is_some() {
            "date"
        } else {
            "unlimited"
        }
        .to_string(),
    );
    let retention = RwSignal::new(entry.retain_attachments);
    let review: crate::notice_policy::NoticeReview =
        serde_json::from_str(&entry.review_json).unwrap_or_default();
    let rule = RwSignal::new(if review.rule.is_empty() {
        "informational".into()
    } else {
        review.rule
    });
    let legal_basis = RwSignal::new(review.legal_basis);
    let not_before = RwSignal::new(review.not_before.map(|d| d.to_string()).unwrap_or_default());
    let decision_on = RwSignal::new(
        review
            .decision_on
            .map(|d| d.to_string())
            .unwrap_or_default(),
    );
    let original_reference = RwSignal::new(review.original_reference);
    let reviewed = RwSignal::new(review.reviewed);
    let archive_title = RwSignal::new(review.archive_title);
    let archive_basis = RwSignal::new(review.archive_basis);
    let archive_until = RwSignal::new(
        review
            .archive_until
            .map(|d| d.to_string())
            .unwrap_or_default(),
    );
    let minimum = Memo::new(move |_| {
        let parse = |value: String| {
            time::Date::parse(
                &value,
                &time::format_description::well_known::Iso8601::DEFAULT,
            )
            .ok()
        };
        let date = parse(posted.get()).ok_or("Vyplňte datum vyvěšení.")?;
        crate::notice_policy::NoticeReview {
            rule: rule.get(),
            not_before: parse(not_before.get()),
            decision_on: parse(decision_on.get()),
            ..Default::default()
        }
        .earliest(date)
    });
    let withdrawal_reason = RwSignal::new(String::new());
    let emergency = RwSignal::new(false);
    let slug = RwSignal::new(entry.slug.clone());
    let content = RwSignal::new(entry.content.clone());
    let published = RwSignal::new(entry.published.unwrap_or(false));
    let preview = RwSignal::new(false);
    let file_busy = RwSignal::new(false);
    let error = RwSignal::new(None);
    let navigate = use_navigate();
    let save = Action::new_local(move |_: &()| {
        let navigate = navigate.clone();
        let payload = match kind {
            Kind::Notice => {
                json!({"title":name.get_untracked(),"description":description.get_untracked(),"reference_number":optional(reference.get_untracked()),"issuer":optional(issuer.get_untracked()),"category_id":category.get_untracked().parse::<i64>().ok(),"published_on":optional(posted.get_untracked()),"withdraw_on":if mode.get_untracked()=="date"{optional(until.get_untracked())}else{None},"duration_days":if mode.get_untracked()=="days"{duration.get_untracked().parse::<i64>().ok()}else{None},"unlimited":mode.get_untracked()=="unlimited","retain_attachments":retention.get_untracked(),"review":{"rule":rule.get_untracked(),"legal_basis":legal_basis.get_untracked(),"not_before":optional(not_before.get_untracked()),"decision_on":optional(decision_on.get_untracked()),"original_reference":original_reference.get_untracked(),"reviewed":reviewed.get_untracked(),"archive_title":archive_title.get_untracked(),"archive_basis":archive_basis.get_untracked(),"archive_until":optional(archive_until.get_untracked())}})
            }
            Kind::Document => {
                json!({"title":name.get_untracked(),"description":description.get_untracked()})
            }
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
    let transition = Action::new_local(move |operation: &String| {
        let operation = operation.clone();
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/{}/{id}/{operation}", kind.api()),
                if operation=="withdraw" {Some(json!({"reason":withdrawal_reason.get_untracked(),"emergency":emergency.get_untracked()}))}else{None},
            )
            .await
            {
                Ok(_) => {
                    ctx.dirty.set(false);
                    if let Some(site)=site {site.refetch();}
                    ctx.changed(if operation == "publish" {
                        "Zveřejnění je uložené. Oznámení dostanou ověření odběratelé při vyvěšení."
                    } else {
                        "Dokument je v archivu."
                    });
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let busy = Signal::derive(move || {
        save.pending().get() || transition.pending().get() || file_busy.get()
    });
    let action_disabled = Signal::derive(move || busy.get() || ctx.dirty.get());
    let heading = if id == 0 {
        kind.create_label().to_string()
    } else {
        entry.title.clone()
    };
    let public_url = match kind {
        Kind::Notice => format!("/uredni-deska/{id}"),
        Kind::Document => format!("/dokumenty/{id}"),
        Kind::Page => format!("/stranky/{}", entry.slug),
    };
    let public_visible = kind == Kind::Page && entry.published == Some(true)
        || kind != Kind::Page
            && !draft
            && status != "scheduled"
            && !(kind == Kind::Document && archived);
    let notice_retention = entry.retain_attachments;
    view! {
        <Title text=format!("{heading} · Správa Vyskře")/>
        <div class="admin-editor-back"><AdminLink href=kind.path() label=kind.title() icon="back"/><span>"/"</span><span>{if id==0{"Nový záznam".into()}else{format!("#{id:04}")}}</span></div>
        <div class="admin-editor-heading"><h1>{heading}</h1><Status value=status.clone()/></div>
        <ErrorMessage error/>
        <div class="admin-editor-grid"><div class="admin-editor-content">
            <form id="content-editor" on:submit=move |ev|{ev.prevent_default();if !busy.get_untracked()&&!locked{save.dispatch(());}}>
                <fieldset disabled=move ||busy.get()||locked>
                    <section class="admin-panel admin-form-section"><div class="admin-panel-heading"><h2>"Základní údaje"</h2><span class="admin-caption">"* Povinné údaje"</span></div>
                        <Field id="entry-name" label="Název" value=name required=true maxlength=if kind==Kind::Page{"200"}else{"300"}/>
                        {if kind==Kind::Page{view!{
                            <Field id="page-slug" label="Adresa stránky" value=slug required=true maxlength="80" help="Například spolky. Malá písmena bez diakritiky, číslice a pomlčky."/>
                            <p class="admin-url">{move||format!("/stranky/{}",slug.get())}</p>
                            <TextArea id="page-content" label="Obsah stránky" value=content required=true maxlength="100000" rows="16"/>
                            <p class="admin-caption">"Formátování: ## nadpis, **tučně**, - položka, [odkaz](/kontakt). HTML a obrázky jsou vypnuté."</p>
                            <button class="button secondary" type="button" on:click=move |_|preview.update(|v|*v=!*v)>{move||if preview.get(){"Zavřít náhled"}else{"Náhled textu"}}</button>
                            <Show when=move||preview.get()><section class="admin-page-preview" aria-label="Náhled textu stránky"><h2>{move||name.get()}</h2><div class="markdown-content" inner_html=move||crate::markdown::render(&content.get())></div></section></Show>
                        }.into_any()}else{view!{
                            <TextArea id="entry-description" label="Popis" value=description/>
                            {(kind==Kind::Notice).then(move ||view!{
                                <div class="admin-form-row"><label class="admin-field" for="notice-category"><span>"Kategorie"</span><select id="notice-category" prop:value=move||category.get() on:change=move |ev|{category.set(event_target_value(&ev));ctx.dirty.set(true);}><option value="">"Bez kategorie"</option>{categories.into_iter().map(|c|view!{<option value=c.id.to_string()>{c.name}</option>}).collect_view()}</select></label><Field id="notice-reference" label="Číslo jednací" value=reference maxlength="200"/></div>
                                <Field id="notice-issuer" label="Původce dokumentu" value=issuer maxlength="300"/>
                            })}
                        }.into_any()}}
                    </section>
                    {(kind==Kind::Notice).then(move ||view!{
                        <section class="admin-panel admin-form-section"><div class="admin-panel-heading"><h2>"Vyvěšení a archiv"</h2><Icon name="calendar"/></div>
                            <label class="admin-field" for="notice-rule"><span>"Pravidlo zveřejnění"</span><select id="notice-rule" prop:value=move||rule.get() on:change=move|ev|{rule.set(event_target_value(&ev));ctx.dirty.set(true);}><option value="informational">"Informativní oznámení"</option><option value="public_notice">"Doručování veřejnou vyhláškou (§ 25)"</option><option value="property_intent">"Majetkový záměr obce"</option><option value="council_meeting">"Oznámení zasedání zastupitelstva"</option><option value="custom">"Jiný právní režim"</option></select></label>
                            <Field id="notice-basis" label="Právní důvod a pravidlo lhůty" value=legal_basis maxlength="2000" help="Odpovědná osoba určí použitelné pravidlo. Výchozích 15 dní není univerzální zákonná lhůta."/>
                            <Field id="notice-minimum" label="Nejdřívější přípustné sejmutí" kind="date" value=not_before help="U jiného právního režimu povinné. U ostatních může prodloužit vypočtené minimum."/>
                            <Show when=move||matches!(rule.get().as_str(),"property_intent"|"council_meeting")><Field id="notice-decision" label="Datum projednání nebo zasedání" kind="date" value=decision_on required=true/></Show>
                            <Field id="notice-original" label="Uložení originálu ve spisové službě" value=original_reference maxlength="2000" help="Interní identifikátor, nezveřejňuje se. Originál musí být uložen i mimo web."/>
                            <Field id="notice-posted" label="Datum vyvěšení" kind="date" value=posted required=true readonly=status=="published" help="Termíny se řídí časem Europe/Prague."/>
                            <label class="admin-field" for="notice-expiry"><span>"Kdy dokument sejmout"</span><select id="notice-expiry" prop:value=move||mode.get() on:change=move|ev|{mode.set(event_target_value(&ev));ctx.dirty.set(true);}><option value="days">"Za počet dní od vyvěšení"</option><option value="date">"V konkrétní datum"</option><option value="unlimited">"Bez časového omezení"</option></select></label>
                            <Show when=move||mode.get()=="days"><label class="admin-field" for="notice-days"><span>"Počet dní *"</span><input id="notice-days" type="number" min="1" max="3650" required prop:value=move||duration.get() on:input=move|ev|{duration.set(event_target_value(&ev));ctx.dirty.set(true);}/><small>"Výchozí je 15 dní. Zvolené právní pravidlo může vyžadovat delší lhůtu. Uveďte odpovídající datum nebo počet dní."</small></label></Show>
                            <Show when=move||mode.get()=="date"><Field id="notice-until" label="Datum sejmutí" kind="date" value=until required=true help="První den, kdy už dokument na desce nevisí."/></Show>
                            <p class="field-note" role="status">{move||match minimum.get(){Ok(Some(day))=>format!("Podle zvoleného pravidla lze sejmout nejdříve {}.",api::date(&day.to_string())),Ok(None)=>"Lhůtu informativního oznámení určuje správce.".into(),Err(e)=>e.into()}}</p>
                            <Show when=move||minimum.get().is_ok_and(|d|d.is_some())><button type="button" class="button secondary" on:click=move |_|{if let Ok(Some(day))=minimum.get_untracked(){until.set(day.to_string());mode.set("date".into());ctx.dirty.set(true);}}>"Použít vypočtený termín"</button></Show>
                            <label class="admin-check"><input type="checkbox" prop:checked=move||retention.get() on:change=move|ev|{retention.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span><strong>"Zachovat přílohy v archivu"</strong><small>"Ve výchozím stavu se jejich obsah při sejmutí odstraní."</small></span></label>
                            <Field id="notice-archive-title" label="Veřejný název v archivu" value=archive_title maxlength="300" help="Bez osobních údajů. Prázdné pole použije jen číslo záznamu. Původní popis se po sejmutí nezobrazuje."/>
                            <Show when=move||retention.get()><Field id="notice-archive-basis" label="Důvod zveřejnění příloh v archivu" value=archive_basis required=true maxlength="2000"/><Field id="notice-archive-until" label="Přílohy v archivu do (výlučně)" kind="date" value=archive_until required=true/></Show>
                            <label class="admin-check"><input type="checkbox" prop:checked=move||reviewed.get() on:change=move|ev|{reviewed.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span>"Zkontroloval/a jsem lhůtu, osobní údaje, přístupnost příloh a uložení originálu mimo web."</span></label>
                            <div class="admin-note"><Icon name="archive"/><p>"Evidenční záznam zůstává vždy. Soubor na webu nenahrazuje úřední originál ve spisové službě."</p></div>
                        </section>
                    })}
                    {(kind==Kind::Page).then(move||view!{<section class="admin-panel admin-form-section"><h2>"Dostupnost stránky"</h2><label class="admin-check"><input type="checkbox" prop:checked=move||published.get() on:change=move|ev|{published.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span><strong>"Zveřejnit na webu"</strong><small>"Změna se projeví po uložení. Bez zaškrtnutí zůstává stránka konceptem."</small></span></label></section>})}
                </fieldset>
            </form>
            {(kind==Kind::Page&&id>0).then(move||view!{<PageHistory id name content busy/>})}
            {(kind==Kind::Notice&&entry.published_at.is_some()).then(move||view!{<IncidentForm id/>})}
            {(kind!=Kind::Page).then(move||view!{<Files kind id files=entry.attachments editable=draft busy working=file_busy reload/>})}
        </div>
        <aside class="admin-editor-sidebar"><section class="admin-panel admin-save-panel"><h2>"Uložení a zveřejnění"</h2>
            {if locked{view!{<div class="admin-note"><Icon name="lock"/><p>{if archived{"Archivovaný záznam je pouze ke čtení."}else{"Zveřejněný dokument je pouze ke čtení."}}</p></div>}.into_any()}else{view!{
                <p>{if id==0&&kind!=Kind::Page{"Nejdříve uložte koncept. Potom přidejte přílohy a dokument zveřejněte."}else{"Uložte změny před dalšími kroky."}}</p>
                <button class="button primary" type="submit" form="content-editor" disabled=move||busy.get()>{move||if save.pending().get(){"Ukládám…"}else if id==0&&kind!=Kind::Page{"Uložit koncept"}else{"Uložit změny"}}</button>
                <span class="admin-save-state" role="status">{move||if ctx.dirty.get(){"Máte neuložené změny"}else if id==0{"Nový koncept"}else{"Všechny změny jsou uložené"}}</span>
            }.into_any()}}
            {(id>0&&kind!=Kind::Page&&draft).then(move||view!{<hr/><ConfirmButton label="Zveřejnit" title="Zveřejnit dokument?" description=if kind==Kind::Notice{"Dokument se zveřejní podle data vyvěšení. Budoucí datum znamená naplánované zveřejnění. Ověření odběratelé dostanou oznámení při zveřejnění."}else{"Dokument i jeho přílohy budou dostupné na webu. Ověření odběratelé dostanou oznámení."} disabled=action_disabled on_confirm=Callback::new(move|()|{transition.dispatch("publish".into());})/>})}
            {(id>0&&kind==Kind::Notice&&!draft&&!archived).then(move||view!{<hr/><label class="admin-field" for="withdrawal-reason"><span>"Důvod ručního sejmutí"</span><input id="withdrawal-reason" maxlength="2000" prop:value=move||withdrawal_reason.get() on:input=move|ev|withdrawal_reason.set(event_target_value(&ev))/></label><label class="admin-check"><input type="checkbox" prop:checked=move||emergency.get() on:change=move|ev|emergency.set(event_target_checked(&ev))/><span>"Mimořádné předčasné sejmutí kvůli incidentu"</span></label><ConfirmButton label="Sejmout do archivu" title="Sejmout dokument z úřední desky?" description=if notice_retention{"Záznam se přesune do archivu. Přílohy zůstanou veřejně dostupné."}else{"Záznam zůstane v archivu. Obsah příloh se nenávratně odstraní podle uloženého nastavení."} danger=true disabled=action_disabled on_confirm=Callback::new(move|()|{transition.dispatch("withdraw".into());})/>})}
            {(id>0&&kind==Kind::Document&&!draft&&!archived).then(move||view!{<hr/><ConfirmButton label="Archivovat dokument" title="Archivovat dokument?" description="Dokument a jeho přílohy přestanou být veřejně dostupné." danger=true disabled=action_disabled on_confirm=Callback::new(move|()|{transition.dispatch("archive".into());})/>})}
            {public_visible.then(move||view!{<a href=public_url target="_blank" rel="noopener" class="admin-public-link">"Otevřít na webu"<Icon name="external"/></a>})}
            {(id>0&&kind==Kind::Notice).then(move||view!{<a class="admin-public-link" href=format!("/api/v1/admin/notices/{id}/evidence") download="doklad-vyveseni.json">"Stáhnout doklad vyvěšení"<Icon name="paper"/></a>})}
            <p class="admin-caption"><Icon name="history"/>"Změny se zapisují do auditu."</p>
        </section></aside></div>
    }
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
