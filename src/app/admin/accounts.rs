use super::{Access, AdminContext, api, ui::*};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Deserialize)]
struct Account {
    id: i64,
    email: String,
    active: bool,
    sessions: i64,
}

#[component]
fn PasswordInput(
    id: &'static str,
    label: &'static str,
    value: RwSignal<String>,
    #[prop(default = false)] new: bool,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let minimum = Signal::derive(move || match ctx.access.get() {
        Access::Ready(session) => session.minimum_password_length,
        _ => 24,
    });
    view! {<label class="admin-field" for=id><span>{move || if new {format!("{label} (alespoň {} znaků)", minimum.get())} else {label.into()}}</span><input id=id type="password" required minlength=move ||if new {minimum.get()} else {1} maxlength="1024" autocomplete=if new {"new-password"} else {"current-password"} prop:value=move ||value.get() on:input=move |ev|value.set(event_target_value(&ev))/></label>}
}

#[component]
pub fn Accounts() -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let own_id = match ctx.access.get_untracked() {
        Access::Ready(s) => s.administrator_id,
        _ => 0,
    };
    let refresh = RwSignal::new(0u64);
    let accounts = LocalResource::new(move || {
        refresh.get();
        async { api::get::<Vec<Account>>("/api/v1/admin/accounts").await }
    });
    let current = RwSignal::new(String::new());
    let new_password = RwSignal::new(String::new());
    let repeat = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let change = Action::new_local(move |_: &()| {
        let matches = new_password.get_untracked() == repeat.get_untracked();
        let body = json!({"current_password":current.get_untracked(),"new_password":new_password.get_untracked()});
        async move {
            error.set(None);
            if !matches {
                error.set(Some(api::Failure {
                    status: 400,
                    message: "Nová hesla se neshodují.".into(),
                }));
                return;
            }
            let result = api::save("PUT", "/api/v1/admin/password", Some(body)).await;
            current.set(String::new());
            new_password.set(String::new());
            repeat.set(String::new());
            match result {
                Ok(_) => {
                    ctx.dirty.set(false);
                    ctx.access.set(Access::SignedOut);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
    let email = RwSignal::new(String::new());
    let initial = RwSignal::new(String::new());
    let authorize = RwSignal::new(String::new());
    let create_error = RwSignal::new(None);
    let create = Action::new_local(move |_: &()| {
        let body = json!({"email":email.get_untracked(),"password":initial.get_untracked(),"current_password":authorize.get_untracked()});
        async move {
            create_error.set(None);
            let result = api::save("POST", "/api/v1/admin/accounts", Some(body)).await;
            initial.set(String::new());
            authorize.set(String::new());
            match result {
                Ok(_) => {
                    email.set(String::new());
                    ctx.dirty.set(false);
                    refresh.update(|v| *v += 1);
                    ctx.changed("Účet správce byl vytvořen.");
                }
                Err(e) => create_error.set(Some(e)),
            }
        }
    });
    view! {
        <Heading title="Účty a hesla" description="Správci mají plný přístup ke správě webu. Změny účtů se zaznamenávají do auditu."/>
        <section class="admin-panel admin-form-section"><h2>"Moje heslo"</h2><p>"Po změně hesla se všechna vaše přihlášení ukončí. Přihlaste se znovu novým heslem."</p>
            <form on:submit=move |ev|{ev.prevent_default(); if !change.pending().get_untracked(){change.dispatch(());}}><fieldset disabled=move ||change.pending().get()>
                <PasswordInput id="account-current" label="Současné heslo" value=current/>
                <PasswordInput id="account-new" label="Nové heslo" value=new_password new=true/>
                <PasswordInput id="account-repeat" label="Nové heslo znovu" value=repeat new=true/>
                <ErrorMessage error/><button class="button primary" type="submit">"Změnit heslo a odhlásit se"</button>
            </fieldset></form>
        </section>
        <section class="admin-panel admin-form-section"><h2>"Správci webu"</h2>
            <Suspense fallback=move ||view!{<Pending/>}>{move ||accounts.get().map(|result|match result {
                Err(error)=>view!{<FailureView error/>}.into_any(),
                Ok(items)=>view!{<div class="admin-accounts">{items.into_iter().map(|account|view!{<AccountRow account own_id refresh/>}).collect_view()}</div>}.into_any(),
            })}</Suspense>
        </section>
        <section class="admin-panel admin-form-section"><h2>"Nový správce"</h2><p>"Počáteční heslo předejte příjemci bezpečnou cestou. Po přihlášení si ho může změnit."</p>
            <form on:submit=move |ev|{ev.prevent_default(); if !create.pending().get_untracked(){create.dispatch(());}}><fieldset disabled=move ||create.pending().get()>
                <Field id="account-email" label="E-mail nového správce" value=email kind="email" required=true maxlength="254"/>
                <PasswordInput id="account-initial" label="Počáteční heslo" value=initial new=true/>
                <PasswordInput id="account-authorize" label="Vaše současné heslo pro potvrzení" value=authorize/>
                <ErrorMessage error=create_error/><button class="button primary" type="submit">"Vytvořit správce"</button>
            </fieldset></form>
        </section>
        <p class="admin-caption">"Zapomenuté heslo obnoví provozovatel příkazem na serveru. Obnova ukončí všechna přihlášení daného správce."</p>
    }
}

#[component]
fn AccountRow(account: Account, own_id: i64, refresh: RwSignal<u64>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let id = account.id;
    let active = account.active;
    let action = Action::new_local(move |revoke: &bool| {
        let revoke = *revoke;
        let body = if revoke {
            json!({"current_password":password.get_untracked()})
        } else {
            json!({"current_password":password.get_untracked(),"active":!active})
        };
        async move {
            error.set(None);
            let path = if revoke {
                format!("/api/v1/admin/accounts/{id}/sessions")
            } else {
                format!("/api/v1/admin/accounts/{id}")
            };
            let result = api::save(if revoke { "DELETE" } else { "PUT" }, &path, Some(body)).await;
            password.set(String::new());
            match result {
                Ok(_) => {
                    if id == own_id {
                        ctx.dirty.set(false);
                        ctx.access.set(Access::SignedOut);
                    } else {
                        refresh.update(|v| *v += 1);
                        ctx.changed("Nastavení účtu bylo změněno.");
                    }
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
    let password_id = format!("account-reauth-{id}");
    let disabled = Signal::derive(move || action.pending().get() || password.get().is_empty());
    view! {<details class="admin-account"><summary><strong>{account.email}</strong>" · "{if active {"Aktivní"} else {"Neaktivní"}}{(id==own_id).then_some(" · váš účet")}<span>{format!(" · {} přihlášení",account.sessions)}</span></summary>
        <p>"Deaktivace zabrání přihlášení a ukončí všechny relace. Posledního aktivního správce nelze deaktivovat."</p>
        <label class="admin-field" for=password_id.clone()><span>"Vaše současné heslo pro potvrzení"</span><input id=password_id.clone() type="password" autocomplete="current-password" maxlength="1024" prop:value=move ||password.get() on:input=move |ev|password.set(event_target_value(&ev))/></label>
        <ErrorMessage error/>
        <div class="admin-dialog-actions">
            <ConfirmButton label=if active {"Deaktivovat účet"} else {"Aktivovat účet"} title="Změna přístupu správce" description="Změna se uloží do auditu. Při deaktivaci správce ztratí přístup k administraci." disabled on_confirm=Callback::new(move |_|{action.dispatch(false);}) danger=active/>
            <ConfirmButton label="Odhlásit všechna zařízení" title="Ukončit všechna přihlášení?" description="Správce se bude muset na všech zařízeních znovu přihlásit." disabled on_confirm=Callback::new(move |_|{action.dispatch(true);})/>
        </div>
    </details>}
}
