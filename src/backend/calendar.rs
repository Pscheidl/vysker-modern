//! RFC 5545 export for one-off imports and public calendar subscriptions.
use super::{Backend, Result, bad, missing};
use crate::events::{EVENT_CATEGORIES, category_label};
use axum::{
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use pulldown_cmark::{Event as MarkdownEvent, Parser, Tag, TagEnd};
use serde::Deserialize;
use time::{Date, OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};
use time_tz::{OffsetDateTimeExt, PrimitiveDateTimeExt, timezones::db::europe::PRAGUE};

#[derive(sqlx::FromRow)]
struct CalendarEvent {
    id: i64,
    calendar_uid: String,
    title: String,
    description: String,
    location: String,
    category: String,
    all_day: bool,
    repeat_every_14_days: bool,
    repeat_until: Option<Date>,
    starts_at: OffsetDateTime,
    ends_at: OffsetDateTime,
    start_time_known: bool,
    end_time_known: bool,
    end_date_known: bool,
    cancelled: bool,
    version: i64,
    updated_at: String,
    withdrawn: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalendarQuery {
    category: Option<String>,
}

pub async fn feed(
    State(state): State<Backend>,
    Query(query): Query<CalendarQuery>,
) -> Result<Response> {
    if query
        .category
        .as_ref()
        .is_some_and(|category| !EVENT_CATEGORIES.iter().any(|(key, _)| *key == category))
    {
        return Err(bad("Neznámá kategorie kalendáře."));
    }
    // One snapshot prevents races between a public event and its withdrawal.
    // Do not page or expire records: clients also need old updates/cancellations.
    let events: Vec<CalendarEvent> = sqlx::query_as(
        "SELECT id,calendar_uid,title,description,location,category,all_day,repeat_every_14_days,repeat_until,starts_at,ends_at,start_time_known,end_time_known,end_date_known,cancelled,version,updated_at,FALSE AS withdrawn
         FROM events WHERE published AND ($1::TEXT IS NULL OR category=$1)
         UNION ALL
         SELECT 0 AS id,w.calendar_uid,'' AS title,'' AS description,'' AS location,w.category,w.all_day,w.repeat_every_14_days,w.repeat_until,w.starts_at,w.ends_at,w.start_time_known,w.end_time_known,w.end_date_known,TRUE AS cancelled,w.version,w.updated_at,TRUE AS withdrawn
         FROM calendar_withdrawals w WHERE w.category=COALESCE($1,'*')
         AND NOT EXISTS(SELECT 1 FROM events e WHERE e.calendar_uid=w.calendar_uid AND e.published AND ($1::TEXT IS NULL OR e.category=$1))
         ORDER BY starts_at,calendar_uid"
    ).bind(&query.category).fetch_all(&state.pool).await?;
    let name = query
        .category
        .as_deref()
        .map(category_label)
        .unwrap_or("Kalendář akcí");
    calendar_response(&events, &state.config.public_url, name, false)
}

pub async fn single(State(state): State<Backend>, Path(id): Path<i64>) -> Result<Response> {
    let event: CalendarEvent =
        sqlx::query_as("SELECT events.*,FALSE AS withdrawn FROM events WHERE id=$1 AND published")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or_else(missing)?;
    calendar_response(&[event], &state.config.public_url, "Kalendář akcí", true)
}

fn calendar_response(
    events: &[CalendarEvent],
    public_url: &str,
    name: &str,
    single_event: bool,
) -> Result<Response> {
    let body = render(events, public_url, name)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                if single_event {
                    "inline; filename=\"akce.ics\""
                } else {
                    "inline; filename=\"kalendar.ics\""
                },
            ),
            // Do not retain public content after it has been withdrawn.
            (header::CACHE_CONTROL, "no-store"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        body,
    )
        .into_response())
}

fn escape_text(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .fold(String::new(), |mut out, c| {
            match c {
                '\\' => out.push_str("\\\\"),
                ';' => out.push_str("\\;"),
                ',' => out.push_str("\\,"),
                '\n' => out.push_str("\\n"),
                _ => out.push(c),
            }
            out
        })
}

/// Fold by UTF-8 octets without splitting a code point, including continuation space.
fn line(out: &mut String, value: &str) {
    let mut octets = 0;
    for ch in value.chars() {
        if octets + ch.len_utf8() > 75 {
            out.push_str("\r\n ");
            octets = 1;
        }
        out.push(ch);
        octets += ch.len_utf8();
    }
    out.push_str("\r\n");
}

