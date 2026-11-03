//! The same concise guide is rendered as HTML and served as Markdown.
use leptos::prelude::*;
use leptos_meta::Title;
#[cfg(not(feature = "demo"))]
use leptos_meta::{Link, Meta};

#[cfg(not(feature = "demo"))]
pub const GUIDE: &str = include_str!("../../docs/for-agents.md");

#[cfg(not(feature = "demo"))]
#[component]
pub fn ForAgents() -> impl IntoView {
    let info = expect_context::<super::SiteResource>();
    // These links return text and JSON documents, so let the browser load them.
    let guide = crate::markdown::render(GUIDE.strip_prefix("# For agents\n").unwrap_or(GUIDE))
        .replace("<a ", "<a rel=\"external\" ");
    view! {
        <Title text="For agents · Vyskeř"/>
        <Meta name="description" content="Public reading tools, MCP connection details and source guidance for the Vyskeř website."/>
        <Link rel="alternate" type_="text/markdown" href="/for-agents.md"/>
        <article class="page-width interior prose" lang="en">
            <header class="page-heading"><h1>"For agents"</h1></header>
            <Suspense fallback=||view!{<p role="status">"Loading the website's operating mode…"</p>}>
                {move || info.get().map(|result|match result {
                    Ok(site)=>view!{<p class="info-banner" aria-label="Operating mode">{if site.production {
                        "Operating mode: production. Public content remains subject to the source's current availability and publication rules."
                    } else {
                        "Operating mode: preview. This website currently asks search engines not to index it. Direct public reading and MCP access are available."
                    }}</p>}.into_any(),
                    Err(_)=>view!{<p role="status">"The operating mode is currently unavailable. Check the machine-readable guide before making assumptions about publication."</p>}.into_any()
                })}
            </Suspense>
            <div class="markdown-content" inner_html=guide></div>
        </article>
    }
}

#[cfg(feature = "demo")]
#[component]
pub fn ForAgents() -> impl IntoView {
    view! {
        <Title text="For agents · Vyskeř"/>
        <article class="page-width interior prose" lang="en">
            <h1>"For agents"</h1>
            <p>"This static demonstration does not provide a live MCP server or public reading API."</p>
        </article>
    }
}
