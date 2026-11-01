use super::{
    AdminContext,
    api::{self, Entry},
    ui::*,
};
use crate::app::components::Icon;
use leptos::prelude::*;
use serde_json::json;

#[component]
pub(super) fn DocumentHelp(label: &'static str, text: &'static str) -> impl IntoView {
    let details = NodeRef::<leptos::html::Details>::new();
    view! {<details class="document-help" node_ref=details on:keydown=move|ev|{if ev.key()=="Escape"{if let Some(details)=details.get(){details.set_open(false);}}}>
        <summary aria-label=format!("Nápověda: {label}")><Icon name="info"/></summary>
        <p>{text}</p>
    </details>}
}

#[component]
pub fn DocumentActions(
    entry: Entry,
    board_enabled: RwSignal<bool>,
    publishing: RwSignal<bool>,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let id = entry.id;
    let ctx = expect_context::<AdminContext>();
    let disabled = Signal::derive(move || busy.get() || ctx.dirty.get());
    let can_post = !matches!(entry.board_status.as_str(), "published" | "scheduled")
        && entry.lock_state != "review"
        && !entry.access_blocked;
    let public = entry.public_access && !entry.access_blocked;
    let scheduled = entry.board_status == "scheduled";
    let editable_draft = entry.lock_state == "draft" && entry.status() == "draft";
    let prepared = editable_draft && entry.board_enabled;
    let can_publish = editable_draft && !entry.board_enabled && !public && !entry.access_blocked;
    let posting_entry = StoredValue::new(entry.clone());
    view! {
        <Show when=move||!ctx.dirty.get() fallback=move||view!{
            <section class="admin-panel admin-form-section" role="status">
                <h2>"Nejdříve uložte změny"</h2>
                <p>{move||if board_enabled.get(){"Uložte změny v panelu Uložení a zveřejnění. Potom se zobrazí potvrzení vyvěšení s aktuálním obsahem a nastavením."}else{"Uložte změny v panelu Uložení a zveřejnění. Potom můžete dokument zveřejnit."}}</p>
            </section>
        }>
            {can_publish.then(move||view!{<PublishDocument id publishing busy=disabled reload/>})}
            {(can_post&&prepared).then(move||view!{<section class="admin-panel admin-form-section"><h2>"Potvrzení vyvěšení"</h2><PostingForm entry=posting_entry.get_value() saved=true publishing busy=disabled reload/></section>})}
            {(can_post&&!editable_draft).then(move||view!{<details class="admin-panel admin-form-section"><summary>"Nové vyvěšení na úřední desce"</summary><p>"Vznikne nový záznam vyvěšení. Obsah, který už je uzamčený, zůstane stejný."</p><PostingForm entry=posting_entry.get_value() saved=false publishing busy=disabled reload/></details>})}
        </Show>
        {scheduled.then(move ||view!{<CancelSchedule id busy=disabled reload/>})}
        <PostingHistory document_id=id postings=entry.postings busy=disabled reload/>
        <DocumentHistory id/>
    }
}

#[component]
fn PublishDocument(
    id: i64,
    publishing: RwSignal<bool>,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let error = RwSignal::new(None);
    let publish = Action::new_local(move |_: &()| {
        publishing.set(true);
        async move {
            error.set(None);
            let result = api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/publish"),
                None,
            )
            .await;
            publishing.try_set(false);
            match result {
                Ok(_) if !reload.is_disposed() => {
                    ctx.dirty.set(false);
                    ctx.changed("Dokument je zveřejněný v katalogu Dokumenty.");
                    reload.update(|value| *value += 1);
                }
                Err(failure) => {
                    error.try_set(Some(failure));
                }
                _ => {}
            }
        }
    });
    view! {<section class="admin-panel admin-form-section"><h2>"Zveřejnění dokumentu"</h2><ErrorMessage error/>
        <p>"Dokument bude veřejně dostupný v katalogu Dokumenty."</p>
        <ConfirmButton label="Zveřejnit dokument" title="Zveřejnit dokument?" description="Zveřejní se uložený obsah a dostupné přílohy." disabled=Signal::derive(move||busy.get()||publish.pending().get()) on_confirm=Callback::new(move|()|{publish.dispatch(());})/>
    </section>}
}

