use super::{
    api::{self, Kind},
    attachment_progress::AttachmentIndexProgress,
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
                    <Stat label="Koncepty dokumentů" value=stats.drafts note="Čekají na zveřejnění" icon="paper"/>
                    <Stat label="Ověření odběratelé" value=stats.subscribers note=format!("{} oznámení čeká na odeslání",stats.pending_mail) icon="mail"/>
                </div>
                <div class="admin-overview-grid"><section class="admin-panel admin-quick"><div class="admin-panel-heading"><h2>"Co potřebujete vyřídit?"</h2><span class="admin-caption">{api::date(&stats.current_date)}</span></div>
                    <Quick href="/admin/dokumenty/nove" title="Přidat dokument" description="Dokumenty ke stažení i vyvěšení na úřední desce." icon="paper"/>
                    <Quick href="/admin/stranky/nove" title="Vytvořit stránku" description="Nové informace pro občany." icon="pages"/>
                </section><section class="admin-panel admin-overview-aside"><span class="admin-kicker"><Icon name="check"/>"PŘEHLED OBSAHU"</span><h2>"Web obce v číslech"</h2><dl><div><dt>"Zveřejněné dokumenty"</dt><dd>{stats.documents}</dd></div><div><dt>"Zveřejněné stránky"</dt><dd>{stats.pages}</dd></div></dl><p>"Obsah, veřejná dostupnost a vyvěšení se evidují samostatně. Po prvním vyvěšení je obsah nevratně uzamčen."</p><AdminLink href="/admin/audit" label="Otevřít audit" icon="history"/></section></div>
            }.into_any()
        })}</Suspense>
        <AttachmentIndexProgress/>
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
    let state = RwSignal::new(if kind == Kind::Document {
        "all".to_string()
    } else {
        String::new()
    });
    let data = LocalResource::new(move || {
        let filter = if kind == Kind::Document {
            "state"
        } else {
            "status"
        };
        let path = format!(
            "/api/v1/admin/{}?limit=21&offset={}&{filter}={}&q={}",
            kind.api(),
            offset.get(),
            state.get(),
            percent_encoding::utf8_percent_encode(&query.get(), percent_encoding::NON_ALPHANUMERIC)
        );
        async move { api::get::<Vec<api::Entry>>(&path).await }
    });
    let description = match kind {
        Kind::Document => "Všechny dokumenty, vyvěšení na úřední desce a společný archiv.",
        Kind::Page => "Obsahové stránky veřejného webu, koncepty a zveřejnění.",
    };
    view! {
        <Title text=format!("{} · Správa Vyskře",kind.title())/><div class="admin-title-row"><Heading title=kind.title() description/><div class="admin-create"><AdminLink href=format!("{}/nove",kind.path()) label=kind.create_label() icon="plus"/></div></div>
        <section class="admin-panel admin-list-panel">
            <form class="admin-filters" on:submit=move|ev|{ev.prevent_default();offset.set(0);query.set(search.get());}>
                <label class="admin-search"><Icon name="search"/><span class="sr-only">"Hledat podle názvu"</span><input type="search" placeholder="Hledat podle názvu…" maxlength="300" prop:value=move||search.get() on:input=move|ev|search.set(event_target_value(&ev))/></label>
                <label class="admin-status-filter"><span class="sr-only">"Stav"</span><select aria-label="Stav" prop:value=move||state.get() on:change=move|ev|{offset.set(0);state.set(event_target_value(&ev));}>
                    {if kind==Kind::Document{view!{<option value="all">"Všechny dokumenty"</option><option value="current">"Aktuálně vyvěšeno"</option><option value="scheduled">"Plánovaná vyvěšení"</option><option value="archive">"Archiv"</option><option value="draft">"Koncepty"</option>}.into_any()}else{view!{<option value="">"Všechny stavy"</option><option value="draft">"Koncepty"</option><option value="published">"Zveřejněno"</option>}.into_any()}}
                </select></label><button class="button secondary" type="submit">"Hledat"</button><button class="admin-refresh" type="button" aria-label="Obnovit seznam" on:click=move |_|data.refetch()><Icon name="refresh"/></button>
            </form>
            <Suspense fallback=Pending>{move||data.get().map(|result|match result{
                Err(error)=>view!{<FailureView error/>}.into_any(),
                Ok(rows)=>{let count=rows.len();view!{
                    <div class="admin-table-wrap"><table class="admin-table"><caption class="sr-only">{kind.title()}</caption><thead><tr><th>"Název"</th><th>"Stav"</th><th>{if kind==Kind::Page{"Adresa stránky"}else{"Úřední deska"}}</th><th><span class="sr-only">"Otevřít"</span></th></tr></thead><tbody>
                        {rows.into_iter().take(20).map(|row|view!{<EntryRow row kind/>}).collect_view()}
                    </tbody></table></div>
                    {(count==0).then(||view!{<div class="admin-empty"><Icon name="search"/><h2>"Žádné záznamy"</h2><p>"Upravte filtr nebo přidejte nový obsah."</p></div>})}<Pager offset count/>
                }.into_any()}
            })}</Suspense>
        </section>
        {(kind==Kind::Document).then(||view!{<AttachmentIndexProgress/>})}
    }
}

