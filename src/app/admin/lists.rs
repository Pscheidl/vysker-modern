use super::{
    api::{self, Kind},
    ui::*,
};
use crate::app::components::Icon;
use leptos::prelude::*;
use leptos_meta::Title;

#[component]
pub fn Overview() -> impl IntoView {
    let data = LocalResource::new(|| api::get::<api::Overview>("/api/v1/admin/overview"));
    view! {<Title text="Přehled správy · Vyskeř"/><Heading title="Vše důležité na jednom místě." description="Přehled obecního webu a rychlý přístup ke správě obsahu."/>
        <Suspense fallback=Pending>{move ||data.get().map(|result|match result {
            Err(error)=>view!{<FailureView error/><button type="button" class="button secondary" on:click=move |_|data.refetch()>"Načíst znovu"</button>}.into_any(),
            Ok(stats)=>view!{
                <div class="admin-stats">
                    <Stat label="Na úřední desce" value=stats.published note=format!("{} naplánováno",stats.scheduled) icon="board"/>
                    <Stat label="Rozpracovaná vyvěšení" value=stats.drafts note="Čekají na zveřejnění" icon="paper"/>
                    <Stat label="Ověření odběratelé" value=stats.subscribers note=format!("{} zpráv čeká na odeslání",stats.pending_mail) icon="mail"/>
                </div>
                <div class="admin-overview-grid"><section class="admin-panel admin-quick"><div class="admin-panel-heading"><h2>"Co potřebujete vyřídit?"</h2><span class="admin-caption">{api::date(&stats.current_date)}</span></div>
                    <Quick href="/admin/uredni-deska/nove" title="Vyvěsit na úřední desku" description="Koncept, přílohy a termín sejmutí." icon="board"/>
                    <Quick href="/admin/dokumenty/nove" title="Přidat dokument" description="Formuláře a další dokumenty ke stažení." icon="paper"/>
                    <Quick href="/admin/stranky/nove" title="Vytvořit stránku" description="Nové informace pro občany." icon="pages"/>
                </section><section class="admin-panel admin-overview-aside"><span class="admin-kicker"><Icon name="check"/>"PŘEHLED OBSAHU"</span><h2>"Web obce v číslech"</h2><dl><div><dt>"Zveřejněné dokumenty"</dt><dd>{stats.documents}</dd></div><div><dt>"Zveřejněné stránky"</dt><dd>{stats.pages}</dd></div></dl><p>"Každá změna obsahu se zapisuje do auditu. Záznamy úřední desky zůstávají v archivu i po odstranění příloh."</p><AdminLink href="/admin/audit" label="Otevřít audit" icon="history"/></section></div>
            }.into_any()
        })}</Suspense>
    }
}
#[component]
fn Stat(
    label: &'static str,
    value: i64,
    #[prop(into)] note: String,
    icon: &'static str,
) -> impl IntoView {
    view! {<section class="admin-stat"><div><span>{label}</span><Icon name=icon/></div><strong>{value}</strong><p>{note}</p></section>}
}
#[component]
fn Quick(
    href: &'static str,
    title: &'static str,
    description: &'static str,
    icon: &'static str,
) -> impl IntoView {
    view! {<div class="admin-quick-row"><div><AdminLink href label=title icon/><p>{description}</p></div><Icon/></div>}
}

