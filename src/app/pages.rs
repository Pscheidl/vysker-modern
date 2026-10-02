use super::{BoardResource, components::*};
use crate::content::Notice;
#[cfg(feature = "demo")]
use crate::content::fold;
use leptos::prelude::*;
use leptos_meta::Title;
#[cfg(feature = "demo")]
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::{components::A, hooks::use_params_map};

#[component]
pub fn Home() -> impl IntoView {
    let board = expect_context::<BoardResource>();
    let archive = RwSignal::new(false);
    view! {
        <Title text="Vyskeř · Obec v Českém ráji"/>
        <div class="page-width">
            <section class="hero" aria-labelledby="home-title">
                <div class="hero-copy"><h1 id="home-title">"Vyskeř"<span>"."</span></h1><p>"Obec v Českém ráji"</p></div>
                <div class="terrain" aria-hidden="true"><img class="terrain-light" src=super::site_url("/images/relief-vysker-light.png") alt="" width="2172" height="724" fetchpriority="high"/><img class="terrain-dark" src=super::site_url("/images/relief-vysker-dark.png") alt="" width="2172" height="724"/></div>
                <div class="hero-date"><Suspense fallback=|| ()>{move || board.get().and_then(Result::ok).map(|b| b.today)}</Suspense></div>
            </section>
            <SearchBox/><Shortcuts/>
        </div>
        <section class="home-board" aria-labelledby="home-board-title">
            <div class="page-width">
                <div class="section-heading"><div class="section-title"><h2 id="home-board-title">"Úřední deska"</h2><Suspense fallback=|| ()>{move || board.get().and_then(Result::ok).map(|b| view!{<span class="count-badge">{posted_count(if cfg!(feature="demo") {b.notices.iter().filter(|n| !n.archived).count()} else {b.published_count})}</span>})}</Suspense></div>
                    <div class="section-tools"><div class="segmented" role="group" aria-label="Zobrazit dokumenty">
                        <button class:active=move || !archive.get() aria-pressed=move || (!archive.get()).to_string() on:click=move |_| archive.set(false)>"Aktuální"</button>
                        <button class:active=move || archive.get() aria-pressed=move || archive.get().to_string() on:click=move |_| archive.set(true)>"Archiv"</button>
                    </div><A href=super::site_url("/uredni-deska") attr:class="text-link">"Všechny dokumenty"<Icon name="external"/></A></div>
                </div>
                <Suspense fallback=Loading>{move || board.get().map(|result| match result {
                    Ok(b) => {
                        let selected: Vec<_> = b.notices.into_iter().filter(|n| n.archived == archive.get()).take(3).collect();
                        if selected.is_empty() {view!{<EmptyState title="Na desce teď není žádný dokument"/>}.into_any()} else {
                            view!{<div class="notice-grid">{selected.into_iter().map(|notice| view!{<NoticeCard notice/>}).collect_view()}</div>}.into_any()
                        }
                    },
                    Err(e) => view!{<LoadError message=e.to_string()/>}.into_any()
                })}</Suspense>
            </div>
        </section>
        <div class="page-width"><Newsletter/><HomeEvents/></div>
    }
}

#[component]
pub fn NoticeBoard() -> impl IntoView {
    view! {<super::search::NoticeListing/>}
}
#[component]
pub fn Documents() -> impl IntoView {
    #[cfg(feature = "demo")]
    view! {<DemoDocumentListing library=true/>}
    #[cfg(not(feature = "demo"))]
    view! {<super::catalog::DocumentLibrary/>}
}

