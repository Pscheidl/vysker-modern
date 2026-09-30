use super::site_url;
use crate::content::Notice;
use leptos::prelude::*;
use leptos_router::{
    components::A,
    hooks::{use_location, use_query_map},
};

#[component]
pub fn Icon(
    #[prop(default = "arrow")] name: &'static str,
    #[prop(default = "")] class: &'static str,
) -> impl IntoView {
    let path = match name {
        "grid" => "M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z",
        "pages" => "M8 3h13v15H8zM3 7v14h14M12 7h5M12 11h5",
        "history" => "M3 11a9 9 0 1 1 2 7M3 4v7h7M12 7v5l3 2",
        "plus" => "M12 4v16M4 12h16",
        "lock" => "M5 10h14v11H5zM8 10V6a4 4 0 0 1 8 0v4M12 14v3",
        "logout" => "M9 4H3v16h6M9 12h12m-5-5 5 5-5 5",
        "refresh" => "M3 11a9 9 0 0 1 15-7l3 3M21 2v5h-5M21 13a9 9 0 0 1-15 7l-3-3M3 22v-5h5",
        "upload" => "M12 16V3m-5 5 5-5 5 5M3 16v5h18v-5",
        "search" => "M21 21l-4.4-4.4M19 10.5a8.5 8.5 0 1 1-17 0 8.5 8.5 0 0 1 17 0",
        "paper" => {
            "M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8l-6-6zM14 2v6h6M8 13h8M8 17h5"
        }
        "board" => {
            "M8 3H5a2 2 0 0 0-2 2v15a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V5a2 2 0 0 0-2-2h-3M8 1h8v5H8zM8 11h8M8 16h5"
        }
        "clock" => "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0M12 6v6l4 2",
        "bin" => "M3 6h18M5 6l1 15h12l1-15M9 6V2h6v4M10 10v7M14 10v7",
        "mail" => {
            "M3 4h18a1 1 0 0 1 1 1v14a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V5a1 1 0 0 1 1-1zM2 5l10 8L22 5"
        }
        "sun" => {
            "M16 12a4 4 0 1 1-8 0 4 4 0 0 1 8 0M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5"
        }
        "moon" => "M20.9 13.2A9 9 0 0 1 10.8 3.1 9 9 0 1 0 20.9 13.2z",
        "menu" => "M3 6h18M3 12h18M3 18h18",
        "close" => "m6 6 12 12M18 6 6 18",
        "external" => "M7 17 17 7M7 7h10v10",
        "back" => "M20 12H4m6-6-6 6 6 6",
        "check" => "m5 12 4 4L19 6",
        "pin" => "M20 10c0 6-8 12-8 12S4 16 4 10a8 8 0 1 1 16 0M15 10a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
        "phone" => "M8 3 5 2 2 5c0 9 8 17 17 17l3-3-1-3-5-2-2 2c-3-1-5-3-6-6l2-2-2-5z",
        "calendar" => "M4 4h16v17H4zM8 2v4M16 2v4M4 9h16M8 13h2M14 13h2M8 17h2",
        "archive" => "M3 3h18v5H3zM5 8v13h14V8M9 12h6",
        "info" => "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0M12 11v6M12 7h.01",
        _ => "M4 12h16m-6-6 6 6-6 6",
    };
    view! { <svg class=format!("icon {class}") viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d=path/></svg> }
}

#[component]
pub fn Logo() -> impl IntoView {
    view! {
        <svg class="logo-mark" viewBox="0 0 64 48" fill="none" aria-hidden="true">
            <g stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M30 2v5m-2-3h4M28 12v5m4-5v5M23 23l5-6h12l8 6H23Zm2 0v9m21-9v9M28 32v-5h4v5m7-6v2"/>
                <path d="M3 39c10 0 14-7 22-7h20c7 0 9 7 16 7M3 46c12 0 20-6 30-6s18 6 28 6"/>
            </g>
            <path d="M30 6c0 1.6-2.8 2.3-2.8 4.3a2.8 2.8 0 0 0 5.6 0C32.8 8.3 30 7.6 30 6Z" fill="currentColor"/>
        </svg>
    }
}

