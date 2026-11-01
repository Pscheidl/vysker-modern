use super::{Access, AdminContext, api, ui::*};
use leptos::prelude::*;
use leptos_meta::Title;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

pub(super) async fn refresh_rental_permissions(ctx: AdminContext) -> api::Result<bool> {
    let session = match api::get::<api::Session>("/api/v1/admin/session").await {
        Ok(session) => session,
        Err(error) => {
            if error.status == 401 {
                let _ = ctx.access.try_set(Access::SignedOut);
            }
            return Err(error);
        }
    };
    let can_manage = session.can_view_identity;
    // Update the shared session without rebuilding the surrounding workspace.
    let _ = ctx.access.try_update_untracked(|access| {
        if let Access::Ready(current) = access {
            *current = session;
        }
    });
    Ok(can_manage)
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/public/rental-identity.js")]
extern "C" {
    fn mount_rental_identity(root: &web_sys::HtmlElement);
    fn unmount_rental_identity(root: &web_sys::HtmlElement);
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen(module = "/public/rental-meter-viewer.js")]
extern "C" {
    fn mount_rental_meter_viewer(root: &web_sys::HtmlElement);
    fn unmount_rental_meter_viewer(root: &web_sys::HtmlElement);
}

#[component]
fn RentalMeterPhotos(id: i64, before_uploaded: bool, after_uploaded: bool) -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_rental_meter_viewer(&element);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_rental_meter_viewer(&element);
            }
        });
    }
    view! {
        <div node_ref=root class="rental-meter-viewer">
            <div class="rental-meter-photos">
                <div><span class="rental-meter-label">"Před pronájmem"</span>{if before_uploaded { view! {
                    <button type="button" class="rental-meter-photo-button" data-meter-photo-url=format!("/api/v1/admin/rentals/{id}/photos/before") data-meter-photo-title="Elektroměr před pronájmem" aria-label="Zobrazit fotografii před pronájmem" aria-haspopup="dialog">"Zobrazit fotografii"</button>
                }.into_any() } else { view! { <span>"Fotografie nebyla nahrána"</span> }.into_any() }}</div>
                <div><span class="rental-meter-label">"Po pronájmu"</span>{if after_uploaded { view! {
                    <button type="button" class="rental-meter-photo-button" data-meter-photo-url=format!("/api/v1/admin/rentals/{id}/photos/after") data-meter-photo-title="Elektroměr po pronájmu" aria-label="Zobrazit fotografii po pronájmu" aria-haspopup="dialog">"Zobrazit fotografii"</button>
                }.into_any() } else { view! { <span>"Fotografie nebyla nahrána"</span> }.into_any() }}</div>
            </div>
            <dialog data-meter-dialog class="rental-meter-dialog" aria-labelledby=format!("rental-meter-dialog-title-{id}")>
                <div class="rental-meter-dialog-header"><h2 data-meter-dialog-title id=format!("rental-meter-dialog-title-{id}")>"Fotografie elektroměru"</h2><button type="button" class="button secondary" data-meter-dialog-close autofocus>"Zavřít fotografii"</button></div>
                <p data-meter-dialog-status class="admin-caption" role="status" aria-live="polite"></p>
                <div data-meter-dialog-image class="rental-meter-dialog-image"></div>
            </dialog>
        </div>
    }
}

#[component]
fn RentalIdentity(id: i64) -> impl IntoView {
    let root = NodeRef::<leptos::html::Div>::new();
    #[cfg(feature = "hydrate")]
    {
        Effect::new(move |_| {
            if let Some(element) = root.get() {
                mount_rental_identity(&element);
            }
        });
        on_cleanup(move || {
            if let Some(element) = root.get_untracked() {
                unmount_rental_identity(&element);
            }
        });
    }
    view! {
        <div node_ref=root class="rental-identity-view" data-identity-url=format!("/api/v1/admin/rentals/{id}/identity")>
            <p class="admin-caption">"Doklad se zobrazí nejvýše na 2 minuty. Každé otevření vyžaduje vaše heslo a zaznamená se do auditu."</p>
            <p data-identity-status class="admin-caption" role="status" aria-live="polite"></p>
            <button type="button" class="button secondary" data-identity-open>"Zobrazit kopii dokladu"</button>
            <form data-identity-form hidden>
                <label class="admin-field"><span>"Pro zobrazení zadejte znovu Vaše heslo"</span><input type="password" data-identity-password autocomplete="off" required/></label>
                <p data-identity-error class="admin-caption" role="alert"></p>
                <div class="admin-actions"><button type="submit" class="button primary" data-identity-submit>"Ověřit a zobrazit"</button><button type="button" class="button secondary" data-identity-close>"Zavřít"</button></div>
            </form>
            <div data-identity-result hidden>
                <div class="rental-identity-toolbar">
                    <p data-identity-timer class="admin-caption" aria-live="off"></p>
                    <button type="button" class="button secondary" data-identity-close>"Zavřít kopii dokladu"</button>
                </div>
                <button type="button" data-identity-enlarge data-identity-image-container class="rental-identity-image-container" aria-label="Zvětšit kopii dokladu" aria-haspopup="dialog"></button>
                <p class="admin-caption rental-identity-hint">"Kliknutím na doklad jej zvětšíte."</p>
            </div>
            <dialog data-identity-dialog class="rental-identity-dialog" aria-labelledby=format!("rental-identity-dialog-title-{id}")>
                <div class="rental-identity-dialog-header">
                    <div><h2 id=format!("rental-identity-dialog-title-{id}")>"Zvětšená kopie občanského průkazu"</h2><p data-identity-timer class="admin-caption" aria-live="off"></p></div>
                    <div class="admin-actions"><button type="button" class="button secondary" data-identity-minimize autofocus>"Zmenšit zobrazení"</button><button type="button" class="button secondary" data-identity-close>"Zavřít kopii dokladu"</button></div>
                </div>
                <div data-identity-enlarged-container class="rental-identity-enlarged-container"></div>
            </dialog>
        </div>
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct Asset {
    pub(super) id: i64,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) rates: RentalRates,
    pub(super) notification_email: String,
    pub(super) require_meter: bool,
    pub(super) active: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(super) struct RentalRate {
    hourly_price: Option<i64>,
    weekend_day_price: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub(super) struct RentalRates {
    nonprofit: RentalRate,
    standard: RentalRate,
}

#[derive(Clone)]
struct RateFields {
    hourly: String,
    weekend_day: String,
}

impl RateFields {
    fn new(rate: &RentalRate) -> Self {
        let display = |value: Option<i64>| value.map(|value| value.to_string()).unwrap_or_default();
        Self {
            hourly: display(rate.hourly_price),
            weekend_day: display(rate.weekend_day_price),
        }
    }

    fn value(&self) -> RentalRate {
        let parse = |value: &str| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value.parse::<i64>().unwrap_or(-1))
            }
        };
        RentalRate {
            hourly_price: parse(&self.hourly),
            weekend_day_price: parse(&self.weekend_day),
        }
    }
}

#[component]
fn RateFieldsView(
    title: &'static str,
    description: &'static str,
    rate: RwSignal<RateFields>,
) -> impl IntoView {
    view! {<fieldset class="rental-rate-group"><legend>{title}</legend><p>{description}</p><div class="rental-rate-fields">
        <label class="admin-field"><span>"Celý den v Kč"</span><input aria-label=format!("{title}: cena za den v Kč") type="number" min="0" max="10000000" step="1" placeholder="Nenastaveno" prop:value=move || rate.get().weekend_day on:input=move |event| rate.update(|value| value.weekend_day=event_target_value(&event))/><small>"Stejná sazba pro všechny dny v týdnu."</small></label>
        <label class="admin-field"><span>"Hodina v Kč"</span><input aria-label=format!("{title}: cena za hodinu v Kč") type="number" min="0" max="10000000" step="1" placeholder="Nenastaveno" prop:value=move || rate.get().hourly on:input=move |event| rate.update(|value| value.hourly=event_target_value(&event))/><small>"Od pondělí do pátku."</small></label>
    </div></fieldset>}
}