#[cfg(feature = "demo")]
#[component]
fn DemoDocumentListing(#[prop(default = false)] library: bool) -> impl IntoView {
    let board = expect_context::<BoardResource>();
    let params = use_query_map();
    let query = RwSignal::new(params.get_untracked().get("q").unwrap_or_default());
    let category = RwSignal::new(params.get_untracked().get("kategorie").unwrap_or_default());
    let sort = RwSignal::new(
        params
            .get_untracked()
            .get("razeni")
            .unwrap_or_else(|| "newest".into()),
    );
    let archived = Memo::new(move |_| params.get().get("stav").as_deref() == Some("archiv"));
    Effect::new(move |_| {
        query.set(params.get().get("q").unwrap_or_default());
        category.set(params.get().get("kategorie").unwrap_or_default());
        sort.set(
            params
                .get()
                .get("razeni")
                .unwrap_or_else(|| "newest".into()),
        );
    });
    let title = if library {
        "Dokumenty a formuláře"
    } else {
        "Úřední deska"
    };
    let description = if library {
        "Zveřejněné dokumenty obce, přehledně podle témat."
    } else {
        "Vše, co obec právě zveřejňuje. Přehledně a na jednom místě."
    };
    let action = if library {
        "/dokumenty"
    } else {
        "/uredni-deska"
    };
    let navigate = use_navigate();
    let clear_filters = Callback::new(move |()| {
        query.set(String::new());
        category.set(String::new());
        sort.set("newest".into());
        let destination = if !library && archived.get() {
            "/uredni-deska?stav=archiv"
        } else {
            action
        };
        navigate(destination, Default::default());
    });
    view! {
        <Title text=format!("{title} · Vyskeř")/>
        <div class="page-width interior">
            <PageHeading title description eyebrow="DOKUMENTY OBCE"/>
            <Show when=move || !library><nav class="page-tabs" aria-label="Stav dokumentů">
                <A href=super::site_url("/uredni-deska") attr:class=move || if archived.get(){""}else{"selected"}>"Aktuálně vyvěšeno"</A>
                <A href=super::site_url("/uredni-deska?stav=archiv") attr:class=move || if archived.get(){"selected"}else{""}>"Archiv dokumentů"</A>
            </nav></Show>
            <Show when=move || archived.get() && !library><p class="info-banner"><Icon name="archive"/>"V archivu zůstává záznam o každém sejmutém dokumentu, i když už jeho přílohy nejsou dostupné."</p></Show>
            <form class="filter-bar" action=super::site_url(action) method="get" role="search">
                <input type="hidden" name="stav" value=move || if archived.get(){"archiv"}else{"aktualni"}/>
                <div class="filter-search"><label class="sr-only" for="filter-query">"Hledat v dokumentech"</label><Icon name="search"/><input id="filter-query" name="q" type="search" placeholder="Název, téma nebo číslo jednací…" value=move || query.get() prop:value=move || query.get() on:input=move |ev| query.set(event_target_value(&ev))/></div>
                <div class="filter-control"><label for="category">"Kategorie"</label><select id="category" name="kategorie" prop:value=move || category.get() on:change=move |ev| category.set(event_target_value(&ev))><option value="">"Všechny kategorie"</option><option>"Veřejná vyhláška"</option><option>"Záměr obce"</option><option>"Zastupitelstvo"</option><option>"Rozpočet"</option><option>"Volby"</option><option>"Dražba"</option><option>"Ostatní"</option></select></div>
                <div class="filter-control"><label for="sort">"Řazení"</label><select id="sort" name="razeni" prop:value=move || sort.get() on:change=move |ev| sort.set(event_target_value(&ev))><option value="newest" selected=move || sort.get()=="newest">"Od nejnovějších"</option><option value="oldest" selected=move || sort.get()=="oldest">"Od nejstarších"</option><option value="title" selected=move || sort.get()=="title">"Podle názvu"</option></select></div>
                <button class="button secondary filter-submit" type="submit">"Hledat"</button>
            </form>
            <Suspense fallback=Loading>{move || board.get().map(|result| match result {
                Ok(b) => {
                    let mut selected:Vec<Notice> = b.notices.into_iter().filter(|n| if library {n.has_files()} else {n.archived==archived.get()}).filter(|n| n.matches(&query.get()) && (category.get().is_empty() || n.category==category.get())).collect();
                    match sort.get().as_str() {"oldest" => selected.sort_by(|a,b| a.posted_iso.cmp(&b.posted_iso)), "title" => selected.sort_by_key(|n| fold(&n.title)), _ => selected.sort_by(|a,b| b.posted_iso.cmp(&a.posted_iso))};
                    let count=selected.len();
                    view!{
                        <div class="results-meta"><p role="status">{format!("Počet dokumentů: {count}")}</p><button class="quiet-button" type="button" on:click=move |_| clear_filters.run(())>"Zrušit filtry"<Icon name="close"/></button></div>
                        {if count==0 {view!{<EmptyState/>}.into_any()} else {view!{<div class="document-list">{selected.into_iter().map(|notice| view!{<NoticeRow notice/>}).collect_view()}</div>}.into_any()}}
                    }.into_any()
                }, Err(e) => view!{<LoadError message=e.to_string()/>}.into_any()
            })}</Suspense>
            <Newsletter/>
        </div>
    }
}