#[component]
pub fn Header() -> impl IntoView {
    let location = use_location();
    let menu = RwSignal::new(false);
    Effect::new(move |_| {
        let _ = location.pathname.get();
        menu.set(false);
    });
    view! {
        <header class="site-header">
            <div class="page-width header-inner">
                <A href=site_url("/") attr:class="brand" attr:aria-label="Vyskeř, úvodní stránka"><Logo/><span>"Vyskeř"</span></A>
                <nav class="desktop-nav" aria-label="Hlavní navigace"><Navigation/></nav>
                <div class="header-actions"><ThemeSwitch/>
                    <A href=site_url("/kontakt") attr:class="contact-link">"Kontakt"<Icon name="external"/></A>
                    <button class="menu-toggle icon-button" type="button" aria-label="Hlavní menu" aria-expanded=move || menu.get().to_string() aria-controls="mobile-menu" on:click=move |_| menu.update(|m| *m=!*m)><Icon name="menu"/></button>
                </div>
            </div>
            <nav id="mobile-menu" class="mobile-nav page-width" aria-label="Mobilní navigace" hidden=move || !menu.get()><Navigation/><A href=site_url("/kontakt")>"Kontakt"</A></nav>
            <noscript><nav class="noscript-nav page-width" aria-label="Navigace"><a href=site_url("/uredni-deska")>"Úřední deska"</a><a href=site_url("/dokumenty")>"Dokumenty"</a><a href=site_url("/obec")>"Obec a úřad"</a><a href=site_url("/kontakt")>"Kontakt"</a></nav></noscript>
        </header>
    }
}

#[component]
fn Navigation() -> impl IntoView {
    let info = expect_context::<super::SiteResource>();
    let links = move || {
        info.get()
            .and_then(Result::ok)
            .map(|i| i.navigation)
            .unwrap_or_else(crate::catalog::default_navigation)
    };
    view! {<Suspense fallback=move||view!{<A href=site_url("/uredni-deska")>"Úřední deska"</A>}>
        {move||links().into_iter().map(|item|view!{<A href=site_url(&item.path) exact=item.path=="/">{item.label}</A>}).collect_view()}
    </Suspense>}
}

#[component]
pub(super) fn ThemeSwitch() -> impl IntoView {
    let dark = RwSignal::new(false);
    Effect::new(move |_| {
        if let Some(root) = document().document_element() {
            dark.set(root.get_attribute("data-theme").as_deref() == Some("dark"));
        }
    });
    let set_theme = move |is_dark: bool| {
        let value = if is_dark { "dark" } else { "light" };
        dark.set(is_dark);
        if let Some(root) = document().document_element() {
            let _ = root.set_attribute("data-theme", value);
        }
        if let Ok(Some(storage)) = window().local_storage() {
            let _ = storage.set_item("vysker-theme", value);
        }
    };
    view! {
        <div class="theme-switch" role="group" aria-label="Barevný motiv">
            <button type="button" class="theme-light" aria-label="Světlý motiv" aria-pressed=move || (!dark.get()).to_string() on:click=move |_| set_theme(false)><Icon name="sun"/></button>
            <button type="button" class="theme-dark" aria-label="Tmavý motiv" aria-pressed=move || dark.get().to_string() on:click=move |_| set_theme(true)><Icon name="moon"/></button>
        </div>
    }
}

#[component]
pub fn SearchBox() -> impl IntoView {
    let params = use_query_map();
    let query = RwSignal::new(params.get_untracked().get("q").unwrap_or_default());
    let input = NodeRef::<leptos::html::Input>::new();
    Effect::new(move |_| query.set(params.get().get("q").unwrap_or_default()));
    #[cfg(any(feature = "hydrate", feature = "demo"))]
    {
        let handle = window_event_listener(leptos::ev::keydown, move |event| {
            if (event.ctrl_key() || event.meta_key()) && event.key().eq_ignore_ascii_case("k") {
                event.prevent_default();
                if let Some(field) = input.get() {
                    let _ = field.focus();
                }
            }
        });
        on_cleanup(move || handle.remove());
    }
    view! {
        <form class="global-search" role="search" action=site_url("/hledat") method="get">
            <Icon name="search"/>
            <label for="site-search" class="sr-only">"Hledat na webu"</label>
            <input node_ref=input id="site-search" name="q" type="search" autocomplete="off" placeholder="Co hledáte?" value=move || query.get() prop:value=move || query.get() on:input=move |ev| query.set(event_target_value(&ev))/>
            <button class="search-submit" type="submit" aria-label="Vyhledat"><span class="search-label">"Hledat"</span><kbd>"Ctrl K"</kbd><Icon name="arrow"/></button>
        </form>
    }
}