fn date(value: Date) -> String {
    format!(
        "{:04}{:02}{:02}",
        value.year(),
        u8::from(value.month()),
        value.day()
    )
}

fn utc(value: OffsetDateTime) -> String {
    let value = value.to_offset(UtcOffset::UTC);
    format!(
        "{}T{:02}{:02}{:02}Z",
        date(value.date()),
        value.hour(),
        value.minute(),
        value.second()
    )
}

fn local(value: OffsetDateTime) -> String {
    let value = value.to_timezone(PRAGUE);
    format!(
        "{}T{:02}{:02}{:02}",
        date(value.date()),
        value.hour(),
        value.minute(),
        value.second()
    )
}

fn prague_timezone(out: &mut String) {
    // Recurring timed events are restricted to 1996 onwards. These are the
    // Europe/Prague EU rules in time-tz's bundled IANA tzdata (tz/europe).
    // The baseline covers January 1996, before the first daylight transition.
    // Do not apply the October rule to historical Czech summer-time periods.
    for item in [
        "BEGIN:VTIMEZONE",
        "TZID:Europe/Prague",
        "BEGIN:STANDARD",
        "DTSTART:19960101T000000",
        "TZOFFSETFROM:+0100",
        "TZOFFSETTO:+0100",
        "TZNAME:CET",
        "END:STANDARD",
        "BEGIN:DAYLIGHT",
        "DTSTART:19960331T020000",
        "TZOFFSETFROM:+0100",
        "TZOFFSETTO:+0200",
        "TZNAME:CEST",
        "RRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=-1SU",
        "END:DAYLIGHT",
        "BEGIN:STANDARD",
        "DTSTART:19961027T030000",
        "TZOFFSETFROM:+0200",
        "TZOFFSETTO:+0100",
        "TZNAME:CET",
        "RRULE:FREQ=YEARLY;BYMONTH=10;BYDAY=-1SU",
        "END:STANDARD",
        "END:VTIMEZONE",
    ] {
        line(out, item);
    }
}

fn recurrence_rule(event: &CalendarEvent) -> Result<String> {
    let mut rule = "RRULE:FREQ=WEEKLY;INTERVAL=2".to_owned();
    if let Some(until) = event.repeat_until {
        let until = if event.all_day || !event.start_time_known {
            date(until)
        } else {
            // An inclusive local date includes any start on that date. RFC
            // 5545 requires UTC UNTIL when DTSTART has a time zone reference.
            let end_of_day = until
                .with_hms(23, 59, 59)
                .map_err(|_| bad("Konec opakování není platný."))?
                .assume_timezone(PRAGUE)
                .take_first()
                .ok_or_else(|| bad("Konec opakování není platný."))?;
            utc(end_of_day)
        };
        rule.push_str(&format!(";UNTIL={until}"));
    }
    Ok(rule)
}

