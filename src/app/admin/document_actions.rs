use super::{
    AdminContext,
    api::{self, Entry},
    ui::*,
};
use crate::app::components::Icon;
use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use serde_json::json;

#[component]
pub fn DocumentActions(entry: Entry, busy: Signal<bool>, reload: RwSignal<u64>) -> impl IntoView {
    let id = entry.id;
    let ctx = expect_context::<AdminContext>();
    let error = RwSignal::new(None);
    let reason = RwSignal::new(String::new());
    let correction_kind = RwSignal::new("correction".to_string());
    let navigate = use_navigate();
    let correction = Action::new_local(move |_: &()| {
        let navigate = navigate.clone();
        let payload =
            json!({"kind":correction_kind.get_untracked(),"reason":reason.get_untracked()});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/corrections"),
                Some(payload),
            )
            .await
            {
                Ok(value) => {
                    if let Some(new_id) = value["id"].as_i64() {
                        ctx.dirty.set(false);
                        ctx.changed("Nový navazující koncept je připraven. Jeho zveřejnění ani vyvěšení zatím neproběhlo.");
                        navigate(&format!("/admin/dokumenty/{new_id}"), Default::default());
                    }
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let visibility_reason = RwSignal::new(String::new());
    let visibility = Action::new_local(move |public: &bool| {
        let payload = json!({"public_access":public,"reason":visibility_reason.get_untracked()});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/visibility"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Veřejná dostupnost byla změněna. Obsah a historie dokumentu zůstávají zachované.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let disabled = Signal::derive(move || {
        busy.get() || ctx.dirty.get() || correction.pending().get() || visibility.pending().get()
    });
    let can_post = !matches!(entry.board_status.as_str(), "published" | "scheduled")
        && entry.lock_state != "review"
        && !entry.access_blocked;
    let public = entry.public_access && !entry.access_blocked;
    let scheduled = entry.board_status == "scheduled";
    view! {
        <section class="admin-panel admin-form-section document-operations"><h2>"Provozní úkony"</h2><ErrorMessage error/>
            <p>"Tyto úkony nemění uložený obsah ani přílohy dokumentu."</p>
            <details><summary>"Vytvořit opravu nebo navazující dokument"</summary>
                <form on:submit=move |ev| {ev.prevent_default();if !disabled.get_untracked(){correction.dispatch(());}}>
                    <label class="admin-field" for="correction-kind"><span>"Vazba nového záznamu"</span><select id="correction-kind" prop:value=move ||correction_kind.get() on:change=move |ev| correction_kind.set(event_target_value(&ev))><option value="correction">"Oprava"</option><option value="supplement">"Doplnění"</option><option value="replacement">"Nahrazení"</option><option value="anonymized">"Veřejné anonymizované znění"</option></select></label>
                    <label class="admin-field" for="correction-reason"><span>"Interní důvod"</span><textarea id="correction-reason" required maxlength="2000" prop:value=move ||reason.get() on:input=move |ev| reason.set(event_target_value(&ev))></textarea></label>
                    <p class="field-note">"Vznikne nový koncept s vlastním ID a vlastními přílohami. Původní dokument zůstane zachován. Nový záznam není automaticky vyvěšen a sám nezahajuje novou lhůtu."</p>
                    <button type="submit" class="button secondary" disabled=move ||disabled.get()>"Vytvořit navazující koncept"</button>
                </form>
            </details>
            <details><summary>{if public {"Ukončit veřejné zpřístupnění"} else {"Veřejná dostupnost"}}</summary>
                <label class="admin-field" for="visibility-reason"><span>"Interní důvod změny dostupnosti"</span><textarea id="visibility-reason" maxlength="2000" prop:value=move ||visibility_reason.get() on:input=move |ev|visibility_reason.set(event_target_value(&ev))></textarea></label>
                <p class="field-note">"Důvod zůstane v interním auditu. Stažení znepřístupní detail i přímé odkazy na přílohy a neodemkne obsah."</p>
                <ConfirmButton label=if public {"Ukončit veřejné zpřístupnění"} else {"Zpřístupnit v katalogu"} title=if public {"Ukončit veřejné zpřístupnění?"} else {"Zpřístupnit dokument v katalogu?"} description=if public {"Dokument nebude veřejně dostupný. Jeho obsah, vyvěšení a audit zůstanou zachovány."} else {"Zveřejní se tento dokument a jeho dostupné přílohy. Tento úkon není vyvěšením na úřední desce."} danger=public disabled=Signal::derive(move ||disabled.get() ||visibility_reason.get().trim().is_empty()) on_confirm=Callback::new(move |()| {visibility.dispatch(!public);})/>
            </details>
            <AttachRelation id busy=disabled reload/>
            <TopicForm id selected=entry.category_id busy=disabled reload/>
        </section>
        {can_post.then(move ||view!{<PostingForm id busy=disabled reload/>})}
        {scheduled.then(move ||view!{<CancelSchedule id busy=disabled reload/>})}
        <PostingHistory document_id=id postings=entry.postings busy=disabled reload/>
        <DocumentHistory id/>
    }
}

#[component]
fn PostingForm(id: i64, busy: Signal<bool>, reload: RwSignal<u64>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let error = RwSignal::new(None);
    let preview = RwSignal::new(None::<api::PublicationPreview>);
    let redacted_preview = RwSignal::new(None::<api::PublicationPreview>);
    let checked = RwSignal::new(false);
    let published_on = RwSignal::new(String::new());
    let withdraw_on = RwSignal::new(String::new());
    let after_action = RwSignal::new(String::new());
    let public_until = RwSignal::new(String::new());
    let redacted_id = RwSignal::new(String::new());
    let reason = RwSignal::new(String::new());
    let rule = RwSignal::new(String::new());
    let legal_basis = RwSignal::new(String::new());
    let original_reference = RwSignal::new(String::new());
    let not_before = RwSignal::new(String::new());
    let decision_on = RwSignal::new(String::new());
    let reviewed = RwSignal::new(false);
    let prepare = Action::new_local(move |_: &()| async move {
        error.set(None);
        preview.set(None);
        redacted_preview.set(None);
        checked.set(false);
        match api::get::<api::PublicationPreview>(&format!("/api/v1/admin/documents/{id}/preview"))
            .await
        {
            Ok(value) => {
                if after_action.get_untracked() == "anonymized" {
                    let Some(derived_id) = redacted_id
                        .get_untracked()
                        .parse::<i64>()
                        .ok()
                        .filter(|value| *value > 0 && *value != id)
                    else {
                        error.set(Some(api::Failure {
                            status: 400,
                            message: "Vyberte ID samostatného anonymizovaného dokumentu.".into(),
                        }));
                        return;
                    };
                    match api::get::<api::PublicationPreview>(&format!(
                        "/api/v1/admin/documents/{derived_id}/preview"
                    ))
                    .await
                    {
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
    });
    let publish = Action::new_local(move |_: &()| {
        let token = preview.get_untracked().map(|v| v.token).unwrap_or_default();
        let anonymized = after_action.get_untracked() == "anonymized";
        let redacted_token = anonymized
            .then(|| redacted_preview.get_untracked().map(|v| v.token))
            .flatten();
        let redacted_document_id = anonymized
            .then(|| redacted_id.get_untracked().parse::<i64>().ok())
            .flatten();
        let payload = json!({"token":token,"published_on":published_on.get_untracked(),"withdraw_on":optional(withdraw_on.get_untracked()),"after_action":after_action.get_untracked(),"public_until":optional(public_until.get_untracked()),"redacted_document_id":redacted_document_id,"redacted_token":redacted_token,"reason":reason.get_untracked(),"review":{"rule":rule.get_untracked(),"legal_basis":legal_basis.get_untracked(),"original_reference":original_reference.get_untracked(),"not_before":optional(not_before.get_untracked()),"decision_on":optional(decision_on.get_untracked()),"reviewed":reviewed.get_untracked()}});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/postings"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    preview.set(None);
                    redacted_preview.set(None);
                    ctx.changed(
                        "Úkon vyvěšení je uložen. Obsah je po skutečném vyvěšení nevratně uzamčen.",
                    );
                    reload.update(|v| *v += 1);
                }
                Err(failure) => {
                    // Every failed attempt requires another review of server state.
                    preview.set(None);
                    redacted_preview.set(None);
                    checked.set(false);
                    error.set(Some(failure));
                }
            }
        }
    });
    let disabled =
        Signal::derive(move || busy.get() || prepare.pending().get() || publish.pending().get());
    view! {<section class="admin-panel admin-form-section"><h2>"Nové vyvěšení na úřední desce"</h2><ErrorMessage error/>
        <p>"Vyvěšení se eviduje samostatně. Opětovné vyvěšení zachová stejný uzamčený obsah a přidá nový záznam historie."</p>
        <form on:submit=move |ev|{ev.prevent_default();if !disabled.get_untracked() && checked.get_untracked() && preview.get_untracked().is_some(){publish.dispatch(());}}>
            <fieldset disabled=move ||disabled.get()>
                <div class="admin-form-row"><label class="admin-field" for="posting-date"><span>"Datum místního vyvěšení *"</span><input id="posting-date" type="date" required prop:value=move ||published_on.get() on:input=move |ev|published_on.set(event_target_value(&ev))/></label>
                <label class="admin-field" for="posting-plan"><span>"Plánované sejmutí"</span><input id="posting-plan" type="date" prop:value=move ||withdraw_on.get() on:input=move |ev|withdraw_on.set(event_target_value(&ev))/><small>"Prázdné pole znamená bez předem určeného konce."</small></label></div>
                <label class="admin-field" for="posting-after"><span>"Po sejmutí *"</span><select id="posting-after" required prop:value=move ||after_action.get() on:change=move |ev|{after_action.set(event_target_value(&ev));preview.set(None);redacted_preview.set(None);checked.set(false);}><option value="">"Vyberte způsob zpřístupnění"</option><option value="retain">"Po sejmutí ponechat přílohy veřejné"</option><option value="anonymized">"Zpřístupnit samostatné anonymizované znění"</option><option value="hide">"Po sejmutí přílohy pouze pro administrátory"</option></select></label>
                <Show when=move ||after_action.get()=="retain"><label class="admin-field" for="posting-public-until"><span>"Konec veřejného zpřístupnění"</span><input id="posting-public-until" type="date" prop:value=move ||public_until.get() on:input=move |ev|public_until.set(event_target_value(&ev))/></label></Show>
                <Show when=move ||after_action.get()=="anonymized"><label class="admin-field" for="posting-redacted"><span>"ID samostatného anonymizovaného dokumentu *"</span><input id="posting-redacted" type="number" min="1" required prop:value=move ||redacted_id.get() on:input=move |ev|{redacted_id.set(event_target_value(&ev));preview.set(None);redacted_preview.set(None);checked.set(false);}/><small>"Použijte již připravený odvozený záznam. Originál se jím nenahradí."</small></label></Show>
                <label class="admin-field" for="posting-reason"><span>"Interní důvod a podklad vyvěšení *"</span><textarea id="posting-reason" required maxlength="2000" prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))></textarea></label>
                <fieldset class="posting-review"><legend>"Kontrola tohoto vyvěšení"</legend>
                    <label class="admin-field" for="posting-rule"><span>"Pravidlo zveřejnění *"</span><select id="posting-rule" required prop:value=move ||rule.get() on:change=move |ev|rule.set(event_target_value(&ev))><option value="">"Vyberte příslušné pravidlo"</option><option value="informational">"Informativní oznámení"</option><option value="public_notice">"Doručování veřejnou vyhláškou (§ 25)"</option><option value="property_intent">"Majetkový záměr obce"</option><option value="council_meeting">"Oznámení zasedání zastupitelstva"</option><option value="custom">"Jiný právní režim"</option></select></label>
                    <label class="admin-field" for="posting-basis"><span>"Právní důvod a pravidlo lhůty *"</span><textarea id="posting-basis" required maxlength="2000" prop:value=move ||legal_basis.get() on:input=move |ev|legal_basis.set(event_target_value(&ev))></textarea></label>
                    <label class="admin-field" for="posting-original"><span>"Uložení originálu ve spisové službě *"</span><input id="posting-original" required maxlength="2000" prop:value=move ||original_reference.get() on:input=move |ev|original_reference.set(event_target_value(&ev))/></label>
                    <label class="admin-field" for="posting-minimum"><span>"Nejdřívější přípustné sejmutí"</span><input id="posting-minimum" type="date" required=move ||rule.get()=="custom" prop:value=move ||not_before.get() on:input=move |ev|not_before.set(event_target_value(&ev))/></label>
                    <Show when=move ||matches!(rule.get().as_str(),"property_intent"|"council_meeting")><label class="admin-field" for="posting-decision"><span>"Datum projednání nebo zasedání *"</span><input id="posting-decision" type="date" required prop:value=move ||decision_on.get() on:input=move |ev|decision_on.set(event_target_value(&ev))/></label></Show>
                    <label class="admin-check"><input type="checkbox" required prop:checked=move ||reviewed.get() on:change=move |ev|reviewed.set(event_target_checked(&ev))/><span>"Pro toto vyvěšení jsem ověřil/a právní režim, lhůtu, osobní údaje, přístupnost a uložení originálu."</span></label>
                </fieldset>
                <button class="button secondary" type="button" on:click=move |_|{prepare.dispatch(());}>"Zkontrolovat finální náhled"</button>
                {move ||preview.get().map(|value|view!{<section class="publication-preview" aria-label="Finální náhled dokumentu"><SnapshotContent preview=value/>
                    {move ||redacted_preview.get().map(|derived|view!{<section class="publication-preview" aria-label="Finální náhled anonymizovaného znění"><h3>"Samostatné anonymizované znění"</h3><SnapshotContent preview=derived/><p class="info-banner">"Anonymizované znění se při potvrzení také nevratně uzamkne. Jde o samostatnou veřejnou kopii, jejíž bajty tím nejsou označeny za původně vyvěšené."</p></section>})}
                    <p class="info-banner"><Icon name="lock"/>"Po vyvěšení již obsah dokumentu nelze upravit."</p>
                    <label class="admin-check"><input type="checkbox" prop:checked=move ||checked.get() on:change=move |ev|checked.set(event_target_checked(&ev))/><span>"Zkontroloval/a jsem uvedený obsah a všechny přílohy."</span></label>
                    <button class="button primary" type="submit" disabled=move ||!checked.get() ||disabled.get()>"Potvrdit vyvěšení této podoby"</button>
                </section>})}
            </fieldset>
        </form>
    </section>}
}

#[component]
fn SnapshotContent(preview: api::PublicationPreview) -> impl IntoView {
    view! {
        <h3>{preview.document.title}</h3><p>{preview.document.description.unwrap_or_default()}</p>
        <dl class="document-facts"><div><dt>"Číslo jednací"</dt><dd>{preview.document.reference_number.unwrap_or_else(||"Neuvedeno".into())}</dd></div><div><dt>"Původce"</dt><dd>{preview.document.issuer.unwrap_or_else(||"Neuvedeno".into())}</dd></div><div><dt>"Datum dokumentu"</dt><dd>{crate::catalog::document_date(preview.document.document_date.as_deref().unwrap_or_default())}</dd></div></dl>
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
fn TopicForm(
    id: i64,
    selected: Option<i64>,
    busy: Signal<bool>,
    reload: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let categories = LocalResource::new(|| api::get::<Vec<api::Category>>("/api/v1/categories"));
    let category = RwSignal::new(selected.map(|v| v.to_string()).unwrap_or_default());
    let reason = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let update = Action::new_local(move |_: &()| {
        let payload = json!({"category_id":category.get_untracked().parse::<i64>().ok(),"reason":reason.get_untracked()});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/topics"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Tematické zařazení bylo uloženo do auditu.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    view! {<details><summary>"Tematické zařazení"</summary><form on:submit=move |ev|{ev.prevent_default();if !busy.get_untracked() && !update.pending().get_untracked(){update.dispatch(());}}><ErrorMessage error/>
        <p>"Téma slouží katalogu. Jeho změna není změnou původního publikovaného obsahu ani úkonem vyvěšení."</p>
        <label class="admin-field" for="document-topic"><span>"Téma"</span><select id="document-topic" prop:value=move ||category.get() on:change=move |ev|category.set(event_target_value(&ev))><option value="">"Bez tématu"</option><Suspense fallback=||()>{move ||categories.get().and_then(Result::ok).map(|items|items.into_iter().map(|item|view!{<option value=item.id.to_string()>{item.name}</option>}).collect_view())}</Suspense></select></label>
        <label class="admin-field" for="topic-reason"><span>"Důvod zařazení *"</span><input id="topic-reason" required maxlength="2000" prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))/></label>
        <button type="submit" class="button secondary" disabled=move ||busy.get() ||update.pending().get()>"Uložit zařazení"</button>
    </form></details>}
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
            <label class="admin-field" for=format!("posting-plan-{id}")><span>"Nový plánovaný termín"</span><input id=format!("posting-plan-{id}") type="date" prop:value=move ||plan.get() on:input=move |ev|plan.set(event_target_value(&ev))/><small>"Prázdné pole zruší budoucí plán. Skutečné události zůstanou zachované."</small></label>
            <div class="admin-actions"><button class="button secondary" type="button" disabled=move ||disabled.get() on:click=move |_|{action.dispatch("plan".into());}>"Změnit plán"</button><ConfirmButton label="Zaznamenat sejmutí" title="Zaznamenat skutečné sejmutí?" description="Sejmutí se zapíše jako nová událost. Dostupnost příloh se upraví podle uložené výslovné volby. Správci si uchované přílohy mohou dál stáhnout a obsah zůstane uzamčený." disabled danger=true on_confirm=Callback::new(move |()|{action.dispatch("withdraw".into());})/></div>
        </details>})}
        <EvidenceCorrection document_id posting_id=id busy reload/>
    </li>}
}

