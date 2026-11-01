use super::{AdminContext, api, rentals::refresh_rental_permissions, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Debug, Deserialize)]
struct PaymentSettings {
    account_number: String,
    bank_code: String,
    iban: String,
    due_days: i32,
}

#[component]
pub fn RentalPaymentSettings() -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let data = LocalResource::new(move || async move {
        let can_manage = refresh_rental_permissions(ctx).await?;
        let settings = api::get::<PaymentSettings>("/api/v1/admin/rental-payment-settings").await?;
        Ok::<_, api::Failure>((settings, can_manage))
    });
    view! {
        <Title text="Platební údaje pronájmů · Správa Vyskře"/>
        <div class="rental-admin">
            <div class="rental-admin-back"><AdminLink href="/admin/pronajmy" label="Zpět na pronájmy" icon="back"/></div>
            <Heading title="Platební údaje" description="Účet a splatnost se předvyplní při přípravě vyúčtování. U jednotlivého nájmu je můžete změnit."/>
            <Suspense fallback=Pending>{move || data.get().map(|result| match result {
                Ok((settings, can_manage)) => view! {<PaymentSettingsForm settings can_manage/>}.into_any(),
                Err(error) => view! {<FailureView error/>}.into_any(),
            })}</Suspense>
        </div>
    }
}

#[component]
fn PaymentSettingsForm(settings: PaymentSettings, can_manage: bool) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let saved = RwSignal::new(settings.clone());
    let account = RwSignal::new(settings.account_number);
    let bank = RwSignal::new(settings.bank_code);
    let iban = RwSignal::new(settings.iban);
    let due = RwSignal::new(settings.due_days.to_string());
    let error = RwSignal::new(None::<api::Failure>);
    let save = Action::new_local(move |_: &()| {
        let input = json!({
            "account_number": account.get_untracked(),
            "bank_code": bank.get_untracked(),
            "iban": iban.get_untracked(),
            "due_days": due.get_untracked().parse::<i32>().unwrap_or(0),
        });
        async move {
            error.set(None);
            match api::save("PUT", "/api/v1/admin/rental-payment-settings", Some(input)).await {
                Ok(value) => match serde_json::from_value::<PaymentSettings>(value) {
                    Ok(settings) => {
                        account.set(settings.account_number.clone());
                        bank.set(settings.bank_code.clone());
                        iban.set(settings.iban.clone());
                        due.set(settings.due_days.to_string());
                        saved.set(settings);
                        ctx.changed("Platební údaje byly uloženy.");
                    }
                    Err(_) => error.set(Some(api::Failure {
                        status: 0,
                        message: "Údaje byly uloženy, ale odpověď nelze přečíst. Obnovte stránku."
                            .into(),
                    })),
                },
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    Effect::new(move |_| {
        let original = saved.get();
        ctx.dirty.set(
            account.get() != original.account_number
                || bank.get() != original.bank_code
                || iban.get() != original.iban
                || due.get() != original.due_days.to_string()
                || save.pending().get(),
        );
    });
    view! {
        <section class="admin-panel admin-form-section rental-detail-card">
            <span class="rental-admin-kicker">"PRO VYÚČTOVÁNÍ PRONÁJMŮ"</span>
            <h2>"Výchozí platební údaje"</h2>
            {(!can_manage).then(|| view! {<p class="admin-caption">"Platební údaje můžete prohlížet. Pro změnu požádejte pověřeného správce."</p>})}
            <form on:submit=move |event| {event.prevent_default(); if can_manage && !save.pending().get_untracked() {save.dispatch(());}}>
                <fieldset disabled=move || !can_manage || save.pending().get()>
                    <label class="admin-field"><span>"Číslo účtu obce"</span>
                        <input type="text" maxlength="17" autocomplete="off" placeholder="19-2000145399" prop:value=move || account.get() on:input=move |event| account.set(event_target_value(&event))/>
                        <small>"Zadejte české číslo účtu, případné předčíslí oddělte pomlčkou."</small>
                    </label>
                    <label class="admin-field"><span>"Kód banky"</span>
                        <input type="text" maxlength="4" inputmode="numeric" autocomplete="off" placeholder="0800" prop:value=move || bank.get() on:input=move |event| bank.set(event_target_value(&event))/>
                    </label>
                    <label class="admin-field"><span>"IBAN pro zahraniční platbu (nepovinné)"</span>
                        <input type="text" maxlength="40" autocomplete="off" prop:value=move || iban.get() on:input=move |event| iban.set(event_target_value(&event))/>
                        <small>"Pokud jej zadáte, musí odpovídat uvedenému účtu. Pro český QR kód se vypočítá automaticky."</small>
                    </label>
                    <label class="admin-field"><span>"Výchozí splatnost ve dnech"</span>
                        <input type="number" min="1" max="90" step="1" required prop:value=move || due.get() on:input=move |event| due.set(event_target_value(&event))/>
                    </label>
                    <p class="admin-caption">"Variabilním symbolem je vždy číslo žádosti o pronájem."</p>
                    <ErrorMessage error/>
                    {can_manage.then(|| view! {<button type="submit" class="button primary">{move || if save.pending().get() {"Ukládám…"} else {"Uložit platební údaje"}}</button>})}
                </fieldset>
            </form>
        </section>
    }
}
