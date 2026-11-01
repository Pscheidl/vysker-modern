use super::{
    AdminContext, api,
    rentals::{Asset, date_display, refresh_rental_permissions},
    ui::*,
};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::Deserialize;
use serde_json::json;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/public/rental-clock.js")]
extern "C" {
    fn rental_prague_clock() -> String;
}

fn current_clock() -> String {
    #[cfg(feature = "hydrate")]
    {
        rental_prague_clock()
    }
    #[cfg(not(feature = "hydrate"))]
    {
        String::new()
    }
}

fn first_available_day(hourly: bool, now: &str) -> String {
    let Some(today) = now.get(..10) else {
        return String::new();
    };
    if hourly {
        return today.to_owned();
    }
    selected_date(today)
        .and_then(|day| day.next_day())
        .and_then(|day| {
            day.format(time::macros::format_description!("[year]-[month]-[day]"))
                .ok()
        })
        .unwrap_or_default()
}

fn start_time_allowed(date: &str, value: &str, now: &str) -> bool {
    if date.is_empty() || value.is_empty() || is_weekend(date) {
        return false;
    }
    let Some(today) = now.get(..10) else {
        return true;
    };
    date > today || (date == today && Some(value) > now.get(11..16))
}

fn period_error(
    start: &str,
    end: &str,
    hourly: bool,
    from: &str,
    to: &str,
    now: &str,
) -> Option<&'static str> {
    if selected_date(start).is_none() {
        return Some("Vyberte datum pronájmu.");
    }
    if start < first_available_day(hourly, now).as_str() {
        return Some(if hourly {
            "Začátek pronájmu nesmí být v minulosti."
        } else {
            "Celodenní pronájem začíná o půlnoci. Vyberte některý z příštích dnů."
        });
    }
    if hourly {
        if !start_time_allowed(start, from, now) {
            return Some("Vyberte budoucí hodinu začátku pronájmu.");
        }
        if to.is_empty() || to <= from {
            return Some("Konec pronájmu musí být později než jeho začátek.");
        }
    } else if selected_date(end).is_none() || end < start {
        return Some("Konec pronájmu nesmí být před jeho začátkem.");
    }
    None
}

fn selected_date(value: &str) -> Option<time::Date> {
    time::Date::parse(
        value,
        time::macros::format_description!("[year]-[month]-[day]"),
    )
    .ok()
}

fn is_weekend(value: &str) -> bool {
    selected_date(value).is_some_and(|date| {
        matches!(
            date.weekday(),
            time::Weekday::Saturday | time::Weekday::Sunday
        )
    })
}

fn last_rental_day(value: &str) -> String {
    let Some(date) = selected_date(value) else {
        return value.to_owned();
    };
    let date = date.checked_add(time::Duration::days(27)).unwrap_or(date);
    date.format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_else(|_| value.to_owned())
}

fn monthly_description(value: &str) -> String {
    let Some(date) = selected_date(value) else {
        return "Každý měsíc ve stejný pořadový den týdne, například druhý čtvrtek. Nejprve zvolte datum prvního termínu.".into();
    };
    let ordinal = (date.day() - 1) / 7 + 1;
    let weekday = [
        "pondělí",
        "úterý",
        "středa",
        "čtvrtek",
        "pátek",
        "sobota",
        "neděle",
    ][usize::from(date.weekday().number_days_from_monday())];
    let missing = if ordinal == 5 {
        " Pokud tento den v měsíci není, měsíc se vynechá."
    } else {
        ""
    };
    format!("Každý měsíc: {ordinal}. {weekday}.{missing}")
}

#[derive(Clone, Deserialize)]
struct PreviewOccurrence {
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    conflict: bool,
}

#[derive(Clone, Deserialize)]
struct Preview {
    occurrences: Vec<PreviewOccurrence>,
    conflict_count: usize,
    total_price: i64,
    occurrence_count: usize,
}