#[derive(Clone, Debug, Deserialize)]
struct BlockedInterval {
    id: i64,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rental {
    id: i64,
    asset_id: i64,
    asset_name: String,
    start_date: String,
    end_date: String,
    start_time: Option<String>,
    end_time: Option<String>,
    invoice_available_at: String,
    invoice_available_unix: i64,
    source: String,
    identity_uploaded: bool,
    #[serde(default = "default_true")]
    require_meter: bool,
    #[serde(default = "default_true")]
    wait_for_payment: bool,
    series_id: Option<i64>,
    recurrence_interval_days: Option<i32>,
    recurrence_interval_months: Option<i32>,
    recurrence_until: Option<String>,
    name: String,
    organization_ico: Option<String>,
    responsible_name: Option<String>,
    email: String,
    phone: String,
    address: String,
    tenant_type: String,
    is_local: bool,
    locality_source: String,
    rate_group: String,
    rate_snapshot: Option<serde_json::Value>,
    event_type: Option<String>,
    status: String,
    rental_price: i64,
    energy_kwh: Option<String>,
    energy_cost: Option<i64>,
    extra_cost: Option<i64>,
    total_price: Option<i64>,
    created_at: String,
    before_uploaded: bool,
    after_uploaded: bool,
    before_photo_taken_at: Option<String>,
    after_photo_taken_at: Option<String>,
    account_iban: Option<String>,
    account_number: Option<String>,
    bank_code: Option<String>,
    payment_iban: Option<String>,
    invoice_issued_on: Option<String>,
    invoice_due_on: Option<String>,
    due_days: Option<i64>,
    invoice_sent_at: Option<String>,
    invoice_mail_status: Option<String>,
    invoice_pdf_attached: Option<bool>,
    billing_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PaymentSettings {
    account_number: String,
    bank_code: String,
    iban: String,
    due_days: i64,
}

fn default_true() -> bool {
    true
}

fn rate_group_label(group: &str) -> &'static str {
    match group {
        "local_person" => "Místní občan s trvalým pobytem",
        "local_organization" => "Organizace se sídlem v obci",
        "external" => "Občan nebo organizace mimo obec",
        _ => "Původní ceník",
    }
}

fn rental_category_label(rental: &Rental) -> &'static str {
    match rental.event_type.as_deref() {
        Some("nonprofit") => "Veřejně prospěšná nezisková akce",
        Some("standard") => "Soukromá akce",
        _ if rental.rate_snapshot.is_some() => rate_group_label(&rental.rate_group),
        _ => "Původní rezervace",
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "pending" => "Žádost",
        "approved" => "Schváleno, nevyúčtováno",
        "invoiced" => "Vyúčtováno, čeká na úhradu",
        "paid" => "Zaplaceno",
        "closed" => "Uzavřeno bez kontroly platby",
        "cancelled" => "Zrušeno",
        "rejected" => "Zamítnuto",
        _ => "Neznámý stav",
    }
}

#[component]
fn RentalsHelp(#[prop(default = false)] assets: bool) -> impl IntoView {
    view! {
        <details class="rental-admin-help">
            <summary>
                <span class="rental-help-icon" aria-hidden="true">"?"</span>
                <span class="rental-help-summary"><strong>{if assets { "Nápověda ke správě majetku" } else { "Nápověda k rezervacím" }}</strong>
                    <small>{if assets { "Ceny, nabídka a blokované termíny" } else { "Žádosti, vyúčtování a uzavření pronájmu" }}</small></span>
                <span class="rental-help-chevron" aria-hidden="true">"⌄"</span>
            </summary>
            <div class="rental-help-content">
                {if assets { view! {
                    <section><h3>"Majetek a ceny"</h3><ul>
                        <li>"Přidejte sál, park nebo jiný majetek. Pro každý typ akce nastavte cenu za celý den a za hodinu. Denní sazba je stejná ve všední dny i o víkendu."</li>
                        <li>"Od pondělí do pátku lze rezervovat hodiny nebo celé dny. Sobotu a neděli pouze jako celé dny, každou samostatně nebo společně. Prázdná cena znamená nenastavenou sazbu. Nulová cena znamená pronájem bez nájemného."</li>
                    </ul></section>
                    <section><h3>"Blokované termíny"</h3><ul>
                        <li>"U každé položky můžete zablokovat hodiny, jednotlivý den nebo delší období, například kvůli údržbě."</li>
                        <li>"Blokovaný termín se veřejnosti zobrazí jako nedostupný. Odstranit jej můžete po potvrzení."</li>
                    </ul></section>

                }.into_any() } else { view! {
                    <section><h3>"Žádosti a přehled"</h3><ul>
                        <li>"Kliknutím na souhrn stavu vyfiltrujete příslušné nájmy. Výběr můžete změnit i v nabídce pod přehledem."</li>
                        <li>"Cena závisí na typu akce. Veřejně prospěšné neziskové akce mají vlastní ceník, soukromé akce druhý. Před schválením zkontrolujte zvolený typ akce."</li>
                        <li>"Nová žádost do rozhodnutí blokuje termín. Správce s oprávněním ke správě rezervací ji může schválit nebo zamítnout, obě akce vyžadují další potvrzení. Ostatní správci mohou údaje prohlížet."</li>
                        <li>"Žádost musí být schválena před začátkem pronájmu. Jinak ji systém automaticky zruší a termín uvolní. Najdete ji ve filtru Zrušené. U celodenního pronájmu je začátkem půlnoc prvního dne."</li>
                    </ul></section>
                    <section><h3>"Vyúčtování a doklady"</h3><ul>
                        <li>"Po skončení nájmu zkontrolujte požadované fotografie elektroměru a doplňte náklady. Vyúčtování odešlete z detailu pronájmu."</li>
                        <li>"Po přijetí platby nájem ručně uzavřete. Kopii dokladu smí zobrazit jen oprávněný správce po zadání hesla."</li>
                    </ul></section>
                }.into_any() }}
                    <section><h3>"Pravidelné rezervace"</h3><ul>
                        <li>"Pověřený správce může vytvořit rezervaci s individuální cenou, která se veřejnosti nezobrazuje. Opakování lze nastavit každý týden, každých 14 dní nebo jednou měsíčně do zvoleného data. Měsíční opakování zachová pořadí a den v týdnu, například druhý čtvrtek. Chybějící pátý výskyt se vynechá."</li>
                        <li>"Pokud se některý termín překrývá s jinou rezervací, nic se neuloží. Upravte termín a zkuste to znovu."</li>
                        <li>"U opakování s energiemi zahrnutými v ceně odešle systém vyúčtování po každém termínu sám. Volba bez čekání na potvrzení platby nájem po odeslání uzavře. Sjednaná cena se nadále platí, nájem však nebude označen jako zaplacený."</li>
                    </ul></section>
            </div>
        </details>
    }
}

pub(super) fn date_display(value: &str) -> String {
    let parts: Vec<_> = value.split('-').collect();
    if parts.len() == 3 {
        if let (Ok(year), Ok(month), Ok(day)) = (
            parts[0].parse::<u16>(),
            parts[1].parse::<u8>(),
            parts[2].parse::<u8>(),
        ) {
            return format!("{day}. {month}. {year}");
        }
    }
    value.to_owned()
}

fn period_display(
    start: &str,
    end: &str,
    start_time: Option<&str>,
    end_time: Option<&str>,
) -> String {
    if let (Some(from), Some(to)) = (start_time, end_time) {
        format!("{}, {} až {}", date_display(start), from, to)
    } else if start == end {
        format!("{}, celý den", date_display(start))
    } else {
        format!("{} až {}", date_display(start), date_display(end))
    }
}

fn rental_status(item: &Rental) -> &'static str {
    if item.status == "invoiced" && !item.wait_for_payment {
        "Vyúčtováno, čeká na odeslání"
    } else {
        status_label(&item.status)
    }
}

fn matches_filter(item: &Rental, filter: &str) -> bool {
    match filter {
        "" => true,
        "invoiced" => item.status == "invoiced" && item.wait_for_payment,
        "closing" => item.status == "invoiced" && !item.wait_for_payment,
        "finished" => matches!(item.status.as_str(), "paid" | "closed"),
        _ => item.status == filter,
    }
}

