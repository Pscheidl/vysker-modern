use super::{components::*, site_url};
use crate::search::{PAGE_SIZE, SearchQuery};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::{components::A, hooks::use_query_map};

fn query(kind: &'static str) -> Memo<SearchQuery> {
    let params = use_query_map();
    Memo::new(move |_| {
        let p = params.get();
        SearchQuery {
            q: p.get("q").unwrap_or_default(),
            kind: kind.into(),
            state: if kind != "notice" {
                "all"
            } else if p.get("state").as_deref() == Some("archive")
                || p.get("stav").as_deref() == Some("archiv")
            {
                "archive"
            } else {
                "current"
            }
            .into(),
            category: p
                .get("kategorie")
                .or_else(|| p.get("category"))
                .unwrap_or_default(),
            sort: p
                .get("razeni")
                .or_else(|| p.get("sort"))
                .unwrap_or_else(|| "newest".into()),
            page: p
                .get("strana")
                .or_else(|| p.get("page"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        }
    })
}
#[component]
fn Filters(
    query: Memo<SearchQuery>,
    path: &'static str,
    #[prop(default = false)] topics: bool,
) -> impl IntoView {
    let categories = Resource::new(|| (), |_| crate::catalog::load_subscription_categories());
    view! {<form class="filter-bar" method="get" action=site_url(path) role="search">
        {(path=="/uredni-deska").then(move||view!{<input type="hidden" name="state" value=move ||query.get().state/>})}
        <div class="filter-search"><label class="sr-only" for="public-query">"Hledat v dokumentech"</label><Icon name="search"/><input id="public-query" type="search" name="q" maxlength="200" placeholder="Název, téma nebo číslo jednací…" prop:value=move ||query.get().q value=move ||query.get().q/></div>
        {topics.then(||view!{<div class="filter-control"><label for="public-category">"Téma"</label><select id="public-category" name="kategorie" prop:value=move ||query.get().category>
            <option value="">"Všechna témata"</option><Suspense fallback=|| ()>{move || categories.get().and_then(Result::ok).map(|items|items.into_iter().map(|category|{let name=category.name.clone();let selected_name=name.clone();view!{<option value=category.name selected=move ||query.get().category==selected_name>{name}</option>}}).collect_view())}</Suspense>
        </select></div>})}
        <div class="filter-control"><label for="public-sort">"Řazení"</label><select id="public-sort" name="razeni" prop:value=move ||query.get().sort><option value="newest" selected=move ||query.get().sort=="newest">"Od nejnovějších"</option><option value="oldest" selected=move ||query.get().sort=="oldest">"Od nejstarších"</option><option value="title" selected=move ||query.get().sort=="title">"Podle názvu"</option></select></div>
        <button class="button secondary" type="submit">"Hledat"</button>
    </form>}
}
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn documents_url(query: &SearchQuery, page: usize) -> String {
    let mut params = Vec::new();
    if !query.q.is_empty() {
        params.push(format!("q={}", encode(&query.q)));
    }
    if !query.category.is_empty() {
        params.push(format!("kategorie={}", encode(&query.category)));
    }
    if !query.sort.is_empty() && query.sort != "newest" {
        params.push(format!("razeni={}", encode(&query.sort)));
    }
    if page > 1 {
        params.push(format!("strana={page}"));
    }
    site_url(&if params.is_empty() {
        "/dokumenty".into()
    } else {
        format!("/dokumenty?{}", params.join("&"))
    })
}
#[component]
fn Pager(query: SearchQuery, page: usize, total: usize, path: &'static str) -> impl IntoView {
    let last = total.div_ceil(PAGE_SIZE).max(1);
    let link = |page| {
        if path == "/dokumenty" {
            return documents_url(&query, page);
        }
        site_url(&format!(
            "{path}?q={}&state={}&kategorie={}&razeni={}&strana={page}",
            encode(&query.q),
            query.state,
            encode(&query.category),
            encode(&query.sort)
        ))
    };
    view! {<nav class="public-pager" aria-label="Stránkování výsledků"><span role="status">{format!("Počet výsledků: {total} · strana {page} z {last}")}</span><div>{(page>1).then(||view!{<A href=link(page-1) attr:class="button secondary">"Předchozí"</A>})}{(page<last).then(||view!{<A href=link(page+1) attr:class="button secondary">"Další"</A>})}</div></nav>}
}
#[component]
pub fn NoticeListing() -> impl IntoView {
    let selected = query("notice");
    view! {<Show when=move ||selected.get().state=="archive" fallback=CurrentNoticeListing>
        {move || {let q=selected.get();view!{<leptos_router::components::Redirect path=documents_url(&q,q.page.max(1))/>}}}
    </Show>}
}
#[component]
fn CurrentNoticeListing() -> impl IntoView {
    let query = query("notice");
    let result = Resource::new(move || query.get(), crate::search::search);
    let documents = move || {
        let query = SearchQuery {
            sort: "newest".into(),
            ..query.get()
        };
        documents_url(&query, 1)
    };
    view! {<Title text="Úřední deska · Vyskeř"/><div class="page-width interior"><PageHeading title="Úřední deska" description="Aktuálně vyvěšené písemnosti obce." eyebrow="DOKUMENTY OBCE"/>
        <p class="field-note" id="board-search-scope">"Hledáte pouze mezi aktuálně vyvěšenými písemnostmi. "<A href=documents>"Všechny dokumenty"</A></p>
        <Filters query path="/uredni-deska" topics=true/>
        <Suspense fallback=Loading>{move ||result.get().map(|r|match r {
            Ok(page)=>view!{<Pager query=query.get() page=page.page total=page.total path="/uredni-deska"/>{if page.items.is_empty() {view!{<NoResults query=query.get() limited=true/>}.into_any()} else {view!{<div class="document-list">{page.items.into_iter().map(|hit|view!{<ResultRow hit/>}).collect_view()}</div>}.into_any()}}}.into_any(),
            Err(e)=>view!{<LoadError message=e.to_string() on_retry=Callback::new(move|()|result.refetch())/>}.into_any(),
        })}</Suspense><Newsletter/>
    </div>}
}
#[component]
pub fn DocumentListing() -> impl IntoView {
    let params = use_query_map();
    let selected = query("document");
    view! {<Show when=move||{let p=params.get();p.get("state").is_some()||p.get("stav").is_some()} fallback=||view!{
        <Results kind="document" path="/dokumenty" title="Dokumenty a formuláře" description="Všechny veřejné dokumenty obce, včetně aktuálně vyvěšených i sejmutých písemností."/>
    }>
        {move||{let q=selected.get();view!{<leptos_router::components::Redirect path=documents_url(&q,q.page.max(1))/>}}}
    </Show>}
}
#[component]
pub fn SearchListing() -> impl IntoView {
    view! {<Results kind="all" path="/hledat" title="Co hledáte?" description="Prohledejte všechny veřejné dokumenty včetně vyvěšených a sejmutých písemností i stránky obce."/>}
}
#[component]
fn Results(
    kind: &'static str,
    path: &'static str,
    title: &'static str,
    description: &'static str,
) -> impl IntoView {
    let query = query(kind);
    let result = Resource::new(move || query.get(), crate::search::search);
    view! {<Title text=format!("{title} · Vyskeř")/><div class="page-width interior"><PageHeading title description/>
        <Filters query path topics=kind=="document"/>
        <Suspense fallback=Loading>{move ||result.get().map(|r|match r {
            Ok(page)=>view!{<Pager query=query.get() page=page.page total=page.total path/>{if page.items.is_empty(){view!{<NoResults query=query.get() limited=kind!="all"/>}.into_any()} else {view!{<div class="document-list">{page.items.into_iter().map(|hit|view!{<ResultRow hit/>}).collect_view()}</div>}.into_any()}}}.into_any(),
            Err(e)=>view!{<LoadError message=e.to_string() on_retry=Callback::new(move|()|result.refetch())/>}.into_any(),
        })}</Suspense>
    </div>}
}

#[component]
fn ResultRow(hit: crate::search::Hit) -> impl IntoView {
    let label = match hit.kind.as_str() {
        "notice" if hit.archived => "ÚŘEDNÍ DESKA · SEJMUTO",
        "notice" => "ÚŘEDNÍ DESKA · VYVĚŠENO",
        "document" => "DOKUMENT",
        _ => "STRÁNKA",
    };
    let dated = hit.kind != "page";
    let date = crate::catalog::document_date(&hit.posted);
    let preview = match hit.preview_html {
        Some(html) => view! {
            <div class="site-result-preview markdown-content" inner_html=html/>
        }
        .into_any(),
        None => view! {
            <p class="site-result-preview">{hit.description}</p>
        }
        .into_any(),
    };
    view! {<article class="site-result"><div><p class="eyebrow">{label}</p><h2><A href=site_url(&hit.path)>{hit.title}</A></h2>{preview}
        <div class="notice-meta">{(!hit.category.is_empty()).then(||view!{<span>{hit.category}</span>})}{dated.then(||view!{<time datetime=(!hit.posted.is_empty()).then_some(hit.posted)>{date}</time>})}</div>
    </div><Icon/></article>}
}

#[component]
fn NoResults(query: SearchQuery, limited: bool) -> impl IntoView {
    let searched = !query.q.trim().is_empty();
    let filtered = !query.category.is_empty();
    let all_url = site_url(&format!("/hledat?q={}", encode(&query.q)));
    view! {<div class="empty-state"><Icon name="search"/><h2>{if searched || filtered {"Žádné odpovídající dokumenty"} else {"Žádné dokumenty k zobrazení"}}</h2>
        {(searched || filtered).then(|| view!{<p>"Zkuste jiný výraz nebo upravte výběr tématu."</p>})}
        {limited.then(||view!{<A href=all_url attr:class="text-link">"Prohledat celý web"<Icon/></A>})}
    </div>}
}