#[component]
pub fn NoticeDetail() -> impl IntoView {
    let params = use_params_map();
    let document = Resource::new(
        move || {
            params
                .get()
                .get("id")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        },
        crate::content::load_notice,
    );
    view! {
        <div class="page-width interior detail-page">
            <A href=super::site_url("/uredni-deska") attr:class="back-link"><Icon name="back"/>"Zpět na úřední desku"</A>
            <Suspense fallback=Loading>{move ||document.get().map(|result|match result {
                Ok(Some(notice))=>view!{<DetailContent notice/>}.into_any(),
                Ok(None)=>view!{<NotFound/>}.into_any(),
                Err(e)=>view!{<LoadError message=e.to_string() on_retry=Callback::new(move|()|document.refetch())/>}.into_any(),
            })}</Suspense>
        </div>
    }
}

#[component]
fn DetailContent(notice: Notice) -> impl IntoView {
    let no_files = !notice.has_files();
    let archived = notice.archived;
    let title = notice.title.clone();
    view! {
        <Title text=format!("{} · Vyskeř",title)/>
        <header class="page-heading detail-heading"><div class="detail-eyebrow"><p class="eyebrow">{notice.category}</p><span class=if archived {"status-badge"}else{"status-badge success"}>{if archived {"Archivováno"}else{"Vyvěšeno"}}</span></div><h1>{notice.title}</h1></header>
        {notice.import_origin.map(|_| view!{<ImportNote/>})}
        <div class="detail-layout"><article class="detail-content"><h2>"Informace o dokumentu"</h2><p class="document-description">{notice.description}</p><dl class="document-facts"><div><dt>"Číslo jednací"</dt><dd>{notice.reference}</dd></div><div><dt>"Zveřejnil"</dt><dd>{notice.issuer}</dd></div></dl>
            <section class="attachments" aria-labelledby="attachments-title"><h2 id="attachments-title">"Přílohy dokumentu"</h2>
                <Show when=move || archived && no_files><div class="info-banner"><Icon name="archive"/><div><strong>"Přílohy už nejsou k dispozici."</strong><p>"Po sejmutí byly odstraněny. Záznam o zveřejnění zůstává v archivu."</p></div></div></Show>
                {notice.attachments.into_iter().map(|file|view!{<div class="attachment"><Icon name="paper"/><div><strong>{file.name}</strong><p>{if file.removed {"Příloha odstraněna".to_string()}else{file.size.clone()}}</p></div>{match file.url {Some(url)=>view!{<a class="text-link" href=url download>"Stáhnout"<Icon/></a>}.into_any(),None=>view!{<span class="attachment-state">{if file.removed {"Nedostupná"}else{"Není k dispozici"}}</span>}.into_any()}}</div>}).collect_view()}
                <Show when=move || !no_files && cfg!(feature="demo")><p class="field-note">"U příloh jsou zatím uvedeny pouze názvy. Soubory ke stažení nejsou k dispozici."</p></Show>
            </section>
        </article><aside class="posting-panel" aria-label="Doba zveřejnění"><Icon name="calendar"/><h2>"Doba zveřejnění"</h2><dl><div><dt>"Vyvěšeno"</dt><dd><time datetime=(!notice.posted_iso.is_empty()).then_some(notice.posted_iso.clone())>{notice.posted}</time></dd></div><div><dt>{if archived {"Sejmuto"}else{"Datum sejmutí"}}</dt><dd>{notice.ends}</dd></div></dl>
            {if !archived {notice.remaining.map(|d|view!{<span class="status-badge">{remaining_text(d)}</span>})}else{None}}
            <p>{if notice.retain_files {"Přílohy se po sejmutí ponechávají v archivu."}else{"Po sejmutí zůstane v archivu záznam bez příloh."}}</p>
        </aside></div>
    }
}

