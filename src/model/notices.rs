//! Úřední deska — datové typy a dotazy.

use sqlx::{FromRow, PgPool};
use time::{Date, OffsetDateTime};

/// Stav vyvěšení. Ukládá se jako TEXT, aby šel obsah databáze přečíst očima.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "lowercase")]
pub enum NoticeStatus {
    /// Rozepsané, na desce není.
    Draft,
    /// Má nastavený den vyvěšení v budoucnu.
    Scheduled,
    /// Visí.
    Published,
    /// Sejmuto, ještě neuklizeno denní úlohou.
    Withdrawn,
    /// Sejmuto a uklizeno. Záznam zůstává dohledatelný vždy.
    Archived,
}

impl NoticeStatus {
    pub fn is_published(self) -> bool {
        matches!(self, NoticeStatus::Published)
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct Notice {
    pub id: i64,
    pub title: String,
    pub reference_number: Option<String>,
    pub category_name: Option<String>,
    pub issuer: Option<String>,
    pub description: Option<String>,
    pub published_on: Option<Date>,
    pub withdraw_on: Option<Date>,
    pub withdrawn_at: Option<OffsetDateTime>,
    pub retain_attachments: bool,
    pub status: NoticeStatus,
}

#[derive(Debug, Clone, FromRow)]
pub struct Attachment {
    pub id: i64,
    pub name: String,
    pub size_bytes: i64,
    pub has_content: bool,
    /// Vyplněné znamená, že soubor byl po sejmutí odstraněn a zůstala jen zmínka.
    pub removed_at: Option<OffsetDateTime>,
}

impl Attachment {
    pub fn available(&self) -> bool {
        self.removed_at.is_none()
    }
}

#[derive(Debug, Clone, Copy, Default, FromRow)]
pub struct NoticeCounts {
    pub published: i64,
    pub withdrawn_this_year: i64,
    pub archived: i64,
}

/// Kolik z doby vyvěšení uplynulo. Počítá se za běhu, do databáze nepatří —
/// jinak by bylo potřeba ji každý den přepisovat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicationPeriod {
    pub total_days: i64,
    pub elapsed_days: i64,
    pub remaining_days: i64,
    /// 0–100. Useknuté do rozsahu, aby proužek nepřetekl.
    pub progress_percent: i64,
}

impl PublicationPeriod {
    /// Poslední tři dny se lhůta zvýrazňuje, ať si jí úřad i občan všimnou.
    pub fn ends_soon(&self) -> bool {
        self.remaining_days <= 3
    }
}

/// `None` = dokument visí bez časového omezení.
pub fn publication_period(
    published_on: Date,
    withdraw_on: Option<Date>,
    current_date: Date,
) -> Option<PublicationPeriod> {
    let withdraw_on = withdraw_on?;

    let total_days = (withdraw_on - published_on).whole_days();
    let elapsed_days = (current_date - published_on)
        .whole_days()
        .clamp(0, total_days.max(0));
    let remaining_days = (total_days - elapsed_days).max(0);

    // Jednodenní vyvěšení by jinak dělilo nulou.
    let progress_percent = if total_days <= 0 {
        100
    } else {
        (elapsed_days * 100 / total_days).clamp(0, 100)
    };

    Some(PublicationPeriod {
        total_days,
        elapsed_days,
        remaining_days,
        progress_percent,
    })
}

// The compact board projection is independent of planned removal dates.
// Actual, documented posting/removal evidence determines membership.
pub const CURRENT: &str = "d.board_status='published' AND d.has_current_posting";
pub const ARCHIVED: &str = "d.board_status IN ('withdrawn','archived') AND d.has_removed_posting";
const PROJECTION: &str = "d.id,CASE WHEN d.metadata_restricted THEN coalesce(d.public_title,'Záznam úřední desky #'||d.id) ELSE d.title END AS title,CASE WHEN d.metadata_restricted THEN NULL ELSE d.reference_number END AS reference_number,c.name AS category_name,CASE WHEN d.metadata_restricted THEN NULL ELSE d.issuer END AS issuer,CASE WHEN d.metadata_restricted THEN NULL ELSE d.description END AS description,d.published_on,d.withdraw_on,d.withdrawn_at::timestamptz AS withdrawn_at,document_files_public(d,(now() AT TIME ZONE 'Europe/Prague')::date) AS retain_attachments,CASE WHEN d.board_status='published' THEN 'published' ELSE 'archived' END AS status";

async fn list(pool: &PgPool, archived: bool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    let predicate = if archived { ARCHIVED } else { CURRENT };
    let query = format!(
        "SELECT {PROJECTION} FROM documents d LEFT JOIN categories c ON c.id=d.category_id WHERE document_is_public(d,$1) AND ({predicate}) ORDER BY d.published_on DESC NULLS LAST,d.id DESC LIMIT $2"
    );
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(query);
    query
        .build_query_as()
        .bind(crate::backend::today(OffsetDateTime::now_utc()))
        .bind(limit.clamp(0, 100))
        .fetch_all(pool)
        .await
}
pub async fn published(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    list(pool, false, limit).await
}
pub async fn archived(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    list(pool, true, limit).await
}
pub async fn by_id(pool: &PgPool, id: i64) -> sqlx::Result<Option<Notice>> {
    Ok(by_ids(pool, &[id]).await?.into_iter().next())
}
pub async fn by_ids(pool: &PgPool, ids: &[i64]) -> sqlx::Result<Vec<Notice>> {
    let query = format!(
        "SELECT {PROJECTION} FROM documents d LEFT JOIN categories c ON c.id=d.category_id WHERE d.id=ANY($1) AND d.board_status IS NOT NULL AND (d.has_current_posting OR d.has_removed_posting) AND document_is_public(d,$2)"
    );
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(query);
    query
        .build_query_as()
        .bind(ids)
        .bind(crate::backend::today(OffsetDateTime::now_utc()))
        .fetch_all(pool)
        .await
}
pub async fn attachments(pool: &PgPool, id: i64) -> sqlx::Result<Vec<Attachment>> {
    let files = crate::backend::documents::public_notice_files(pool, id, OffsetDateTime::now_utc())
        .await
        .map_err(|e| sqlx::Error::Protocol(e.1.into()))?;
    Ok(files
        .into_iter()
        .map(|f| Attachment {
            id: f.id,
            name: f.name,
            size_bytes: f.size_bytes,
            has_content: f.available,
            removed_at: f.removed_at.and_then(|v| {
                OffsetDateTime::parse(&v, &time::format_description::well_known::Rfc3339).ok()
            }),
        })
        .collect())
}
pub async fn counts(pool: &PgPool) -> sqlx::Result<NoticeCounts> {
    let query = format!(
        "SELECT count(*) FILTER(WHERE {CURRENT}) AS published,count(*) FILTER(WHERE ({ARCHIVED}) AND extract(year FROM d.withdrawn_at::timestamptz)=extract(year FROM $1::date)) AS withdrawn_this_year,count(*) FILTER(WHERE {ARCHIVED}) AS archived FROM documents d WHERE document_is_public(d,$1) AND d.board_status IS NOT NULL"
    );
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(query);
    query
        .build_query_as()
        .bind(crate::backend::today(OffsetDateTime::now_utc()))
        .fetch_one(pool)
        .await
}
pub async fn next_withdrawal(pool: &PgPool) -> sqlx::Result<Option<Date>> {
    let query = format!(
        "SELECT min(d.withdraw_on) FROM documents d WHERE document_is_public(d,$1) AND ({CURRENT}) AND d.withdraw_on>$1"
    );
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(query);
    query
        .build_query_scalar()
        .bind(crate::backend::today(OffsetDateTime::now_utc()))
        .fetch_one(pool)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn no_withdrawal_date_has_no_period() {
        assert_eq!(
            publication_period(date!(2026 - 09 - 01), None, date!(2026 - 09 - 10)),
            None
        );
    }

    #[test]
    fn publication_day_starts_at_zero() {
        let l = publication_period(
            date!(2026 - 09 - 24),
            Some(date!(2026 - 10 - 09)),
            date!(2026 - 09 - 24),
        )
        .unwrap();
        assert_eq!(
            (
                l.total_days,
                l.elapsed_days,
                l.remaining_days,
                l.progress_percent
            ),
            (15, 0, 15, 0)
        );
        assert!(!l.ends_soon());
    }

    #[test]
    fn middle_of_publication_period() {
        let l = publication_period(
            date!(2026 - 09 - 24),
            Some(date!(2026 - 10 - 09)),
            date!(2026 - 09 - 28),
        )
        .unwrap();
        assert_eq!(
            (l.elapsed_days, l.remaining_days, l.progress_percent),
            (4, 11, 26)
        );
    }

    #[test]
    fn last_three_days_are_highlighted() {
        let l = publication_period(
            date!(2026 - 09 - 19),
            Some(date!(2026 - 10 - 01)),
            date!(2026 - 09 - 28),
        )
        .unwrap();
        assert_eq!((l.elapsed_days, l.remaining_days), (9, 3));
        assert!(l.ends_soon());
    }

    #[test]
    fn elapsed_period_does_not_exceed_one_hundred_percent() {
        let l = publication_period(
            date!(2026 - 09 - 01),
            Some(date!(2026 - 09 - 16)),
            date!(2026 - 10 - 20),
        )
        .unwrap();
        assert_eq!(
            (l.elapsed_days, l.remaining_days, l.progress_percent),
            (15, 0, 100)
        );
    }

    #[test]
    fn same_day_period_does_not_divide_by_zero() {
        let l = publication_period(
            date!(2026 - 09 - 10),
            Some(date!(2026 - 09 - 10)),
            date!(2026 - 09 - 10),
        )
        .unwrap();
        assert_eq!((l.total_days, l.progress_percent), (0, 100));
    }

    #[test]
    fn date_before_publication_does_not_produce_negative_progress() {
        let l = publication_period(
            date!(2026 - 10 - 01),
            Some(date!(2026 - 10 - 16)),
            date!(2026 - 09 - 28),
        )
        .unwrap();
        assert_eq!((l.elapsed_days, l.progress_percent), (0, 0));
    }
}
