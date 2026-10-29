//! Public calendar, permanent event detail, and the homepage feed.
use super::{
    components::{Icon, PageHeading},
    pages::NotFound,
    site_url,
};
use crate::events::{
    EVENT_CATEGORIES, EVENT_PAGE_SIZE, Event, category_label, load_calendar_url, load_event,
    load_events,
};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::{
    components::A,
    hooks::{use_params_map, use_query_map},
};

#[component]
fn EventLoading() -> impl IntoView {
    view! {<div class="loading-state" role="status"><span class="loading-dot"></span>"Načítám akce…"</div>}
}

#[component]
fn EventFailure(on_retry: Callback<()>) -> impl IntoView {
    view! {<div class="empty-state" role="alert"><Icon name="info"/><h2>"Akce se nepodařilo načíst"</h2><p>"Zkuste načtení zopakovat."</p><button class="button secondary" type="button" on:click=move |_|on_retry.run(())>"Zkusit znovu"</button></div>}
}

#[component]
fn EventTimes(event: Event) -> impl IntoView {
    let start = event.start_label();
    let end = event.end_label();
    let start_datetime = if event.start_time_known && !event.all_day {
        event.display_start().to_owned()
    } else {
        event.display_start().chars().take(10).collect::<String>()
    };
    let end_datetime = if !event.end_date_known {
        None
    } else if event.end_time_known && !event.all_day {
        Some(event.display_end().to_owned())
    } else {
        Some(event.display_end().chars().take(10).collect::<String>())
    };
    view! {<p class="event-times"><span>"Začátek: "<time datetime=start_datetime>{start}</time></span><br/><span>"Konec: "<time datetime=end_datetime>{end}</time></span></p>}
}

#[component]
fn EventRow(event: Event) -> impl IntoView {
    let href = site_url(&event.href());
    let export_url = site_url(&format!("/api/v1/events/{}/calendar.ics", event.id));
    let export_label = format!("Přidat do kalendáře: {}", event.title);
    let times = event.clone();
    let recurrence = event.recurrence_label();
    let export_hint = if event.repeat_every_14_days {
        "Jednorázová kopie celé řady opakování do kalendáře"
    } else {
        "Jednorázová kopie akce do kalendáře"
    };
    view! {<article class="site-result event-row" class:is-cancelled=event.cancelled>
        <div>{event.cancelled.then(||view!{<span class="status-badge warning">"Zrušeno"</span>})}
            <h2><A href>{event.title}</A></h2>
            {(event.category != "other").then(||view!{<p class="event-category">{category_label(&event.category)}</p>})}
            <EventTimes event=times/>
            {recurrence.map(|label|view!{<p class="event-recurrence field-note">{label}</p>})}
            {(!event.location.is_empty()).then(||view!{<p><Icon name="pin"/>{event.location}</p>})}
            <a class="button secondary event-row-calendar" href=export_url rel="external" type="text/calendar" aria-label=export_label title=export_hint>
                <Icon name="calendar"/>"Přidat do kalendáře"
            </a>
        </div><Icon/>
    </article>}
}

fn calendar_link(archived: bool, page: i64) -> String {
    site_url(&format!(
        "/kalendar?stav={}&strana={page}",
        if archived { "archiv" } else { "nadchazejici" }
    ))
}

#[component]
fn EventPager(archived: bool, page: i64, more: bool) -> impl IntoView {
    view! {<nav class="public-pager" aria-label="Stránkování akcí"><span role="status">{format!("Strana {page}")}</span><div>
        {(page>1).then(||view!{<A href=calendar_link(archived, page-1) attr:class="button secondary">"Předchozí"</A>})}
        {(more && page<5001).then(||view!{<A href=calendar_link(archived, page+1) attr:class="button secondary">"Další"</A>})}
    </div></nav>}
}

#[component]
fn CalendarSubscriptionLinks(base_url: String) -> impl IntoView {
    let category = RwSignal::new(String::new());
    let base_url = StoredValue::new(base_url);
    let feed_url = Memo::new(move |_| {
        let base = base_url.get_value();
        let selected = category.get();
        if selected.is_empty() {
            base
        } else {
            format!("{base}?category={selected}")
        }
    });
    let subscription_url = move || {
        let url = feed_url.get();
        url.strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .map(|url| format!("webcal://{url}"))
            .unwrap_or(url)
    };
    view! {<div class="calendar-subscription-controls">
        <label for="calendar-subscription-topic">"Co chcete odebírat?"</label>
        <select id="calendar-subscription-topic" prop:value=move ||category.get() on:change=move |ev|category.set(event_target_value(&ev))>
            <option value="">"Celý kalendář"</option>
            {EVENT_CATEGORIES.iter().copied().filter(|(value,_)|*value != "other").map(|(value,label)|view!{<option value=value>{label}</option>}).collect_view()}
        </select>
        <a class="button primary" href=subscription_url><Icon name="calendar"/>"Odebírat v aplikaci"</a>
        <label for="calendar-subscription-url">"Adresa pro odběr"</label>
        <input id="calendar-subscription-url" type="text" readonly prop:value=move ||feed_url.get() aria-describedby="calendar-subscription-copy"
            on:click=move |ev|{event_target::<web_sys::HtmlInputElement>(&ev).select();}
            on:focus=move |ev|{event_target::<web_sys::HtmlInputElement>(&ev).select();}/>
        <p id="calendar-subscription-copy" class="field-note">"Kliknutím adresu označíte. Pak ji zkopírujte do své kalendářové aplikace jako nový odběr."</p>
    </div>}
}

