use super::{AdminContext, api, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::{Value, json};

const PAGE_SIZE: i64 = 20;

#[derive(Clone, Deserialize)]
struct Subscriber {
    id: i64,
    email: String,
    status: String,
    verified_at: Option<i64>,
    unsubscribed_at: Option<i64>,
}

#[derive(Clone, Deserialize)]
struct SubscriberList {
    items: Vec<Subscriber>,
    total: i64,
    limit: i64,
    offset: i64,
}

fn state_label(status: &str) -> &'static str {
    match status {
        "active" => "Aktivní",
        "unsubscribed" => "Odhlášený",
        _ => "Čeká na potvrzení",
    }
}

fn date(value: Option<i64>) -> String {
    value
        .and_then(|value| time::OffsetDateTime::from_unix_timestamp(value).ok())
        .and_then(|value| {
            value
                .format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "Neuvedeno".into())
}

#[component]
pub fn Subscribers() -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let search = RwSignal::new(String::new());
    let query = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let offset = RwSignal::new(0_i64);
    let refresh = RwSignal::new(0_u64);
    let selected = RwSignal::new(None::<i64>);
    let data = LocalResource::new(move || {
        refresh.get();
        let path = format!(
            "/api/v1/admin/subscribers?limit={PAGE_SIZE}&offset={}&status={}&q={}",
            offset.get(),
            status.get(),
            percent_encoding::utf8_percent_encode(&query.get(), percent_encoding::NON_ALPHANUMERIC),
        );
        async move { api::get::<SubscriberList>(&path).await }
    });
    let leave_detail = move || {
        if !ctx.leave() {
            return false;
        }
        selected.set(None);
        ctx.dirty.set(false);
        true
    };
    view! {
        <Title text="Odběratelé · Správa Vyskře"/>
        <Heading title="Odběratelé novinek" description="Vyhledání odběru, doložení souhlasu a vyřízení odvolání nebo výmazu. Přihlášení a potvrzení odběru probíhá ve veřejném formuláři."/>
        <section class="admin-panel admin-list-panel">
            <form class="admin-filters" on:submit=move |ev| {
                ev.prevent_default();
                if leave_detail() { offset.set(0); query.set(search.get_untracked()); }
            }>
                <label class="admin-search"><span class="sr-only">"Hledat podle e-mailu"</span><input type="search" placeholder="Hledat e-mail…" maxlength="254" prop:value=move ||search.get() on:input=move |ev|search.set(event_target_value(&ev))/></label>
                <label class="admin-status-filter"><span class="sr-only">"Stav odběru"</span><select aria-label="Stav odběru" prop:value=move ||status.get() on:change=move |ev| {
                    if leave_detail() { offset.set(0); status.set(event_target_value(&ev)); }
                    else { status.set(status.get_untracked()); }
                }><option value="">"Všechny stavy"</option><option value="active">"Aktivní"</option><option value="pending">"Čekají na potvrzení"</option><option value="unsubscribed">"Odhlášení"</option></select></label>
                <button class="button secondary" type="submit">"Hledat"</button>
                <button class="button secondary" type="button" on:click=move |_|data.refetch()>"Obnovit seznam"</button>
            </form>
            <p class="admin-caption">"Hledání nerozlišuje velká a malá písmena. Znaky % a _ se hledají doslovně. Aktivní odběr vyžaduje doložený potvrzený souhlas."</p>
            <Suspense fallback=Pending>{move ||data.get().map(|result|match result {
                Err(error)=>view!{<FailureView error/>}.into_any(),
                Ok(page)=>{
                    let first = if page.items.is_empty() {0} else {page.offset + 1};
                    let last = if page.items.is_empty() {0} else {page.offset + page.items.len() as i64};
                    let previous = (page.offset - page.limit).max(0);
                    let next = page.offset + page.limit;
                    // Brace comparisons in attributes so `>` cannot close the RSX tag.
                    view! {
                        <p role="status">{format!("Celkem {} odběratelů. Zobrazeno {first} až {last}.", page.total)}</p>
                        <div class="admin-table-wrap"><table class="admin-table"><caption class="sr-only">"Odběratelé novinek"</caption><thead><tr><th>"E-mail"</th><th>"Stav"</th><th>"Potvrzení / odhlášení (UTC)"</th><th>"Správa"</th></tr></thead><tbody>
                            {page.items.into_iter().map(|subscriber|{
                                let id = subscriber.id;
                                view!{<tr><td>{subscriber.email}<span class="admin-row-id">{format!("#{id}")}</span></td><td>{state_label(&subscriber.status)}</td><td class="admin-table-meta"><span>{date(subscriber.verified_at)}</span><small>{date(subscriber.unsubscribed_at)}</small></td><td><button type="button" class="button secondary" on:click=move |_|{if selected.get_untracked()!=Some(id) && leave_detail(){selected.set(Some(id));}}>"Důkazy a správa"</button></td></tr>}
                            }).collect_view()}
                        </tbody></table></div>
                        {(page.total==0).then(||view!{<p class="admin-empty">"Žádní odběratelé neodpovídají hledání."</p>})}
                        <nav class="admin-pager" aria-label="Stránkování odběratelů"><span>{format!("Strana {}",page.offset/page.limit+1)}</span><div>
                            <button class="button secondary" type="button" disabled={page.offset==0} on:click=move |_|{if leave_detail(){offset.set(previous);}}>"Předchozí"</button>
                            <button class="button secondary" type="button" disabled={next>=page.total} on:click=move |_|{if leave_detail(){offset.set(next);}}>"Další"</button>
                        </div></nav>
                    }.into_any()
                }
            })}</Suspense>
        </section>
        {move ||selected.get().map(|id|view!{<SubscriberDetail id selected refresh/>})}
    }
}