#[component]
fn MunicipalityPreview() -> impl IntoView {
    view! {
        <Title text="Obec a úřad · Vyskeř"/>
        <div class="page-width interior"><PageHeading title="Doma na Vyskři." description="Obec v Českém ráji. Informace pro každodenní život i návštěvu úřadu."/>
            <div class="municipality-intro"><div><h2>"Obec, ve které žijeme"</h2><p>"Na Vyskři se potkává krajina Českého ráje s každodenním životem obce. Tady najdete důležité dokumenty, praktické informace i to, co se u nás právě chystá."</p><A href=super::site_url("/kontakt") attr:class="button primary">"Kontakty na obecní úřad"<Icon/></A></div><div class="mini-terrain" aria-hidden="true"><img class="terrain-light" src=super::site_url("/images/relief-vysker-light.png") alt="" loading="lazy"/><img class="terrain-dark" src=super::site_url("/images/relief-vysker-dark.png") alt="" loading="lazy"/></div></div>
            <section id="sluzby" class="services-section"><h2>"Co potřebujete vyřídit?"</h2><div class="service-grid">
                <article><Icon name="bin"/><h3>"Odpady a svoz"</h3><p>"Informace ke třídění odpadu a nejbližším svozům."</p><A href=super::site_url("/kalendar#svoz") attr:class="text-link">"Termíny v kalendáři"<Icon/></A></article>
                <article><Icon name="paper"/><h3>"Místní poplatky"</h3><p>"Podklady k platbám a informace získáte na obecním úřadě."</p><A href=super::site_url("/kontakt") attr:class="text-link">"Spojit se s úřadem"<Icon/></A></article>
                <article><Icon name="board"/><h3>"Zastupitelstvo"</h3><p>"Pozvánky na veřejná zasedání a zveřejněné dokumenty."</p><A href=super::site_url("/uredni-deska?kategorie=Zastupitelstvo") attr:class="text-link">"Dokumenty zastupitelstva"<Icon/></A></article>
            </div></section><Newsletter/>
        </div>
    }
}

#[component]
fn ContactPreview() -> impl IntoView {
    view! {
        <Title text="Kontakt a úřední hodiny · Vyskeř"/>
        <div class="page-width interior"><PageHeading title="Jsme tu pro vás." description="Zavolejte, napište nebo se zastavte na obecním úřadě." eyebrow="KONTAKT"/>
            <div class="contact-grid"><section class="contact-panel"><h2>"Obecní úřad Vyskeř"</h2><address><p><Icon name="pin"/><span>"Vyskeř 50"<br/>"512 64 Vyskeř"</span></p><p><Icon name="phone"/><a href="tel:+420481320211">"+420 481 320 211"</a></p><p><Icon name="mail"/><a href="mailto:vysker@cmail.cz">"vysker@cmail.cz"</a></p></address><p class="field-note">"Před osobní návštěvou mimo úřední hodiny se prosím domluvte telefonicky."</p></section>
            <section id="uredni-hodiny" class="hours-panel"><Icon name="clock"/><h2>"Úřední hodiny"</h2><dl><div><dt>"Pondělí"</dt><dd>"16.00–18.00"</dd></div><div><dt>"Středa"</dt><dd>"16.00–18.00"</dd></div></dl><p>"Těšíme se na vaši návštěvu."</p></section></div>
            <section class="contact-followup"><h2>"Hledáte konkrétní dokument?"</h2><p>"Zkuste nejprve vyhledávání. Dokumenty najdete i podle čísla jednacího."</p><SearchBox/></section>
        </div>
    }
}

#[component]
pub fn Subscribe() -> impl IntoView {
    view! {<Title text="Odběr novinek · Vyskeř"/><div class="page-width interior"><PageHeading title="Důležité zprávy vám neutečou." description="Nové dokumenty a oznámení z úřední desky přímo do vaší schránky." eyebrow="ODBĚR NOVINEK"/><Newsletter/><div class="steps-grid"><article><span>"01"</span><h2>"Zadáte e-mail"</h2><p>"Stačí adresa, na kterou chcete dostávat nové dokumenty."</p></article><article><span>"02"</span><h2>"Potvrdíte odběr"</h2><p>"Ve zprávě otevřete ověřovací odkaz a potvrďte přihlášení."</p></article><article><span>"03"</span><h2>"Budete v obraze"</h2><p>{if cfg!(feature="demo"){"Rozesílání novinek zde není aktivní."}else{"Z každé zprávy se snadno odhlásíte."}}</p></article></div><p class="field-note">"Ochrana soukromí: "<A href=super::site_url("/ochrana-udaju")>"jak nakládáme s údaji"</A>"."</p></div>}
}

