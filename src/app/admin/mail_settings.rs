use super::{Access, AdminContext, api, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Deserialize)]
struct Settings {
    #[serde(default)]
    capture_only: bool,
    provider: String,
    username: String,
    sender_name: String,
    password_configured: bool,
    smtp_host: String,
    smtp_port: u16,
    smtp_tls: String,
    email_from: String,
}

#[derive(Deserialize)]
struct TestResult {
    recipient: String,
}

fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> api::Result<T> {
    serde_json::from_value(value).map_err(|_| api::Failure {
        status: 0,
        message: "Server vrátil neočekávanou odpověď. Obnovte stránku a ověřte uložené nastavení."
            .into(),
    })
}

#[component]
pub fn MailSettings() -> impl IntoView {
    let settings = LocalResource::new(move || async {
        api::get::<Settings>("/api/v1/admin/mail-settings").await
    });
    view! {
        <Title text="Nastavení odesílání · Správa Vyskře"/>
        <div class="admin-editor-back"><AdminLink href="/admin/posta" label="Zpět na rozesílání" icon="back"/></div>
        <Heading title="Nastavení odesílání" description="Připojte Gmail nebo Google Workspace pro odesílání novinek, potvrzení odběru a obnovy hesel. Uložená změna se použije bez restartu webu."/>
        <Suspense fallback=Pending>{move || settings.get().map(|result| match result {
            Ok(initial) => view! { <SettingsForm initial/> }.into_any(),
            Err(error) => view! {
                <FailureView error/>
                <button class="button secondary" type="button" on:click=move |_| settings.refetch()>"Zkusit znovu"</button>
            }.into_any(),
        })}</Suspense>
    }
}

