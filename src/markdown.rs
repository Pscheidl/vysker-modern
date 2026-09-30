//! A restricted Markdown renderer shared by the server and editor preview.
use pulldown_cmark::{Event, HeadingLevel, LinkType, Parser, Tag, TagEnd, html};

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
    let events = Parser::new(source).filter_map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Some(Event::Text(value)),
        Event::Start(Tag::HtmlBlock) | Event::End(TagEnd::HtmlBlock) => None,
        // Remote images would let page authors embed visitor tracking.
        Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => None,
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