#[cfg(feature = "demo")]
#[component]
fn CalendarPreview() -> impl IntoView {
    view! {<Title text="Kalendář akcí · Vyskeř"/><div class="page-width interior"><PageHeading title="Dění na Vyskři." description="Setkání, události a praktické termíny na jednom místě." eyebrow="KALENDÁŘ AKCÍ"/>
    <div class="calendar-list"><details id="setkani"><summary><span class="date-tile"><strong>"10"</strong><span>"ŘÍJEN"</span></span><span><span class="eyebrow">"SPOLEČNĚ V OBCI"</span><strong>"Podzimní setkání sousedů"</strong><span>"Sobota 10. října 2026"</span></span><Icon/></summary><div class="event-description"><p>"Příležitost potkat se se sousedy a strávit společně podzimní odpoledne. Podrobnosti o místě a programu zveřejní obec před akcí."</p><A href=super::site_url("/kontakt") attr:class="text-link">"Kontakt na pořadatele"<Icon/></A></div></details>
    <details id="svoz"><summary><span class="date-tile"><strong>"12"</strong><span>"ŘÍJEN"</span></span><span><span class="eyebrow">"PRAKTICKÉ INFORMACE"</span><strong>"Svoz bioodpadu"</strong><span>"Pondělí 12. října 2026"</span></span><Icon/></summary><div class="event-description"><p>"Informace ke svozu bioodpadu a přistavení nádob poskytne obecní úřad."</p><A href=super::site_url("/kontakt") attr:class="text-link">"Zeptat se na svoz"<Icon/></A></div></details></div><p class="field-note">"Události a termíny jsou pouze ilustrační. Nejde o potvrzené akce obce."</p>
    </div>}
}

#[component]
fn AccessibilityPreview() -> impl IntoView {
    view! {<Title text="Přístupnost · Vyskeř"/><div class="page-width interior prose"><PageHeading title="Web pro každého." description="Přístupnost a ovládání webu." eyebrow="PŘÍSTUPNOST"/><h2>"Ovládání klávesnicí"</h2><p>"Mezi odkazy a ovládacími prvky se pohybujete klávesou Tab. První odkaz umožňuje přeskočit rovnou na obsah stránky. Vyhledávání otevřete pomocí Ctrl K nebo ⌘ K na stránkách s vyhledávacím polem."</p><h2>"Čitelnost a motiv"</h2><p>"Světlý a tmavý motiv přepnete v záhlaví. Web respektuje nastavení omezeného pohybu a obsah se přizpůsobuje velikosti obrazovky."</p><h2>"Stav přístupnosti"</h2><p>"Úplné prohlášení o přístupnosti bude doplněno po ověření přístupnosti webu a jeho dokumentů."</p><h2>"Narazili jste na problém?"</h2><p>"Dejte nám vědět na "<a href="mailto:vysker@cmail.cz">"vysker@cmail.cz"</a>"."</p></div>}
}

#[component]
pub fn Privacy() -> impl IntoView {
    let privacy = Resource::new(|| (), |_| crate::catalog::load_privacy_notice());
    view! {<Title text="Ochrana údajů · Vyskeř"/><div class="page-width interior prose">
        <PageHeading title="Vaše soukromí." description="Účely zpracování, doba uchování a vaše práva." eyebrow="OCHRANA ÚDAJŮ"/>
        <Suspense fallback=||view!{<p role="status">"Načítám informace…"</p>}>
        {move ||privacy.get().map(|result|match result{
            Ok(Some(notice))=>view!{<PrivacyDetails policy=notice.policy/>}.into_any(),
            Ok(None)=>view!{<p>"Informace o zpracování osobních údajů nejsou dostupné. Odběr novinek není aktivní."</p>}.into_any(),
            Err(_)=>view!{<p role="alert">"Informace se nepodařilo načíst. Zkuste to později."</p>}.into_any(),
        })}</Suspense>
        <h2>"Cookies a paměť prohlížeče"</h2><p>"Používáme pouze nezbytné technické prostředky. Přihlášení správců používá cookie obec_session s platností nejvýše 8 hodin. Cookie chrání přístup do administrace a při odhlášení se odstraní."</p>
        <p>"Volbu světlého či tmavého motivu ukládáme pod klíčem vysker-theme v localStorage do její změny nebo vymazání dat webu v prohlížeči."</p>
        <p>"Web nepoužívá analytické ani reklamní cookies, profilování ani automatizované rozhodování. Písma a obrázky načítáme přímo z tohoto webu."</p>
    </div>}
}

