use super::components::PageHeading;
use leptos::prelude::*;
use leptos_meta::{Meta, Title};

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/public/rentals.js")]
extern "C" {
    fn mount_rentals(root: &web_sys::HtmlElement);
    fn mount_rental_meter(root: &web_sys::HtmlElement);
    fn unmount_rental(root: &web_sys::HtmlElement);
}

#[component]
pub fn Rentals() -> impl IntoView {
    view! {<RentalPublicAccess><RentalsForm/></RentalPublicAccess>}
}

#[component]
pub fn RentalMeter() -> impl IntoView {
    view! {<RentalPublicAccess><RentalMeterForm/></RentalPublicAccess>}
}

#[component]
fn RentalPublicAccess(children: ChildrenFn) -> impl IntoView {
    // Check the current server state on every entry, including client-side navigation.
    let access = Resource::new_blocking(|| (), |_| crate::catalog::load_site_info());
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.insert_header(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
    }
    view! {
        <Suspense fallback=|| view! {<p class="page-width interior" role="status">"Načítám…"</p>}>
            {move || access.get().map(|result| {
                if result.is_ok_and(|info| info.rentals_enabled) {
                    children().into_any()
                } else {
                    view! {<RentalsUnavailable/>}.into_any()
                }
            })}
        </Suspense>
    }
}

#[component]
fn RentalsUnavailable() -> impl IntoView {
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    view! {
        <Title text="Pronájmy nejsou dostupné · Vyskeř"/>
        <Meta name="robots" content="noindex, nofollow"/>
        <div class="page-width interior">
            <PageHeading title="Pronájmy nejsou dostupné" description="Informace o pronájmu vám poskytne obecní úřad." eyebrow="OBEC VYSKEŘ"/>
            <p><a href="tel:+420481329211">"481 329 211"</a>" · "<a href="mailto:vysker@craj.cz">"vysker@craj.cz"</a></p>
            <a class="button secondary" href="/">"Zpět na úvodní stránku"</a>
        </div>
    }
}

