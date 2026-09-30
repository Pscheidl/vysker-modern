//! Calendar administration using the existing session, CSRF, and form components.
use super::{AdminContext, api, ui::*};
use crate::{
    app::components::Icon,
    events::{Event, EventInput, local_datetime},
};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_navigate;
use serde_json::json;

#[component]
pub fn Events() -> impl IntoView {
    let offset = RwSignal::new(0usize);
    let data = LocalResource::new(move || {
        let path = format!("/api/v1/admin/events?limit=21&offset={}", offset.get());
        async move { api::get::<Vec<Event>>(&path).await }
    });
    view! {<Title text="Kalendář akcí · Správa Vyskře"/>
        <div class="admin-title-row"><Heading title="Kalendář akcí" description="Termíny, místa konání, koncepty a zveřejněné akce. Zrušení akce zůstane na veřejném webu viditelné."/>
            <div class="admin-create"><AdminLink href="/admin/kalendar/nove" label="Nová akce" icon="plus"/></div>
        </div>
        <section class="admin-panel admin-list-panel">
            <div class="admin-filters"><button class="button secondary" type="button" on:click=move |_|data.refetch()>"Obnovit seznam"</button></div>
            <Suspense fallback=Pending>{move ||data.get().map(|result|match result {
                Err(error)=>view!{<FailureView error/>}.into_any(),
                Ok(items)=>{
                    let count=items.len();
                    view!{<div class="admin-table-wrap"><table class="admin-table"><caption class="sr-only">"Kalendář akcí"</caption><thead><tr><th>"Název a místo"</th><th>"Termín"</th><th>"Stav"</th></tr></thead><tbody>
                        {items.into_iter().take(20).map(|event|{
                            let start=event.start_label();
                            let end=event.end_label();
                            view!{<tr><td class="admin-name"><AdminLink href=format!("/admin/kalendar/{}",event.id) label=event.title icon="calendar"/><small>{event.location}</small></td>
                                <td class="admin-table-meta"><span>{start}</span><small>"Do "{end}</small></td>
                                <td><Status value=if event.published {"published"} else {"draft"}/>{event.cancelled.then(||view!{<span class="status-badge warning">"Zrušeno"</span>})}</td></tr>}
                        }).collect_view()}
                    </tbody></table></div>
                    {(count==0).then(||view!{<div class="admin-empty"><Icon name="calendar"/><h2>"Žádné akce"</h2><p>"Novou akci přidáte tlačítkem Nová akce."</p></div>})}
                    <Pager offset count/>
                    }.into_any()
                }
            })}</Suspense>
        </section>
    }
}

#[component]
pub fn EventEditor(id: i64) -> impl IntoView {
    let reload = RwSignal::new(0u64);
    let data = LocalResource::new(move || {
        reload.get();
        async move {
            if id == 0 {
                Ok(Event::default())
            } else {
                api::get::<Event>(&format!("/api/v1/admin/events/{id}")).await
            }
        }
    });
    view! {<Suspense fallback=Pending>{move ||data.get().map(|result|match result {
        Ok(event)=>view!{<EventForm event reload/>}.into_any(),
        Err(error)=>view!{<FailureView error/><button class="button secondary" type="button" on:click=move |_|data.refetch()>"Načíst znovu"</button><AdminLink href="/admin/kalendar" label="Zpět na kalendář" icon="back"/>}.into_any(),
    })}</Suspense>}
}

#[component]
fn EventDateField(
    id: &'static str,
    label: &'static str,
    local: RwSignal<String>,
    precise: RwSignal<String>,
    explicit: RwSignal<bool>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    view! {<label class="admin-field" for=id><span>{label}" *"</span>
        <input id=id type=move ||if explicit.get() {"text"} else {"datetime-local"} step="any" required maxlength="64"
            aria-describedby="event-time-help" prop:value=move ||if explicit.get(){precise.get()}else{local.get()}
            on:input=move |ev|{if explicit.get_untracked(){precise.set(event_target_value(&ev));}else{local.set(event_target_value(&ev));}ctx.dirty.set(true);}/>
    </label>}
}