#[component]
fn PrivacyDetails(policy: crate::privacy::PrivacyPolicy) -> impl IntoView {
    let r = policy.retention;
    view! {
        <p>"Verze informací: "{policy.version}</p>
        <h2>"Správce a pověřenec"</h2><p>{policy.controller_name}", "{policy.controller_address}</p>
        <p>"Kontakt správce: "<a href=format!("mailto:{}",policy.controller_email)>{policy.controller_email.clone()}</a></p>
        <p>"Pověřenec pro ochranu osobních údajů: "<a href=format!("mailto:{}",policy.dpo_email)>{policy.dpo_email.clone()}</a></p>
        <h2>"Odběr novinek"</h2><p>"Na základě dobrovolného souhlasu podle čl. 6 odst. 1 písm. a) GDPR používáme e-mailovou adresu k zasílání nových dokumentů včetně úřední desky. Evidujeme přesné znění souhlasu a čas žádosti, potvrzení a případného odvolání. Bez poskytnutí adresy nelze odběr zajistit, používání ostatních částí webu tím není omezeno."</p>
        <p>"Souhlas kdykoli odvoláte odkazem v každé novince bez přihlášení. Odvolání nemá vliv na zákonnost předchozího zpracování."</p>
        <p>{format!("Nepotvrzené žádosti uchováváme nejvýše {} dní. Aktivní odběr trvá do odvolání souhlasu. Po odhlášení se adresa a doklad souhlasu odstraní nejpozději za {} dní. Záznamy poštovní fronty uchováváme nejvýše {} dní od vytvoření.",r.pending_days,r.withdrawn_days,r.mail_days)}</p>
        <p>"Důvod uchování dokladu o souhlasu: "{policy.consent_evidence_legal_basis}</p>
        <h2>"Zveřejněné dokumenty"</h2><p>{policy.public_records_legal_basis}</p><p>{format!("Rozsah údajů a dobu zveřejnění určuje typ dokumentu. Interní texty a doklady vyvěšení uchováváme {} dní po sejmutí. Potom zůstane jen minimální evidenční záznam. Úřední originály a spisovou službu spravuje obec odděleně.",r.notice_internal_days)}</p>
        <h2>"Provoz a bezpečnost"</h2><p>{policy.security_legal_basis}</p>
        <p>{format!("Audit obsahuje identifikátor správce, provedenou operaci, dotčený záznam a čas. Uchovává se {} dní. Provozní logy mají lhůtu {} dní. Ochrana proti zneužití uchovává otisky technických identifikátorů nejvýše 24 hodin. Aplikace do HTTP logů nezapisuje těla požadavků, e-mailové adresy ani parametry s ověřovacími tokeny.",r.audit_days,r.operational_log_days)}</p>
        <h2>"Příjemci a předávání údajů"</h2><p>{policy.processors}</p><p>{policy.international_transfers}</p>
        <p>{format!("Záložní kopie mohou údaje obsahovat nejvýše {} dní. Po obnově se znovu uplatní lhůty uchování. Vyřizování žádostí o výmaz a odvolání souhlasu zahrnuje také postup obnovy ze záloh.",r.backup_days)}</p>
        <h2>"Vaše práva"</h2><p>"U správce nebo pověřence můžete požádat o přístup k údajům, jejich opravu, výmaz či omezení zpracování. Podle právního důvodu zpracování máte také právo na přenositelnost nebo vznést námitku. Souhlas s odběrem lze odvolat kdykoli."</p>
        <p>"Stížnost můžete podat u "<a href="https://uoou.gov.cz/">"Úřadu pro ochranu osobních údajů"</a>"."</p>
    }
}