#[component]
pub fn Shortcuts() -> impl IntoView {
    view! {
        <nav class="shortcuts" aria-label="Nejčastější služby">
            <A href=site_url("/uredni-deska")><Icon name="board"/><span>"Úřední deska"</span><Icon class="shortcut-arrow"/></A>
            <A href=site_url("/dokumenty")><Icon name="paper"/><span>"Dokumenty a formuláře"</span><Icon class="shortcut-arrow"/></A>
            <A href=site_url("/kontakt#uredni-hodiny")><Icon name="clock"/><span>"Úřední hodiny"</span><Icon class="shortcut-arrow"/></A>
            <A href=site_url("/obec#sluzby")><Icon name="bin"/><span>"Odpady a poplatky"</span><Icon class="shortcut-arrow"/></A>
        </nav>
    }
}

#[component]
pub fn NoticeCard(notice: Notice) -> impl IntoView {
    let href = notice.href();
    view! {
        <article class="notice-card">
            <div class="notice-meta"><span>{notice.category}</span><time datetime=notice.posted_iso>{notice.posted}</time></div>
            <h3><A href=site_url(&href)>{notice.title}</A></h3>
            <div class="notice-card-bottom"><span>{if notice.archived { "Sejmuto " } else { "Sejmutí " }}{notice.ends}</span><Icon/></div>
            {if !notice.archived { notice.remaining.filter(|d| *d <= 6).map(|days| view! {<span class="status-badge warning">{remaining_text(days)}</span>}) } else {None}}
        </article>
    }
}

pub fn remaining_text(days: i64) -> String {
    match days {
        d if d < 0 => "Lhůta uplynula".into(),
        0 => "Končí dnes".into(),
        1 => "Končí zítra".into(),
        2..=4 => format!("Končí za {days} dny"),
        _ => format!("Končí za {days} dní"),
    }
}

#[component]
pub fn NoticeRow(notice: Notice) -> impl IntoView {
    let href = notice.href();
    let archived_without_files = notice.archived && !notice.has_files();
    view! {
        <article class="document-row">
            <div class="file-icon"><Icon name=if notice.archived {"archive"} else {"paper"}/></div>
            <div class="document-copy"><div class="notice-meta"><span>{notice.category}</span><span>{notice.reference}</span></div>
                <h2><A href=site_url(&href)>{notice.title.clone()}</A></h2>
                <p>"Vyvěšeno "<time datetime=notice.posted_iso>{notice.posted}</time>
                {if archived_without_files {view! {<span class="removed-label">"Záznam bez příloh"</span>}.into_any()} else {().into_any()}}</p>
            </div>
            <div class="document-date"><span>{if notice.archived {"Sejmuto"} else {"Sejmutí"}}</span><strong>{notice.ends}</strong>
            {if !notice.archived {notice.remaining.filter(|d| *d<=6).map(|d| view!{<span class="status-badge warning">{remaining_text(d)}</span>})} else {None}}</div>
            <Icon class="row-arrow"/>
        </article>
    }
}

#[component]
pub fn Newsletter() -> impl IntoView {
    let privacy = Resource::new(|| (), |_| crate::catalog::load_privacy_notice());
    view! {<section class="newsletter" aria-labelledby="newsletter-title">
        <Icon name="mail" class="newsletter-icon"/>
        <div class="newsletter-copy"><h2 id="newsletter-title">"Nové dokumenty do e-mailu"</h2><p>"Odběr je dobrovolný. Aktivujete jej potvrzením e-mailové adresy."</p></div>
        <Suspense fallback=||view!{<p role="status">"Načítám informace o odběru…"</p>}>
            {move || privacy.get().map(|result|match result {
                Ok(Some(notice))=>view!{<NewsletterForm notice/>}.into_any(),
                Ok(None)=>view!{<p class="newsletter-message">{if cfg!(feature="demo"){"Toto je ukázkový náhled. Adresy neukládáme a zprávy neposíláme."}else{"Odběr novinek zatím není aktivní."}}</p>}.into_any(),
                Err(_)=>view!{<p class="newsletter-message" role="alert">"Informace o odběru se nepodařilo načíst. Zkuste to později."</p>}.into_any(),
            })}
        </Suspense>
    </section>}
}