#[component]
pub(super) fn BoardSettings(
    published_on: RwSignal<String>,
    duration_days: RwSignal<String>,
    retention: RwSignal<Option<bool>>,
    #[prop(default = false)] mark_dirty: bool,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let removal = Memo::new(move |_| {
        let day = time::Date::parse(
            &published_on.get(),
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .ok()?;
        let days = duration_days
            .get()
            .parse::<i64>()
            .ok()
            .filter(|days| *days > 0 && *days <= 3650)?;
        day.checked_add(time::Duration::days(days + 1))
            .map(|date| api::date(&date.to_string()))
    });
    view! {<div class="document-board-settings">
        <div class="admin-form-row">
            <div class="admin-field"><div class="document-field-label"><label for="document-posted">"Datum vyvěšení *"</label><DocumentHelp label="Datum vyvěšení" text="Dnešní datum vyvěsí dokument po potvrzení náhledu. Budoucí datum naplánuje vyvěšení. Datum nahrání souboru ani původního zveřejnění tím nenahrazujte."/></div><input id="document-posted" type="date" required prop:value=move||published_on.get() on:input=move|ev|{published_on.set(event_target_value(&ev));if mark_dirty{ctx.dirty.set(true);}}/></div>
            <div class="admin-field"><div class="document-field-label"><label for="document-days">"Počet celých dní vyvěšení *"</label><DocumentHelp label="Počet celých dní vyvěšení" text="Počet celých kalendářních dní po dni vyvěšení. Výpočet neurčuje zákonnou lhůtu. U doručování veřejnou vyhláškou zohledněte víkendy a svátky a případně počet dní prodlužte."/></div><input id="document-days" type="number" min="1" max="3650" required prop:value=move||duration_days.get() on:input=move|ev|{duration_days.set(event_target_value(&ev));if mark_dirty{ctx.dirty.set(true);}}/></div>
        </div>
        {move||removal.get().map(|date|view!{<p class="field-note">"Plánované sejmutí: "{date}". Automatické sejmutí proběhne nejdříve na začátku tohoto dne."</p>})}
        <div class="admin-field"><div class="document-field-label"><label for="document-attachment-access">"Dostupnost příloh po sejmutí *"</label><DocumentHelp label="Dostupnost příloh po sejmutí" text="Veřejné přílohy zůstanou ke stažení i v archivu. Při volbě pouze pro administrátory je po sejmutí stáhnou jen správci. Název a popis mohou zůstat veřejné, proto nesmějí obsahovat údaje, které mají být skryté. Tato volba sama neopravňuje ke zveřejnění osobních údajů a nemění uzamčení obsahu."/></div><select id="document-attachment-access" required prop:value=move||match retention.get(){Some(true)=>"public",Some(false)=>"private",None=>""} on:change=move|ev|{retention.set(match event_target_value(&ev).as_str(){"public"=>Some(true),"private"=>Some(false),_=>None});if mark_dirty{ctx.dirty.set(true);}}><option value="">"Vyberte dostupnost příloh"</option><option value="public">"Po sejmutí ponechat přílohy veřejné"</option><option value="private">"Po sejmutí přílohy pouze pro administrátory"</option></select></div>
    </div>}
}