#[component]
fn SubscriberDetail(
    id: i64,
    selected: RwSignal<Option<i64>>,
    refresh: RwSignal<u64>,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let revision = RwSignal::new(0_u64);
    let evidence = LocalResource::new(move || {
        revision.get();
        async move { api::get::<Value>(&format!("/api/v1/admin/subscribers/{id}/export")).await }
    });
    let action = Action::new_local(move |erase: &bool| {
        let erase = *erase;
        let body = json!({"current_password":password.get_untracked()});
        async move {
            error.set(None);
            let (method, path) = if erase {
                ("DELETE", format!("/api/v1/admin/subscribers/{id}"))
            } else {
                ("POST", format!("/api/v1/admin/subscribers/{id}/withdraw"))
            };
            let result = api::save(method, &path, Some(body)).await;
            // The administrator may have confirmed leaving while the request ran.
            if password.try_set(String::new()).is_some() {
                return;
            }
            ctx.dirty.set(false);
            match result {
                Ok(_) => {
                    refresh.update(|value| *value += 1);
                    if erase {
                        ctx.changed("Údaje odběratele, souhlasy, odkazy a uchovaná pošta byly vymazány. V auditu zůstává pouze číselné ID záznamu.");
                        selected.set(None);
                    } else {
                        ctx.changed("Odběr byl odvolán a čekající zprávy zrušeny. Zpráva již předaná k odeslání může ještě dorazit.");
                        revision.update(|value| *value += 1);
                    }
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    Effect::new(move |_| {
        ctx.dirty
            .set(!password.get().is_empty() || action.pending().get())
    });
    on_cleanup(move || {
        ctx.dirty.set(false);
    });
    let disabled = Signal::derive(move || action.pending().get() || password.get().is_empty());
    view! {
        <section class="admin-panel admin-form-section" aria-label="Důkazy a správa odběratele">
            <div class="admin-panel-heading"><h2>{format!("Odběratel #{id}")}</h2><button type="button" class="button secondary" on:click=move |_|{if ctx.leave(){selected.set(None);ctx.dirty.set(false);}}>"Zavřít detail"</button></div>
            <Suspense fallback=Pending>{move ||evidence.get().map(|result|match result {
                Err(error)=>view!{<FailureView error/><button type="button" class="button secondary" on:click=move |_|evidence.refetch()>"Načíst důkazy znovu"</button>}.into_any(),
                Ok(value)=>{
                    let address = value["subscriber"]["email"].as_str().unwrap_or_default().to_owned();
                    let state = state_label(value["subscriber"]["status"].as_str().unwrap_or_default());
                    let note = value["mail_history_note"].as_str().unwrap_or_default().to_owned();
                    let evidence_json = serde_json::to_string_pretty(&value).unwrap_or_default();
                    view!{<p><strong>{address}</strong>" · "{state}</p>
                        <p>"Export obsahuje původní znění souhlasů, informace o soukromí platné při žádosti a uchovanou historii pošty. Nahlédnutí i stažení se zaznamenává do auditu."</p>
                        <p class="admin-caption">{note}</p>
                        <details><summary>"Zobrazit úplné důkazy v JSON"</summary><pre style="white-space: pre-wrap; overflow-wrap: anywhere">{evidence_json}</pre></details>
                        <a class="button secondary" href=format!("/api/v1/admin/subscribers/{id}/export?download=true") download=format!("odberatel-{id}.json") target="_blank" rel="noopener noreferrer">"Stáhnout soukromý export JSON"</a>
                    }.into_any()
                }
            })}</Suspense>
            <h3>"Odvolání odběru a výmaz"</h3>
            <p>"Odvolání zastaví další rozesílání, zruší čekající zprávy a zneplatní všechny odkazy pro potvrzení i odhlášení. Zpráva, kterou už poštovní server převzal nebo právě odesílá, může ještě dorazit."</p>
            <p>"Výmaz trvale odstraní identitu, doklady souhlasu, odkazy a uchovanou poštu. V auditu zůstane číselné ID odběratele. Během probíhajícího odesílání je nutné s výmazem počkat a akci zopakovat podle pokynu serveru."</p>
            <label class="admin-field" for="subscriber-password"><span>"Vaše současné heslo pro potvrzení"</span><input id="subscriber-password" type="password" autocomplete="current-password" maxlength="1024" disabled=move ||action.pending().get() prop:value=move ||password.get() on:input=move |ev|password.set(event_target_value(&ev))/></label>
            <ErrorMessage error/>
            <Show when=move ||action.pending().get()><Pending/></Show>
            <div class="admin-dialog-actions">
                <ConfirmButton label="Odvolat odběr" title="Odvolat tento odběr?" description="Čekající zprávy budou zrušeny a potvrzovací i odhlašovací odkazy zneplatněny. Zpráva právě předávaná poštovnímu serveru může ještě dorazit. Doklad souhlasu se uchová." disabled on_confirm=Callback::new(move |_|{if !action.pending().get_untracked(){action.dispatch(false);}})/>
                <ConfirmButton label="Vymazat odběratele" title=format!("Trvale vymazat odběratele #{id}?") description="Odstraníte e-mail, doklady souhlasu a uchovanou poštu tohoto odběratele. Operaci nelze vrátit. Nový odběr musí příjemce znovu sám přihlásit a potvrdit." disabled danger=true on_confirm=Callback::new(move |_|{if !action.pending().get_untracked(){action.dispatch(true);}})/>
            </div>
        </section>
    }
}