#[component]
fn CalendarSubscriptions() -> impl IntoView {
    let url = Resource::new(|| (), |_| load_calendar_url());
    let query = use_query_map();
    view! {<details class="calendar-subscriptions" id="odber-kalendare" open=move ||query.get().get("odber").as_deref()==Some("1")>
        <summary><Icon name="calendar"/><span>"Odebírat kalendář"</span></summary>
        <div class="calendar-subscription-body">
            <p>"Odběr průběžně přenáší nové akce, změny termínů i zrušení. Vyberte celý kalendář nebo jen téma, které vás zajímá."</p>
            <Suspense fallback=||view!{<p role="status">"Načítám adresu kalendáře…"</p>}>
                {move ||url.get().map(|result|match result {
                    Ok(url) if !url.is_empty()=>view!{<CalendarSubscriptionLinks base_url=url/>}.into_any(),
                    _=>view!{<p role="alert">"Adresu odběru se nepodařilo načíst. "<button class="text-link" type="button" on:click=move |_|url.refetch()>"Zkusit znovu"</button></p>}.into_any(),
                })}
            </Suspense>
            <div class="calendar-subscription-help">
                <p><strong>"iPhone: "</strong>"Klepněte na Odebírat v aplikaci a potvrďte přidání kalendáře. "<a href="https://support.apple.com/cs-cz/guide/iphone/iph3d1110d4/ios">"Návod Apple"</a></p>
                <p><strong>"Kalendář Google: "</strong>"První odběr přidejte v prohlížeči na počítači přes Jiné kalendáře → Přidat → Pomocí adresy URL. Vložte adresu výše. Kalendář pak můžete zobrazit i v mobilní aplikaci. "<a href="https://support.google.com/calendar/answer/37100?co=GENIE.Platform%3DDesktop&hl=cs">"Návod Google"</a></p>
                <p>"Rychlost aktualizací řídí vaše aplikace nebo poskytovatel, změny se nemusí projevit hned. Připomínky závisí na aplikaci a vašem nastavení."</p>
                <p>"Na obecním webu se neregistrujete a web nemá přístup k vašemu soukromému kalendáři. Pro jednorázovou kopii zvolte u akce Přidat do kalendáře."</p>
            </div>
        </div>
    </details>}
}

#[component]
pub fn Calendar() -> impl IntoView {
    let intro = Resource::new(|| (), |_| crate::catalog::load_page("kalendar".into()));
    let params = use_query_map();
    let query = Memo::new(move |_| {
        let params = params.get();
        let archived = params.get("stav").as_deref() == Some("archiv");
        let page = params
            .get("strana")
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|v| (1..=5001).contains(v))
            .unwrap_or(1);
        (archived, page)
    });
    let events = Resource::new(
        move || query.get(),
        |(archived, page)| async move {
            load_events(archived, EVENT_PAGE_SIZE + 1, (page - 1) * EVENT_PAGE_SIZE)
                .await
                .map(|items| (archived, page, items))
        },
    );
    view! {<Title text="Kalendář akcí · Vyskeř"/><div class="page-width interior">
        <PageHeading title="Dění na Vyskři." description="Setkání, události a praktické termíny na jednom místě." eyebrow="KALENDÁŘ AKCÍ"/>
        <Suspense fallback=|| ()>{move ||intro.get().and_then(Result::ok).flatten().map(|page|view!{
            <div class="markdown-content calendar-intro" inner_html=crate::markdown::render(&page.content)></div>
        })}</Suspense>
        <CalendarSubscriptions/>
        <nav class="page-tabs" aria-label="Období akcí">
            <A href=site_url("/kalendar") attr:class=move ||if !query.get().0 {"selected"} else {""}>"Nadcházející a probíhající"</A>
            <A href=site_url("/kalendar?stav=archiv") attr:class=move ||if query.get().0 {"selected"} else {""}>"Archiv akcí"</A>
        </nav>
        <p class="field-note">"Časy uvádíme v místním čase pro Vyskeř (Europe/Prague). U opakovaných akcí zobrazujeme nejbližší probíhající nebo nadcházející termín, v archivu poslední. Zrušené akce jsou označené."</p>
        <Suspense fallback=EventLoading>{move ||events.get().map(|result|match result {
            Err(_)=>view!{<EventFailure on_retry=Callback::new(move |_|events.refetch())/>}.into_any(),
            Ok((archived,page,items))=>{
                let more=items.len()>EVENT_PAGE_SIZE as usize;
                view!{
                    {if items.is_empty() {view!{<div class="empty-state"><Icon name="calendar"/><h2>{if archived {"V archivu zatím nejsou žádné akce"} else {"Nejsou zveřejněné žádné nadcházející akce"}}</h2>{(page>1).then(||view!{<A href=calendar_link(archived,1) attr:class="text-link">"Zpět na první stranu"</A>})}</div>}.into_any()}
                    else {view!{<div class="document-list calendar-events">{items.into_iter().take(EVENT_PAGE_SIZE as usize).map(|event|view!{<EventRow event/>}).collect_view()}</div>}.into_any()}}
                    <EventPager archived page more/>
                }.into_any()
            }
        })}</Suspense>
    </div>}
}