#[component]
pub fn NotFound() -> impl IntoView {
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(axum::http::StatusCode::NOT_FOUND);
    }
    view! {<Title text="Stránka nenalezena · Vyskeř"/><section class="page-width not-found"><span class="eyebrow">"CHYBA 404"</span><h1>"Tudy cesta nevede."</h1><p>"Stránka nebo dokument, který hledáte, tu není. Zkuste vyhledávání nebo se vraťte na úvod."</p><div><A href=super::site_url("/") attr:class="button primary"><Icon name="back"/>"Zpět na přehled"</A><A href=super::site_url("/hledat") attr:class="button secondary">"Vyhledat na webu"</A></div></section>}
}

#[component]
pub fn Contact() -> impl IntoView {
    view! {<ManagedPage slug="kontakt"><ContactPreview/></ManagedPage>}
}

#[component]
pub fn Calendar() -> impl IntoView {
    #[cfg(feature = "demo")]
    return view! {<ManagedPage slug="kalendar"><CalendarPreview/></ManagedPage>};
    #[cfg(not(feature = "demo"))]
    view! {<super::events::Calendar/>}
}

#[component]
pub fn Accessibility() -> impl IntoView {
    view! {<ManagedPage slug="pristupnost"><AccessibilityPreview/></ManagedPage>}
}

#[component]
pub fn Municipality() -> impl IntoView {
    view! {<ManagedPage slug="obec"><MunicipalityPreview/></ManagedPage>}
}

#[component]
pub fn RequiredInformation() -> impl IntoView {
    view! {<ManagedPage slug="povinne-informace"><div class="page-width interior prose"><PageHeading title="Povinné informace" description="Informace podle zákona č. 106/1999 Sb."/><p>"Povinné informace zde zatím nejsou zveřejněny."</p></div></ManagedPage>}
}
#[component]
fn ManagedPage(slug: &'static str, children: ChildrenFn) -> impl IntoView {
    let info = expect_context::<super::SiteResource>();
    let page = Resource::new(move || slug, |slug| crate::catalog::load_page(slug.into()));
    view! {<Suspense fallback=Loading>{move||{
        match (info.get(),page.get()) {
            (_,Some(Ok(Some(page))))=>view!{<Title text=format!("{} · Vyskeř",page.title)/><div class="page-width interior prose"><h1>{page.title}</h1><div class="markdown-content" inner_html=crate::markdown::render(&page.content)></div></div>}.into_any(),
            (Some(Ok(i)),Some(Ok(None))) if !i.production=>children().into_any(),
            (Some(_),Some(_))=>view!{<div class="page-width interior prose"><h1>"Informace nejsou dostupné"</h1><p>"Stránku se nepodařilo načíst. Zkuste to později."</p></div>}.into_any(),
            _=>view!{<Loading/>}.into_any()
        }
    }}</Suspense>}
}
#[component]
pub fn PagesIndex() -> impl IntoView {
    let info = expect_context::<super::SiteResource>();
    view! {<div class="page-width interior prose"><PageHeading title="Informace obce" description="Přehled zveřejněných obsahových stránek."/><Suspense fallback=Loading>{move||info.get().map(|r|match r {Ok(info)=>view!{<ul>{info.pages.into_iter().map(|p|view!{<li><A href=super::site_url(&format!("/stranky/{}",p.slug))>{p.title}</A></li>}).collect_view()}</ul>}.into_any(),Err(_)=>view!{<p>"Stránky se nepodařilo načíst."</p>}.into_any()})}</Suspense></div>}
}

#[component]
pub fn SearchResults() -> impl IntoView {
    view! {<super::search::SearchListing/>}
}

#[component]
fn HomeEvents() -> impl IntoView {
    #[cfg(feature = "demo")]
    return view! {<PreviewOnly><section class="events-strip" aria-label="Ilustrační události"><h2>"Ilustrační události"</h2><A href=super::site_url("/kalendar#setkani")><span>"10. října"</span>"Podzimní setkání sousedů"</A><A href=super::site_url("/kalendar#svoz")><span>"12. října"</span>"Svoz bioodpadu"</A><A href=super::site_url("/kalendar") attr:class="text-link">"Kalendář akcí"<Icon name="external"/></A></section></PreviewOnly>};
    #[cfg(not(feature = "demo"))]
    view! {<super::events::UpcomingEvents/>}
}