fn plain_description(value: &str) -> String {
    let mut out = String::new();
    for event in Parser::new(value) {
        match event {
            MarkdownEvent::Text(text)
            | MarkdownEvent::Code(text)
            | MarkdownEvent::Html(text)
            | MarkdownEvent::InlineHtml(text) => out.push_str(&text),
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => out.push('\n'),
            MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                if dest_url.starts_with("https://") || dest_url.starts_with("http://") {
                    out.push_str(&format!("({dest_url}) "));
                }
            }
            MarkdownEvent::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item | TagEnd::CodeBlock,
            )
            | MarkdownEvent::Rule => out.push('\n'),
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn render(events: &[CalendarEvent], public_url: &str, name: &str) -> Result<String> {
    let mut out = String::new();
    for item in [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Obecni web//Kalendar//CS",
        "CALSCALE:GREGORIAN",
        "X-WR-TIMEZONE:Europe/Prague",
    ] {
        line(&mut out, item);
    }
    line(
        &mut out,
        &format!("X-WR-CALNAME:{}", escape_text(&format!("Vyskeř · {name}"))),
    );
    if events
        .iter()
        .any(|event| event.repeat_every_14_days && !event.all_day && event.start_time_known)
    {
        prague_timezone(&mut out);
    }
    for event in events {
        let updated = OffsetDateTime::parse(&event.updated_at, &Rfc3339)
            .map_err(|_| bad("Datum změny akce není platné."))?;
        line(&mut out, "BEGIN:VEVENT");
        line(
            &mut out,
            &format!("UID:{}", escape_text(&event.calendar_uid)),
        );
        line(&mut out, &format!("SEQUENCE:{}", event.version));
        line(&mut out, &format!("DTSTAMP:{}", utc(updated)));
        line(&mut out, &format!("LAST-MODIFIED:{}", utc(updated)));
        if event.all_day || !event.start_time_known {
            let start = event.starts_at.to_timezone(PRAGUE).date();
            let end = if event.end_date_known {
                event.ends_at.to_timezone(PRAGUE).date()
            } else {
                start
            };
            line(&mut out, &format!("DTSTART;VALUE=DATE:{}", date(start)));
            if let Some(exclusive_end) = end.next_day() {
                line(
                    &mut out,
                    &format!("DTEND;VALUE=DATE:{}", date(exclusive_end)),
                );
            }
        } else if event.repeat_every_14_days {
            line(
                &mut out,
                &format!("DTSTART;TZID=Europe/Prague:{}", local(event.starts_at)),
            );
            // A seconds-only duration keeps the original elapsed duration,
            // even when an instance crosses a clock change. Its local start
            // stays fixed by TZID and the recurrence rule.
            let seconds = event.ends_at.unix_timestamp() - event.starts_at.unix_timestamp();
            if event.end_date_known && event.end_time_known && seconds > 0 {
                line(&mut out, &format!("DURATION:PT{seconds}S"));
            }
        } else {
            line(&mut out, &format!("DTSTART:{}", utc(event.starts_at)));
            // RFC 5545 requires DTEND strictly after DTSTART. Unknown end is omitted.
            if event.end_date_known
                && event.end_time_known
                && event.ends_at.unix_timestamp() > event.starts_at.unix_timestamp()
            {
                line(&mut out, &format!("DTEND:{}", utc(event.ends_at)));
            }
        }
        if event.repeat_every_14_days {
            line(&mut out, &recurrence_rule(event)?);
        }
        line(
            &mut out,
            if event.cancelled {
                "STATUS:CANCELLED"
            } else {
                "STATUS:CONFIRMED"
            },
        );
        line(&mut out, "TRANSP:TRANSPARENT");
        if event.withdrawn {
            line(&mut out, "SUMMARY:Akce již není v tomto kalendáři");
        } else {
            line(&mut out, &format!("SUMMARY:{}", escape_text(&event.title)));
            line(
                &mut out,
                &format!("LOCATION:{}", escape_text(&event.location)),
            );
            line(
                &mut out,
                &format!(
                    "CATEGORIES:{}",
                    escape_text(category_label(&event.category))
                ),
            );
            let detail_url = format!("{public_url}/kalendar/{}", event.id);
            let mut description = plain_description(&event.description);
            if !event.all_day && !event.start_time_known {
                description.push_str("\nPůvodní web čas začátku neuváděl. Záznam je v kalendáři zobrazen jako celodenní.");
            }
            if !event.all_day && (!event.end_date_known || !event.end_time_known) {
                description.push_str("\nPůvodní web přesný konec neuváděl.");
            }
            description.push_str(&format!("\n\nPodrobnosti: {detail_url}"));
            line(
                &mut out,
                &format!("DESCRIPTION:{}", escape_text(description.trim())),
            );
            line(&mut out, &format!("URL:{detail_url}"));
        }
        line(&mut out, "END:VEVENT");
    }
    line(&mut out, "END:VCALENDAR");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escape_and_folding_preserve_utf8_and_prevent_new_properties() {
        let value = format!(
            "{},;\\\r\nBEGIN:VEVENT\rEND:VEVENT",
            "Žluťoučký🐎".repeat(30)
        );
        let escaped = escape_text(&value);
        let mut out = String::new();
        line(&mut out, &format!("DESCRIPTION:{escaped}"));
        assert!(out.split("\r\n").all(|line| line.len() <= 75));
        assert_eq!(
            out.replace("\r\n ", ""),
            format!("DESCRIPTION:{escaped}\r\n")
        );
        assert!(escaped.ends_with("\\,\\;\\\\\\nBEGIN:VEVENT\\nEND:VEVENT"));
        assert!(!out.contains("\r\nBEGIN:VEVENT"));
    }
}