#[component]
fn SettingsForm(initial: Settings) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let provider = RwSignal::new(initial.provider.clone());
    let username = RwSignal::new(initial.username.clone());
    let sender_name = RwSignal::new(initial.sender_name.clone());
    let saved = RwSignal::new(initial);
    let password = RwSignal::new(String::new());
    let authorize = RwSignal::new(String::new());
    let test_authorize = RwSignal::new(String::new());
    let error = RwSignal::new(None);
    let test_error = RwSignal::new(None);
    let test_success = RwSignal::new(None::<String>);
    let is_google = Signal::derive(move || provider.get() == "google");
    let password_required = Signal::derive(move || {
        let current = saved.get();
        current.provider != "google"
            || !current.password_configured
            || !username
                .get()
                .trim()
                .eq_ignore_ascii_case(&current.username)
    });
    let dirty = Signal::derive(move || {
        let current = saved.get();
        provider.get() != current.provider
            || (is_google.get()
                && (username.get().trim() != current.username
                    || sender_name.get().trim() != current.sender_name
                    || !password.get().is_empty()))
    });
    Effect::new(move |_| {
        ctx.dirty.set(dirty.get());
    });
    let save = Action::new_local(move |_: &()| {
        let app_password = password.get_untracked();
        let body = json!({
            "provider": provider.get_untracked(),
            "username": if is_google.get_untracked() { username.get_untracked() } else { String::new() },
            "sender_name": if is_google.get_untracked() { sender_name.get_untracked() } else { String::new() },
            "password": if is_google.get_untracked() && !app_password.is_empty() { Some(app_password) } else { None },
            "current_password": authorize.get_untracked(),
        });
        async move {
            error.set(None);
            test_error.set(None);
            test_success.set(None);
            let result = api::save("PUT", "/api/v1/admin/mail-settings", Some(body))
                .await
                .and_then(decode::<Settings>);
            authorize.set(String::new());
            test_authorize.set(String::new());
            match result {
                Ok(current) => {
                    provider.set(current.provider.clone());
                    username.set(current.username.clone());
                    sender_name.set(current.sender_name.clone());
                    password.set(String::new());
                    saved.set(current);
                    ctx.dirty.set(false);
                    ctx.changed("Nastavení odesílání bylo uloženo. Nyní odešlete zkušební e-mail.");
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    let send_test = Action::new_local(move |_: &()| {
        let body = json!({"current_password": test_authorize.get_untracked()});
        async move {
            test_error.set(None);
            test_success.set(None);
            let result = api::save("POST", "/api/v1/admin/mail-settings/test", Some(body))
                .await
                .and_then(decode::<TestResult>);
            test_authorize.set(String::new());
            match result {
                Ok(result) => test_success.set(Some(if saved.get_untracked().capture_only {
                    format!("Zkušební e-mail pro {} byl uložen do Mailpitu. Do skutečné schránky se neposílá.", result.recipient)
                } else {
                    format!("SMTP server přijal zkušební e-mail pro {}. Zkontrolujte schránku i složku se spamem. Přijetí serverem ještě nepotvrzuje doručení do schránky.", result.recipient)
                })),
                Err(failure) => test_error.set(Some(failure)),
            }
        }
    });
    let busy = Signal::derive(move || save.pending().get() || send_test.pending().get());
    let recipient = match ctx.access.get_untracked() {
        Access::Ready(session) => session.email,
        _ => String::new(),
    };
    view! {
        <section class="admin-panel admin-form-section">
            <h2>"Odesílací účet"</h2>
            <Show when=move || saved.get().capture_only>
                <p role="status">"Testovací provoz: všechny e-maily zůstávají v Mailpitu. Do skutečných schránek se neposílají a odesílací účet nelze změnit."</p>
            </Show>
            <form on:submit=move |ev| {
                ev.prevent_default();
                if !busy.get_untracked() && dirty.get_untracked() { save.dispatch(()); }
            }>
                <fieldset disabled=move || busy.get() || saved.get().capture_only>
                    <label class="admin-field" for="mail-provider"><span>"Způsob odesílání"</span>
                        <select id="mail-provider" prop:value=move || provider.get() on:change=move |ev| {
                            provider.set(event_target_value(&ev));
                            password.set(String::new());
                            test_success.set(None);
                        }>
                            <option value="environment">"Stávající nastavení serveru"</option>
                            <option value="google">"Google (Gmail / Google Workspace)"</option>
                        </select>
                    </label>
                    <Show when=move || is_google.get() fallback=move || view! {
                        <p class="admin-caption">"Web použije odesílání nastavené provozovatelem serveru. Uložením této volby se připojení k účtu Google odstraní."</p>
                    }>
                        <p class="admin-caption">"U účtu Google zapněte dvoufázové ověření a vytvořte heslo aplikace. Běžné heslo účtu ani samostatný API klíč pro toto připojení nefungují. Google Workspace může hesla aplikací omezovat podle pravidel organizace."</p>
                        <p class="admin-actions">
                            <a class="text-link" href="https://myaccount.google.com/apppasswords" target="_blank" rel="noopener noreferrer">"Vytvořit heslo aplikace u Googlu"</a>
                            <a class="text-link" href="https://support.google.com/accounts/answer/185833?hl=cs" target="_blank" rel="noopener noreferrer">"Nápověda Googlu"</a>
                        </p>
                        <label class="admin-field" for="mail-username"><span id="mail-username-label">"E-mail účtu Google"</span>
                            <input id="mail-username" type="email" required maxlength="254" autocomplete="off" spellcheck="false" aria-labelledby="mail-username-label" aria-describedby="mail-username-help" prop:value=move || username.get() on:input=move |ev| username.set(event_target_value(&ev))/>
                            <small id="mail-username-help">"Tato adresa se použije také jako adresa odesílatele."</small>
                        </label>
                        <label class="admin-field" for="mail-sender"><span id="mail-sender-label">"Jméno odesílatele"</span>
                            <input id="mail-sender" type="text" maxlength="120" placeholder="Obec Vyskeř" aria-labelledby="mail-sender-label" aria-describedby="mail-sender-help" prop:value=move || sender_name.get() on:input=move |ev| sender_name.set(event_target_value(&ev))/>
                            <small id="mail-sender-help">"Například Obec Vyskeř. Bez vyplnění se zobrazí e-mailová adresa."</small>
                        </label>
                        <label class="admin-field" for="mail-app-password"><span id="mail-app-password-label">"Heslo aplikace Google"</span>
                            <input id="mail-app-password" type="password" required=move || password_required.get() maxlength="128" autocomplete="new-password" aria-labelledby="mail-app-password-label" aria-describedby="mail-app-password-help" prop:value=move || password.get() on:input=move |ev| password.set(event_target_value(&ev))/>
                            <small id="mail-app-password-help">{move || if password_required.get() {
                                "Zadejte 16 písmen hesla aplikace vytvořeného pro tento účet Google. Mezery mezi skupinami můžete ponechat."
                            } else {
                                "Heslo aplikace je uložené a nezobrazuje se. Prázdné pole ponechá stávající heslo stejného účtu."
                            }}</small>
                        </label>
                    </Show>
                    <label class="admin-field" for="mail-authorize"><span>"Vaše heslo do administrace pro uložení"</span>
                        <input id="mail-authorize" type="password" required maxlength="1024" autocomplete="current-password" prop:value=move || authorize.get() on:input=move |ev| authorize.set(event_target_value(&ev))/>
                    </label>
                    <ErrorMessage error/>
                    <div class="admin-actions">
                        <button class="button primary" type="submit" disabled=move || !dirty.get()>{move || if save.pending().get() {"Ukládám…"} else {"Uložit nastavení"}}</button>
                        <button class="button secondary" type="button" disabled=move || !dirty.get() on:click=move |_| {
                            let current = saved.get_untracked();
                            provider.set(current.provider);
                            username.set(current.username);
                            sender_name.set(current.sender_name);
                            password.set(String::new());
                            authorize.set(String::new());
                            error.set(None);
                        }>"Zrušit změny"</button>
                    </div>
                </fieldset>
            </form>
        </section>
        <section class="admin-panel admin-form-section" aria-labelledby="mail-test-heading">
            <h2 id="mail-test-heading">"Ověřit uložené nastavení"</h2>
            <p>{format!("Zkušební e-mail odešleme na adresu vašeho účtu správce: {recipient}.")}</p>
            <p class="admin-caption">"Test použije poslední uložené nastavení. Neuložené změny nejprve uložte nebo zrušte."</p>
            <p class="admin-caption">{move || format!("Používaný odesílatel: {}", saved.get().email_from)}</p>
            <details class="admin-caption"><summary>"Technické údaje připojení"</summary>
                <p>{move || {
                    let current = saved.get();
                    format!("SMTP: {} · port {} · zabezpečení {}", current.smtp_host, current.smtp_port, current.smtp_tls)
                }}</p>
            </details>
            <form on:submit=move |ev| {
                ev.prevent_default();
                if !busy.get_untracked() && !dirty.get_untracked() { send_test.dispatch(()); }
            }>
                <fieldset disabled=move || busy.get() || dirty.get()>
                    <label class="admin-field" for="mail-test-authorize"><span>"Vaše heslo do administrace pro test"</span>
                        <input id="mail-test-authorize" type="password" required maxlength="1024" autocomplete="current-password" prop:value=move || test_authorize.get() on:input=move |ev| test_authorize.set(event_target_value(&ev))/>
                    </label>
                    <button class="button secondary" type="submit">{move || if send_test.pending().get() {"Odesílám…"} else {"Odeslat zkušební e-mail"}}</button>
                </fieldset>
            </form>
            <ErrorMessage error=test_error/>
            {move || test_success.get().map(|message| view! { <p role="status">{message}</p> })}
        </section>
    }
}
