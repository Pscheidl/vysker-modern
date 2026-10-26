use super::{AdminContext, api, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::json;

const PAGE_SIZE: usize = 20;

#[derive(Clone, Deserialize)]
struct Settings {
    recipients: Vec<String>,
}

#[derive(Clone, Deserialize)]
struct Report {
    id: i64,
    description: String,
    address: Option<String>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    reporter_name: Option<String>,
    reporter_email: Option<String>,
    reporter_phone: Option<String>,
    created_at: String,
    status: String,
    has_photo: bool,
    photo_count: i64,
    #[serde(default)]
    resolution_comment: Option<String>,
    #[serde(default)]
    resolution_updated_at: Option<String>,
    #[serde(default)]
    resolution_photo_ids: Vec<i64>,
    request_ip: String,
    request_user_agent: Option<String>,
    request_accept_language: Option<String>,
    client_timezone: Option<String>,
    browser_id_hash: Option<String>,
    browser_fingerprint_hash: Option<String>,
    authenticated_admin_id: Option<i64>,
    authenticated_admin_email: Option<String>,
    metadata_recorded_at: Option<i64>,
    metadata_expires_at: Option<i64>,
    hidden: bool,
}

#[derive(Clone, Copy)]
enum ReportChange {
    Status(&'static str),
    Hidden(bool),
}

fn state_label(status: &str) -> &'static str {
    crate::reports::status_label(status)
}

fn reported_at(value: &str) -> String {
    crate::events::date_label(value)
}

fn location(report: &Report) -> String {
    match (&report.address, report.latitude, report.longitude) {
        (Some(address), Some(lat), Some(lon)) => format!("{address} · {lat:.6}, {lon:.6}"),
        (Some(address), _, _) => address.clone(),
        (_, Some(lat), Some(lon)) => format!("{lat:.6}, {lon:.6}"),
        _ => "Místo neuvedeno".into(),
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> api::Result<T> {
    serde_json::from_value(value).map_err(|_| api::Failure {
        status: 0,
        message: "Server vrátil neočekávanou odpověď. Obnovte stránku a ověřte uložené údaje."
            .into(),
    })
}

fn allow_report_change(dirty: RwSignal<bool>) -> bool {
    if !dirty.get_untracked() {
        return true;
    }
    #[cfg(feature = "hydrate")]
    {
        web_sys::window().is_some_and(|window| {
            window
                .confirm_with_message(
                    "Řešení hlášení obsahuje neuložené změny. Opravdu je chcete zahodit?",
                )
                .unwrap_or(false)
        })
    }
    #[cfg(not(feature = "hydrate"))]
    false
}

#[component]
pub fn Reports() -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let settings_dirty = RwSignal::new(false);
    let resolution_dirty = RwSignal::new(false);
    let updating = RwSignal::new(false);
    let offset = RwSignal::new(0_usize);
    let selected = RwSignal::new(None::<Report>);
    let refresh = RwSignal::new(0_u64);
    let settings = LocalResource::new(move || async {
        api::get::<Settings>("/api/v1/admin/report-settings").await
    });
    let reports = LocalResource::new(move || {
        refresh.get();
        let path = format!(
            "/api/v1/admin/reports?limit={}&offset={}",
            PAGE_SIZE + 1,
            offset.get()
        );
        async move { api::get::<Vec<Report>>(&path).await }
    });
    Effect::new(move |_| {
        ctx.dirty
            .set(settings_dirty.get() || resolution_dirty.get() || updating.get())
    });
    on_cleanup(move || ctx.dirty.set(false));
    view! {
        <Title text="Podněty a závady · Správa Vyskře"/>
        <Heading title="Podněty a závady" description="Návrhy na zlepšení obce, hlášení závad a průběh jejich řešení. Nová hlášení se zveřejňují automaticky, nevhodné hlášení můžete skrýt."/>
        <Suspense fallback=Pending>{move || settings.get().map(|result| match result {
            Ok(initial) => view! { <Recipients initial dirty=settings_dirty/> }.into_any(),
            Err(error) => view! {
                <FailureView error/>
                <button class="button secondary" type="button" on:click=move |_| settings.refetch()>"Zkusit načíst příjemce znovu"</button>
            }.into_any(),
        })}</Suspense>
        <section class="admin-panel admin-list-panel" aria-label="Přijatá hlášení">
            <div class="admin-filters">
                <h2>"Přijatá hlášení"</h2>
                <button class="button secondary" type="button" disabled=move || updating.get() on:click=move |_| {
                    if !allow_report_change(resolution_dirty) { return; }
                    selected.set(None);
                    reports.refetch();
                }>"Obnovit hlášení"</button>
                <a class="text-link" href="/hlaseni-zavad" target="_blank" rel="noopener noreferrer">"Veřejný přehled"</a>
            </div>
            <Suspense fallback=Pending>{move || reports.get().map(|result| match result {
                Err(error) => view! { <FailureView error/> }.into_any(),
                Ok(items) => {
                    let count = items.len();
                    view! {
                        {(count == 0).then(|| view! { <p class="admin-empty">"Zatím zde nejsou žádná hlášení."</p> })}
                        <div class="admin-table-wrap"><table class="admin-table">
                            <caption class="sr-only">"Hlášení od nejnovějšího"</caption>
                            <thead><tr><th>"Popis a místo"</th><th>"Nahlášeno"</th><th>"Stav a zveřejnění"</th><th>"Správa"</th></tr></thead>
                            <tbody>{items.into_iter().take(PAGE_SIZE).map(|report| {
                                let id = report.id;
                                let brief: String = report.description.chars().take(100).collect();
                                let brief = if report.description.chars().count() > 100 { format!("{brief}…") } else { brief };
                                let place = location(&report);
                                let date = reported_at(&report.created_at);
                                let state = state_label(&report.status);
                                let visibility = if report.hidden { "Skryto" } else { "Veřejné" };
                                let photo = report.has_photo;
                                view! { <tr>
                                    <td class="admin-name"><strong>{brief}</strong><small>{place}</small><span class="admin-row-id">{format!("#{id}")}{photo.then_some(" · S fotografií")}</span></td>
                                    <td class="admin-table-meta"><time datetime=report.created_at.clone()>{date}</time></td>
                                    <td>{state}<small>{visibility}</small></td>
                                    <td><button class="button secondary" type="button" disabled=move || updating.get() on:click=move |_| { if allow_report_change(resolution_dirty) { selected.set(Some(report.clone())); } }>{format!("Detail #{id}")}</button></td>
                                </tr> }
                            }).collect_view()}</tbody>
                        </table></div>
                        <nav class="admin-pager" aria-label="Stránkování hlášení"><span>{format!("Strana {}", offset.get_untracked() / PAGE_SIZE + 1)}</span><div>
                            <button class="button secondary" type="button" disabled=move || offset.get() == 0 || updating.get() on:click=move |_| {
                                if !allow_report_change(resolution_dirty) { return; }
                                selected.set(None);
                                offset.update(|value| *value = value.saturating_sub(PAGE_SIZE));
                            }>"Předchozí"</button>
                            <button class="button secondary" type="button" disabled=move || count <= PAGE_SIZE || updating.get() on:click=move |_| {
                                if !allow_report_change(resolution_dirty) { return; }
                                selected.set(None);
                                offset.update(|value| *value += PAGE_SIZE);
                            }>"Další"</button>
                        </div></nav>
                    }.into_any()
                },
            })}</Suspense>
        </section>
        {move || selected.get().map(|report| view! { <ReportDetail report selected refresh updating resolution_dirty/> })}
    }
}

#[component]
fn Recipients(initial: Settings, dirty: RwSignal<bool>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let saved = RwSignal::new(initial.recipients.join("\n"));
    let recipients = RwSignal::new(saved.get_untracked());
    let error = RwSignal::new(None);
    let save = Action::new_local(move |_: &()| {
        let addresses: Vec<String> = recipients
            .get_untracked()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        async move {
            error.set(None);
            match api::save(
                "PUT",
                "/api/v1/admin/report-settings",
                Some(json!({ "recipients": addresses })),
            )
            .await
            .and_then(decode::<Settings>)
            {
                Ok(settings) => {
                    let current = settings.recipients.join("\n");
                    saved.set(current.clone());
                    recipients.set(current);
                    ctx.changed("Příjemci upozornění na podněty a závady byli uloženi.");
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    Effect::new(move |_| dirty.set(recipients.get() != saved.get() || save.pending().get()));
    view! {
        <section class="admin-panel admin-form-section" aria-labelledby="report-recipients-heading">
            <h2 id="report-recipients-heading">"E-mailová upozornění"</h2>
            <p class="admin-caption" id="report-recipients-help">"Každou e-mailovou adresu napište na samostatný řádek. Seznam je ve výchozím stavu prázdný. Bez příjemců se upozornění neposílají, hlášení se dál přijímají a zobrazují veřejně."</p>
            <form on:submit=move |ev| {
                ev.prevent_default();
                if !save.pending().get_untracked() && dirty.get_untracked() { save.dispatch(()); }
            }>
                <fieldset disabled=move || save.pending().get()>
                    <label class="admin-field" for="report-recipients"><span>"Příjemci upozornění"</span>
                        <textarea id="report-recipients" rows="4" maxlength="10000" spellcheck="false" autocomplete="off" aria-describedby="report-recipients-help" prop:value=move || recipients.get() on:input=move |ev| recipients.set(event_target_value(&ev))></textarea>
                    </label>
                    <ErrorMessage error/>
                    <div class="admin-actions">
                        <button class="button primary" type="submit" disabled=move || !dirty.get()>{move || if save.pending().get() { "Ukládám…" } else { "Uložit příjemce" }}</button>
                        <button class="button secondary" type="button" disabled=move || !dirty.get() on:click=move |_| {
                            recipients.set(saved.get_untracked());
                            error.set(None);
                        }>"Zrušit změny příjemců"</button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}

#[component]
fn SenderMetadata(report: Report) -> impl IntoView {
    let account = match (
        report.authenticated_admin_email,
        report.authenticated_admin_id,
    ) {
        (Some(email), Some(id)) => format!("{email} (ID {id})"),
        (Some(email), None) => email,
        (None, Some(id)) => format!("Účet ID {id}"),
        (None, None) => "Přihlášený účet nebyl zaznamenán".into(),
    };
    let unavailable = || "Není k dispozici".to_string();
    let request_ip = if report.request_ip.trim().is_empty() {
        unavailable()
    } else {
        report.request_ip
    };
    view! {
        <section aria-labelledby="report-sender-title" style="overflow-wrap: anywhere">
            <h3 id="report-sender-title">"Údaje odesílatele"</h3>
            <p class="admin-caption">"Viditelné pouze v administraci. Vyplněné jméno, kontakty a údaje prohlížeče nejsou ověřené. IP adresa i prohlížeč mohou být sdílené a samy o sobě neprokazují totožnost člověka."</p>
            <p><strong>"Kdo nahlásil: "</strong>{report.reporter_name.unwrap_or_else(|| "Anonymní hlášení".into())}</p>
            <p><strong>"Kontaktní e-mail: "</strong>{report.reporter_email.unwrap_or_else(|| "Neuveden".into())}</p>
            <p><strong>"Telefon: "</strong>{report.reporter_phone.unwrap_or_else(|| "Neuveden".into())}</p>
            <p><strong>"Přihlášený účet na tomto webu: "</strong>{account}</p>
            <h4>"Technické údaje odesílatele"</h4>
            <p><strong>"IP adresa požadavku: "</strong><code>{request_ip}</code></p>
            <p><strong>"Náhodný identifikátor prohlížeče: "</strong><code>{report.browser_id_hash.unwrap_or_else(unavailable)}</code></p>
            <p><strong>"Orientační otisk prohlížeče: "</strong><code>{report.browser_fingerprint_hash.unwrap_or_else(unavailable)}</code></p>
            <p class="admin-caption">"Identifikátor pomáhá rozpoznat opakovaná hlášení ze stejného prohlížeče. Vymazáním cookie se změní. Orientační otisk může být shodný u více různých zařízení."</p>
            <p><strong>"Prohlížeč (User-Agent): "</strong>{report.request_user_agent.unwrap_or_else(unavailable)}</p>
            <p><strong>"Jazyk prohlížeče: "</strong>{report.request_accept_language.unwrap_or_else(unavailable)}</p>
            <p><strong>"Časové pásmo: "</strong>{report.client_timezone.unwrap_or_else(unavailable)}</p>
            {report.metadata_recorded_at.map(|recorded| { let datetime = api::unix_date(recorded); let label = format!("{} UTC", reported_at(&datetime)); view! { <p class="admin-caption">"Zaznamenáno "<time datetime=datetime>{label}</time></p> } })}
            {report.metadata_expires_at.map(|expires| { let datetime = api::unix_date(expires); let label = format!("{} UTC", reported_at(&datetime)); view! { <p class="admin-caption">"Technické údaje uchováváme do "<time datetime=datetime>{label}</time>"."</p> } })}
        </section>
    }
}

#[component]
fn ReportDetail(
    report: Report,
    selected: RwSignal<Option<Report>>,
    refresh: RwSignal<u64>,
    updating: RwSignal<bool>,
    resolution_dirty: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let id = report.id;
    let current = RwSignal::new(report);
    let error = RwSignal::new(None);
    let update = Action::new_local(move |change: &ReportChange| {
        let body = match change {
            ReportChange::Status(status) => json!({ "status": status }),
            ReportChange::Hidden(hidden) => json!({ "hidden": hidden }),
        };
        async move {
            error.set(None);
            updating.set(true);
            let result = api::save("PUT", &format!("/api/v1/admin/reports/{id}"), Some(body))
                .await
                .and_then(decode::<Report>);
            if updating.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(report) => {
                    current.set(report);
                    refresh.update(|value| *value += 1);
                    ctx.changed("Hlášení bylo aktualizováno.");
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let initial = current.get_untracked();
    let coordinates = initial.latitude.zip(initial.longitude);
    let sender = initial.clone();
    view! {
        <section class="admin-panel admin-form-section" aria-label="Detail hlášení" style="margin-top: 22px">
            <div class="admin-panel-heading"><h2>{format!("Hlášení #{id}")}</h2><button class="button secondary" type="button" disabled=move || updating.get() on:click=move |_| { if allow_report_change(resolution_dirty) { selected.set(None); } }>"Zavřít detail"</button></div>
            <p class="admin-caption"><time datetime=initial.created_at.clone()>{reported_at(&initial.created_at)}</time>" · "{move || state_label(&current.get().status)}" · "{move || if current.get().hidden { "Skryto na veřejném webu" } else { "Veřejné hlášení" }}</p>
            <p style="white-space: pre-wrap; overflow-wrap: anywhere">{initial.description}</p>
            <p style="overflow-wrap: anywhere"><strong>"Místo: "</strong>{location(&current.get_untracked())}</p>
            {coordinates.map(|(lat, lon)| view! { <p><a class="text-link" href=format!("https://www.openstreetmap.org/?mlat={lat}&mlon={lon}#map=18/{lat}/{lon}") target="_blank" rel="noopener noreferrer">"Otevřít místo v OpenStreetMap"</a></p> })}
            <SenderMetadata report=sender/>
            <div class="report-detail-photos" aria-label="Fotografie hlášení">{(1..=initial.photo_count).map(|position| view! {
                <a class="report-detail-photo" href=format!("/api/v1/admin/reports/{id}/photos/{position}") target="_blank" rel="noopener noreferrer" aria-label=format!("Otevřít fotografii {position} hlášení")><img src=format!("/api/v1/admin/reports/{id}/photos/{position}") alt=format!("Fotografie {position} k hlášení #{id}") loading="lazy" style="display: block; max-width: 100%; max-height: 420px; object-fit: contain; border-radius: 8px"/></a>
            }).collect_view()}</div>
            <ErrorMessage error/>
            <div class="admin-actions">
                <button class="button primary" type="button" disabled=move || updating.get() || resolution_dirty.get() || current.get().status == "in_progress" on:click=move |_| { update.dispatch(ReportChange::Status("in_progress")); }>"Převzít k řešení"</button>
                <button class="button secondary" type="button" disabled=move || updating.get() || resolution_dirty.get() || current.get().status == "resolved" on:click=move |_| { update.dispatch(ReportChange::Status("resolved")); }>"Označit jako vyřešené"</button>
                <button class="button secondary" type="button" disabled=move || updating.get() || resolution_dirty.get() || current.get().status == "reported" on:click=move |_| { update.dispatch(ReportChange::Status("reported")); }>"Vrátit mezi nahlášené"</button>
                <button class="button secondary" type="button" disabled=move || updating.get() || resolution_dirty.get() on:click=move |_| {
                    let report = current.get_untracked();
                    update.dispatch(ReportChange::Hidden(!report.hidden));
                }>{move || if current.get().hidden { "Znovu zveřejnit" } else { "Skrýt na veřejném webu" }}</button>
                <Show when=move || !current.get().hidden><a class="text-link" href=format!("/hlaseni-zavad/{id}") target="_blank" rel="noopener noreferrer">"Otevřít veřejný detail"</a></Show>
            </div>
            <ResolutionEditor current refresh updating dirty=resolution_dirty/>
        </section>
    }
}

#[derive(Clone)]
struct ResolutionPhoto {
    file: web_sys::File,
    preview: String,
}

fn release_preview(url: &str) {
    #[cfg(feature = "hydrate")]
    let _ = web_sys::Url::revoke_object_url(url);
    #[cfg(not(feature = "hydrate"))]
    let _ = url;
}

fn resolution_failure(message: &str) -> api::Failure {
    api::Failure {
        status: 0,
        message: message.into(),
    }
}

async fn save_resolution(
    id: i64,
    comment: String,
    removed: Vec<i64>,
    photos: Vec<ResolutionPhoto>,
    resolved: bool,
) -> api::Result<Report> {
    #[cfg(feature = "hydrate")]
    {
        let session: api::Session = api::get("/api/v1/admin/session").await?;
        let network = || {
            resolution_failure(
                "Spojení se serverem se přerušilo. Údaje a fotografie zůstávají ve formuláři. Před opakováním ověřte ve veřejném detailu nebo v nové kartě administrace, zda už bylo řešení uloženo.",
            )
        };
        let body = web_sys::FormData::new().map_err(|_| network())?;
        body.append_with_str("resolution_comment", &comment)
            .map_err(|_| network())?;
        body.append_with_str(
            "remove_photo_ids",
            &serde_json::to_string(&removed).map_err(|_| network())?,
        )
        .map_err(|_| network())?;
        if resolved {
            body.append_with_str("status", "resolved")
                .map_err(|_| network())?;
        }
        for photo in photos {
            body.append_with_blob_and_filename("photo", &photo.file, &photo.file.name())
                .map_err(|_| network())?;
        }
        let response =
            gloo_net::http::Request::put(&format!("/api/v1/admin/reports/{id}/resolution"))
                .credentials(web_sys::RequestCredentials::SameOrigin)
                .header("X-CSRF-Token", &session.csrf_token)
                .body(body)
                .map_err(|_| network())?
                .send()
                .await
                .map_err(|_| network())?;
        let status = response.status();
        let value = response
            .json::<serde_json::Value>()
            .await
            .map_err(|_| network())?;
        if !response.ok() {
            return Err(api::Failure {
                status,
                message: value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Řešení se nepodařilo uložit. Zkontrolujte údaje a zkuste to znovu.")
                    .into(),
            });
        }
        decode(value)
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (id, comment, removed, photos, resolved);
        Err(resolution_failure(
            "Řešení lze uložit po načtení formuláře.",
        ))
    }
}

#[component]
fn ResolutionEditor(
    current: RwSignal<Report>,
    refresh: RwSignal<u64>,
    updating: RwSignal<bool>,
    dirty: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let id = current.get_untracked().id;
    let comment = RwSignal::new(
        current
            .get_untracked()
            .resolution_comment
            .unwrap_or_default(),
    );
    let removed = RwSignal::new(Vec::<i64>::new());
    let photos = RwSignal::new_local(Vec::<ResolutionPhoto>::new());
    let input = NodeRef::<leptos::html::Input>::new();
    let error = RwSignal::new(None::<api::Failure>);
    let changed = Memo::new(move |_| {
        comment.get() != current.get().resolution_comment.unwrap_or_default()
            || !removed.get().is_empty()
            || !photos.with(Vec::is_empty)
    });
    Effect::new(move |_| dirty.set(changed.get()));
    on_cleanup(move || {
        for photo in photos.get_untracked() {
            release_preview(&photo.preview);
        }
        dirty.set(false);
    });
    let save = Action::new_local(move |resolved: &bool| {
        let resolved = *resolved;
        let comment_value = comment.get_untracked();
        let removed_value = removed.get_untracked();
        let photos_value = photos.get_untracked();
        async move {
            updating.set(true);
            error.set(None);
            let result =
                save_resolution(id, comment_value, removed_value, photos_value, resolved).await;
            if updating.try_set(false).is_some() {
                return;
            }
            match result {
                Ok(report) => {
                    for photo in photos.get_untracked() {
                        release_preview(&photo.preview);
                    }
                    photos.set(Vec::new());
                    removed.set(Vec::new());
                    comment.set(report.resolution_comment.clone().unwrap_or_default());
                    current.set(report);
                    refresh.update(|value| *value += 1);
                    ctx.changed("Řešení hlášení bylo uloženo.");
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    view! {
        <section class="admin-form-section" aria-label="Řešení hlášení" style="margin-top: 28px">
            <h3>"Řešení hlášení"</h3>
            <p class="admin-caption">"Komentář a fotografie k řešení budou vidět ve veřejném detailu. Jsou nepovinné a původní hlášení zůstane zachované."</p>
            {move || current.get().resolution_updated_at.map(|date| view! { <p class="admin-caption">"Naposledy upraveno: "<time datetime=date.clone()>{reported_at(&date)}</time></p> })}
            <form on:submit=move |event| { event.prevent_default(); if !updating.get_untracked() && changed.get_untracked() { save.dispatch(false); } }>
                <fieldset disabled=move || updating.get()>
                    <label class="admin-field" for="report-resolution-comment"><span>"Komentář k řešení (nepovinné)"</span><textarea id="report-resolution-comment" rows="4" maxlength="5000" prop:value=move || comment.get() on:input=move |event| comment.set(event_target_value(&event)) placeholder="Popište provedené kroky nebo další postup."></textarea></label>
                    <div class="report-detail-photos" aria-label="Uložené fotografie řešení">{move || {
                        let removed_ids = removed.get();
                        current.get().resolution_photo_ids.into_iter().filter(|photo_id| !removed_ids.contains(photo_id)).map(move |photo_id| view! {
                            <figure class="report-photo-preview-item">
                                <a class="report-detail-photo" href=format!("/api/v1/admin/reports/{id}/resolution-photos/{photo_id}") target="_blank" rel="noopener noreferrer"><img src=format!("/api/v1/admin/reports/{id}/resolution-photos/{photo_id}") alt="Fotografie k řešení hlášení" loading="lazy"/></a>
                                <button class="text-link" type="button" aria-label=format!("Odebrat uloženou fotografii řešení {photo_id}") on:click=move |_| removed.update(|items| items.push(photo_id))>"Odebrat fotografii"</button>
                            </figure>
                        }).collect_view()
                    }}</div>
                    <Show when=move || !removed.get().is_empty()><button class="text-link" type="button" on:click=move |_| removed.set(Vec::new())>"Vrátit odebrané fotografie"</button><p class="admin-caption">"Fotografie budou odebrány až po uložení řešení."</p></Show>
                    <label class="admin-field" for="report-resolution-photos"><span>"Fotografie řešení (nepovinné)"</span><input node_ref=input id="report-resolution-photos" type="file" multiple accept="image/jpeg,image/png,image/webp" aria-describedby="report-resolution-photo-help" on:change=move |_| {
                        let Some(element) = input.get() else { return; };
                        let Some(files) = element.files() else { return; };
                        let selected: Vec<web_sys::File> = (0..files.length()).filter_map(|index| files.item(index)).collect();
                        element.set_value("");
                        error.set(None);
                        let invalid_size = selected.iter().any(|file| file.size() > 20_000_000.0);
                        let invalid_type = selected.iter().any(|file| {
                            let kind = file.type_();
                            let name = file.name().to_lowercase();
                            !matches!(kind.as_str(), "image/jpeg" | "image/png" | "image/webp")
                                && !(kind.is_empty() && [".jpg", ".jpeg", ".png", ".webp"].iter().any(|extension| name.ends_with(extension)))
                        });
                        let total = photos.with(|items| items.iter().map(|item| item.file.size()).sum::<f64>()) + selected.iter().map(|file| file.size()).sum::<f64>();
                        if invalid_size || invalid_type || total > 50_000_000.0 {
                            let message = if invalid_size { "Jedna fotografie může mít nejvýše 20 MB." } else if invalid_type { "Vyberte fotografie JPG, PNG nebo WebP." } else { "Nové fotografie dohromady přesahují 50 MB." };
                            error.set(Some(resolution_failure(message)));
                            return;
                        }
                        #[cfg(feature = "hydrate")]
                        for file in selected {
                            match web_sys::Url::create_object_url_with_blob(&file) {
                                Ok(preview) => photos.update(|items| items.push(ResolutionPhoto { file, preview })),
                                Err(_) => error.set(Some(resolution_failure("Náhled fotografie se nepodařilo načíst. Zkuste soubor vybrat znovu."))),
                            }
                        }
                    }/></label>
                    <p id="report-resolution-photo-help" class="admin-caption">"JPG, PNG nebo WebP, nejvýše 20 MB na fotografii a 50 MB nových fotografií celkem. Přidávat je můžete postupně. Uložené fotografie včetně nových mohou mít dohromady nejvýše 50 MB."</p>
                    <div class="report-photo-previews" aria-label="Nové fotografie řešení">{move || photos.get().into_iter().enumerate().map(|(index, photo)| {
                        let name = photo.file.name();
                        view! { <figure class="report-photo-preview-item" data-resolution-photo=name.clone()><img src=photo.preview alt=format!("Náhled fotografie řešení {name}")/><figcaption>{name.clone()}</figcaption><button class="text-link" type="button" aria-label=format!("Odebrat novou fotografii řešení {name}") on:click=move |_| photos.update(|items| {
                            if index < items.len() { let photo = items.remove(index); release_preview(&photo.preview); }
                        })>"Odebrat fotografii"</button></figure> }
                    }).collect_view()}</div>
                    <ErrorMessage error/>
                    <div class="admin-actions">
                        <button class="button secondary" type="submit" disabled=move || !changed.get()>"Uložit řešení"</button>
                        <button class="button primary" type="button" disabled=move || !changed.get() && current.get().status == "resolved" on:click=move |_| { if !updating.get_untracked() { save.dispatch(true); } }>"Uložit a označit jako vyřešené"</button>
                        <button class="button secondary" type="button" disabled=move || !changed.get() on:click=move |_| {
                            for photo in photos.get_untracked() { release_preview(&photo.preview); }
                            photos.set(Vec::new()); removed.set(Vec::new());
                            comment.set(current.get_untracked().resolution_comment.unwrap_or_default()); error.set(None);
                        }>"Zrušit změny řešení"</button>
                    </div>
                </fieldset>
            </form>
        </section>
    }
}