#[component]
fn PostingForm(
    entry: Entry,
    saved: bool,
    publishing: RwSignal<bool>,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let id = entry.id;
    let ctx = expect_context::<AdminContext>();
    let error = RwSignal::new(None);
    let preview = RwSignal::new(None::<api::PublicationPreview>);
    let redacted_preview = RwSignal::new(None::<api::PublicationPreview>);
    let checked = RwSignal::new(false);
    let published_on = RwSignal::new(if saved {
        entry.published_on.unwrap_or_default()
    } else {
        String::new()
    });
    let duration_days = RwSignal::new(if saved {
        entry
            .duration_days
            .map(|days| days.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    });
    let retention = RwSignal::new(if saved {
        entry.retain_attachments
    } else {
        None
    });
    let anonymized = RwSignal::new(false);
    let redacted_id = RwSignal::new(String::new());
    let reason = RwSignal::new(String::new());
    let preview_generation = RwSignal::new(0u64);
    Effect::new(move |_| {
        let _ = (
            published_on.get(),
            duration_days.get(),
            retention.get(),
            anonymized.get(),
            redacted_id.get(),
            ctx.dirty.get(),
        );
        preview_generation.update(|generation| *generation = generation.wrapping_add(1));
        preview.set(None);
        redacted_preview.set(None);
        checked.set(false);
    });
    let prepare = Action::new_local(move |_: &()| {
        let requested_generation = preview_generation.get_untracked();
        let derived = anonymized.get_untracked();
        let derived_id = redacted_id
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|value| *value > 0 && *value != id);
        async move {
            if preview.is_disposed() {
                return;
            }
            error.set(None);
            preview.set(None);
            redacted_preview.set(None);
            checked.set(false);
            let result = api::get::<api::PublicationPreview>(&format!(
                "/api/v1/admin/documents/{id}/preview"
            ))
            .await;
            // Editing can replace this form while the request is in flight.
            if preview.is_disposed()
                || preview_generation.try_get_untracked() != Some(requested_generation)
                || ctx.dirty.try_get_untracked() != Some(false)
            {
                return;
            }
            match result {
                Ok(value) => {
                    if derived {
                        let Some(derived_id) = derived_id else {
                            error.set(Some(api::Failure {
                                status: 400,
                                message: "Vyberte ID samostatného anonymizovaného dokumentu."
                                    .into(),
                            }));
                            return;
                        };
                        let result = api::get::<api::PublicationPreview>(&format!(
                            "/api/v1/admin/documents/{derived_id}/preview"
                        ))
                        .await;
                        if preview.is_disposed()
                            || preview_generation.try_get_untracked() != Some(requested_generation)
                            || ctx.dirty.try_get_untracked() != Some(false)
                        {
                            return;
                        }
                        match result {
                            Ok(derived) => redacted_preview.set(Some(derived)),
                            Err(failure) => {
                                error.set(Some(failure));
                                return;
                            }
                        }
                    }
                    preview.set(Some(value));
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let publish = Action::new_local(move |_: &()| {
        let token = preview
            .get_untracked()
            .map(|value| value.token)
            .unwrap_or_default();
        let derived = anonymized.get_untracked();
        let explanation = reason.get_untracked();
        let payload = json!({
            "token":token,"published_on":published_on.get_untracked(),"duration_days":duration_days.get_untracked().parse::<i64>().ok(),
            "after_action":if derived{"anonymized"}else if retention.get_untracked()==Some(true){"retain"}else{"hide"},
            "redacted_document_id":if derived{redacted_id.get_untracked().parse::<i64>().ok()}else{None},
            "redacted_token":if derived{redacted_preview.get_untracked().map(|value|value.token)}else{None},
            "reason":if explanation.trim().is_empty(){"Vyvěšení schválené obsluhou po kontrole finálního náhledu.".to_string()}else{explanation}
        });
        publishing.set(true);
        async move {
            error.set(None);
            let result = api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/postings"),
                Some(payload),
            )
            .await;
            publishing.try_set(false);
            match result {
                Ok(_) if !reload.is_disposed() => {
                    preview.try_set(None);
                    redacted_preview.try_set(None);
                    checked.try_set(false);
                    ctx.dirty.set(false);
                    ctx.changed(
                        "Vyvěšení je potvrzené. Po skutečném vyvěšení je obsah nevratně uzamčen.",
                    );
                    reload.update(|value| *value += 1);
                }
                Err(failure) => {
                    preview.try_set(None);
                    redacted_preview.try_set(None);
                    checked.try_set(false);
                    error.try_set(Some(failure));
                }
                _ => {}
            }
        }
    });
    let disabled =
        Signal::derive(move || busy.get() || prepare.pending().get() || publish.pending().get());
    let complete = Signal::derive(move || {
        !published_on.get().is_empty()
            && duration_days
                .get()
                .parse::<i64>()
                .is_ok_and(|days| days > 0 && days <= 3650)
            && retention.get().is_some()
    });
    view! {<form class="document-posting-confirmation" on:submit=move|ev|{ev.prevent_default();if !disabled.get_untracked()&&complete.get_untracked()&&checked.get_untracked()&&preview.get_untracked().is_some(){publish.dispatch(());}}><ErrorMessage error/>
        <fieldset disabled=move||disabled.get()>
            {if saved{view!{<p>"Náhled použije uložený obsah, přílohy a následující nastavení vyvěšení."</p><dl class="document-facts"><div><dt>"Datum vyvěšení"</dt><dd>{move||api::date(&published_on.get())}</dd></div><div><dt>"Počet celých dní vyvěšení"</dt><dd>{move||duration_days.get()}</dd></div><div><dt>"Přílohy po sejmutí"</dt><dd>{move||match retention.get(){Some(true)=>"Zůstanou veřejné",Some(false)=>"Pouze pro administrátory",None=>"Volba chybí"}}</dd></div></dl>}.into_any()}else{view!{<BoardSettings published_on duration_days retention/>}.into_any()}}
            <details><summary>"Další možnosti vyvěšení"</summary>
                <label class="admin-field" for="posting-reason"><span>"Interní poznámka k vyvěšení (nepovinné)"</span><textarea id="posting-reason" maxlength="2000" prop:value=move||reason.get() on:input=move|ev|reason.set(event_target_value(&ev))></textarea></label>
                <label class="admin-check"><input type="checkbox" prop:checked=move||anonymized.get() on:change=move|ev|anonymized.set(event_target_checked(&ev))/><span>"Po sejmutí zpřístupnit samostatné anonymizované znění"</span></label>
                <Show when=move||anonymized.get()><label class="admin-field" for="posting-redacted"><span>"ID propojeného anonymizovaného dokumentu *"</span><input id="posting-redacted" type="number" min="1" required prop:value=move||redacted_id.get() on:input=move|ev|redacted_id.set(event_target_value(&ev))/><small>"Tato volba ukončí veřejnou dostupnost originálu a použije samostatnou schválenou kopii."</small></label></Show>
            </details>
            <button class="button secondary" type="button" disabled=move||!complete.get() on:click=move |_|{prepare.dispatch(());}>"Zkontrolovat finální náhled"</button>
            {move||preview.get().map(|value|view!{<section class="publication-preview" aria-label="Finální náhled dokumentu"><SnapshotContent preview=value/>
                {move||redacted_preview.get().map(|derived|view!{<section class="publication-preview" aria-label="Finální náhled anonymizovaného znění"><h3>"Samostatné anonymizované znění"</h3><SnapshotContent preview=derived/><p class="info-banner">"Anonymizované znění se při potvrzení také nevratně uzamkne. Jde o samostatnou veřejnou kopii, jejíž bajty tím nejsou označeny za původně vyvěšené."</p></section>})}
                <div class="document-field-label"><p class="info-banner"><Icon name="lock"/>"Po vyvěšení již obsah dokumentu nelze upravit."</p><DocumentHelp label="Uzamčení po vyvěšení" text="Uzamkne se název, text i všechny přílohy této podoby. Sejmutí dokumentu ani změna dostupnosti příloh podle zvoleného režimu obsah znovu neodemknou."/></div>
                <label class="admin-check"><input type="checkbox" prop:checked=move||checked.get() on:change=move|ev|checked.set(event_target_checked(&ev))/><span>"Zkontroloval/a jsem uvedený obsah a všechny přílohy."</span></label>
                <button class="button primary" type="submit" disabled=move||!checked.get()||disabled.get()>"Potvrdit vyvěšení této podoby"</button>
            </section>})}
        </fieldset>
    </form>}
}

#[component]
fn SnapshotContent(preview: api::PublicationPreview) -> impl IntoView {
    view! {
        <h3>{preview.document.title}</h3><p>{preview.document.description.unwrap_or_default()}</p>
        <dl class="document-facts">{preview.document.reference_number.map(|value|view!{<div><dt>"Číslo jednací"</dt><dd>{value}</dd></div>})}{preview.document.issuer.filter(|value|!value.trim().is_empty()).map(|value|view!{<div><dt>"Původce převzatého dokumentu"</dt><dd>{value}</dd></div>})}<div><dt>"Datum vytvoření dokumentu"</dt><dd>{crate::catalog::document_date(preview.document.document_date.as_deref().unwrap_or_default())}</dd></div></dl>
        <h4>"Přílohy v uloženém pořadí"</h4><ol>{preview.attachments.into_iter().map(|file|view!{<li>{if file.available{view!{<a href=format!("/api/v1/admin/attachments/{}",file.id) target="_blank" rel="noopener">{file.name}</a>}.into_any()}else{view!{<strong>{file.name}</strong><span>"Soubor není dostupný. Před vyvěšením vyžaduje ověření."</span>}.into_any()}}<small>{format!("{} B",file.size_bytes)}</small>{(!file.sha256.is_empty()).then(||view!{<code class="file-fingerprint">{file.sha256}</code>})}</li>}).collect_view()}</ol>
    }
}

#[component]
fn CancelSchedule(id: i64, busy: Signal<bool>, reload: RwSignal<u64>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let reason = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let cancel = Action::new_local(move |_: &()| {
        let payload = json!({"reason":reason.get_untracked(),"emergency":false,"expected_status":"scheduled"});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/notices/{id}/withdraw"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Plánované vyvěšení je zrušené. Přílohy zůstávají zachované a obsah, který již byl vyvěšen, zůstává uzamčený.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let disabled = Signal::derive(move || {
        busy.get() || cancel.pending().get() || reason.get().trim().is_empty()
    });
    view! {<section class="admin-panel admin-form-section"><h2>"Zrušit plánované vyvěšení"</h2><ErrorMessage error/>
        <label class="admin-field" for="cancel-schedule-reason"><span>"Důvod zrušení plánovaného vyvěšení"</span><textarea id="cancel-schedule-reason" maxlength="2000" prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))></textarea></label>
        <ConfirmButton label="Zrušit plánované vyvěšení" title="Zrušit plánované vyvěšení?" description="Přílohy zůstanou zachované. Pokud se dokument mezitím skutečně vyvěsil, zrušení plánu se neprovede." disabled danger=true on_confirm=Callback::new(move |()|{cancel.dispatch(());})/>
    </section>}
}

#[component]
fn PostingHistory(
    document_id: i64,
    postings: Vec<api::Posting>,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    view! {<section class="admin-panel admin-form-section"><h2>"Historie vyvěšení"</h2>
        {if postings.is_empty(){view!{<p>"Žádné evidované vyvěšení."</p>}.into_any()}else{view!{<ul class="posting-history">{postings.into_iter().map(|posting|view!{<PostingRecord document_id posting busy reload/>}).collect_view()}</ul>}.into_any()}}
    </section>}
}

#[component]
fn PostingRecord(
    document_id: i64,
    posting: api::Posting,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let id = posting.id;
    let ctx = expect_context::<AdminContext>();
    let error = RwSignal::new(None);
    let reason = RwSignal::new(String::new());
    let emergency = RwSignal::new(false);
    let plan = RwSignal::new(posting.withdraw_on.clone().unwrap_or_default());
    let active = posting.origin == "local"
        && posting.withdrawn_at.is_none()
        && posting.withdrawn_on.is_none();
    let action = Action::new_local(move |operation: &String| {
        let operation = operation.clone();
        let payload = if operation == "plan" {
            json!({"withdraw_on":optional(plan.get_untracked()),"reason":reason.get_untracked()})
        } else {
            json!({"reason":reason.get_untracked(),"emergency":emergency.get_untracked()})
        };
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/postings/{id}/{operation}"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Provozní událost byla zapsána do historie.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let disabled = Signal::derive(move || {
        busy.get() || action.pending().get() || reason.get().trim().is_empty()
    });
    let actual_withdrawal = crate::catalog::document_event_date(
        posting.withdrawn_at.as_deref(),
        posting.withdrawn_on.as_deref(),
    );
    view! {<li><h3>{format!("Vyvěšení #{id}")}</h3><p>{if posting.origin=="local" {"Místní vyvěšení"}else{"Převzatá historie zdroje"}}</p>
        <dl class="document-facts"><div><dt>"Vyvěšeno"</dt><dd>{crate::catalog::document_event_date(posting.published_at.as_deref(),posting.published_on.as_deref())}</dd></div><div><dt>"Plánované sejmutí"</dt><dd>{posting.withdraw_on.as_deref().map(api::date).unwrap_or_else(||"Není určeno".into())}</dd></div><div><dt>"Skutečné sejmutí"</dt><dd>{actual_withdrawal}</dd></div></dl>
        <p class="field-note">{match posting.after_action.as_str(){"retain"=>"Po sejmutí zůstávají přílohy veřejně dostupné.","anonymized"=>"Po sejmutí se zpřístupní samostatné anonymizované znění.","hide"=>"Po sejmutí jsou přílohy dostupné pouze administrátorům.",_=>"Režim po sejmutí vyžaduje ověření."}}</p>
        {active.then(move ||view!{<details><summary>"Zaznamenat sejmutí nebo změnit plán"</summary><ErrorMessage error/>
            <label class="admin-field" for=format!("posting-reason-{id}")><span>"Interní důvod *"</span><textarea id=format!("posting-reason-{id}") maxlength="2000" prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))></textarea></label>
            <label class="admin-check"><input type="checkbox" prop:checked=move ||emergency.get() on:change=move |ev|emergency.set(event_target_checked(&ev))/><span>"Předčasné sejmutí kvůli incidentu"</span></label>
            <div class="admin-field"><div class="document-field-label"><label for=format!("posting-plan-{id}")>"Nový plánovaný termín"</label><DocumentHelp label="Změna plánovaného termínu" text="Vyberte datum, na jehož začátku lze dokument nejdříve sejmout. Musí zůstat zachována potřebná doba vyvěšení včetně případného posunu kvůli víkendu nebo svátku. Změna potřebuje důvod a zachová původní plán v historii."/></div><input id=format!("posting-plan-{id}") type="date" prop:value=move ||plan.get() on:input=move |ev|plan.set(event_target_value(&ev))/></div>
            <div class="admin-actions"><button class="button secondary" type="button" disabled=move ||disabled.get()||plan.get().is_empty() on:click=move |_|{if !disabled.get_untracked()&&!plan.get_untracked().is_empty(){action.dispatch("plan".into());}}>"Změnit plán"</button><ConfirmButton label="Zaznamenat sejmutí" title="Zaznamenat skutečné sejmutí?" description="Sejmutí se zapíše jako nová událost. Dostupnost příloh se upraví podle uložené výslovné volby. Správci si uchované přílohy mohou dál stáhnout a obsah zůstane uzamčený." disabled danger=true on_confirm=Callback::new(move |()|{action.dispatch("withdraw".into());})/></div>
        </details>})}
        <EvidenceCorrection document_id posting_id=id busy reload/>
    </li>}
}