#[component]
fn RentalsForm() -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_rentals(&element);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_rental(&element);
            }
        });
    }
    view! {
        <Title text="Pronájmy · Vyskeř"/>
        <div class="page-width interior rentals-page" node_ref=root>
            <PageHeading title="Pronájmy" description="Prohlédněte si dostupnost a požádejte o pronájem obecního sálu nebo dalšího majetku obce." eyebrow="PRO VÁS"/>
            <div class="rental-layout report-compose">
                <section aria-labelledby="rental-calendar-title">
                    <h2 id="rental-calendar-title"><span class="rental-step" aria-hidden="true">"1"</span>"Vyberte termín"</h2>
                    <label class="report-field" for="rental-asset"><span>"Co si chcete pronajmout"</span><select id="rental-asset" required disabled><option>"Načítám nabídku…"</option></select></label>
                    <p id="rental-asset-description" class="field-note"></p>
                    <label class="report-field" for="rental-event-type"><span>"Typ akce"</span><select id="rental-event-type" disabled aria-describedby="rental-event-help"><option value="standard">"Soukromá akce"</option><option value="nonprofit">"Veřejně prospěšná nezisková akce"</option></select></label>
                    <p id="rental-event-help" class="field-note">"Cena se řídí typem akce. Vaši volbu zkontroluje obec při schvalování žádosti."</p>
                    <p id="rental-prices" class="rental-price-summary" aria-live="polite"></p>
                    <div class="rental-mode" role="group" aria-label="Délka pronájmu">
                        <button type="button" id="rental-mode-hour" aria-pressed="true" disabled>"Po hodinách"</button>
                        <button type="button" id="rental-mode-day" aria-pressed="false" disabled>"Celé dny"</button>
                    </div>
                    <p class="field-note rental-mode-note">"Od pondělí do pátku po hodinách nebo na celý den. O víkendu pouze celé dny, sobotu a neděli i jednotlivě. Cena za celý den je vždy stejná."</p>
                    <div class="rental-calendar-card">
                    <div class="rental-calendar-controls"><button type="button" class="button secondary" id="rental-previous" aria-label="Předchozí měsíc" title="Předchozí měsíc" disabled>"‹"</button><strong id="rental-month"></strong><button type="button" class="button secondary" id="rental-next" aria-label="Další měsíc" title="Další měsíc" disabled>"›"</button></div>
                    <p id="rental-calendar-help" class="field-note rental-picker-help">"Vyberte den v kalendáři."</p>
                    <div id="rental-calendar" aria-describedby="rental-calendar-help"></div>
                    <div class="rental-calendar-legend" aria-label="Legenda kalendáře"><span><i class="rental-legend-pending" aria-hidden="true"></i>"Žádost čeká na schválení, termín se může uvolnit"</span><span><i class="rental-legend-booked" aria-hidden="true"></i>"Obsazeno"</span><span><i class="rental-legend-partial" aria-hidden="true"></i>"Část dne obsazena"</span><span><i class="rental-legend-selected" aria-hidden="true"></i>"Vybraný termín"</span></div>
                    </div>
                    <div class="rental-date-fields">
                        <label class="report-field" for="rental-start"><span id="rental-start-label">"Datum"</span><input id="rental-start" form="rental-form" name="start_date" type="date" required disabled/></label>
                        <label class="report-field" id="rental-end-field" for="rental-end" hidden><span>"Do včetně"</span><input id="rental-end" form="rental-form" name="end_date" type="date" required disabled/></label>
                    </div>
                    <section id="rental-hours-panel" class="rental-hours-panel" hidden aria-labelledby="rental-hours-title">
                        <h3 id="rental-hours-title">"Vyberte hodiny"</h3>
                        <p id="rental-hours-help" class="field-note"></p>
                        <div id="rental-time-fields" class="rental-time-fields" hidden>
                            <label class="report-field" for="rental-start-time"><span>"Od kolika"</span><select id="rental-start-time" form="rental-form" name="start_time" disabled><option value="">"Vyberte začátek"</option></select></label>
                            <label class="report-field" for="rental-end-time"><span>"Do kolika"</span><select id="rental-end-time" form="rental-form" name="end_time" disabled><option value="">"Vyberte konec"</option></select></label>
                        </div>
                        <div id="rental-hours" class="rental-hours" role="group" aria-label="Hodinová dostupnost" aria-describedby="rental-hours-help"></div>
                    </section>
                    <p id="rental-picked-price" class="rental-picked-price" hidden></p>
                    <p id="rental-calendar-message" class="rental-message" role="status" aria-live="polite"></p>
                    <button type="button" class="rental-clear" id="rental-clear" disabled>"Vymazat výběr"</button>
                </section>
                <section aria-labelledby="rental-form-title">
                    <h2 id="rental-form-title"><span class="rental-step" aria-hidden="true">"2"</span>"Žádost o pronájem"</h2>
                    <p id="rental-selection-summary" class="rental-selection-summary" role="status" aria-live="polite">"Nejprve si vyberte termín. Platí se až po vyúčtování."</p>
                    <p class="field-note">"Žádost musí obec schválit před začátkem pronájmu. Nepotvrzená žádost se při jeho zahájení automaticky zruší."</p>
                    <p class="field-note">"Klíče se přebírají na obecním úřadě v předchozí pracovní den. Vrací se poslední den pronájmu nebo následující pracovní den na začátku otevírací doby úřadu."</p>
                    <form id="rental-form" enctype="multipart/form-data">
                        <fieldset id="rental-fields" disabled>
                            <label class="report-field" for="rental-tenant-type"><span>"Kdo si prostor pronajímá"</span><select id="rental-tenant-type" name="tenant_type"><option value="person">"Fyzická osoba"</option><option value="organization">"Firma nebo organizace"</option></select></label>
                            <label class="report-field" for="rental-name"><span id="rental-name-label">"Jméno a příjmení"</span><input id="rental-name" name="name" autocomplete="name" maxlength="120" required/></label>
                            <div id="rental-organization-fields" hidden>
                                <label class="report-field" for="rental-company-id"><span>"IČO"</span><input id="rental-company-id" name="organization_ico" type="text" inputmode="numeric" autocomplete="off" minlength="8" maxlength="8" pattern="[0-9]{8}" disabled aria-describedby="rental-company-id-help"/></label>
                                <p id="rental-company-id-help" class="field-note">"Zadejte 8 číslic včetně případných nul na začátku."</p>
                                <label class="report-field" for="rental-responsible-name"><span>"Jméno a příjmení odpovědné osoby"</span><input id="rental-responsible-name" name="responsible_name" autocomplete="name" maxlength="120" disabled/></label>
                                <p class="field-note">"Za firmu nebo organizaci jedná tato osoba. Níže nahrajte kopii jejího dokladu."</p>
                            </div>
                            <label class="report-field" for="rental-email"><span>"E-mail"</span><input id="rental-email" name="email" type="email" autocomplete="email" maxlength="254" required/></label>
                            <label class="report-field" for="rental-phone"><span>"Telefon"</span><input id="rental-phone" name="phone" type="tel" autocomplete="tel" maxlength="40" required/></label>
                            <div id="rental-person-address">
                              <div class="rental-address-search">
                                <label class="report-field" for="rental-address"><span id="rental-address-label">"Adresa"</span><input id="rental-address" name="address" autocomplete="off" maxlength="300" required role="combobox" aria-autocomplete="list" aria-expanded="false" aria-controls="rental-address-results" aria-describedby="rental-address-help rental-address-status"/></label>
                                <ul id="rental-address-results" class="rental-address-results" role="listbox" aria-label="Nalezené adresy" hidden></ul>
                              </div>
                            <p id="rental-address-help" class="rental-address-help">"Našeptávání adres je aktivní. Můžete vybrat z nabídky nebo ponechat vlastní adresu. K našeptávání se využívá služba "<a href="https://photon.komoot.io/" target="_blank" rel="noopener noreferrer">"Photon"</a>" s daty "<a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noopener noreferrer">"OpenStreetMap"</a>"."</p>
                            <p id="rental-address-status" class="rental-address-status" role="status" aria-live="polite"></p>
                            </div>
                            <label class="report-field" for="rental-identity"><span id="rental-identity-label">"Kopie občanského průkazu"</span><input id="rental-identity" name="identity" type="file" accept="image/jpeg,image/png,image/webp" required aria-describedby="rental-identity-help"/></label>
                            <p id="rental-identity-help" class="field-note">"JPG, PNG nebo WebP, nejvýše 10 MB. Přístup ke kopii je zabezpečený. Zobrazit ji může pouze pověřený pracovník obce. Po zaplacení a uzavření pronájmu se kopie automaticky odstraní z databáze."</p>
                            <label class="rental-consent"><input type="checkbox" name="consent" value="true" required/> <span id="rental-consent-label">"Souhlasím s dočasným uložením kopie mého občanského průkazu pro účely tohoto pronájmu."</span></label>
                            <p class="field-note"><a href="/ochrana-udaju">"Informace o zpracování osobních údajů"</a></p>
                            <p id="rental-message" class="rental-message" role="status" aria-live="polite"></p>
                            <button class="button primary" id="rental-submit" type="submit">"Odeslat žádost"</button>
                        </fieldset>
                    </form>
                </section>
            </div>
            <aside class="rental-contact" aria-labelledby="rental-contact-title">
                <div><h2 id="rental-contact-title">"Potřebujete poradit?"</h2><p>"S výběrem termínu, ceníkem nebo vyplněním žádosti vám pomůže obecní úřad."</p></div>
                <div class="rental-contact-links"><a href="tel:+420481329211">"481 329 211"</a><a href="mailto:vysker@craj.cz">"vysker@craj.cz"</a></div>

            </aside>
            <noscript><p class="info-banner">"Pro zobrazení dostupnosti a odeslání žádosti zapněte JavaScript nebo se obraťte na obecní úřad."</p></noscript>
        </div>
    }
}

