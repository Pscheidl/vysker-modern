use super::{Access, AdminContext, api, ui::*};
use leptos::prelude::*;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;

#[derive(Clone, Deserialize)]
struct Account {
    id: i64,
    email: String,
    active: bool,
    sessions: i64,
    #[serde(default)]
    is_primary: bool,
    #[serde(default)]
    can_view_identity: bool,
    #[serde(default)]
    can_manage_permissions: bool,
    #[serde(default)]
    can_create_accounts: bool,
    #[serde(default)]
    can_manage_mail_settings: bool,
}

#[derive(Deserialize)]
struct Profile {
    display_name: Option<String>,
}

#[component]
fn ProfileSettings(dirty: RwSignal<bool>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let initial = match ctx.access.get_untracked() {
        Access::Ready(session) => session.display_name.unwrap_or_default(),
        _ => String::new(),
    };
    let saved_name = RwSignal::new(initial.clone());
    let display_name = RwSignal::new(initial);
    let error = RwSignal::new(None);
    let save = Action::new_local(move |_: &()| {
        let input = json!({"display_name": display_name.get_untracked()});
        async move {
            error.set(None);
            let result = api::save("PUT", "/api/v1/admin/profile", Some(input)).await;
            match result {
                Ok(value) => match serde_json::from_value::<Profile>(value) {
                    Ok(profile) => {
                        let name = profile.display_name.clone().unwrap_or_default();
                        saved_name.set(name.clone());
                        display_name.set(name);
                        ctx.changed("Výchozí jméno u komentářů bylo uloženo.");
                        // Keep the mounted account forms and their unsaved values.
                        // The next page reads this updated session when it mounts.
                        ctx.access.update_untracked(|access| {
                            if let Access::Ready(session) = access {
                                session.display_name = profile.display_name;
                            }
                        });
                    }
                    Err(_) => error.set(Some(api::Failure {
                        status: 0,
                        message: "Server vrátil neočekávanou odpověď. Obnovte stránku a ověřte uložené jméno.".into(),
                    })),
                },
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    Effect::new(move |_| {
        dirty.set(display_name.get() != saved_name.get() || save.pending().get());
    });
    view! {
        <section class="admin-panel admin-form-section" id="moje-jmeno">
            <h2>"Moje jméno u komentářů"</h2>
            <p>"Toto jméno se předvyplní u nových komentářů k podnětům a závadám a zobrazí se veřejně. U každého komentáře ho můžete změnit."</p>
            <form on:submit=move |ev| { ev.prevent_default(); if !save.pending().get_untracked() { save.dispatch(()); } }>
                <fieldset disabled=move || save.pending().get()>
                    <label class="admin-field" for="account-display-name">
                        <span>"Výchozí jméno autora"</span>
                        <input id="account-display-name" type="text" autocomplete="name" maxlength="120" aria-describedby="account-display-name-help"
                            prop:value=move || display_name.get()
                            on:input=move |ev| display_name.set(event_target_value(&ev))/>
                    </label>
                    <p id="account-display-name-help" class="admin-caption">"Pokud necháte jméno prázdné, budete ho vyplňovat u každého nového komentáře. Uložené komentáře se tím nezmění."</p>
                    <ErrorMessage error/>
                    <button class="button primary" type="submit">{move || if save.pending().get() { "Ukládám…" } else { "Uložit výchozí jméno" }}</button>
                </fieldset>
            </form>
        </section>
    }
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
    let can_manage_permissions = match ctx.access.get_untracked() {
        Access::Ready(s) => s.can_manage_permissions,
        _ => false,
    };
    let (can_create_accounts, can_view_identity, can_manage_mail_settings) =
        match ctx.access.get_untracked() {
            Access::Ready(s) => (
                s.can_create_accounts,
                s.can_view_identity,
                s.can_manage_mail_settings,
            ),
            _ => (false, false, false),
        };
    let refresh = RwSignal::new(0u64);
    let profile_dirty = RwSignal::new(false);
    let dirty_rows = RwSignal::new(BTreeSet::<i64>::new());
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
    let new_view_identity = RwSignal::new(false);
    let new_manage_permissions = RwSignal::new(false);
    let new_create_accounts = RwSignal::new(false);
    let new_manage_mail_settings = RwSignal::new(false);
    let create_error = RwSignal::new(None);
    let create = Action::new_local(move |_: &()| {
        let body = json!({"email":email.get_untracked(),"password":initial.get_untracked(),"current_password":authorize.get_untracked(),"can_view_identity":new_view_identity.get_untracked(),"can_manage_permissions":new_manage_permissions.get_untracked(),"can_create_accounts":new_create_accounts.get_untracked(),"can_manage_mail_settings":new_manage_mail_settings.get_untracked()});
        async move {
            create_error.set(None);
            let result = api::save("POST", "/api/v1/admin/accounts", Some(body)).await;
            initial.set(String::new());
            authorize.set(String::new());
            match result {
                Ok(_) => {
                    email.set(String::new());
                    new_view_identity.set(false);
                    new_manage_permissions.set(false);
                    new_create_accounts.set(false);
                    new_manage_mail_settings.set(false);
                    refresh.update(|v| *v += 1);
                    ctx.changed("Účet správce byl vytvořen.");
                }
                Err(e) => create_error.set(Some(e)),
            }
        }
    });
    Effect::new(move |_| {
        ctx.dirty.set(
            profile_dirty.get()
                || !dirty_rows.get().is_empty()
                || !current.get().is_empty()
                || !new_password.get().is_empty()
                || !repeat.get().is_empty()
                || !email.get().is_empty()
                || !initial.get().is_empty()
                || !authorize.get().is_empty()
                || new_view_identity.get()
                || new_manage_permissions.get()
                || new_create_accounts.get()
                || new_manage_mail_settings.get()
                || change.pending().get()
                || create.pending().get(),
        );
    });
    view! {
        <Heading title="Účty a hesla" description="Správci mají základní přístup ke správě webu. Správu rezervací a přístup k dokladům, vytváření účtů, změnu nastavení pošty a správu oprávnění je nutné jednotlivě povolit. Změny účtů se zaznamenávají do auditu."/>
        <ProfileSettings dirty=profile_dirty/>
        <section class="admin-panel admin-form-section"><h2>"Moje heslo"</h2><p>"Po změně hesla se všechna vaše přihlášení ukončí. Přihlaste se znovu novým heslem."</p>
            <form on:submit=move |ev|{ev.prevent_default(); if !change.pending().get_untracked(){change.dispatch(());}}><fieldset disabled=move ||change.pending().get()>
                <PasswordInput id="account-current" label="Současné heslo" value=current/>
                <PasswordInput id="account-new" label="Nové heslo" value=new_password new=true/>
                <PasswordInput id="account-repeat" label="Nové heslo znovu" value=repeat new=true/>
                <ErrorMessage error/><button class="button primary" type="submit">"Změnit heslo a odhlásit se"</button>
            </fieldset></form>
        </section>
        <section class="admin-panel admin-form-section"><h2>"Správci webu"</h2>
            {(!can_manage_permissions).then(|| view! { <p class="admin-caption">"Se základním přístupem můžete spravovat web a prohlížet rezervace. Pro správu rezervací a přístup k dokladům, vytváření účtů, změnu nastavení pošty a správu oprávnění jsou potřeba další práva. Bez oprávnění ke správě práv nelze měnit oprávnění ani spravovat přístupy jiných účtů s dodatečnými oprávněními."</p> })}
            <Suspense fallback=move ||view!{<Pending/>}>{move ||accounts.get().map(|result|match result {
                Err(error)=>view!{<FailureView error/>}.into_any(),
                Ok(items)=>view!{<div class="admin-accounts">{items.into_iter().map(|account|view!{<AccountRow account own_id refresh dirty_rows can_manage_permissions can_create_accounts can_view_identity can_manage_mail_settings/>}).collect_view()}</div>}.into_any(),
            })}</Suspense>
        </section>
        {can_create_accounts.then(|| view! { <section class="admin-panel admin-form-section"><h2>"Nový správce"</h2><p>"Počáteční heslo předejte příjemci bezpečnou cestou. Po přihlášení si ho může změnit. Můžete mu přidělit pouze oprávnění, která sami máte."</p>
            <form on:submit=move |ev|{ev.prevent_default(); if !create.pending().get_untracked(){create.dispatch(());}}><fieldset disabled=move ||create.pending().get()>
                <Field id="account-email" label="E-mail nového správce" value=email kind="email" required=true maxlength="254"/>
                <PasswordInput id="account-initial" label="Počáteční heslo" value=initial new=true/>
                <fieldset class="admin-field admin-permissions"><legend>"Dodatečná oprávnění"</legend>
                    <label><input type="checkbox" disabled=!can_view_identity prop:checked=move || new_view_identity.get() on:change=move |ev| {let checked=event_target_checked(&ev); new_view_identity.set(checked); if !checked {new_manage_mail_settings.set(false);}}/>" Správa rezervací a přístup k dokladům"</label>
                    <label><input type="checkbox" disabled=!can_create_accounts prop:checked=move || new_create_accounts.get() on:change=move |ev| {let checked=event_target_checked(&ev); new_create_accounts.set(checked); if !checked {new_manage_mail_settings.set(false);}}/>" Vytváření účtů správců"</label>
                    <label><input type="checkbox" disabled=move || !can_manage_mail_settings || !new_view_identity.get() || !new_create_accounts.get() || !new_manage_permissions.get() prop:checked=move || new_manage_mail_settings.get() on:change=move |ev| new_manage_mail_settings.set(event_target_checked(&ev))/>" Změna nastavení odesílacího účtu"</label>
                    <label><input type="checkbox" disabled=!can_manage_permissions prop:checked=move || new_manage_permissions.get() on:change=move |ev| {let checked=event_target_checked(&ev); new_manage_permissions.set(checked); if !checked {new_manage_mail_settings.set(false);}}/>" Správa oprávnění ostatních správců"</label>
                    <small>"Změnu odesílacího účtu lze povolit pouze správci se všemi třemi ostatními oprávněními. Odebrání některého z nich současně odebere i toto oprávnění."</small>
                </fieldset>
                <PasswordInput id="account-authorize" label="Vaše současné heslo pro potvrzení" value=authorize/>
                <ErrorMessage error=create_error/><button class="button primary" type="submit">"Vytvořit správce"</button>
            </fieldset></form>
        </section> })}
        <p class="admin-caption">"Zapomenuté heslo obnoví provozovatel příkazem na serveru. Obnova ukončí všechna přihlášení daného správce."</p>
    }
}

#[component]
fn AccountRow(
    account: Account,
    own_id: i64,
    refresh: RwSignal<u64>,
    dirty_rows: RwSignal<BTreeSet<i64>>,
    can_manage_permissions: bool,
    can_create_accounts: bool,
    can_view_identity: bool,
    can_manage_mail_settings: bool,
) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let id = account.id;
    let active = account.active;
    let primary = account.is_primary;
    let initial_view = account.can_view_identity;
    let initial_manage = account.can_manage_permissions;
    let initial_create = account.can_create_accounts;
    let initial_mail = account.can_manage_mail_settings;
    let view_identity = RwSignal::new(initial_view);
    let manage_permissions = RwSignal::new(initial_manage);
    let create_accounts = RwSignal::new(initial_create);
    let manage_mail_settings = RwSignal::new(initial_mail);
    let save_permissions = Action::new_local(move |_: &()| {
        let body = json!({"current_password":password.get_untracked(),"can_view_identity":view_identity.get_untracked(),"can_manage_permissions":manage_permissions.get_untracked(),"can_create_accounts":create_accounts.get_untracked(),"can_manage_mail_settings":manage_mail_settings.get_untracked()});
        async move {
            error.set(None);
            let result = api::save(
                "PUT",
                &format!("/api/v1/admin/accounts/{id}/permissions"),
                Some(body),
            )
            .await;
            password.set(String::new());
            match result {
                Ok(_) => {
                    refresh.update(|v| *v += 1);
                    ctx.changed("Oprávnění správce byla uložena.");
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
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
    let disabled = Signal::derive(move || {
        action.pending().get() || save_permissions.pending().get() || password.get().is_empty()
    });
    Effect::new(move |_| {
        let dirty = !password.get().is_empty()
            || action.pending().get()
            || save_permissions.pending().get()
            || view_identity.get() != initial_view
            || manage_permissions.get() != initial_manage;
        let dirty = dirty
            || create_accounts.get() != initial_create
            || manage_mail_settings.get() != initial_mail;
        dirty_rows.update(|rows| {
            if dirty {
                rows.insert(id);
            } else {
                rows.remove(&id);
            }
        });
    });
    on_cleanup(move || {
        let _ = dirty_rows.try_update(|rows| {
            rows.remove(&id);
        });
    });
    view! {<details class="admin-account"><summary><strong>{account.email}</strong>" · "{if active {"Aktivní"} else {"Neaktivní"}}{(id==own_id).then_some(" · váš účet")}{primary.then_some(" · první správce")}<span>{format!(" · {} přihlášení",account.sessions)}</span></summary>
        <p>"Deaktivace zabrání přihlášení a ukončí všechny relace. Posledního aktivního správce nelze deaktivovat."</p>
        {if can_manage_permissions { view! { <fieldset class="admin-field admin-permissions" disabled=primary><legend>"Dodatečná oprávnění"</legend>
            <label><input type="checkbox" disabled=move || !can_view_identity || (manage_mail_settings.get() && !can_manage_mail_settings) prop:checked=move || view_identity.get() on:change=move |ev| {let checked=event_target_checked(&ev); view_identity.set(checked); if !checked && can_manage_mail_settings {manage_mail_settings.set(false);}}/>" Správa rezervací a přístup k dokladům"</label>
            <label><input type="checkbox" disabled=move || !can_create_accounts || (manage_mail_settings.get() && !can_manage_mail_settings) prop:checked=move || create_accounts.get() on:change=move |ev| {let checked=event_target_checked(&ev); create_accounts.set(checked); if !checked && can_manage_mail_settings {manage_mail_settings.set(false);}}/>" Vytváření účtů správců"</label>
            <label><input type="checkbox" disabled=move || !can_manage_mail_settings || !view_identity.get() || !create_accounts.get() || !manage_permissions.get() prop:checked=move || manage_mail_settings.get() on:change=move |ev| manage_mail_settings.set(event_target_checked(&ev))/>" Změna nastavení odesílacího účtu"</label>
            <label><input type="checkbox" disabled=move || manage_mail_settings.get() && !can_manage_mail_settings prop:checked=move || manage_permissions.get() on:change=move |ev| {let checked=event_target_checked(&ev); manage_permissions.set(checked); if !checked && can_manage_mail_settings {manage_mail_settings.set(false);}}/>" Správa oprávnění ostatních správců"</label>
            <small>"Změnu odesílacího účtu lze povolit pouze správci se všemi třemi ostatními oprávněními. Odebrání některého z nich současně odebere i toto oprávnění."</small>
        </fieldset> }.into_any() } else { view! { <p>{format!("Správa rezervací a přístup k dokladům: {}. Vytváření účtů: {}. Změna nastavení pošty: {}. Správa oprávnění: {}.", if initial_view {"ano"} else {"ne"}, if initial_create {"ano"} else {"ne"}, if initial_mail {"ano"} else {"ne"}, if initial_manage {"ano"} else {"ne"})}</p> }.into_any() }}
        {primary.then_some(view! { <p class="admin-caption">"První správce má vždy všechna oprávnění."</p> })}
        <label class="admin-field" for=password_id.clone()><span>"Vaše současné heslo pro potvrzení"</span><input id=password_id.clone() type="password" autocomplete="current-password" maxlength="1024" prop:value=move ||password.get() on:input=move |ev|password.set(event_target_value(&ev))/></label>
        <ErrorMessage error/>
        <div class="admin-dialog-actions">
            {(can_manage_permissions && !primary).then(|| view! { <button type="button" class="button secondary" disabled=move || disabled.get() || (view_identity.get() == initial_view && manage_permissions.get() == initial_manage && create_accounts.get() == initial_create && manage_mail_settings.get() == initial_mail) on:click=move |_| {save_permissions.dispatch(());} >"Uložit oprávnění"</button> })}
            <ConfirmButton label=if active {"Deaktivovat účet"} else {"Aktivovat účet"} title="Změna přístupu správce" description="Změna se uloží do auditu. Při deaktivaci správce ztratí přístup k administraci." disabled on_confirm=Callback::new(move |_|{action.dispatch(false);}) danger=active/>
            <ConfirmButton label="Odhlásit všechna zařízení" title="Ukončit všechna přihlášení?" description="Správce se bude muset na všech zařízeních znovu přihlásit." disabled on_confirm=Callback::new(move |_|{action.dispatch(true);})/>
        </div>
    </details>}
}