#[component]
fn EntryRow(row: api::Entry, kind: Kind) -> impl IntoView {
    let status = row.status().to_string();
    let href = format!("{}/{}", kind.path(), row.id);
    let lock_label = match row.lock_state.as_str() {
        "locked"
            if matches!(
                row.board_status.as_str(),
                "published" | "archived" | "withdrawn"
            ) =>
        {
            "Uzamčeno po vyvěšení"
        }
        "locked" => "Uzamčeno",
        "review" => "K ověření",
        _ if status == "archived" => "Archivováno",
        _ if status == "published" => "Zveřejněno",
        _ => "Koncept",
    };
    let board_label = match row.board_status.as_str() {
        "published" => "Aktuálně vyvěšeno",
        "scheduled" => "Vyvěšení naplánováno",
        "archived" | "withdrawn" => "Sejmuto",
        _ => "Nevyvěšeno",
    };
    let dates = matches!(row.board_status.as_str(), "published" | "scheduled");
    let posted = row
        .published_on
        .as_deref()
        .map(|date| format!("Od {}", api::date(date)))
        .unwrap_or_else(|| "Datum není známo".into());
    let removal = row
        .withdraw_on
        .as_deref()
        .map(|date| format!("Plánované sejmutí {}", api::date(date)))
        .unwrap_or_else(|| "Termín sejmutí není určen".into());
    view! {<tr>
        <td class="admin-name"><AdminLink href=href.clone() label=row.title icon=if kind==Kind::Page{"pages"}else{"paper"}/><span class="admin-row-id">{format!("#{:04}",row.id)}</span></td>
        <td>{if kind==Kind::Page{view!{<Status value=status/>}.into_any()}else{view!{<strong>{lock_label}</strong><small>{if row.is_public_now.unwrap_or(row.public_access&&!row.access_blocked){"Veřejně dostupný"}else{"Neveřejný"}}</small>}.into_any()}}</td>
        <td class="admin-table-meta">{if kind==Kind::Page{view!{<span>{crate::catalog::page_path(&row.slug)}</span>}.into_any()}else{view!{<span>{board_label}</span>{dates.then(||view!{<small>{posted}</small><small>{removal}</small>})}}.into_any()}}</td>
        <td class="admin-row-open"><AdminLink href label="Otevřít"/></td>
    </tr>}
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
        "subscription_preferences_updated" => "Změna výběru odběru",
        "verification" | "overeni" => "Ověření odběru",
        "schedule_missed" => "Zmeškané plánované zveřejnění",
        "schedule_cancelled" => "Zrušení plánovaného zveřejnění",
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
        "issue_report" => "Podnět nebo závada",
        "report_settings" => "Příjemci podnětů a závad",
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
        Ok(rows)=>{let count=rows.len();view!{<div class="admin-table-wrap"><table class="admin-table"><caption class="sr-only">"Historie rozesílání"</caption><thead><tr><th>"Příjemce"</th><th>"Zpráva"</th><th>"Stav a čas v UTC"</th><th>"Přihlášení k odběru"</th></tr></thead><tbody>{rows.into_iter().take(20).map(|row|view!{<tr><td>{row.email}</td><td><strong>{row.subject}</strong><small>{match row.purpose.as_str(){"verification"=>"Ověření adresy","recovery"=>"Obnova hesla",_=>"Novinky"}}" · #"{row.id}</small></td><td>{match row.status.as_str(){"sent"=>"Přijato SMTP serverem","cancelled"=>"Zrušeno","expired"=>"Odkaz vypršel před odesláním","retrying"=>"Čeká na další pokus",_=>"Ve frontě"}}<small>{api::unix_date(row.sent_at.unwrap_or(row.created_at))}</small><small>{format!("Pokusů: {}",row.attempts)}</small></td><td>{row.subscriber_id.map(|id|view!{<a class="text-link" href=format!("/api/v1/admin/subscribers/{id}/export") download="doklad-odberu.json">"Stáhnout doklad"</a>})}</td></tr>}).collect_view()}</tbody></table></div><Pager offset count/>}.into_any()}
    })}</Suspense></section>}
}