#[component]
pub fn RentalCreate() -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let data = LocalResource::new(move || async move {
        let allowed = refresh_rental_permissions(ctx).await?;
        let assets = api::get::<Vec<Asset>>("/api/v1/admin/rental-assets").await?;
        Ok::<_, api::Failure>((allowed, assets))
    });
    view! {
        <Title text="Nová rezervace · Správa Vyskře"/>
        <div class="rental-admin">
            <div class="rental-admin-back"><AdminLink href="/admin/pronajmy" label="Zpět na pronájmy" icon="back"/></div>
            <Heading title="Vytvořit rezervaci" description="Jednorázový nebo pravidelný pronájem s individuálně sjednanou cenou."/>
            <Suspense fallback=Pending>{move || data.get().map(|result| match result {
                Err(error) => view! {<FailureView error/>}.into_any(),
                Ok((false, _)) => view! {<p class="admin-caption">"Pro vytvoření rezervace potřebujete oprávnění ke správě rezervací."</p>}.into_any(),
                Ok((true, assets)) => view! {<RentalCreateForm assets/>}.into_any(),
            })}</Suspense>
        </div>
    }
}

#[component]
fn RentalCreateForm(assets: Vec<Asset>) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let assets = StoredValue::new(assets);
    let asset = RwSignal::new(String::new());
    let hourly = RwSignal::new(true);
    let preferred_hourly = RwSignal::new(true);
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let start_time = RwSignal::new("20:00".to_owned());
    let end_time = RwSignal::new("21:00".to_owned());
    let name = RwSignal::new(String::new());
    let email = RwSignal::new(String::new());
    let phone = RwSignal::new(String::new());
    let address = RwSignal::new(String::new());
    let tenant_type = RwSignal::new("person".to_owned());
    let organization_ico = RwSignal::new(String::new());
    let responsible_name = RwSignal::new(String::new());
    let consent = RwSignal::new(false);
    let identity = NodeRef::<leptos::html::Input>::new();
    let event_type = RwSignal::new("standard".to_owned());
    let price = RwSignal::new(String::new());
    let require_meter = RwSignal::new(true);
    let wait_for_payment = RwSignal::new(true);
    let recurrence = RwSignal::new("0".to_owned());
    let until = RwSignal::new(String::new());
    let preview = RwSignal::new(None::<Preview>);
    let error = RwSignal::new(None::<api::Failure>);
    let created = RwSignal::new(None::<(usize, Option<i64>)>);
    let clock = RwSignal::new(current_clock());
    #[cfg(feature = "hydrate")]
    {
        if let Ok(timer) = leptos::leptos_dom::helpers::set_interval_with_handle(
            move || {
                let now = current_clock();
                if clock.try_get_untracked().as_ref() != Some(&now) {
                    let _ = clock.try_set(now);
                }
            },
            std::time::Duration::from_secs(1),
        ) {
            on_cleanup(move || timer.clear());
        }
    }
    let term_error = Signal::derive(move || {
        period_error(
            &start.get(),
            &end.get(),
            hourly.get(),
            &start_time.get(),
            &end_time.get(),
            &clock.get(),
        )
    });
    Effect::new(move |_| {
        let now = clock.get();
        let date = start.get();
        let from = start_time.get();
        if hourly.get()
            && !date.is_empty()
            && !from.is_empty()
            && !start_time_allowed(&date, &from, &now)
        {
            start_time.set(String::new());
            end_time.set(String::new());
        }
        if term_error.get().is_some() {
            preview.set(None);
        }
    });
    let input = Signal::derive(move || {
        json!({
            "asset_id": asset.get().parse::<i64>().unwrap_or(0),
            "name": name.get(), "email": email.get(), "phone": phone.get(),
            "address": if tenant_type.get() == "organization" {String::new()} else {address.get()},
            "tenant_type": tenant_type.get(), "event_type": event_type.get(),
            "organization_ico": (tenant_type.get() == "organization").then(|| organization_ico.get()),
            "responsible_name": (tenant_type.get() == "organization").then(|| responsible_name.get()),
            "consent": tenant_type.get() == "organization" && consent.get(),
            "start_date": start.get(), "end_date": if hourly.get() {start.get()} else {end.get()},
            "start_time": hourly.get().then(|| start_time.get()),
            "end_time": hourly.get().then(|| end_time.get()),
            "rental_price": price.get().parse::<i64>().unwrap_or(-1),
            "require_meter": require_meter.get(), "wait_for_payment": wait_for_payment.get(),
            "recurrence_interval_days": recurrence.get().parse::<i32>().unwrap_or(0),
            "recurrence_interval_months": if recurrence.get() == "monthly" {1} else {0},
            "recurrence_until": (recurrence.get() != "0").then(|| until.get()),
        })
    });
    Effect::new(move |_| {
        let _ = input.get();
        preview.set(None);
    });
    let check = Action::new_local(move |_: &()| {
        let body = input.get_untracked();
        async move {
            let _ = error.try_set(None);
            let _ = preview.try_set(None);
            if let Some(message) = period_error(
                &start.get_untracked(),
                &end.get_untracked(),
                hourly.get_untracked(),
                &start_time.get_untracked(),
                &end_time.get_untracked(),
                &current_clock(),
            ) {
                let _ = error.try_set(Some(api::Failure {
                    status: 422,
                    message: message.into(),
                }));
                return;
            }
            let result =
                api::save("POST", "/api/v1/admin/rentals/preview", Some(body.clone())).await;
            if input.try_get_untracked().as_ref() != Some(&body) {
                return;
            }
            match result {
                Ok(value) => {
                    match serde_json::from_value::<Preview>(value) {
                        Ok(result) => {
                            let _ = preview.try_set(Some(result));
                        }
                        Err(_) => {
                            let _ = error.try_set(Some(api::Failure {status: 0, message: "Přehled termínů se nepodařilo načíst. Zkuste kontrolu zopakovat.".into()}));
                        }
                    }
                }
                Err(failure) => {
                    let _ = error.try_set(Some(failure));
                }
            }
        }
    });
    let save = Action::new_local(move |_: &()| {
        let body = input.get_untracked();
        let organization = tenant_type.get_untracked() == "organization";
        let identity_file = identity
            .get_untracked()
            .and_then(|input| input.files())
            .and_then(|files| files.item(0));
        async move {
            let _ = error.try_set(None);
            if let Some(message) = period_error(
                &start.get_untracked(),
                &end.get_untracked(),
                hourly.get_untracked(),
                &start_time.get_untracked(),
                &end_time.get_untracked(),
                &current_clock(),
            ) {
                let _ = error.try_set(Some(api::Failure {
                    status: 422,
                    message: message.into(),
                }));
                let _ = preview.try_set(None);
                return;
            }
            let result = if organization {
                match identity_file {
                    Some(file) if file.size() <= 10.0 * 1024.0 * 1024.0 => {
                        api::rental_with_identity("/api/v1/admin/rentals", body, file).await
                    }
                    Some(_) => Err(api::Failure {
                        status: 413,
                        message: "Kopie občanského průkazu je příliš velká. Maximální velikost je 10 MiB."
                            .into(),
                    }),
                    None => Err(api::Failure {
                        status: 422,
                        message: "Přiložte kopii dokladu odpovědné osoby.".into(),
                    }),
                }
            } else {
                api::save("POST", "/api/v1/admin/rentals", Some(body)).await
            };
            // A delayed response must not clear a newer editor's dirty state.
            if created.try_get_untracked().is_none() {
                return;
            }
            match result {
                Ok(value) => {
                    let count = value
                        .get("rentals")
                        .and_then(|items| items.as_array())
                        .map_or(0, Vec::len);
                    let first = value
                        .get("rentals")
                        .and_then(|items| items.as_array())
                        .and_then(|items| items.first())
                        .and_then(|item| item.get("id"))
                        .and_then(|id| id.as_i64());
                    let _ = ctx.dirty.try_set(false);
                    let _ = created.try_set(Some((count, first)));
                    let _ = ctx.flash.try_set(Some("Rezervace byla vytvořena.".into()));
                }
                Err(failure) => {
                    let _ = error.try_set(Some(failure));
                    let _ = preview.try_set(None);
                }
            }
        }
    });
    let busy = Signal::derive(move || check.pending().get() || save.pending().get());
    view! {
        {move || if let Some((count, first)) = created.get() {
            view! {<section class="admin-panel admin-form-section rental-create-success">
                <span class="rental-admin-kicker">"HOTOVO"</span><h2>"Rezervace je uložená"</h2>
                <p>{format!("Počet vytvořených termínů: {count}. Každý má vlastní číslo rezervace a vyúčtování.")}</p>
                <div class="admin-actions">{first.map(|id| view! {<AdminLink href=format!("/admin/pronajmy/{id}") label="Otevřít rezervaci" icon="calendar"/>})}<AdminLink href="/admin/pronajmy" label="Přehled rezervací" icon="grid"/></div>
            </section>}.into_any()
        } else { view! {
        <form class="rental-create-form" on:input=move |_| ctx.dirty.set(true) on:change=move |_| ctx.dirty.set(true) on:submit=move |event| {
            event.prevent_default();
            if !busy.get_untracked() { check.dispatch(()); }
        }>
            <fieldset disabled=move || busy.get()>
                <div class="rental-create-grid">
                    <section class="admin-panel admin-form-section rental-detail-card">
                        <span class="rental-admin-kicker">"1 · TERMÍN"</span><h2>"Co a kdy rezervovat"</h2>
                        <label class="admin-field"><span>"Předmět pronájmu"</span><select required prop:value=move || asset.get() on:change=move |event| {
                            let selected = event_target_value(&event);
                            if let Ok(id) = selected.parse::<i64>() {
                                assets.with_value(|items| {if let Some(item) = items.iter().find(|item| item.id == id) {require_meter.set(item.require_meter);}});
                            }
                            asset.set(selected);
                        }><option value="">"Vyberte majetek"</option>{assets.get_value().into_iter().filter(|item| item.active).map(|item| view! {<option value=item.id.to_string()>{item.name}</option>}).collect_view()}</select></label>
                        <label class="admin-field"><span>"Typ akce"</span><select required prop:value=move || event_type.get() on:change=move |event| event_type.set(event_target_value(&event))>
                            <option value="standard">"Soukromá akce"</option><option value="nonprofit">"Veřejně prospěšná nezisková akce"</option>
                        </select></label>
                        <label class="admin-field"><span>"Délka pronájmu"</span><select prop:value=move || if hourly.get() {"hourly"} else {"daily"} on:change=move |event| {
                            let use_hours=event_target_value(&event) == "hourly" && !is_weekend(&start.get_untracked());
                            preferred_hourly.set(use_hours);
                            hourly.set(use_hours);
                            end.set(start.get_untracked());
                        }><option value="hourly" disabled=move || is_weekend(&start.get())>"Po hodinách"</option><option value="daily">"Celé dny"</option></select></label>
                        <div class="rental-create-pair">
                            <label class="admin-field"><span>"Datum"</span><input type="date" required min=move || first_available_day(hourly.get(), &clock.get()) prop:value=move || start.get() on:input=move |event| {
                                let value=event_target_value(&event);
                                hourly.set(!is_weekend(&value) && preferred_hourly.get_untracked());
                                end.set(value.clone());
                                start.set(value);
                            }/></label>
                            <Show when=move || !hourly.get()><label class="admin-field"><span>"Do včetně"</span><input type="date" required min=move || start.get().max(first_available_day(false, &clock.get())) max=move || last_rental_day(&start.get()) prop:value=move || end.get() on:input=move |event| end.set(event_target_value(&event))/></label></Show>
                        </div>
                        <Show when=move || !start.get().is_empty()><p class="rental-create-mode" role="status">{move || if hourly.get() {"Pronájem po hodinách"} else {"Pronájem na celé dny"}}</p></Show>
                        <Show when=move || hourly.get()><div class="rental-create-pair">
                            <label class="admin-field"><span>"Od hodin"</span><select required prop:value=move || start_time.get() on:change=move |event| {
                                let value = event_target_value(&event);
                                if end_time.get_untracked() <= value {
                                    let next = value.get(..2).and_then(|hour| hour.parse::<u8>().ok()).map(|hour| format!("{:02}:00", hour + 1)).unwrap_or_default();
                                    end_time.set(next);
                                }
                                start_time.set(value);
                            }><option value="">"Vyberte začátek"</option>{(0..24).map(|hour| {let value=format!("{hour:02}:00"); let candidate=value.clone(); view! {<option value=value.clone() disabled=move || !start_time_allowed(&start.get(), &candidate, &clock.get())>{value.clone()}</option>}}).collect_view()}</select></label>
                            <label class="admin-field"><span>"Do hodin"</span><select required prop:value=move || end_time.get() on:change=move |event| end_time.set(event_target_value(&event))><option value="">"Vyberte konec"</option>{(1..=24).map(|hour| {let value=format!("{hour:02}:00"); let candidate=value.clone(); view! {<option value=value.clone() disabled=move || start_time.get().is_empty() || candidate <= start_time.get()>{value.clone()}</option>}}).collect_view()}</select></label>
                        </div></Show>
                        <Show when=move || !start.get().is_empty() && term_error.get().is_some()><p class="rental-error" role="status">{move || term_error.get()}</p></Show>
                        <p class="admin-caption">"Od pondělí do pátku si vyberte hodiny nebo celé dny. Sobotu a neděli lze rezervovat pouze jako celé dny. Souvislý pronájem může trvat nejvýše 28 dní."</p>
                        <label class="admin-field"><span>"Opakování"</span><select prop:value=move || recurrence.get() on:change=move |event| recurrence.set(event_target_value(&event))><option value="0">"Jednorázově"</option><option value="7">"Každý týden"</option><option value="14">"Každých 14 dní"</option><option value="monthly">"Jednou měsíčně"</option></select></label>
                        <Show when=move || recurrence.get() == "monthly"><p class="admin-caption" role="status">{move || monthly_description(&start.get())}</p></Show>
                        <Show when=move || recurrence.get() != "0"><label class="admin-field"><span>"Opakovat do včetně"</span><input type="date" required min=move || start.get() prop:value=move || until.get() on:input=move |event| until.set(event_target_value(&event))/><small>"Termíny lze připravit nejvýše na rok dopředu. Před uložením zobrazíme všechny výskyty i případné kolize."</small></label></Show>
                    </section>
                    <section class="admin-panel admin-form-section rental-detail-card">
                        <span class="rental-admin-kicker">"2 · NÁJEMCE"</span><h2>"Kdo si prostor pronajímá"</h2>
                        <label class="admin-field"><span>"Typ nájemce"</span><select prop:value=move || tenant_type.get() on:change=move |event| tenant_type.set(event_target_value(&event))>
                            <option value="person">"Fyzická osoba"</option><option value="organization">"Firma nebo organizace"</option>
                        </select></label>
                        <label class="admin-field"><span>{move || if tenant_type.get() == "organization" {"Název firmy nebo organizace"} else {"Jméno a příjmení"}}</span><input type="text" required maxlength="120" autocomplete=move || if tenant_type.get() == "organization" {"organization"} else {"name"} prop:value=move || name.get() on:input=move |event| name.set(event_target_value(&event))/></label>
                        <Show when=move || tenant_type.get() == "organization">
                            <label class="admin-field"><span>"IČO"</span><input type="text" required inputmode="numeric" pattern="[0-9]{8}" minlength="8" maxlength="8" prop:value=move || organization_ico.get() on:input=move |event| organization_ico.set(event_target_value(&event))/><small>"Osm číslic včetně případných nul na začátku."</small></label>
                            <label class="admin-field"><span>"Jméno a příjmení odpovědné osoby"</span><input type="text" required maxlength="120" autocomplete="name" prop:value=move || responsible_name.get() on:input=move |event| responsible_name.set(event_target_value(&event))/></label>
                        </Show>
                        <label class="admin-field"><span>"E-mail pro vyúčtování"</span><input type="email" required maxlength="254" autocomplete="email" prop:value=move || email.get() on:input=move |event| email.set(event_target_value(&event))/></label>
                        <label class="admin-field"><span>"Telefon"</span><input type="tel" required maxlength="40" autocomplete="tel" prop:value=move || phone.get() on:input=move |event| phone.set(event_target_value(&event))/></label>
                        <Show when=move || tenant_type.get() != "organization"><label class="admin-field"><span>"Adresa"</span><input type="text" required maxlength="300" autocomplete="street-address" prop:value=move || address.get() on:input=move |event| address.set(event_target_value(&event))/></label>
                            <p class="admin-caption">"Rezervaci zadáváte jako pověřený pracovník obce. U fyzické osoby se zde kopie dokladu nenahrává. Ověření dokladu provede úřední osoba."</p>
                        </Show>
                        <Show when=move || tenant_type.get() == "organization">
                            <label class="admin-field"><span>"Kopie občanského průkazu odpovědné osoby"</span><input node_ref=identity type="file" accept="image/jpeg,image/png,image/webp" required on:change=move |_| preview.set(None)/><small>"JPG, PNG nebo WebP, nejvýše 10 MB. Kopii uvidí pouze pověřený pracovník obce."</small></label>
                            <label class="rental-consent"><input type="checkbox" required prop:checked=move || consent.get() on:change=move |event| consent.set(event_target_checked(&event))/><span>"Potvrzuji, že odpovědná osoba souhlasí s dočasným uložením kopie svého občanského průkazu pro účely tohoto pronájmu."</span></label>
                        </Show>
                    </section>
                    <section class="admin-panel admin-form-section rental-detail-card rental-create-billing">
                        <span class="rental-admin-kicker">"3 · INDIVIDUÁLNÍ DOHODA"</span><h2>"Cena a vyúčtování"</h2>
                        <label class="admin-field"><span>"Sjednaná cena za jeden termín v Kč"</span><input type="number" required min="0" max="100000000" step="1" prop:value=move || price.get() on:input=move |event| price.set(event_target_value(&event))/><small>"Celková cena za jeden vybraný blok hodin nebo dní. Veřejnost tuto cenu nikdy neuvidí. Bude uvedena pouze ve správě a ve vyúčtování nájemce."</small></label>
                        <div class="rental-create-options">
                            <label><input type="checkbox" prop:checked=move || !require_meter.get() on:change=move |event| require_meter.set(!event_target_checked(&event))/><span><strong>"Energie zahrnuté v ceně pronájmu"</strong><small>"Fotografie elektroměru se nenahrávají."</small></span></label>
                            <label><input type="checkbox" prop:checked=move || !wait_for_payment.get() on:change=move |event| wait_for_payment.set(!event_target_checked(&event))/><span><strong>"Nečekat s uzavřením na potvrzení platby"</strong><small>"Vyúčtování s platebními údaji se odešle a pronájem se potom uzavře. Sjednaná cena se nadále platí, pouze se nesleduje její úhrada. Nájem nebude označen jako zaplacený."</small></span></label>
                        </div>
                        <Show when=move || recurrence.get() != "0" && !require_meter.get()><p class="rental-create-notice">"Po každém termínu systém automaticky odešle vyúčtování se sjednanou cenou."</p></Show>
                    </section>
                </div>
                <ErrorMessage error/>
                <button class="button primary" type="submit" disabled=move || term_error.get().is_some()>{move || if check.pending().get() {"Kontroluji termíny…"} else {"Zkontrolovat termíny"}}</button>
            </fieldset>
        </form>
        {move || preview.get().map(|result| {
            let can_save = result.conflict_count == 0;
            view! {<section class="admin-panel admin-form-section rental-create-preview" aria-live="polite">
                <span class="rental-admin-kicker">"KONTROLA PŘED ULOŽENÍM"</span>
                <h2>{if can_save {"Všechny termíny jsou volné"} else {"Některé termíny jsou obsazené"}}</h2>
                <p>{format!("Počet termínů: {} · Cena za jeden termín: {} Kč · Celkem: {} Kč", result.occurrence_count, price.get_untracked(), result.total_price)}</p>
                {(!can_save).then(|| view! {<p class="rental-error">"Žádná rezervace zatím nevznikla. Upravte termín nebo čas a kontrolu zopakujte."</p>})}
                <ul class="rental-preview-occurrences">{result.occurrences.into_iter().map(|occurrence| {
                    let period = if let (Some(from), Some(to)) = (occurrence.start_time, occurrence.end_time) {
                        format!("{}, {} až {}", date_display(&occurrence.start_date), from, to)
                    } else if occurrence.start_date == occurrence.end_date {
                        format!("{}, celý den", date_display(&occurrence.start_date))
                    } else {format!("{} až {}", date_display(&occurrence.start_date), date_display(&occurrence.end_date))};
                    view! {<li class:conflict=occurrence.conflict><span>{period}</span><strong>{if occurrence.conflict {"Obsazeno"} else {"Volno"}}</strong></li>}
                }).collect_view()}</ul>
                {can_save.then(|| view! {<button type="button" class="button primary" disabled=move || busy.get() || term_error.get().is_some() on:click=move |_| {if !busy.get_untracked() {save.dispatch(());}}>{move || if save.pending().get() {"Ukládám…"} else {"Potvrdit a vytvořit rezervaci"}}</button>})}
            </section>}
        })}
        }.into_any() }}
    }
}