#[component]
fn NewsletterForm(notice: crate::privacy::PrivacyNotice) -> impl IntoView {
    let email = RwSignal::new(String::new());
    let fingerprint = StoredValue::new(notice.fingerprint);
    let request = Action::new(|input: &(String, String)| {
        let (email, fingerprint) = input.clone();
        async move { crate::catalog::subscribe_email(email, fingerprint).await }
    });
    let ready = RwSignal::new(false);
    Effect::new(move |_| ready.set(true));
    view! {<form action=site_url("/odber") class="newsletter-form" on:submit=move |ev| {
        ev.prevent_default(); request.dispatch((email.get(),fingerprint.get_value()));
    }>
        <div class="email-field"><label for="newsletter-email">"E-mailová adresa"</label><input id="newsletter-email" type="email" autocomplete="email" placeholder="vas@email.cz" required maxlength="254" on:input=move |ev|{email.set(event_target_value(&ev));request.value().set(None);}/></div>
        <button class="button primary" type="submit" disabled=move ||!ready.get() ||request.pending().get()>{move ||if request.pending().get(){"Odesílám…"}else{"Přihlásit k odběru"}}</button>
        <p class="newsletter-consent">{notice.consent_text}" "<A href=site_url("/ochrana-udaju")>"Ochrana údajů"</A></p>
        {move ||request.value().get().map(|result|view!{<p role="status" class="newsletter-message">{match result{Ok(())=>"Pokud je potřeba odběr potvrdit, pošleme vám ověřovací e-mail. Zkontrolujte svou schránku.".to_owned(),Err(error)=>error.to_string()}}</p>})}
        {notice.policy.approved_on.is_none().then(||view!{<p class="field-note">"Vývojová ukázka. Podmínky odběru dosud nebyly schváleny obcí."</p>})}
        <noscript><p>"Pro odeslání formuláře zapněte JavaScript."</p></noscript>
    </form>}
}

#[component]
pub fn Footer() -> impl IntoView {
    view! {
        <footer class="site-footer page-width">
            <span>"Obec Vyskeř"</span>
            <nav aria-label="Patička"><A href=site_url("/povinne-informace")>"Povinné informace"</A><A href=site_url("/stranky")>"Informace obce"</A><A href=site_url("/pristupnost")>"Přístupnost"</A><A href=site_url("/ochrana-udaju")>"Ochrana údajů"</A><A href=site_url("/kontakt")>"Kontakt"</A>{(!cfg!(feature="demo")).then(||view!{<A href="/admin">"Správa webu"</A>})}</nav>
            <PreviewOnly><span class="preview-label"><span class="preview-dot"></span>"Vývojový náhled · ukázkový obsah"</span></PreviewOnly>
        </footer>
    }
}

#[component]
pub fn PageHeading(
    title: &'static str,
    description: &'static str,
    #[prop(default = "OBEC VYSKEŘ")] eyebrow: &'static str,
) -> impl IntoView {
    view! {<header class="page-heading"><p class="eyebrow">{eyebrow}</p><h1>{title}</h1><p class="page-description">{description}</p></header>}
}

#[component]
pub fn Loading() -> impl IntoView {
    view! {<div class="loading-state" role="status"><span class="loading-dot"></span>"Načítám dokumenty…"</div>}
}

#[component]
pub fn LoadError(
    message: String,
    #[prop(optional)] on_retry: Option<Callback<()>>,
) -> impl IntoView {
    let board = expect_context::<super::BoardResource>();
    view! {<div class="empty-state" role="alert"><Icon name="info"/><h2>"Dokumenty nejsou právě dostupné"</h2><p>{message}</p><button class="button secondary" type="button" on:click=move |_|if let Some(retry)=on_retry{retry.run(())}else{board.refetch()}>"Zkusit znovu"</button></div>}
}

#[component]
pub fn EmptyState(
    #[prop(default = "Žádné dokumenty k zobrazení")] title: &'static str,
) -> impl IntoView {
    view! {<div class="empty-state"><Icon name="search"/><h2>{title}</h2><p>"Zkuste jiný výraz nebo upravte výběr kategorie."</p></div>}
}

pub fn posted_count(count: usize) -> String {
    match count {
        1 => "1 vyvěšený".into(),
        2..=4 => format!("{count} vyvěšené"),
        _ => format!("{count} vyvěšených"),
    }
}

#[component]
pub fn SkipLink() -> impl IntoView {
    let location = use_location();
    let target = move || {
        let search = location.search.get();
        let query = if search.is_empty() {
            String::new()
        } else {
            format!("?{search}")
        };
        format!("{}{query}#obsah", location.pathname.get())
    };
    view! {<a class="skip-link" href=target on:click=move |ev| {
        use leptos::wasm_bindgen::JsCast;
        if let Some(main)=document().get_element_by_id("obsah").and_then(|el|el.dyn_into::<web_sys::HtmlElement>().ok()) {
            ev.prevent_default();
            let _=main.focus();
        }
    }>"Přeskočit na obsah"</a>}
}

#[component]
pub fn PreviewOnly(children: ChildrenFn) -> impl IntoView {
    let info = expect_context::<super::SiteResource>();
    view! {<Suspense fallback=||()><Show when=move||info.get().is_some_and(|r|r.is_ok_and(|i|!i.production))>{children()}</Show></Suspense>}
}
