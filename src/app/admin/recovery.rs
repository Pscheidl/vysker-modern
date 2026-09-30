use super::{api, ui::*};
use leptos::prelude::*;
use serde_json::{Value, json};

#[component]
pub fn Recovery() -> impl IntoView {
    let email = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let repeated = RwSignal::new(String::new());
    let token = RwSignal::new(String::new());
    let minimum = RwSignal::new(24usize);
    let error = RwSignal::new(None);
    let done = RwSignal::new(false);
    let generation = RwSignal::new(0_u64);
    #[cfg(feature = "hydrate")]
    {
        let receive = move || {
            if let Ok(fragment) = window().location().hash() {
                if let Some(secret) = fragment.strip_prefix("#token=") {
                    generation.update(|v| *v += 1);
                    token.set(secret.to_owned());
                    done.set(false);
                    error.set(None);
                    password.set(String::new());
                    repeated.set(String::new());
                    let _ = window().history().and_then(|h| {
                        h.replace_state_with_url(
                            &wasm_bindgen::JsValue::NULL,
                            "",
                            Some("/admin/obnova"),
                        )
                    });
                }
            }
        };
        Effect::new(move |_| receive());
        let listener = window_event_listener(leptos::ev::hashchange, move |_| receive());
        on_cleanup(move || listener.remove());
    }
    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match api::get::<Value>("/api/v1/admin/password-policy").await {
                Ok(v) => minimum.set(v["minimum_password_length"].as_u64().unwrap_or(24) as usize),
                Err(e) => error.set(Some(e)),
            }
        });
    });
    let submit = Action::new_local(move |_: &()| {
        let submitted_generation = generation.get_untracked();
        let secret = token.get_untracked();
        let resetting = !secret.is_empty();
        let input = if resetting {
            json!({"token":secret,"password":password.get_untracked()})
        } else {
            json!({"email":email.get_untracked()})
        };
        async move {
            error.set(None);
            if resetting && password.get_untracked() != repeated.get_untracked() {
                error.set(Some(api::Failure {
                    status: 400,
                    message: "Hesla se neshodují.".into(),
                }));
                return;
            }
            let path = if resetting {
                "/api/v1/admin/password-reset"
            } else {
                "/api/v1/admin/password-recovery"
            };
            let result = api::request::<Value>("POST", path, Some(input), None).await;
            if generation.try_get_untracked() != Some(submitted_generation) {
                return;
            }
            match result {
                Ok(_) => {
                    password.set(String::new());
                    repeated.set(String::new());
                    done.set(true);
                }
                Err(e) => error.set(Some(e)),
            }
        }
    });
    view! {<div class="admin-login"><div class="admin-login-card"><h1>"Obnova hesla"</h1>
        <Show when=move||!done.get() fallback=move||view!{<p role="status">{if token.get().is_empty(){"Pokud pro tuto adresu existuje aktivní účet, přijde e-mail s odkazem. Zkontrolujte i nevyžádanou poštu."}else{"Heslo bylo změněno. Přihlaste se novým heslem."}}</p>}>
            <p>"Odkaz pro obnovu platí 30 minut. Změna hesla odhlásí všechna zařízení."</p>
            <form on:submit=move|ev|{ev.prevent_default();if !submit.pending().get_untracked(){submit.dispatch(());}}><fieldset disabled=move||submit.pending().get()>
                <Show when=move||token.get().is_empty() fallback=move||view!{
                    <label class="admin-field" for="reset-password"><span>{move||format!("Nové heslo (alespoň {} znaků)",minimum.get())}</span><input id="reset-password" type="password" autocomplete="new-password" required minlength=move||minimum.get() maxlength="1024" prop:value=move||password.get() on:input=move|ev|password.set(event_target_value(&ev))/></label>
                    <label class="admin-field" for="reset-repeat"><span>"Nové heslo znovu"</span><input id="reset-repeat" type="password" autocomplete="new-password" required maxlength="1024" prop:value=move||repeated.get() on:input=move|ev|repeated.set(event_target_value(&ev))/></label>
                }><label class="admin-field" for="recovery-email"><span>"E-mail"</span><input id="recovery-email" type="email" autocomplete="username" required maxlength="254" prop:value=move||email.get() on:input=move|ev|email.set(event_target_value(&ev))/></label></Show>
                <ErrorMessage error/><button class="button primary" type="submit">{move||if token.get().is_empty(){"Poslat odkaz"}else{"Změnit heslo"}}</button>
            </fieldset></form>
        </Show><a href="/admin" class="back-link">"Zpět k přihlášení"</a>
    </div></div>}
}