#[component]
fn EventForm(event: Event, reload: RwSignal<u64>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let id = event.id;
    let version = event.version;
    let original = StoredValue::new(event.clone());
    let title = RwSignal::new(event.title.clone());
    let description = RwSignal::new(event.description.clone());
    let location = RwSignal::new(event.location.clone());
    let start = RwSignal::new(local_datetime(&event.starts_at));
    let end = RwSignal::new(local_datetime(&event.ends_at));
    let start_precise = RwSignal::new(event.starts_at.clone());
    let end_precise = RwSignal::new(event.ends_at.clone());
    let explicit = RwSignal::new(false);
    let published = RwSignal::new(event.published);
    let cancelled = RwSignal::new(event.cancelled);
    let error = RwSignal::new(None::<api::Failure>);
    let conflict_error = RwSignal::new(None::<api::Failure>);
    let current = RwSignal::new(None::<Event>);
    let navigate = use_navigate();
    let navigate_after_save = navigate.clone();
    let save = Action::new_local(move |_: &()| {
        let navigate = navigate_after_save.clone();
        let previous = original.get_value();
        // Keep an unchanged stored instant, including its DST offset and precision.
        let date_input = |local: String, precise: String, stored: String| {
            if explicit.get_untracked() {
                precise
            } else if id > 0 && local == local_datetime(&stored) {
                stored
            } else {
                local
            }
        };
        let input = EventInput {
            title: title.get_untracked(),
            description: description.get_untracked(),
            location: location.get_untracked(),
            starts_at: date_input(
                start.get_untracked(),
                start_precise.get_untracked(),
                previous.starts_at,
            ),
            ends_at: date_input(
                end.get_untracked(),
                end_precise.get_untracked(),
                previous.ends_at,
            ),
            published: published.get_untracked(),
            cancelled: cancelled.get_untracked(),
            expected_version: if id == 0 { None } else { Some(version) },
        };
        async move {
            error.set(None);
            let path = if id == 0 {
                "/api/v1/admin/events".into()
            } else {
                format!("/api/v1/admin/events/{id}")
            };
            match api::save(
                if id == 0 { "POST" } else { "PUT" },
                &path,
                Some(json!(input)),
            )
            .await
            {
                Ok(value) => {
                    ctx.dirty.set(false);
                    ctx.changed("Akce byla uložena.");
                    if id == 0 {
                        if let Some(saved_id) = value["id"].as_i64() {
                            navigate(&format!("/admin/kalendar/{saved_id}"), Default::default());
                        }
                    } else {
                        reload.update(|v| *v += 1);
                    }
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let remove = Action::new_local(move |_: &()| {
        let navigate = navigate.clone();
        async move {
            error.set(None);
            match api::save(
                "DELETE",
                &format!("/api/v1/admin/events/{id}"),
                Some(json!({"expected_version":version})),
            )
            .await
            {
                Ok(_) => {
                    ctx.dirty.set(false);
                    ctx.changed("Akce byla smazána.");
                    navigate("/admin/kalendar", Default::default());
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let compare = Action::new_local(move |_: &()| async move {
        conflict_error.set(None);
        match api::get::<Event>(&format!("/api/v1/admin/events/{id}")).await {
            Ok(event) => current.set(Some(event)),
            Err(failure) => conflict_error.set(Some(failure)),
        }
    });
    let busy = Signal::derive(move || {
        save.pending().get() || remove.pending().get() || compare.pending().get()
    });
    let conflicted = move || error.get().is_some_and(|e| e.status == 409);
    let save_disabled = Signal::derive(move || busy.get() || conflicted());
    let heading = if id == 0 {
        "Nová akce".into()
    } else {
        event.title.clone()
    };
    view! {<Title text=format!("{heading} · Správa Vyskře")/>
        <div class="admin-editor-back"><AdminLink href="/admin/kalendar" label="Kalendář akcí" icon="back"/><span>"/"</span><span>{if id==0 {"Nová akce".into()} else {format!("#{id:04}")}}</span></div>
        <div class="admin-editor-heading"><h1>{heading}</h1><Status value=if event.published {"published"} else {"draft"}/>{event.cancelled.then(||view!{<span class="status-badge warning">"Zrušeno"</span>})}</div>
        <ErrorMessage error/>
        <Show when=conflicted>
            <section class="admin-panel admin-form-section" aria-label="Konflikt úprav"><h2>"Akci mezitím změnil jiný správce"</h2><p>"Vaše rozepsané údaje zůstávají ve formuláři. Před dalším uložením zkontrolujte aktuální verzi."</p>
                <button class="button secondary" type="button" disabled=move ||busy.get() on:click=move |_|{if !busy.get_untracked(){compare.dispatch(());}}>"Zobrazit aktuální verzi pro porovnání"</button>
                <ErrorMessage error=conflict_error/>
                {move ||current.get().map(|event|view!{<CurrentVersion event/>})}
                <ConfirmButton label="Načíst aktuální verzi" title="Zahodit rozepsané úpravy?" description="Formulář se nahradí aktuální uloženou verzí. Potřebné rozepsané údaje si předem zkopírujte." disabled=busy on_confirm=Callback::new(move |_|{ctx.dirty.set(false);reload.update(|v|*v+=1);})/>
            </section>
        </Show>
        <div class="admin-editor-grid"><div class="admin-editor-content">
            <form id="event-editor" on:submit=move |ev|{ev.prevent_default();if !save_disabled.get_untracked(){save.dispatch(());}}>
                <fieldset disabled=move ||busy.get()>
                    <section class="admin-panel admin-form-section"><div class="admin-panel-heading"><h2>"Údaje o akci"</h2><span class="admin-caption">"* Povinné údaje"</span></div>
                        <Field id="event-title" label="Název akce" value=title required=true maxlength="200"/>
                        <Field id="event-location" label="Místo konání" value=location maxlength="300"/>
                        <TextArea id="event-description" label="Popis akce" value=description maxlength="10000"/>
                    </section>
                    <section class="admin-panel admin-form-section"><h2>"Termín"</h2>
                        <p id="event-time-help">{move ||if explicit.get(){"Zadejte datum a čas včetně UTC posunu, například 2026-10-25T02:30:00+02:00. Veřejný web zobrazí odpovídající místní čas ve Vyskři. Před uložením zkontrolujte oba termíny."}else{"Čas odpovídá místnímu času ve Vyskři (Europe/Prague). Při změně letního času může být nutné zadat UTC posun."}}</p>
                        <label class="admin-checkbox"><input type="checkbox" prop:checked=move ||explicit.get() on:change=move |ev|explicit.set(event_target_checked(&ev))/><span>"Zadat čas s UTC posunem"</span></label>
                        <EventDateField id="event-start" label="Začátek" local=start precise=start_precise explicit/>
                        <EventDateField id="event-end" label="Konec" local=end precise=end_precise explicit/>
                    </section>
                    <section class="admin-panel admin-form-section"><h2>"Zveřejnění a stav"</h2>
                        <label class="admin-checkbox"><input type="checkbox" prop:checked=move ||published.get() on:change=move |ev|{published.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span>"Zveřejnit akci"</span></label>
                        <p>"Zveřejněná akce je dostupná v kalendáři a na své stálé adrese. Koncept vidí pouze správci."</p>
                        <label class="admin-checkbox"><input type="checkbox" prop:checked=move ||cancelled.get() on:change=move |ev|{cancelled.set(event_target_checked(&ev));ctx.dirty.set(true);}/><span>"Akce je zrušena"</span></label>
                        <p>"Při zrušení ponechte akci zveřejněnou, aby se návštěvníci o změně dozvěděli."</p>
                    </section>
                </fieldset>
                <button class="button primary" type="submit" disabled=move ||save_disabled.get()>{move ||if save.pending().get(){"Ukládám…"}else{"Uložit akci"}}</button>
            </form>
        </div><aside class="admin-editor-sidebar"><section class="admin-panel admin-form-section"><h2>"Akce v kalendáři"</h2>
            {(id>0).then(||view!{<p>{format!("Verze {version}")}</p><p>{format!("Stálá adresa: /kalendar/{id}")}</p>
                {event.published.then(||view!{<a class="text-link" href=format!("/kalendar/{id}") target="_blank" rel="noopener">"Otevřít veřejný detail"<Icon name="external"/></a>})}
                <p>"Smazání odstraní i veřejný detail. Pro zrušený termín použijte označení Akce je zrušena."</p>
                <ConfirmButton label="Smazat akci" title="Smazat tuto akci?" description="Akce i její veřejný detail budou odstraněny. Záznam o smazání zůstane v auditu." disabled=save_disabled danger=true on_confirm=Callback::new(move |_|{if !save_disabled.get_untracked(){remove.dispatch(());}})/>
            })}
            <p class="admin-caption">"Změny se ukládají do auditu."</p>
        </section></aside></div>
    }
}

#[component]
fn CurrentVersion(event: Event) -> impl IntoView {
    let start = event.start_label();
    let end = event.end_label();
    view! {<section class="admin-form-section"><h3>{format!("Uložená verze {}",event.version)}</h3><p><strong>{event.title}</strong></p><p>{event.location}</p><p>{start}" až "{end}</p><p>{if event.published {"Zveřejněno"}else{"Koncept"}}{event.cancelled.then_some(" · Zrušeno")}</p><div>{event.description.split('\n').map(|line|view!{<p>{line.to_owned()}</p>}).collect_view()}</div></section>}
}