#[component]
fn RentalSummary(items: Vec<Rental>, filter: RwSignal<String>) -> impl IntoView {
    let stats = [
        ("pending", "Žádosti", "Čekají na rozhodnutí"),
        ("approved", "Nevyúčtované", "Schválené pronájmy"),
        ("invoiced", "Čekají na úhradu", "Vyúčtované nájmy"),
        ("finished", "Uzavřené", "Zaplacené i bez kontroly platby"),
    ];
    view! { <div class="rental-admin-summary" role="group" aria-label="Přehled podle stavu">
        {stats.into_iter().map(|(state, label, hint)| {
            let count = items.iter().filter(|item| matches_filter(item, state)).count();
            view! { <button type="button" class="rental-admin-stat" class:active=move || filter.get() == state
                aria-pressed=move || (filter.get() == state).to_string()
                on:click=move |_| filter.set(if filter.get_untracked() == state { String::new() } else { state.into() })>
                <span class="rental-admin-stat-top"><span>{label}</span><i class=format!("rental-admin-dot rental-state-{state}")></i></span>
                <strong>{count}</strong><small>{hint}</small>
            </button> }
        }).collect_view()}
    </div> }
}

#[component]
pub fn Rentals() -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let ctx = expect_context::<AdminContext>();
    let can_manage = RwSignal::new(None::<bool>);
    let data = LocalResource::new(move || async move {
        let allowed = refresh_rental_permissions(ctx).await?;
        let _ = can_manage.try_set(Some(allowed));
        api::get::<Vec<Rental>>("/api/v1/admin/rentals").await
    });
    view! {
        <div class="rental-admin"><div class="admin-title-row">
            <header class="rental-overview-heading"><h2>"Přehled pronájmů"</h2><p>"Žádosti, schválené pronájmy a vyúčtování obecního majetku."</p></header>
            <div class="admin-create rental-admin-actions">{move || can_manage.get().map(|allowed| {
                if allowed { view! { <AdminLink href="/admin/pronajmy/nova" label="Vytvořit rezervaci" icon="calendar"/><AdminLink href="/admin/pronajmy/majetek" label="Správa majetku a cen" icon="grid"/> }.into_any() }
                else { view! { <AdminLink href="/admin/pronajmy/majetek" label="Majetek a ceny" icon="grid"/> }.into_any() }
            })}<AdminLink href="/admin/pronajmy/platby" label="Platební údaje" icon="paper"/></div>
        </div>
        <RentalsHelp/>
        <Suspense fallback=Pending>{move || data.get().map(|result| match result {
            Err(error) => view! { <FailureView error/> }.into_any(),
            Ok(items) => {
                let all_items = items.clone();
                let current = filter.get();
                let items: Vec<_> = items.into_iter().filter(|item| matches_filter(item, &current)).collect();
                view! { <RentalSummary items=all_items filter/>
        <section class="admin-panel admin-list-panel rental-admin-list">
            <div class="admin-filters rental-admin-filters">
                <label class="admin-field"><span>"Stav pronájmu"</span>
                    <select prop:value=move || filter.get() on:change=move |ev| filter.set(event_target_value(&ev))>
                        <option value="">"Všechny"</option>
                        <option value="pending">"Žádosti"</option>
                        <option value="approved">"Nevyúčtované"</option>
                        <option value="invoiced">"Čekají na úhradu"</option>
                        <option value="closing">"Čekají na odeslání a uzavření"</option>
                        <option value="finished">"Všechny uzavřené"</option>
                        <option value="paid">"Zaplacené"</option>
                        <option value="closed">"Uzavřené bez kontroly platby"</option>
                        <option value="rejected">"Zamítnuté"</option>
                        <option value="cancelled">"Zrušené"</option>
                    </select>
                </label>
                <button class="button secondary" type="button" on:click=move |_| data.refetch()>"Obnovit seznam"</button>
            </div>
                    {if items.is_empty() {
                        view! { <div class="admin-empty"><h2>"Žádné pronájmy"</h2><p>"Pro zvolený stav zatím nejsou žádné záznamy."</p></div> }.into_any()
                    } else {
                        view! { <div class="admin-table-wrap"><table class="admin-table">
                            <caption class="sr-only">"Seznam pronájmů"</caption>
                            <thead><tr><th>"Žadatel a majetek"</th><th>"Termín"</th><th>"Stav"</th><th>"Cena pronájmu"</th></tr></thead>
                            <tbody>{items.into_iter().map(|item| {
                                let period = period_display(&item.start_date, &item.end_date, item.start_time.as_deref(), item.end_time.as_deref());
                                let label = rental_status(&item);
                                let group = rental_category_label(&item);
                                view! {
                                <tr><td class="admin-name"><AdminLink href=format!("/admin/pronajmy/{}", item.id) label=item.name icon="calendar"/><small>{item.asset_name}</small><small class="rental-rate-label">{group}</small></td>
                                    <td class="rental-admin-date">{period}{item.series_id.map(|_| view! {<small class="rental-recurrence-label">{if item.recurrence_interval_months == Some(1) {"Jednou měsíčně"} else if item.recurrence_interval_days == Some(14) {"Každých 14 dní"} else {"Každý týden"}}</small>})}</td>
                                    <td><span class=format!("rental-admin-status rental-state-{}", item.status)><i></i>{label}</span>{item.billing_error.as_ref().filter(|message| !message.is_empty()).map(|_| view! {<small class="rental-billing-warning">"Vyúčtování vyžaduje pozornost"</small>})}</td><td><strong>{format!("{} Kč", item.rental_price)}</strong></td></tr>
                            }}).collect_view()}</tbody>
                        </table></div> }.into_any()
                    }}
        </section> }.into_any()
            }
        })}</Suspense></div>
    }
}

#[component]
pub fn RentalAssets() -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let can_manage = RwSignal::new(None::<bool>);
    let data = LocalResource::new(move || async move {
        let allowed = refresh_rental_permissions(ctx).await?;
        let _ = can_manage.try_set(Some(allowed));
        api::get::<Vec<Asset>>("/api/v1/admin/rental-assets").await
    });
    let adding = RwSignal::new(false);
    let loaded = RwSignal::new(false);
    let assets = RwSignal::new(Vec::<Asset>::new());
    let dirty_forms = RwSignal::new(BTreeSet::<(i64, u8)>::new());
    Effect::new(move |_| {
        if let Some(Ok(items)) = data.get() {
            if !loaded.get_untracked() {
                assets.set(items);
                loaded.set(true);
            }
        }
    });
    Effect::new(move |_| ctx.dirty.set(!dirty_forms.get().is_empty()));
    view! {
        <Title text="Majetek k pronájmu · Správa Vyskře"/>
        <div class="rental-admin"><div class="rental-admin-back"><AdminLink href="/admin/pronajmy" label="Zpět na pronájmy" icon="back"/></div>
        <div class="admin-title-row"><Heading title="Majetek k pronájmu" description="Nastavte nabídku, ceny a adresu pro upozornění u každé položky."/>
            <Show when=move || can_manage.get() == Some(true)><button class="button primary" type="button" on:click=move |_| adding.set(true)>"Přidat majetek"</button></Show>
        </div>
        <RentalsHelp assets=true/>
        <Show when=move || can_manage.get() == Some(false)><p class="admin-caption">"Rezervace a majetek můžete pouze prohlížet. Pro změny požádejte pověřeného správce."</p></Show>
        <Show when=move || adding.get() && can_manage.get() == Some(true)><AssetForm asset=Asset { active: true, require_meter: true, ..Default::default() } can_manage=true dirty_forms on_saved=Callback::new(move |saved: Asset| { assets.update(|items| items.push(saved)); adding.set(false); })/></Show>
        <Suspense fallback=Pending>{move || data.get().and_then(|result| match result {
            Err(error) => Some(view! { <FailureView error/> }.into_any()),
            Ok(_) => None,
        })}</Suspense>
        <Show when=move || loaded.get() && assets.get().is_empty()><p>"Zatím není přidán žádný majetek."</p></Show>
        <div class="rental-asset-grid"><For each=move || assets.get() key=|asset| asset.id children=move |asset| view! {
            <AssetForm asset can_manage=can_manage.get_untracked().unwrap_or(false) dirty_forms on_saved=Callback::new(move |saved: Asset| { assets.update(|items| { if let Some(item) = items.iter_mut().find(|item| item.id == saved.id) { *item = saved; } }); })/>
        }/></div>
        </div>
    }
}