#[component]
pub fn Entries(kind: Kind) -> impl IntoView {
    let offset = RwSignal::new(0usize);
    let search = RwSignal::new(String::new());
    let query = RwSignal::new(String::new());
    let status = RwSignal::new(String::new());
    let data = LocalResource::new(move || {
        let path = format!(
            "/api/v1/admin/{}?limit=21&offset={}&status={}&q={}",
            kind.api(),
            offset.get(),
            status.get(),
            percent_encoding::utf8_percent_encode(&query.get(), percent_encoding::NON_ALPHANUMERIC)
        );
        async move { api::get::<Vec<api::Entry>>(&path).await }
    });
    let description = match kind {
        Kind::Notice => {
            "Koncepty, aktuální vyvěšení i archiv. Všechny záznamy zůstávají dohledatelné."
        }
        Kind::Document => "Správa formulářů, obecních dokumentů a jejich příloh.",
        Kind::Page => "Obsahové stránky veřejného webu, koncepty a zveřejnění.",
    };
    view! {
        <Title text=format!("{} · Správa Vyskře",kind.title())/><div class="admin-title-row"><Heading title=kind.title() description/><div class="admin-create"><AdminLink href=format!("{}/nove",kind.path()) label=kind.create_label() icon="plus"/></div></div>
        <section class="admin-panel admin-list-panel">
            <form class="admin-filters" on:submit=move|ev|{ev.prevent_default();offset.set(0);query.set(search.get());}>
                <label class="admin-search"><Icon name="search"/><span class="sr-only">"Hledat podle názvu"</span><input type="search" placeholder="Hledat podle názvu…" maxlength="300" prop:value=move||search.get() on:input=move|ev|search.set(event_target_value(&ev))/></label>
                <label class="admin-status-filter"><span class="sr-only">"Stav"</span><select aria-label="Stav" prop:value=move||status.get() on:change=move|ev|{offset.set(0);status.set(event_target_value(&ev));}><option value="">"Všechny stavy"</option><option value="draft">"Koncepty"</option>
                    {if kind==Kind::Notice{view!{<option value="published">"Vyvěšeno"</option><option value="scheduled">"Naplánováno"</option>}.into_any()}else{view!{<option value="published">"Zveřejněno"</option>}.into_any()}}
                    {(kind!=Kind::Page).then(||view!{<option value="archived">"Archiv"</option>})}
                </select></label><button class="button secondary" type="submit">"Hledat"</button><button class="admin-refresh" type="button" aria-label="Obnovit seznam" on:click=move |_|data.refetch()><Icon name="refresh"/></button>
            </form>
            <Suspense fallback=Pending>{move||data.get().map(|result|match result{
                Err(error)=>view!{<FailureView error/>}.into_any(),
                Ok(rows)=>{
                    let count=rows.len();
                    view!{<div class="admin-table-wrap"><table class="admin-table"><caption class="sr-only">{kind.title()}</caption><thead><tr><th>"Název"</th><th>"Stav"</th><th>{if kind==Kind::Notice{"Vyvěšeno / sejmutí"}else if kind==Kind::Page{"Adresa stránky"}else{"Typ"}}</th><th><span class="sr-only">"Otevřít"</span></th></tr></thead><tbody>
                        {rows.into_iter().take(20).map(|row|{let state=row.status().to_string();let href=format!("{}/{}",kind.path(),row.id);view!{<tr><td class="admin-name"><AdminLink href=href.clone() label=row.title icon=if kind==Kind::Page{"pages"}else{"paper"}/><span class="admin-row-id">{format!("#{:04}",row.id)}</span></td><td><Status value=state/></td><td class="admin-table-meta">{if kind==Kind::Notice{view!{<span>{row.published_on.as_deref().map(api::date).unwrap_or_else(||"Datum vyvěšení není uvedeno".into())}</span><small>{row.withdraw_on.as_deref().map(|d|format!("Do {}",api::date(d))).unwrap_or_else(||"Datum sejmutí není uvedeno".into())}</small>}.into_any()}else if kind==Kind::Page{view!{<span>{format!("/stranky/{}",row.slug)}</span>}.into_any()}else{view!{<span>"Obecný dokument"</span>}.into_any()}}</td><td class="admin-row-open"><AdminLink href label="Otevřít"/></td></tr>}}).collect_view()}
                    </tbody></table></div>
                    {(count==0).then(||view!{<div class="admin-empty"><Icon name="search"/><h2>"Žádné záznamy"</h2><p>"Upravte hledání nebo přidejte nový obsah."</p></div>})}
                    <Pager offset count/>
                    }.into_any()
                }
            })}</Suspense>
        </section>
    }
}