#[component]
pub fn EventDetail() -> impl IntoView {
    let params = use_params_map();
    let event = Resource::new(
        move || {
            params
                .get()
                .get("id")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
        },
        load_event,
    );
    view! {<div class="page-width interior prose event-detail">
        <A href=site_url("/kalendar") attr:class="back-link"><Icon name="back"/>"Zpět na kalendář"</A>
        <Suspense fallback=EventLoading>{move ||event.get().map(|result|match result {
            Err(_)=>view!{<EventFailure on_retry=Callback::new(move |_|event.refetch())/>}.into_any(),
            Ok(None)=>view!{<NotFound/>}.into_any(),
            Ok(Some(item))=>{
                let times=item.clone();
                let recurrence=item.recurrence_label();
                let export_url=site_url(&format!("/api/v1/events/{}/calendar.ics",item.id));
                view!{<Title text=format!("{} · Kalendář · Vyskeř", item.title)/>
                    <header class="page-heading"><p class="eyebrow">"KALENDÁŘ AKCÍ"</p><h1>{item.title}</h1></header>
                    {item.cancelled.then(||view!{<p class="info-banner warning"><Icon name="info"/><strong>{if item.repeat_every_14_days {"Celá řada opakování je zrušena."} else {"Akce je zrušena."}}</strong>" Termín ponecháváme pro informaci."</p>})}
                    <EventTimes event=times/>
                    {recurrence.map(|label|view!{<p class="event-recurrence field-note">{label}</p>})}
                    <p class="event-category">{category_label(&item.category)}</p>
                    <p class="field-note">"Místní čas pro Vyskeř (Europe/Prague)."</p>
                    <p><strong>"Místo: "</strong>{if item.location.is_empty() {"Místo bude upřesněno".into()} else {item.location}}</p>
                    <div class="event-description-text markdown-content" inner_html=crate::markdown::render(&item.description)></div>
                    {item.source_url.map(|url|view!{<p class="field-note">"Zdroj: "<a class="text-link" href=url rel="external">"ČEZ Distribuce"</a></p>})}
                    <section class="event-calendar-action" aria-label="Uložit akci do kalendáře">
                        <a class="button secondary" href=export_url rel="external" type="text/calendar"><Icon name="calendar"/>"Přidat do kalendáře"</a>
                        {item.repeat_every_14_days.then(||view!{<p>"Do kalendáře se přidá celá řada opakování každých 14 dní."</p>})}
                        <p>"Akci můžete otevřít v kalendářové aplikaci. Pokud prohlížeč soubor ICS stáhne, otevřete jej. Ukládáte jednorázovou kopii, do které se pozdější změny automaticky nepřenesou."</p>
                        {(!item.all_day && !item.start_time_known).then(||view!{<p>"Čas začátku není známý, proto bude kopie v kalendáři uvedena bez času."</p>})}
                        <A href=site_url("/kalendar?odber=1#odber-kalendare") attr:class="text-link">"Chci odebírat kalendář i se změnami"<Icon name="external"/></A>
                    </section>
                }.into_any()
            }
        })}</Suspense>
    </div>}
}

#[component]
pub fn UpcomingEvents() -> impl IntoView {
    let events = Resource::new(|| (), |_| load_events(false, 3, 0));
    view! {<section class="events-strip" aria-labelledby="upcoming-events-title"><h2 id="upcoming-events-title">"Dění v obci"</h2>
        <Suspense fallback=EventLoading>{move ||events.get().map(|result|match result {
            Err(_)=>view!{<p role="alert">"Akce se nepodařilo načíst. "<button class="text-link" type="button" on:click=move |_|events.refetch()>"Zkusit znovu"</button></p>}.into_any(),
            Ok(items) if items.is_empty()=>view!{<p>"Nejsou zveřejněné žádné nadcházející akce."</p>}.into_any(),
            Ok(items)=>items.into_iter().map(|event|{
                let href=site_url(&event.href());
                let date=event.start_label();
                let datetime=if event.start_time_known && !event.all_day {event.display_start().to_owned()} else {event.display_start().chars().take(10).collect()};
                let recurrence=event.repeat_every_14_days.then_some(" · každých 14 dní");
                view!{<A href><span><time datetime=datetime>{date}</time></span>{event.cancelled.then(||view!{<strong class="status-badge warning">"Zrušeno"</strong>})}{event.title}{recurrence}</A>}
            }).collect_view().into_any(),
        })}</Suspense>
        <A href=site_url("/kalendar") attr:class="text-link">"Kalendář akcí"<Icon name="external"/></A>
    </section>}
}