#[component]
fn EvidenceCorrection(
    document_id: i64,
    posting_id: i64,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let field = RwSignal::new("started_on".to_string());
    let corrected_date = RwSignal::new(String::new());
    let reason = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let correct = Action::new_local(move |_: &()| {
        let payload = json!({"posting_id":posting_id,"field":field.get_untracked(),"corrected_date":optional(corrected_date.get_untracked()),"reason":reason.get_untracked()});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/documents/{document_id}/event-corrections"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Oprava evidenčního údaje je zapsaná jako dodatková událost. Původní hodnota zůstala zachována.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    view! {<details><summary>"Opravit chybný evidenční údaj"</summary><form on:submit=move |ev|{ev.prevent_default();if !busy.get_untracked()&&!correct.pending().get_untracked(){correct.dispatch(());}}><ErrorMessage error/>
        <p>"Tento úkon zaznamená opravu doloženého údaje se zachováním původní hodnoty. Není novým vyvěšením ani sejmutím."</p>
        <label class="admin-field" for=format!("evidence-field-{posting_id}")><span>"Opravovaný údaj"</span><select id=format!("evidence-field-{posting_id}") prop:value=move ||field.get() on:change=move |ev|field.set(event_target_value(&ev))><option value="started_on">"Evidované datum vyvěšení"</option><option value="removed_on">"Evidované datum skutečného sejmutí"</option></select></label>
        <label class="admin-field" for=format!("evidence-date-{posting_id}")><span>"Opravené datum"</span><input id=format!("evidence-date-{posting_id}") type="date" prop:value=move ||corrected_date.get() on:input=move |ev|corrected_date.set(event_target_value(&ev))/><small>"Prázdné pole výslovně zaznamená, že datum není doloženo."</small></label>
        <label class="admin-field" for=format!("evidence-reason-{posting_id}")><span>"Interní vysvětlení a podklad opravy *"</span><textarea id=format!("evidence-reason-{posting_id}") required maxlength="2000" prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))></textarea></label>
        <button class="button secondary" type="submit" disabled=move ||busy.get()||correct.pending().get()>"Zapsat dodatkovou opravu"</button>
    </form></details>}
}