#[component]
fn RentalMeterForm() -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_rental_meter(&element);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_rental(&element);
            }
        });
    }
    view! {
        <Title text="Fotografie elektroměru · Vyskeř"/>
        <Meta name="robots" content="noindex, nofollow"/>
        <Meta name="referrer" content="no-referrer"/>
        <div class="page-width interior report-compose rental-meter" node_ref=root>
            <PageHeading title="Fotografie elektroměru" description="Nahrajte fotografie stavu elektroměru před pronájmem a po něm." eyebrow="PRONÁJEM"/>
            <p id="meter-info" class="field-note" role="status">"Načítám údaje o pronájmu…"</p>
            <p class="rental-meter-intro">"Fotografii před pronájmem můžete uložit hned a druhou doplnit po skončení. Můžete nahrát i obě najednou. Tento odkaz lze používat opakovaně a fotografie měnit až do vystavení vyúčtování."</p>
            <form id="meter-form" enctype="multipart/form-data">
                <fieldset id="meter-fields" disabled>
                    <div class="rental-meter-upload-grid">
                        <section class="rental-meter-upload-card" data-meter-card="before" aria-labelledby="meter-before-title">
                            <div class="rental-meter-upload-heading"><h2 id="meter-before-title">"Před pronájmem"</h2><p data-meter-status class="rental-meter-upload-status" role="status" aria-live="polite">"Načítám…"</p></div>
                            <div data-meter-preview class="rental-meter-upload-preview"><p data-meter-placeholder>"Fotografie elektroměru při převzetí"</p></div>
                            <p data-meter-preview-note class="field-note" role="status"></p>
                            <label class="report-field" for="meter-before"><span>"Fotografie před pronájmem"</span><input id="meter-before" name="before" type="file" accept="image/jpeg,image/png,image/webp" aria-describedby="meter-photo-help"/></label>
                            <button type="button" data-meter-clear class="rental-clear" hidden>"Zrušit výběr"</button>
                        </section>
                        <section class="rental-meter-upload-card" data-meter-card="after" aria-labelledby="meter-after-title">
                            <div class="rental-meter-upload-heading"><h2 id="meter-after-title">"Po pronájmu"</h2><p data-meter-status class="rental-meter-upload-status" role="status" aria-live="polite">"Načítám…"</p></div>
                            <div data-meter-preview class="rental-meter-upload-preview"><p data-meter-placeholder>"Fotografie elektroměru při vrácení"</p></div>
                            <p data-meter-preview-note class="field-note" role="status"></p>
                            <label class="report-field" for="meter-after"><span>"Fotografie po pronájmu"</span><input id="meter-after" name="after" type="file" accept="image/jpeg,image/png,image/webp" aria-describedby="meter-photo-help"/></label>
                            <button type="button" data-meter-clear class="rental-clear" hidden>"Zrušit výběr"</button>
                        </section>
                    </div>
                    <p id="meter-photo-help" class="field-note">"JPG, PNG nebo WebP, každá fotografie nejvýše 10 MB a 60 megapixelů. Zkontrolujte, že je stav na fotografii čitelný."</p>
                    <p id="meter-message" class="rental-message" role="status" aria-live="polite"></p>
                    <button id="meter-submit" class="button primary" type="submit" disabled>"Uložit fotografie"</button>
                </fieldset>
            </form>
            <noscript><p class="info-banner">"Pro nahrání fotografie zapněte JavaScript nebo se obraťte na obecní úřad."</p></noscript>
        </div>
    }
}
