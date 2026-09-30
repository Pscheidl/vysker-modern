//! Public calendar, permanent event detail, and the homepage feed.
use super::{
    components::{Icon, PageHeading},
    pages::NotFound,
    site_url,
};
use crate::events::{EVENT_PAGE_SIZE, Event, load_event, load_events};
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
    view! {<p class="event-times"><span>"Začátek: "<time datetime=event.starts_at>{start}</time></span><br/><span>"Konec: "<time datetime=event.ends_at>{end}</time></span></p>}
}

#[component]
fn EventRow(event: Event) -> impl IntoView {
    let href = site_url(&event.href());
    let times = event.clone();
    view! {<article class="site-result event-row" class:is-cancelled=event.cancelled>
        <div>{event.cancelled.then(||view!{<span class="status-badge warning">"Zrušeno"</span>})}
            <h2><A href>{event.title}</A></h2><EventTimes event=times/>
            {(!event.location.is_empty()).then(||view!{<p><Icon name="pin"/>{event.location}</p>})}
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
        <nav class="page-tabs" aria-label="Období akcí">
            <A href=site_url("/kalendar") attr:class=move ||if !query.get().0 {"selected"} else {""}>"Nadcházející a probíhající"</A>
            <A href=site_url("/kalendar?stav=archiv") attr:class=move ||if query.get().0 {"selected"} else {""}>"Archiv akcí"</A>
        </nav>
        <p class="field-note">"Časy uvádíme v místním čase pro Vyskeř (Europe/Prague). Zrušené akce jsou označené."</p>
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
                view!{<Title text=format!("{} · Kalendář · Vyskeř", item.title)/>
                    <header class="page-heading"><p class="eyebrow">"KALENDÁŘ AKCÍ"</p><h1>{item.title}</h1></header>
                    {item.cancelled.then(||view!{<p class="info-banner warning"><Icon name="info"/><strong>"Akce je zrušena."</strong>" Původní termín ponecháváme pro informaci."</p>})}
                    <EventTimes event=times/>
                    <p class="field-note">"Místní čas pro Vyskeř (Europe/Prague)."</p>
                    <p><strong>"Místo: "</strong>{if item.location.is_empty() {"Místo bude upřesněno".into()} else {item.location}}</p>
                    <div class="event-description-text">{item.description.split('\n').map(|line|view!{<p>{line.to_owned()}</p>}).collect_view()}</div>
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
                view!{<A href><span><time datetime=event.starts_at>{date}</time></span>{event.cancelled.then(||view!{<strong class="status-badge warning">"Zrušeno"</strong>})}{event.title}</A>}
            }).collect_view().into_any(),
        })}</Suspense>
        <A href=site_url("/kalendar") attr:class="text-link">"Kalendář akcí"<Icon name="external"/></A>
    </section>}
}
