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

fn render_content(source: &str, preview: bool) -> String {
    let mut local_images = Vec::new();
    let events = parser(source).filter_map(|event| match event {
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