#[component]
fn AttachRelation(id: i64, busy: Signal<bool>, reload: RwSignal<u64>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let related_id = RwSignal::new(String::new());
    let kind = RwSignal::new("supplement".to_string());
    let reason = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let link = Action::new_local(move |_: &()| {
        let payload = json!({"related_id":related_id.get_untracked().parse::<i64>().ok(),"kind":kind.get_untracked(),"reason":reason.get_untracked()});
        async move {
            error.set(None);
            match api::save(
                "POST",
                &format!("/api/v1/admin/documents/{id}/relations"),
                Some(payload),
            )
            .await
            {
                Ok(_) => {
                    ctx.changed("Vazba na navazující dokument byla zaznamenána.");
                    reload.update(|v| *v += 1);
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    view! {<details><summary>"Připojit existující doplnění nebo opravu"</summary><form on:submit=move |ev|{ev.prevent_default();if !busy.get_untracked()&&!link.pending().get_untracked(){link.dispatch(());}}><ErrorMessage error/>
        <p>"Vyberte samostatný navazující záznam. Připojení nezmění jeho veřejnou dostupnost ani nezaloží vyvěšení."</p>
        <label class="admin-field" for="relation-document"><span>"ID navazujícího dokumentu *"</span><input id="relation-document" type="number" min="1" required prop:value=move ||related_id.get() on:input=move |ev|related_id.set(event_target_value(&ev))/></label>
        <label class="admin-field" for="relation-kind"><span>"Druh vazby"</span><select id="relation-kind" prop:value=move ||kind.get() on:change=move |ev|kind.set(event_target_value(&ev))><option value="supplement">"Doplnění"</option><option value="correction">"Oprava"</option><option value="replacement">"Nahrazení"</option><option value="anonymized">"Anonymizované znění"</option></select></label>
        <label class="admin-field" for="relation-reason"><span>"Interní důvod *"</span><textarea id="relation-reason" required maxlength="2000" prop:value=move ||reason.get() on:input=move |ev|reason.set(event_target_value(&ev))></textarea></label>
        <button class="button secondary" type="submit" disabled=move ||busy.get()||link.pending().get()>"Připojit vazbu"</button>
    </form></details>}
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