#[component]
pub fn AuditLog() -> impl IntoView {
    let offset = RwSignal::new(0usize);
    let data = LocalResource::new(move || {
        let path = format!("/api/v1/admin/audit?limit=21&offset={}", offset.get());
        async move { api::get::<Vec<api::Audit>>(&path).await }
    });
    view! {<Title text="Audit · Správa Vyskře"/><Heading title="Historie změn" description="Dohledatelný přehled činností správců a automatických úloh."/>
        <div class="admin-readonly"><Icon name="lock"/><span>"Pouze ke čtení. Záznamy nelze upravovat ani mazat."</span><button class="button secondary" type="button" on:click=move |_|data.refetch()>"Obnovit"</button></div>
        <section class="admin-panel admin-list-panel"><Suspense fallback=Pending>{move||data.get().map(|result|match result{
            Err(error)=>view!{<FailureView error/>}.into_any(),
            Ok(rows)=>{let count=rows.len();view!{<div class="admin-table-wrap"><table class="admin-table admin-audit"><caption class="sr-only">"Auditní záznamy"</caption><thead><tr><th>"Čas (UTC)"</th><th>"Operace"</th><th>"Záznam"</th><th>"Provedl"</th></tr></thead><tbody>{rows.into_iter().take(20).map(|row|view!{<tr><td><time datetime=row.occurred_at.clone()>{api::date(&row.occurred_at)}<small>{row.occurred_at.get(11..19).unwrap_or("").to_owned()}</small></time><span class="admin-row-id">{format!("#{}",row.id)}</span></td><td>{operation(&row.operation)}</td><td>{object(&row.entity_type).to_owned()}" "<span class="admin-mono">{row.entity_id.map(|id|format!("#{id}")).unwrap_or_default()}</span></td><td>{row.actor_email.unwrap_or_else(||"Systém / veřejný požadavek".into())}</td></tr>}).collect_view()}</tbody></table></div><Pager offset count/>}.into_any()}
        })}</Suspense></section>
    }
}
// Older immutable audit events keep their original identifiers.
fn operation(value: &str) -> String {
    match value {
        "created" | "vytvoreni" => "Vytvoření",
        "updated" | "uprava" => "Úprava",
        "published" | "vyveseno" | "zverejneni" => "Zveřejnění",
        "scheduled" | "naplanovano" => "Naplánování",
        "withdrawn" | "sejmuti" => "Sejmutí do archivu",
        "archived" | "archivace" => "Archivace",
        "uploaded" | "nahrani" => "Nahrání přílohy",
        "removed" | "odebrani" => "Odebrání přílohy",
        "logged_in" | "prihlaseni" => "Přihlášení",
        "logged_out" | "unsubscribe" | "odhlaseni" => "Odhlášení",
        "subscription_requested" | "zadost_o_odber" => "Žádost o odběr",
        "verification" | "overeni" => "Ověření odběru",
        "schedule_missed" => "Zmeškané plánované zveřejnění",
        "availability_incident" => "Výpadek dostupnosti",
        "password_changed" => "Změna hesla",
        "password_reset_by_operator" => "Obnova hesla provozovatelem",
        "password_recovery_requested" => "Žádost o obnovu hesla",
        "password_recovered" => "Obnova hesla e-mailem",
        "subscriber_exported" => "Export údajů odběratele",
        "subscriber_withdrawn" => "Odvolání odběru správcem",
        "subscriber_erased" => "Výmaz odběratele",
        "deleted" => "Odstranění",
        "activated" => "Aktivace účtu",
        "deactivated" => "Deaktivace účtu",
        "sessions_revoked" => "Odhlášení všech zařízení",
        "retention_applied" => "Úklid po uplynutí lhůty uchování",
        "test_requested" => "Vyžádání zkušebního e-mailu",
        "test_sent" => "Přijetí zkušebního e-mailu SMTP serverem",
        _ => value,
    }
    .into()
}
fn object(value: &str) -> &str {
    match value {
        "notice" | "vyveseni" => "Úřední deska",
        "document" | "dokument" => "Dokument",
        "page" | "stranka" => "Stránka",
        "event" => "Akce",
        "navigation" => "Navigace",
        "category" => "Kategorie",
        "attachment" | "soubor" => "Příloha",
        "administrator" => "Správce",
        "subscriber" | "odberatel" => "Odběratel",
        "privacy" => "Ochrana údajů",
        "mail_settings" => "Nastavení odesílání",
        _ => value,
    }
}

#[component]
pub fn MailHistory() -> impl IntoView {
    let offset = RwSignal::new(0usize);
    let data = LocalResource::new(move || {
        let path = format!("/api/v1/admin/mail?limit=21&offset={}", offset.get());
        async move { api::get::<Vec<api::MailRecord>>(&path).await }
    });
    view! {<Title text="Rozesílání · Správa Vyskře"/><div class="admin-title-row"><Heading title="Rozesílané zprávy" description="Komu byla zpráva určena a zda ji přijal SMTP server. Záznamy zůstávají po schválenou dobu uchování."/><div class="admin-create"><AdminLink href="/admin/posta/nastaveni" label="Nastavení odesílání" icon="mail"/></div></div><p>"Přijetí SMTP serverem nepotvrzuje doručení do schránky ani přečtení."</p><button class="button secondary" type="button" on:click=move |_|data.refetch()>"Obnovit"</button>
    <section class="admin-panel admin-list-panel"><Suspense fallback=Pending>{move||data.get().map(|result|match result {
        Err(error)=>view!{<FailureView error/>}.into_any(),
        Ok(rows)=>{let count=rows.len();view!{<div class="admin-table-wrap"><table class="admin-table"><caption class="sr-only">"Historie rozesílání"</caption><thead><tr><th>"Příjemce"</th><th>"Zpráva"</th><th>"Stav a čas v UTC"</th><th>"Přihlášení k odběru"</th></tr></thead><tbody>{rows.into_iter().take(20).map(|row|view!{<tr><td>{row.email}</td><td><strong>{row.subject}</strong><small>{match row.purpose.as_str(){"verification"=>"Ověření adresy","recovery"=>"Obnova hesla",_=>"Nový dokument"}}" · #"{row.id}</small></td><td>{match row.status.as_str(){"sent"=>"Přijato SMTP serverem","cancelled"=>"Zrušeno","expired"=>"Odkaz vypršel před odesláním","retrying"=>"Čeká na další pokus",_=>"Ve frontě"}}<small>{api::unix_date(row.sent_at.unwrap_or(row.created_at))}</small><small>{format!("Pokusů: {}",row.attempts)}</small></td><td>{row.subscriber_id.map(|id|view!{<a class="text-link" href=format!("/api/v1/admin/subscribers/{id}/export") download="doklad-odberu.json">"Stáhnout doklad"</a>})}</td></tr>}).collect_view()}</tbody></table></div><Pager offset count/>}.into_any()}
    })}</Suspense></section>}
}
