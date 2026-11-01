//! A restricted Markdown renderer shared by the server and editor preview.
use pulldown_cmark::{Event, HeadingLevel, LinkType, Options, Parser, Tag, TagEnd, html};

fn parser(source: &str) -> Parser<'_> {
    Parser::new_ext(
        source,
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH,
    )
}

fn safe_image(url: &str) -> bool {
    ["/api/v1/legacy-media/", "/api/v1/page-images/"]
        .iter()
        .any(|prefix| {
            url.strip_prefix(prefix)
                .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Only actual image nodes count, never examples inside code blocks or plain links.
pub fn contains_image(source: &str, url: &str) -> bool {
    parser(source)
        .any(|event| matches!(event, Event::Start(Tag::Image { dest_url, .. }) if dest_url.as_ref() == url))
}

pub fn image_urls(source: &str) -> impl Iterator<Item = String> + '_ {
    parser(source).filter_map(|event| match event {
        Event::Start(Tag::Image { dest_url, .. }) if safe_image(&dest_url) => {
            Some(dest_url.to_string())
        }
        _ => None,
    })
}

fn safe_link(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    !value.chars().any(|c| c.is_control() || c == '\\')
        && (value.starts_with('/') && !value.starts_with("//")
            || value.starts_with('#')
            || ["https://", "http://", "mailto:", "tel:"]
                .iter()
                .any(|p| lower.starts_with(p)))
}
pub fn render(source: &str) -> String {
    render_content(source, false)
}

pub fn render_preview(source: &str) -> String {
    render_content(source, true)
}

/// Search rows link to the page as a whole, so their excerpts contain no nested links or images.
/// Parse the complete source before shortening text to preserve Markdown and reference links.
pub fn render_search_preview(source: &str) -> String {
    let mut remaining = 1200usize;
    let mut events = Vec::new();
    let mut open = Vec::new();
    let mut skipped_depth = 0usize;
    for (event, range) in parser(source).into_offset_iter() {
        // Empty blocks also need a bound, even when they consume no text budget.
        if events.len() >= 256 {
            break;
        }
        if skipped_depth > 0 {
            match event {
                Event::Start(_) => skipped_depth += 1,
                Event::End(_) => skipped_depth -= 1,
                _ => {}
            }
            continue;
        }
        if matches!(event, Event::Start(Tag::Image { .. }))
            || (open.is_empty()
                && matches!(event, Event::Start(Tag::Paragraph))
                && source
                    .get(range)
                    .is_some_and(|text| text.trim() == "{{relief}}"))
        {
            skipped_depth = 1;
            continue;
        }
        let event = match event {
            Event::Start(Tag::Link { .. }) | Event::End(TagEnd::Link) => continue,
            Event::Start(Tag::Heading { .. }) => Event::Start(Tag::Heading {
                level: HeadingLevel::H3,
                id: None,
                classes: vec![],
                attrs: vec![],
            }),
            Event::End(TagEnd::Heading(_)) => Event::End(TagEnd::Heading(HeadingLevel::H3)),
            Event::Html(value) | Event::InlineHtml(value) => Event::Text(value),
            other => other,
        };
        match &event {
            Event::Start(tag) => open.push(tag.to_end()),
            Event::End(_) => {
                open.pop();
            }
            Event::Text(value) | Event::Code(value) => {
                if let Some((end, _)) = value.char_indices().nth(remaining) {
                    let shortened = format!("{}…", &value[..end]).into();
                    events.push(if matches!(event, Event::Code(_)) {
                        Event::Code(shortened)
                    } else {
                        Event::Text(shortened)
                    });
                    break;
                }
                remaining -= value.chars().count();
            }
            _ => {}
        }
        events.push(event);
    }
    events.extend(open.into_iter().rev().map(Event::End));
    render_events(events.into_iter(), false)
}

/// Render a page body, recognizing one standalone relief marker at top level.
/// Other Markdown uses, including code samples, remain literal text.
#[derive(Debug, PartialEq, Eq)]
pub enum PageBlock {
    Markup(String),
    Relief,
}

pub fn render_page(source: &str, preview: bool) -> Vec<PageBlock> {
    let mut depth = 0usize;
    let mut found = false;
    let mut skipping = false;
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (event, range) in parser(source).into_offset_iter() {
        if skipping {
            if matches!(event, Event::End(TagEnd::Paragraph)) {
                skipping = false;
            }
            continue;
        }
        if !found
            && depth == 0
            && matches!(event, Event::Start(Tag::Paragraph))
            && source
                .get(range)
                .is_some_and(|text| text.trim() == "{{relief}}")
        {
            found = true;
            skipping = true;
            continue;
        }
        match &event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
        if found {
            after.push(event);
        } else {
            before.push(event);
        }
    }
    let mut blocks = Vec::new();
    if !before.is_empty() || !found {
        blocks.push(PageBlock::Markup(render_events(
            before.into_iter(),
            preview,
        )));
    }
    if found {
        blocks.push(PageBlock::Relief);
        if !after.is_empty() {
            blocks.push(PageBlock::Markup(render_events(after.into_iter(), preview)));
        }
    }
    blocks
}

fn render_content(source: &str, preview: bool) -> String {
    render_events(parser(source), preview)
}

fn render_events<'a>(events: impl Iterator<Item = Event<'a>>, preview: bool) -> String {
    let mut local_images = Vec::new();
    let events = events.filter_map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Some(Event::Text(value)),
        Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => None,
        // Embed only application-owned images with publication-aware endpoints.
        Event::Start(mut tag @ Tag::Image { .. }) => {
            let local_image = matches!(&tag, Tag::Image { dest_url, .. } if safe_image(dest_url));
            local_images.push(local_image);
            if preview && local_image {
                if let Tag::Image { dest_url, .. } = &mut tag {
                    if let Some(id) = dest_url.strip_prefix("/api/v1/legacy-media/") {
                        *dest_url = format!("/api/v1/admin/legacy-media/{id}").into();
                    }
                }
            }
            local_image.then_some(Event::Start(tag))
        }
        Event::End(TagEnd::Image) => {
            let keep = local_images.pop().unwrap_or(false);
            keep.then_some(Event::End(TagEnd::Image))
        }
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => Some(Event::Start(Tag::Link {
            link_type,
            dest_url: if safe_link(&dest_url)
                || (link_type == LinkType::Email && safe_link(&format!("mailto:{dest_url}")))
            {
                dest_url
            } else {
                "#".into()
            },
            title,
            id,
        })),
        Event::Start(Tag::Heading {
            level: HeadingLevel::H1,
            ..
        }) => Some(Event::Start(Tag::Heading {
            level: HeadingLevel::H2,
            id: None,
            classes: vec![],
            attrs: vec![],
        })),
        Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
            Some(Event::End(TagEnd::Heading(HeadingLevel::H2)))
        }
        other => Some(other),
    });
    let mut result = String::new();
    html::push_html(&mut result, events);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_previews_parse_complete_markdown_and_close_truncated_formatting() {
        let text = "ž".repeat(600);
        let html = render_search_preview(&format!(
            "# Nadpis\n\n**{text}**\n\n[Kontakt][kontakt]\n\n[kontakt]: /obec"
        ));
        assert!(html.contains("<h3>Nadpis</h3>"));
        assert!(html.contains(&format!("<strong>{text}</strong>")));
        assert!(html.contains("<p>Kontakt</p>"));
        assert!(!html.contains("[kontakt]"));
        assert!(!html.contains("<a "));

        let html = render_search_preview(&format!("**{}**", "ž".repeat(1400)));
        assert_eq!(
            html,
            format!("<p><strong>{}…</strong></p>\n", "ž".repeat(1200))
        );
    }

    #[test]
    fn search_previews_bound_markup_without_text() {
        let html = render_search_preview(&"-\n".repeat(1000));
        assert!(html.len() < 4000);
        assert!(html.ends_with("</ul>\n"));
        assert_eq!(html.matches("<li>").count(), html.matches("</li>").count());
        assert!(render_search_preview(&"---\n\n".repeat(1000)).len() < 4000);
    }

    #[test]
    fn search_previews_omit_embeds_and_links_and_escape_raw_html() {
        let html = render_search_preview(
            "{{relief}}\n\n![Kaple](/api/v1/page-images/12)\n\n- **První**\n- *Druhá*\n\n`kód` [Kontakt](/obec) [x](javascript:alert)\n\n<script>alert(1)</script>",
        );
        assert!(html.contains("<li><strong>První</strong></li>"));
        assert!(html.contains("<li><em>Druhá</em></li>"));
        assert!(html.contains("<code>kód</code> Kontakt x"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        for forbidden in [
            "{{relief}}",
            "Kaple",
            "<img",
            "<a ",
            "<script",
            "javascript:",
        ] {
            assert!(!html.contains(forbidden), "{html}");
        }
        assert!(render_search_preview("`{{relief}}`").contains("<code>{{relief}}</code>"));
    }

    #[test]
    fn relief_marker_is_a_standalone_page_block_only() {
        let blocks = render_page("# Nadpis\n\nÚvod.\n\n{{relief}}\n\n**Za modelem**", false);
        assert_eq!(blocks.len(), 3);
        assert!(
            matches!(&blocks[0], PageBlock::Markup(html) if html.contains("<h2>Nadpis</h2>") && html.contains("Úvod."))
        );
        assert_eq!(blocks[1], PageBlock::Relief);
        assert!(
            matches!(&blocks[2], PageBlock::Markup(html) if html.contains("<strong>Za modelem</strong>"))
        );
        for source in [
            "`{{relief}}`",
            "```text\n{{relief}}\n```",
            "> {{relief}}",
            "- {{relief}}",
            "Před {{relief}} za",
            "\\{{relief}}",
            "text\n{{relief}}",
        ] {
            assert!(
                matches!(
                    render_page(source, false).as_slice(),
                    [PageBlock::Markup(_)]
                ),
                "{source}"
            );
        }
        let repeated = render_page("{{relief}}\n\n{{relief}}", true);
        assert_eq!(
            repeated
                .iter()
                .filter(|b| matches!(b, PageBlock::Relief))
                .count(),
            1
        );
        assert!(matches!(&repeated[1], PageBlock::Markup(html) if html.contains("{{relief}}")));
        let references = render_page("[odkaz][ref]\n\n{{relief}}\n\n[ref]: /obec", false);
        assert!(
            matches!(&references[0], PageBlock::Markup(html) if html.contains("href=\"/obec\""))
        );
        // The standard renderer is also used for privacy texts and emails.
        assert!(render("{{relief}}").contains("{{relief}}"));
    }
    #[test]
    fn editor_previews_imports_privately_without_changing_public_urls() {
        let source = "![Old](/api/v1/legacy-media/7) ![New](/api/v1/page-images/7) ![Remote](https://tracker.test/a.png)";
        let preview = render_preview(source);
        assert!(preview.contains("src=\"/api/v1/admin/legacy-media/7\""));
        assert!(preview.contains("src=\"/api/v1/page-images/7\""));
        assert_eq!(preview.matches("<img").count(), 2);
        let public = render(source);
        assert!(public.contains("src=\"/api/v1/legacy-media/7\""));
        assert!(!public.contains("/admin/"));
        assert_eq!(
            image_urls(source).collect::<Vec<_>>(),
            ["/api/v1/legacy-media/7", "/api/v1/page-images/7"]
        );
        assert!(!render_preview("![Forged](/api/v1/admin/legacy-media/7)").contains("<img"));
    }
    #[test]
    fn tables_strikethrough_and_local_images_are_rendered_safely() {
        let html = render(
            "| Den | Čas |\n| --- | --- |\n| Pondělí | 8–12 |\n\n~~staré~~\n\n![Kaple \"u lesa\"](/api/v1/page-images/12)",
        );
        assert!(html.contains("<table>"));
        assert!(html.contains("<td>Pondělí</td>"));
        assert!(html.contains("<del>staré</del>"));
        assert!(html.contains("src=\"/api/v1/page-images/12\""));
        assert!(html.contains("alt=\"Kaple &quot;u lesa&quot;\""));
        for url in [
            "/api/v1/page-images/12?x=1",
            "/api/v1/page-images/../admin",
            "/api/v1/page-images/",
            "//tracker.test/x",
            "data:image/png;base64,abc",
            "https://tracker.test/x",
            "/images/unverified.svg",
        ] {
            assert!(
                !render(&format!("![popis]({url})")).contains("<img"),
                "{url}"
            );
        }
        let url = "/api/v1/page-images/12";
        assert!(contains_image(&format!("![kaple]({url})"), url));
        assert!(!contains_image(&format!("`![kaple]({url})`"), url));
        assert!(!contains_image(&format!("[kaple]({url})"), url));
    }
    #[test]
    fn formatting_is_available_without_active_content_or_trackers() {
        let html = render(
            "# Nadpis\n\n**Text**\n\n- Položka\n\n[Odkaz](https://vysker.cz)\n\n<script>alert(1)</script>\n\n[x](javascript:alert) ![alt](https://tracker.test/pixel)\n\n[x](data:text/html,hello) [x](//tracker.test) [x](/kontakt)",
        );
        assert!(html.contains("<h2>Nadpis</h2>"));
        assert!(html.contains("<strong>Text</strong>"));
        assert!(html.contains("<li>Položka</li>"));
        assert!(html.contains("href=\"/kontakt\""));
        assert!(render("<office@example.cz>").contains("href=\"mailto:office@example.cz\""));
        for forbidden in ["<script", "<img", "javascript:", "data:text", "href=\"//"] {
            assert!(!html.contains(forbidden), "{html}");
        }
    }
}