#[component]
fn DocumentHistory(id: i64) -> impl IntoView {
    let open = RwSignal::new(false);
    let offset = RwSignal::new(0usize);
    let history = LocalResource::new(move || {
        let open = open.get();
        let offset = offset.get();
        async move {
            if open {
                api::get::<Vec<serde_json::Value>>(&format!(
                    "/api/v1/admin/documents/{id}/history?limit=21&offset={offset}"
                ))
                .await
            } else {
                Ok(vec![])
            }
        }
    });
    view! {<details class="admin-panel admin-form-section" on:toggle=move |_|{open.set(true);}><summary>"Zobrazit historii a interní audit"</summary><p>"Interní důvody a zdrojové změny nejsou součástí veřejného detailu."</p>
        <Suspense fallback=Pending>{move ||history.get().map(|result|match result{
            Ok(items)=>{let count=items.len();view!{<ul class="document-audit">{items.into_iter().take(20).map(|item|{
                let operation=item["operation"].as_str().or(item["kind"].as_str()).unwrap_or_default();
                let time=item["occurred_at"].as_str().unwrap_or_default().to_owned();
                let label=event_label(operation);
                let reason=item["reason"].as_str().unwrap_or_default().to_owned();
                let author=item["actor_id"].as_i64().map(|id|format!("Správce #{id}")).unwrap_or_else(||"Systém / import".into());
                view!{<li><strong>{label}</strong><time>{crate::catalog::document_event_date(Some(&time),None)}</time><p>{author}</p>{(!reason.is_empty()).then(||view!{<p>{reason}</p>})}<details><summary>"Podrobnosti změny"</summary><pre>{serde_json::to_string_pretty(&item["payload"]).unwrap_or_default()}</pre></details></li>}
            }).collect_view()}</ul><Pager offset count/>}.into_any()},
            Err(error)=>view!{<FailureView error/>}.into_any(),
        })}</Suspense>
    </details>}
}

fn event_label(kind: &str) -> &'static str {
    match kind {
        "posted" => "Skutečné vyvěšení",
        "removed" => "Skutečné sejmutí",
        "posting_scheduled" => "Naplánované vyvěšení",
        "posting_review" => "Kontrola před vyvěšením",
        "schedule_cancelled" => "Zrušené plánované vyvěšení",
        "schedule_missed" => "Zmeškané plánované vyvěšení",
        "plan_changed" => "Změna budoucího plánu",
        "visibility_changed" => "Změna veřejné dostupnosti",
        "topic_changed" | "catalogued" => "Tematické zařazení",
        "related_draft_created" => "Vytvořen navazující koncept",
        "relation_added" => "Připojen navazující dokument",
        "evidence_corrected" => "Dodatková oprava evidenčního údaje",
        "migration_captured" => "Podoba zachycená při migraci",
        "source_captured" => "Podoba převzatá ze zdroje",
        "import_revision_accepted" => "Přijetí zdrojové revize jako nového konceptu",
        "import_revision_rejected" => "Zamítnutí zdrojové revize",
        _ => "Provozní událost",
    }
}

fn optional(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}