#[component]
fn AssetForm(
    asset: Asset,
    can_manage: bool,
    dirty_forms: RwSignal<BTreeSet<(i64, u8)>>,
    on_saved: Callback<Asset>,
) -> impl IntoView {
    let id = asset.id;
    let saved = RwSignal::new(asset.clone());
    let name = RwSignal::new(asset.name);
    let description = RwSignal::new(asset.description);
    let nonprofit = RwSignal::new(RateFields::new(&asset.rates.nonprofit));
    let standard = RwSignal::new(RateFields::new(&asset.rates.standard));
    let notification_email = RwSignal::new(asset.notification_email);
    let require_meter = RwSignal::new(asset.require_meter);
    let active = RwSignal::new(asset.active);
    let error = RwSignal::new(None::<api::Failure>);
    let ctx = expect_context::<AdminContext>();
    let save = Action::new_local(move |_: &()| {
        let body = json!({
            "name":name.get_untracked(), "description":description.get_untracked(),
            "rates": RentalRates {
                nonprofit: nonprofit.get_untracked().value(),
                standard: standard.get_untracked().value(),
            },
            "notification_email":notification_email.get_untracked(),
            "require_meter":require_meter.get_untracked(), "active":active.get_untracked(),
        });
        async move {
            error.set(None);
            let method = if id == 0 { "POST" } else { "PUT" };
            let path = if id == 0 {
                "/api/v1/admin/rental-assets".to_string()
            } else {
                format!("/api/v1/admin/rental-assets/{id}")
            };
            let result = api::save(method, &path, Some(body)).await;
            if error.try_get_untracked().is_none() {
                return;
            }
            match result {
                Ok(value) => match serde_json::from_value::<Asset>(value) {
                    Ok(item) => {
                        saved.set(item.clone());
                        ctx.changed("Majetek byl uložen.");
                        on_saved.run(item);
                    }
                    Err(_) => error.set(Some(api::Failure {
                        status: 0,
                        message:
                            "Majetek se uložil, ale odpověď nebylo možné přečíst. Obnovte stránku."
                                .into(),
                    })),
                },
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    Effect::new(move |_| {
        let original = saved.get();
        let changed = name.get() != original.name
            || description.get() != original.description
            || nonprofit.get().value() != original.rates.nonprofit
            || standard.get().value() != original.rates.standard
            || notification_email.get() != original.notification_email
            || require_meter.get() != original.require_meter
            || active.get() != original.active
            || save.pending().get();
        dirty_forms.update(|forms| {
            if changed {
                forms.insert((id, 0));
            } else {
                forms.remove(&(id, 0));
            }
        });
    });
    on_cleanup(move || {
        let _ = dirty_forms.try_update(|forms| {
            forms.remove(&(id, 0));
        });
    });
    view! { <section class="admin-panel admin-form-section rental-asset-card">
        <header class="rental-asset-card-header"><div><span class="rental-admin-kicker">{if id == 0 { "NOVÁ POLOŽKA" } else { "PŘEDMĚT PRONÁJMU" }}</span>
            <h2>{move || if id == 0 { "Nový majetek".to_string() } else { name.get() }}</h2></div>
            {if id != 0 { view! { <span class="rental-asset-visibility" class:enabled=move || active.get()>{move || if active.get() {"Veřejně v nabídce"} else {"Skryto"}}</span> }.into_any() } else { ().into_any() }}
        </header>
        <form on:submit=move |ev| { ev.prevent_default(); if !save.pending().get_untracked() { save.dispatch(()); } }>
            <fieldset disabled=move || !can_manage || save.pending().get()>
                <div class="rental-asset-fields">
                <label class="admin-field"><span>"Název"</span><input type="text" required maxlength="120" prop:value=move || name.get() on:input=move |ev| name.set(event_target_value(&ev))/></label>
                <label class="admin-field"><span>"Popis"</span><textarea maxlength="5000" prop:value=move || description.get() on:input=move |ev| description.set(event_target_value(&ev))></textarea></label>
                <label class="admin-field"><span>"E-mail pro upozornění"</span><input type="email" required maxlength="254" prop:value=move || notification_email.get() on:input=move |ev| notification_email.set(event_target_value(&ev))/></label>
                </div>
                <section class="rental-asset-rates" aria-label="Veřejný ceník">
                    <h3>"Veřejný ceník"</h3>
                    <p class="admin-caption">"Od pondělí do pátku lze rezervovat hodiny nebo celé dny. Sobotu a neděli pouze jako celé dny. Denní sazba je stejná pro všechny dny v týdnu."</p>
                    <p class="rental-rate-hint">"Prázdné pole znamená nenastavenou sazbu, tento termín pak nelze veřejně rezervovat. Nula znamená bez nájemného, případné energie se vyúčtují zvlášť."</p>
                    <div class="rental-rates-grid">
                        <RateFieldsView title="Veřejně prospěšné neziskové akce" description="Ceník pro veřejně prospěšnou akci, která není pořádaná za účelem zisku." rate=nonprofit/>
                        <RateFieldsView title="Soukromá akce" description="Ceník pro soukromé akce." rate=standard/>
                    </div>
                </section>
                <div class="rental-asset-options"><label><input type="checkbox" prop:checked=move || require_meter.get() on:change=move |ev| require_meter.set(event_target_checked(&ev))/ >" Vyžadovat fotky elektroměru"</label>
                <label><input type="checkbox" prop:checked=move || active.get() on:change=move |ev| active.set(event_target_checked(&ev))/ >" Nabízet veřejnosti"</label></div>
                <ErrorMessage error/>
                {can_manage.then(|| view! { <div class="admin-actions rental-asset-save"><button class="button primary" type="submit">{move || if save.pending().get() { "Ukládám…" } else { "Uložit" }}</button></div> })}
            </fieldset>
        </form>
        {(id != 0).then(|| view! { <AssetBlocks asset_id=id can_manage dirty_forms/> })}
    </section> }
}

#[component]
fn AssetBlocks(
    asset_id: i64,
    can_manage: bool,
    dirty_forms: RwSignal<BTreeSet<(i64, u8)>>,
) -> impl IntoView {
    let start = RwSignal::new(String::new());
    let end = RwSignal::new(String::new());
    let hourly = RwSignal::new(false);
    let start_time = RwSignal::new("08:00".to_owned());
    let end_time = RwSignal::new("09:00".to_owned());
    let error = RwSignal::new(None::<api::Failure>);
    let ctx = expect_context::<AdminContext>();
    let data = LocalResource::new(move || async move {
        api::get::<Vec<BlockedInterval>>(&format!("/api/v1/admin/rental-assets/{asset_id}/blocks"))
            .await
    });
    let add = Action::new_local(move |_: &()| {
        let body = json!({"start_date":start.get_untracked(), "end_date":if hourly.get_untracked() {start.get_untracked()} else {end.get_untracked()},
            "start_time":hourly.get_untracked().then(|| start_time.get_untracked()), "end_time":hourly.get_untracked().then(|| end_time.get_untracked())});
        async move {
            error.set(None);
            let result = api::save(
                "POST",
                &format!("/api/v1/admin/rental-assets/{asset_id}/blocks"),
                Some(body),
            )
            .await;
            if error.try_get_untracked().is_none() {
                return;
            }
            match result {
                Ok(_) => {
                    start.set(String::new());
                    end.set(String::new());
                    data.refetch();
                    ctx.changed("Termín byl zablokován.");
                }
                Err(failure) => error.set(Some(failure)),
            }
        }
    });
    Effect::new(move |_| {
        let changed = !start.get().is_empty() || !end.get().is_empty() || add.pending().get();
        dirty_forms.update(|forms| {
            if changed {
                forms.insert((asset_id, 1));
            } else {
                forms.remove(&(asset_id, 1));
            }
        });
    });
    on_cleanup(move || {
        let _ = dirty_forms.try_update(|forms| {
            forms.remove(&(asset_id, 1));
        });
    });
    view! { <section class="rental-asset-blocks">
        <div class="rental-blocks-heading"><div><h3>"Blokované termíny"</h3><p>"Hodiny nebo dny, které obec nenabízí k pronájmu."</p></div></div>
        {can_manage.then(|| view! { <form class="rental-block-form" on:submit=move |ev| { ev.prevent_default(); if !add.pending().get_untracked() { add.dispatch(()); } }>
            <fieldset disabled=move || add.pending().get()>
                <label class="admin-field"><span>"Rozsah blokace"</span><select prop:value=move || if hourly.get() {"hours"} else {"days"} on:change=move |ev| hourly.set(event_target_value(&ev) == "hours")><option value="days">"Celé dny"</option><option value="hours">"Blok hodin"</option></select></label>
                <label class="admin-field"><span>"Od"</span><input type="date" required prop:value=move || start.get() on:input=move |ev| start.set(event_target_value(&ev))/></label>
                <Show when=move || !hourly.get()><label class="admin-field"><span>"Do včetně"</span><input type="date" required min=move || start.get() prop:value=move || end.get() on:input=move |ev| end.set(event_target_value(&ev))/></label></Show>
                <Show when=move || hourly.get()>
                    <label class="admin-field"><span>"Od hodin"</span><select prop:value=move || start_time.get() on:change=move |ev| start_time.set(event_target_value(&ev))>{(0..24).map(|hour| {let value=format!("{hour:02}:00"); view! {<option value=value.clone()>{value.clone()}</option>}}).collect_view()}</select></label>
                    <label class="admin-field"><span>"Do hodin"</span><select prop:value=move || end_time.get() on:change=move |ev| end_time.set(event_target_value(&ev))>{(1..=24).map(|hour| {let value=format!("{hour:02}:00"); view! {<option value=value.clone()>{value.clone()}</option>}}).collect_view()}</select></label>
                </Show>
                <button class="button secondary" type="submit">{move || if add.pending().get() { "Ukládám…" } else { "Blokovat termín" }}</button>
            </fieldset>
        </form> })}
        <ErrorMessage error/>
        <Suspense fallback=Pending>{move || data.get().map(|result| match result {
            Err(failure) => view! { <FailureView error=failure/> }.into_any(),
            Ok(items) if items.is_empty() => view! { <p class="rental-blocks-empty">"Žádné blokované termíny."</p> }.into_any(),
            Ok(items) => view! { <div class="rental-blocks-list" role="region" aria-label="Seznam blokovaných termínů" tabindex="0"><ul>{items.into_iter().map(|item| view! {
                <AssetBlockRow asset_id block=item can_manage on_removed=Callback::new(move |_| {data.refetch();})/>
            }).collect_view()}</ul></div> }.into_any(),
        })}</Suspense>
    </section> }
}

#[component]
fn AssetBlockRow(
    asset_id: i64,
    block: BlockedInterval,
    can_manage: bool,
    on_removed: Callback<()>,
) -> impl IntoView {
    let id = block.id;
    let date_label = period_display(
        &block.start_date,
        &block.end_date,
        block.start_time.as_deref(),
        block.end_time.as_deref(),
    );
    let description = format!("Opravdu chcete uvolnit blokovaný termín {date_label}?");
    let error = RwSignal::new(None::<api::Failure>);
    let ctx = expect_context::<AdminContext>();
    let remove = Action::new_local(move |_: &()| async move {
        error.set(None);
        let result = api::save(
            "DELETE",
            &format!("/api/v1/admin/rental-assets/{asset_id}/blocks/{id}"),
            None,
        )
        .await;
        if error.try_get_untracked().is_none() {
            return;
        }
        match result {
            Ok(_) => {
                on_removed.run(());
                ctx.changed("Blokovaný termín byl odstraněn.");
            }
            Err(failure) => error.set(Some(failure)),
        }
    });
    view! { <li><span>{date_label}</span>
        {can_manage.then(|| view! { <ConfirmButton label="Odstranit blokaci" title="Odstranit blokaci?" description disabled=Signal::derive(move || remove.pending().get()) on_confirm=Callback::new(move |_| {remove.dispatch(());}) danger=true/> })}
        <ErrorMessage error/>
    </li> }
}

#[component]
pub fn RentalDetail(id: i64) -> impl IntoView {
    let ctx = expect_context::<AdminContext>();
    let data = LocalResource::new(move || async move {
        let can_view_identity = refresh_rental_permissions(ctx).await?;
        let rental = api::get::<Rental>(&format!("/api/v1/admin/rentals/{id}")).await?;
        let payment_settings = if can_view_identity && rental.status == "approved" {
            Some(api::get::<PaymentSettings>("/api/v1/admin/rental-payment-settings").await?)
        } else {
            None
        };
        Ok((rental, can_view_identity, payment_settings))
    });
    view! {
        <Title text="Detail pronájmu · Správa Vyskře"/>
        <div class="rental-admin"><div class="rental-admin-back"><AdminLink href="/admin/pronajmy" label="Zpět na pronájmy" icon="back"/></div>
        <Suspense fallback=Pending>{move || data.get().map(|result| match result {
            Err(error) => view! { <FailureView error/> }.into_any(),
            Ok((rental, can_view_identity, payment_settings)) => view! { <RentalDetailContent rental can_view_identity payment_settings on_changed=Callback::new(move |_| data.refetch())/> }.into_any(),
        })}</Suspense></div>
    }
}

#[component]
fn RentalDetailContent(
    rental: Rental,
    can_view_identity: bool,
    payment_settings: Option<PaymentSettings>,
    on_changed: Callback<()>,
) -> impl IntoView {
    let id = rental.id;
    let status = rental.status.clone();
    let displayed_status = rental_status(&rental);
    let period = period_display(
        &rental.start_date,
        &rental.end_date,
        rental.start_time.as_deref(),
        rental.end_time.as_deref(),
    );
    let source_admin = rental.source == "admin";
    let organization = rental.tenant_type == "organization";
    let legacy_category = rental.event_type.is_none();
    let category_label = rental_category_label(&rental);
    let locality_description = if rental.rate_snapshot.is_none() {
        "Původní rezervace si zachovává sjednanou cenu a tehdejší pravidla."
    } else if !rental.is_local {
        "Místní sazba není uplatněna."
    } else if source_admin {
        "Místní bydliště nebo sídlo uvedl pověřený pracovník obce."
    } else if rental.locality_source == "automatic" {
        "Místní adresa byla předvyplněna z našeptávače. Před schválením ověřte trvalý pobyt nebo sídlo."
    } else {
        "Žadatel uvedl místní trvalý pobyt nebo sídlo ručně. Před schválením údaj ověřte."
    };
    let require_meter = rental.require_meter;
    let wait_for_payment = rental.wait_for_payment;
    let series_id = rental.series_id;
    let automatic = series_id.is_some() && !require_meter;
    let decision_details = format!(
        "Žadatel: {}. Předmět: {}. Termín: {}.",
        rental.name, rental.asset_name, period
    );
    let decision_details_reject = decision_details.clone();
    let energy_kwh = RwSignal::new(rental.energy_kwh.clone().unwrap_or_else(|| "0".into()));
    let energy_cost = RwSignal::new(rental.energy_cost.unwrap_or(0).to_string());
    let extra_cost = RwSignal::new(rental.extra_cost.unwrap_or(0).to_string());
    let account_number = RwSignal::new(
        payment_settings
            .as_ref()
            .map(|settings| settings.account_number.clone())
            .unwrap_or_default(),
    );
    let bank_code = RwSignal::new(
        payment_settings
            .as_ref()
            .map(|settings| settings.bank_code.clone())
            .unwrap_or_default(),
    );
    let account_iban = RwSignal::new(
        payment_settings
            .as_ref()
            .map(|settings| settings.iban.clone())
            .unwrap_or_default(),
    );
    let due_days = RwSignal::new(
        payment_settings
            .as_ref()
            .map(|settings| settings.due_days)
            .unwrap_or(7)
            .to_string(),
    );
    let rental_price = rental.rental_price;
    let total_preview = Signal::derive(move || {
        energy_cost
            .get()
            .parse::<i64>()
            .ok()
            .and_then(|energy| {
                extra_cost
                    .get()
                    .parse::<i64>()
                    .ok()
                    .and_then(|extra| energy.checked_add(extra))
            })
            .and_then(|costs| rental_price.checked_add(costs))
            .map(|total| format!("Celkem k úhradě: {total} Kč"))
            .unwrap_or_else(|| "Zadejte platné částky pro zobrazení celkové ceny.".into())
    });
    let error = RwSignal::new(None::<api::Failure>);
    let invoice_error = RwSignal::new(None::<api::Failure>);
    let ctx = expect_context::<AdminContext>();
    let invoice_attachment = NodeRef::<leptos::html::Input>::new();
    let invoice_filename = RwSignal::new(None::<String>);
    let invoice_available_description =
        rental
            .invoice_available_at
            .split_once('T')
            .map(|(date, time)| {
                format!(
                    "Vyúčtování lze odeslat od {} v {}.",
                    date_display(date),
                    time.get(..5).unwrap_or(time)
                )
            });
    let draft_initial = StoredValue::new((
        energy_kwh.get_untracked(),
        energy_cost.get_untracked(),
        extra_cost.get_untracked(),
        account_number.get_untracked(),
        bank_code.get_untracked(),
        account_iban.get_untracked(),
        due_days.get_untracked(),
    ));
    let invoice_editable = status == "approved" && can_view_identity && !automatic;
    let invoice_time_ready = RwSignal::new(false);
    let meter_ready = !require_meter || (rental.before_uploaded && rental.after_uploaded);
    let missing_photos = if require_meter && !rental.before_uploaded && !rental.after_uploaded {
        Some(
            "Před odesláním musí nájemce doplnit fotografie elektroměru před pronájmem i po něm. Poté obnovte detail pronájmu.",
        )
    } else if require_meter && !rental.before_uploaded {
        Some(
            "Před odesláním musí nájemce doplnit fotografii elektroměru před pronájmem. Poté obnovte detail pronájmu.",
        )
    } else if require_meter && !rental.after_uploaded {
        Some(
            "Před odesláním musí nájemce doplnit fotografii elektroměru po pronájmu. Poté obnovte detail pronájmu.",
        )
    } else {
        None
    };
    #[cfg(feature = "hydrate")]
    if invoice_editable {
        let available = rental.invoice_available_unix;
        Effect::new(move |_| {
            let update = move || {
                let _ = invoice_time_ready.try_set(
                    available > 0 && web_sys::js_sys::Date::now() / 1000.0 >= available as f64,
                );
            };
            update();
            if let Ok(interval) =
                set_interval_with_handle(update, std::time::Duration::from_secs(1))
            {
                on_cleanup(move || interval.clear());
            }
        });
    }
    Effect::new(move |_| {
        if invoice_editable {
            let current = (
                energy_kwh.get(),
                energy_cost.get(),
                extra_cost.get(),
                account_number.get(),
                bank_code.get(),
                account_iban.get(),
                due_days.get(),
            );
            let _ = ctx
                .dirty
                .try_set(current != draft_initial.get_value() || invoice_filename.get().is_some());
        }
    });
    on_cleanup(move || {
        let _ = ctx.dirty.try_set(false);
    });
    let action = Action::new_local(move |operation: &String| {
        let operation = operation.clone();
        let attachment = if operation == "invoice" {
            invoice_attachment
                .get_untracked()
                .and_then(|input| input.files())
                .and_then(|files| files.item(0))
        } else {
            None
        };
        let body = if operation == "invoice" {
            Some(json!({
                "energy_kwh": if require_meter {energy_kwh.get_untracked()} else {"0".to_owned()},
                "energy_cost": if require_meter {energy_cost.get_untracked().parse::<i64>().unwrap_or(-1)} else {0},
                "extra_cost": extra_cost.get_untracked().parse::<i64>().unwrap_or(-1),
                "account_number": account_number.get_untracked(),
                "bank_code": bank_code.get_untracked(),
                "account_iban": account_iban.get_untracked(),
                "due_days": due_days.get_untracked().parse::<i64>().unwrap_or(0),
            }))
        } else {
            None
        };
        async move {
            error.set(None);
            invoice_error.set(None);
            let result = if operation == "cancel-series" {
                api::save(
                    "POST",
                    &format!(
                        "/api/v1/admin/rental-series/{}/cancel",
                        series_id.unwrap_or(0)
                    ),
                    None,
                )
                .await
            } else if let (Some(file), Some(invoice)) = (attachment, body.clone()) {
                api::invoice_with_attachment(
                    &format!("/api/v1/admin/rentals/{id}/invoice-with-attachment"),
                    invoice,
                    file,
                )
                .await
            } else {
                api::save(
                    "POST",
                    &format!("/api/v1/admin/rentals/{id}/{operation}"),
                    body,
                )
                .await
            };
            // Ignore responses after this detail was unmounted, including its
            // shared notifications and the next editor's unsaved-change flag.
            if error.try_get_untracked().is_none() {
                return;
            }
            match result {
                Ok(_) => {
                    let _ = ctx.dirty.try_set(false);
                    ctx.changed("Změna pronájmu byla uložena.");
                    on_changed.run(());
                }
                Err(failure) => {
                    if operation == "invoice" {
                        invoice_error.set(Some(failure));
                    } else {
                        error.set(Some(failure));
                    }
                }
            }
        }
    });
    view! {
        <div class="rental-detail-heading"><Heading title=format!("Pronájem č. {id}") description=format!("{} · {}", rental.asset_name, displayed_status)/>
            <span class=format!("rental-admin-status rental-state-{status}")><i></i>{displayed_status}</span></div>
        {rental.billing_error.clone().filter(|message| !message.is_empty()).map(|message| view! {<div class="rental-billing-alert" role="alert"><strong>"Vyúčtování vyžaduje pozornost"</strong><p>{message}</p><AdminLink href="/admin/pronajmy/platby" label="Zkontrolovat platební údaje" icon="paper"/></div>})}
        {(!can_view_identity).then(|| view! { <p class="admin-caption">"Rezervaci můžete pouze prohlížet. Pro změny požádejte pověřeného správce."</p> })}
        <div class="rental-detail-grid"><div class="rental-detail-main">
        <section class="admin-panel admin-form-section rental-detail-card">
            <h2>"Údaje o pronájmu"</h2>
            <dl class="rental-detail-facts">
                <div><dt>"Termín"</dt><dd>{period.clone()}</dd></div>
                <div><dt>{if legacy_category {"Nájemce"} else if organization {"Název firmy nebo organizace"} else {"Jméno a příjmení"}}</dt><dd>{rental.name}</dd></div>
                {organization.then(|| view! {
                    <div><dt>"IČO"</dt><dd>{rental.organization_ico.clone().unwrap_or_else(|| "U původní rezervace nebylo uvedeno".into())}</dd></div>
                    <div><dt>"Odpovědná osoba"</dt><dd>{rental.responsible_name.clone().unwrap_or_else(|| "U původní rezervace nebyla uvedena".into())}</dd></div>
                })}
                <div><dt>"E-mail"</dt><dd>{rental.email}</dd></div>
                <div><dt>"Telefon"</dt><dd>{rental.phone}</dd></div>
                {(!organization).then(|| view! {<div><dt>"Adresa"</dt><dd>{rental.address}</dd></div>})}
                <div><dt>{if legacy_category {"Původní cenová skupina"} else {"Typ akce"}}</dt><dd>{category_label}</dd></div>
                <div><dt>"Nájemné"</dt><dd>{format!("{} Kč", rental.rental_price)}</dd></div>
                <div><dt>"Rezervaci zadal"</dt><dd>{if source_admin {"Pověřený pracovník obce"} else {"Nájemce"}}</dd></div>
                <div><dt>"Energie"</dt><dd>{if require_meter {"Vyúčtovat podle podkladů"} else {"Zahrnuté v ceně pronájmu"}}</dd></div>
                <div><dt>"Kontrola platby"</dt><dd>{if wait_for_payment {"Čekat na potvrzení úhrady"} else {"Po odeslání uzavřít bez kontroly platby"}}</dd></div>
            </dl>
            {legacy_category.then(|| view! {<p class="rental-locality-note">{locality_description}</p>})}
            {(!legacy_category && !source_admin && rental.event_type.as_deref() == Some("nonprofit") && status == "pending").then(|| view! {<p class="rental-locality-note">"Žadatel uvedl veřejně prospěšnou neziskovou akci. Před schválením tuto volbu zkontrolujte."</p>})}
            {source_admin.then(|| view! {<p class="admin-caption">"Individuální cena je neveřejná. Uvidí ji pouze správa a příjemce vyúčtování."</p>})}
            {series_id.map(|series| view! {<div class="rental-series-summary"><strong>{format!("Pravidelná rezervace č. {series}")}</strong><p>{format!("{} do {}", if rental.recurrence_interval_months == Some(1) {"Jednou měsíčně"} else if rental.recurrence_interval_days == Some(14) {"Každých 14 dní"} else {"Každý týden"}, rental.recurrence_until.as_deref().map(date_display).unwrap_or_default())}</p>
                {automatic.then(|| view! {<p>"Vyúčtování se odešle automaticky po každém termínu."</p>})}
            </div>})}
            <ErrorMessage error/>
            {if status == "pending" {
                if can_view_identity { view! { <div class="admin-actions rental-detail-decision">
                    <ConfirmButton label="Schválit žádost" title="Schválit žádost?" description=decision_details disabled=Signal::derive(move || action.pending().get()) on_confirm=Callback::new(move |_| { action.dispatch("approve".to_string()); })/>
                    <ConfirmButton label="Zamítnout žádost" title="Zamítnout žádost?" description=decision_details_reject disabled=Signal::derive(move || action.pending().get()) on_confirm=Callback::new(move |_| { action.dispatch("reject".to_string()); }) danger=true/>
                </div> }.into_any() } else { view! { <p class="admin-caption">"Ke schválení nebo zamítnutí žádosti nemáte oprávnění."</p> }.into_any() }
            } else { ().into_any() }}
            {if status == "invoiced" && wait_for_payment && can_view_identity { view! { <div class="admin-actions rental-detail-decision">
                <button class="button primary" type="button" disabled=move || action.pending().get() on:click=move |_| {
                    #[cfg(feature = "hydrate")]
                    let confirmed = window().confirm_with_message("Potvrzujete přijetí platby? Uzavřením nájmu bude smazána kopie dokladu.").unwrap_or(false);
                    #[cfg(not(feature = "hydrate"))]
                    let confirmed = false;
                    if confirmed { action.dispatch("paid".to_string()); }
                }>"Potvrdit úhradu a uzavřít nájem"</button>
            </div> }.into_any() } else { ().into_any() }}
            {(source_admin && status == "approved" && can_view_identity).then(|| view! {<div class="admin-actions rental-detail-decision">
                <ConfirmButton label="Zrušit tento termín" title="Zrušit rezervaci?" description=format!("Zrušíte pouze tento termín: {period}. Zrušit lze jen dosud nezahájenou rezervaci.") disabled=Signal::derive(move || action.pending().get()) on_confirm=Callback::new(move |_| {action.dispatch("cancel".to_owned());}) danger=true/>
            </div>})}
            {(series_id.is_some() && can_view_identity).then(|| view! {<div class="admin-actions rental-detail-decision">
                <ConfirmButton label="Zrušit budoucí opakování" title="Zrušit budoucí termíny série?" description="Uvolní se všechny dosud nezahájené a nevyúčtované termíny této pravidelné rezervace. Starší termíny a vyúčtování zůstanou zachované." disabled=Signal::derive(move || action.pending().get()) on_confirm=Callback::new(move |_| {action.dispatch("cancel-series".to_owned());}) danger=true/>
            </div>})}
        </section>
        {require_meter.then(|| view! {<section class="admin-panel admin-form-section rental-detail-card rental-meter-card">
            <div class="rental-card-heading"><div><span class="rental-admin-kicker">"PODKLADY PRO VYÚČTOVÁNÍ"</span><h2>"Elektroměr"</h2></div></div>
            <RentalMeterPhotos id before_uploaded=rental.before_uploaded after_uploaded=rental.after_uploaded/>
            {rental.before_uploaded.then(|| view! { <p>{format!("Před pronájmem, orientační čas pořízení: {}", rental.before_photo_taken_at.unwrap_or_else(|| "není k dispozici".into()))}</p> })}
            {rental.after_uploaded.then(|| view! { <p>{format!("Po pronájmu, orientační čas pořízení: {}", rental.after_photo_taken_at.unwrap_or_else(|| "není k dispozici".into()))}</p> })}
            {(rental.before_uploaded || rental.after_uploaded).then(|| view! { <p class="admin-caption">"Čas pochází z údajů fotoaparátu a není ověřeným dokladem o pořízení."</p> })}
        </section>})}</div><aside class="rental-detail-aside">
        <section class="admin-panel admin-form-section rental-detail-card rental-identity-card"><span class="rental-admin-kicker">"CHRÁNĚNÝ ÚDAJ"</span><h2>{if organization {"Kopie občanského průkazu odpovědné osoby"} else {"Kopie občanského průkazu"}}</h2>
            {rental.identity_uploaded.then(|| view! {
                {if can_view_identity { view! { <RentalIdentity id/> }.into_any() }
                    else { view! { <p class="admin-caption">"Pro zobrazení kopie dokladu nemáte oprávnění."</p> }.into_any() }}
            })}
            {(!rental.identity_uploaded).then(|| view! { <p class="admin-caption">{if matches!(status.as_str(), "paid" | "closed" | "rejected" | "cancelled") {"Kopie dokladu byla odstraněna."} else if source_admin && !organization {"U rezervace fyzické osoby zadané pověřeným pracovníkem se kopie dokladu nenahrává. Ověření dokladu provede úřední osoba."} else if source_admin && organization {"U této původní rezervace nebyla kopie dokladu odpovědné osoby nahrána."} else {"Kopie dokladu není k dispozici."}}</p> })}
        </section>
        {if status == "approved" && automatic {view! {<section class="admin-panel admin-form-section rental-detail-card rental-invoice-card"><h2>"Automatické vyúčtování"</h2><p>"Po skončení tohoto termínu systém odešle vyúčtování se sjednanou cenou. Energie se nevyúčtovávají."</p><p>{if wait_for_payment {"Po odeslání vyúčtování nájem počká na ruční potvrzení úhrady."} else {"Po přijetí e-mailu odesílacím serverem se nájem automaticky uzavře bez kontroly platby."}}</p><AdminLink href="/admin/pronajmy/platby" label="Výchozí platební údaje" icon="paper"/><button class="button secondary" type="button" on:click=move |_| on_changed.run(())>"Obnovit stav"</button></section>}.into_any()} else if status == "approved" && can_view_identity { view! { <section class="admin-panel admin-form-section rental-detail-card rental-invoice-card">
            <h2>"Připravit vyúčtování"</h2>
            {invoice_available_description.map(|text| view! { <p class="rental-invoice-available" data-invoice-available>{text}</p> })}
            <Show when=move || !invoice_time_ready.get()><p class="admin-caption">"Podklady můžete připravit už nyní. Odeslání se zpřístupní po skončení pronájmu."</p></Show>
            {missing_photos.map(|text| view! { <p class="rental-invoice-missing" data-invoice-missing>{text}</p> })}
            <p>{if require_meter {"Zkontrolujte fotografie elektroměru, zadejte spotřebu a výslednou cenu energií. Nájemné se přičte automaticky."} else {"Energie se u tohoto pronájmu nevyúčtovávají. Nájemné se přičte automaticky."}}</p>
            <form on:submit=move |ev| {
                ev.prevent_default();
                if action.pending().get_untracked() || !invoice_time_ready.get_untracked() || !meter_ready { return; }
                let file = invoice_attachment.get_untracked().and_then(|input| input.files()).and_then(|files| files.item(0));
                if file.as_ref().is_some_and(|file| file.size() > 10.0 * 1024.0 * 1024.0) {
                    invoice_error.set(Some(api::Failure { status: 413, message: "Faktura je příliš velká. Maximální velikost je 10 MiB.".into() }));
                    return;
                }
                action.dispatch("invoice".to_string());
            }>
                <fieldset class="rental-invoice-fields" disabled=move || action.pending().get()>
                    {require_meter.then(|| view! {<label class="admin-field"><span>"Spotřeba elektřiny v kWh"</span><input type="text" inputmode="decimal" required prop:value=move || energy_kwh.get() on:input=move |ev| energy_kwh.set(event_target_value(&ev))/></label>
                    <label class="admin-field"><span>"Celková cena energie v Kč"</span><input type="number" min="0" step="1" required prop:value=move || energy_cost.get() on:input=move |ev| energy_cost.set(event_target_value(&ev))/></label>})}
                    <label class="admin-field"><span>"Další náklady v Kč"</span><input type="number" min="0" step="1" required prop:value=move || extra_cost.get() on:input=move |ev| extra_cost.set(event_target_value(&ev))/></label>
                    <p class="admin-caption">"Platební údaje jsou předvyplněné z nastavení. Změny zde platí pouze pro toto vyúčtování."</p>
                    <label class="admin-field"><span>"Číslo účtu včetně případného předčíslí"</span><input type="text" inputmode="numeric" autocomplete="off" maxlength="17" required=move || total_preview.get() != "Celkem k úhradě: 0 Kč" prop:value=move || account_number.get() on:input=move |ev| account_number.set(event_target_value(&ev))/></label>
                    <label class="admin-field"><span>"Kód banky"</span><input type="text" inputmode="numeric" autocomplete="off" maxlength="4" required=move || total_preview.get() != "Celkem k úhradě: 0 Kč" prop:value=move || bank_code.get() on:input=move |ev| bank_code.set(event_target_value(&ev))/></label>
                    <label class="admin-field"><span>"IBAN pro zahraniční platby (nepovinný)"</span><input type="text" maxlength="40" autocomplete="off" prop:value=move || account_iban.get() on:input=move |ev| account_iban.set(event_target_value(&ev))/><small>"Pokud IBAN vyplníte, musí odpovídat uvedenému účtu. Při nulové částce účet není potřeba."</small></label>
                    <label class="admin-field"><span>"Splatnost ve dnech"</span><input type="number" min="1" max="90" required prop:value=move || due_days.get() on:input=move |ev| due_days.set(event_target_value(&ev))/></label>
                    {(!wait_for_payment).then(|| view! {<p class="admin-caption">"Sjednaná cena se nadále platí. Po odeslání vyúčtování se nájem uzavře bez čekání na potvrzení úhrady."</p>})}
                    <label class="admin-field"><span>"Faktura od účetní (nepovinné PDF)"</span>
                        <input node_ref=invoice_attachment type="file" accept="application/pdf" hidden=move || invoice_filename.get().is_some() on:change=move |_| {
                            invoice_filename.set(invoice_attachment.get_untracked().and_then(|input| input.files()).and_then(|files| files.item(0)).map(|file| file.name()));
                            invoice_error.set(None);
                        }/>
                        <small>"Můžete přiložit jednu vlastní fakturu ve formátu PDF, maximálně 10 MiB. Zkontrolujte, že patří nájemci a odpovídá vyúčtování. Odešleme ji společně s vyúčtováním."</small>
                    </label>
                    <Show when=move || invoice_filename.get().is_some()>
                        <div class="rental-invoice-attachment"><span data-invoice-filename>{move || invoice_filename.get().unwrap_or_default()}</span><button type="button" class="rental-invoice-attachment-remove" aria-label="Odebrat vybranou fakturu" title="Odebrat vybranou fakturu" disabled=move || action.pending().get() on:click=move |_| {
                            if action.pending().get_untracked() { return; }
                            if let Some(input) = invoice_attachment.get_untracked() { input.set_value(""); }
                            invoice_error.set(None);
                            invoice_filename.set(None);
                            request_animation_frame(move || {
                                if let Some(input) = invoice_attachment.get_untracked() { let _ = input.focus(); }
                            });
                        }>"×"</button></div>
                    </Show>
                    <ErrorMessage error=invoice_error/>
                    <div class="rental-invoice-total"><strong>{move || total_preview.get()}</strong><small>{format!("Variabilní symbol: {id}")}</small></div>
                    <button class="button primary" type="submit" disabled=move || action.pending().get() || !invoice_time_ready.get() || !meter_ready>"Odeslat vyúčtování"</button>
                </fieldset>
            </form>
        </section> }.into_any() } else if matches!(status.as_str(), "invoiced" | "paid" | "closed") { view! { <section class="admin-panel admin-form-section rental-detail-card rental-invoice-card">
            <h2>"Vyúčtování"</h2>
            <p><strong>"Odeslání e-mailu: "</strong>{match rental.invoice_mail_status.as_deref() {
                Some("sent") => "Předáno odesílacímu serveru",
                Some("retrying") => "Odeslání se nezdařilo, systém to zkusí znovu",
                Some("pending") => "Čeká na odeslání",
                _ => "Stav není dostupný",
            }}</p>
            <button class="button secondary" type="button" on:click=move |_| on_changed.run(())>"Obnovit stav odeslání"</button>
            {rental.invoice_sent_at.map(|sent| view! { <p>{format!("Předáno serveru: {sent}")}</p> })}
            {matches!(rental.invoice_mail_status.as_deref(), Some("sent")).then(|| view! { <p class="admin-caption">"Přijetí odesílacím serverem ještě nepotvrzuje doručení do schránky nájemníka."</p> })}
            {if matches!(rental.invoice_mail_status.as_deref(), Some("sent")) {
                match rental.invoice_pdf_attached {
                    Some(true) => Some(view! { <p>"PDF vyúčtování bylo přiloženo."</p> }),
                    Some(false) => Some(view! { <p>"PDF se nepodařilo vytvořit, vyúčtování bylo odesláno v textu e-mailu."</p> }),
                    None => None,
                }
            } else { None }}
            {require_meter.then(|| view! {<p>{format!("Spotřeba: {} kWh", rental.energy_kwh.unwrap_or_default())}</p><p>{format!("Energie: {} Kč", rental.energy_cost.unwrap_or(0))}</p>})}
            <p>{format!("Další náklady: {} Kč", rental.extra_cost.unwrap_or(0))}</p>
            <p><strong>{format!("Celkem: {} Kč", rental.total_price.unwrap_or(0))}</strong></p>
            {(!wait_for_payment).then(|| view! {<p>"Uzavření nevyžaduje kontrolu platby. Tento stav nepotvrzuje, že nájemce zaplatil."</p>})}
            <p>{format!("Variabilní symbol: {id}")}</p>
            {match (rental.account_number.as_deref(), rental.bank_code.as_deref()) {
                (Some(account), Some(bank)) if !account.is_empty() && !bank.is_empty() => Some(view! { <p>{format!("Účet: {account}/{bank}")}</p> }),
                _ => None,
            }}
            {rental.payment_iban.as_deref().filter(|iban| !iban.is_empty()).or_else(|| {
                if rental.account_number.as_deref().is_none_or(str::is_empty) {
                    rental.account_iban.as_deref().filter(|iban| !iban.is_empty())
                } else {
                    None
                }
            }).map(|iban| view! { <p>{format!("IBAN: {iban}")}</p> })}
            {rental.invoice_issued_on.as_deref().map(|date| view! { <p>{format!("Datum vystavení: {}", date_display(date))}</p> })}
            {rental.invoice_due_on.as_deref().map(|date| view! { <p>{format!("Datum splatnosti: {}", date_display(date))}</p> })}
        </section> }.into_any() } else { ().into_any() }}
        </aside></div>
    }
}
