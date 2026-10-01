//! Úřední deska — datové typy a dotazy.

use sqlx::{FromRow, PgPool};
use time::{Date, OffsetDateTime};

/// Stav vyvěšení. Ukládá se jako TEXT, aby šel obsah databáze přečíst očima.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "lowercase")]
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

async fn public_row(
    pool: &PgPool,
    record: crate::backend::notices::NoticeRecord,
) -> sqlx::Result<Notice> {
    let record = record.into_public(OffsetDateTime::now_utc());
    let category_name: Option<String> =
        sqlx::query_scalar("SELECT name FROM categories WHERE id=$1")
            .bind(record.category_id)
            .fetch_optional(pool)
            .await?;
    Ok(Notice {
        id: record.id,
        title: record.title,
        reference_number: record.reference_number,
        category_name,
        issuer: record.issuer,
        description: record.description,
        published_on: record.published_on,
        withdraw_on: record.withdraw_on,
        withdrawn_at: record.withdrawn_at.and_then(|v| {
            OffsetDateTime::parse(&v, &time::format_description::well_known::Rfc3339).ok()
        }),
        retain_attachments: record.retain_attachments,
        status: if record.status == "published" {
            NoticeStatus::Published
        } else {
            NoticeStatus::Archived
        },
    })
}
async fn list(pool: &PgPool, archived: bool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    let day = crate::backend::today(OffsetDateTime::now_utc());
    let records:Vec<crate::backend::notices::NoticeRecord>=sqlx::query_as("SELECT * FROM notices WHERE ($1=FALSE AND status='published' AND (published_on IS NULL OR published_on<=$2) AND (withdraw_on IS NULL OR withdraw_on>$3)) OR ($4=TRUE AND (status IN ('withdrawn','archived') OR (status='published' AND withdraw_on<=$5))) ORDER BY published_on DESC NULLS LAST,id DESC LIMIT $6")
        .bind(archived).bind(day).bind(day).bind(archived).bind(day).bind(limit).fetch_all(pool).await?;
    let mut result = Vec::with_capacity(records.len());
    for row in records {
        result.push(public_row(pool, row).await?);
    }
    Ok(result)
}
pub async fn published(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    list(pool, false, limit).await
}
pub async fn archived(pool: &PgPool, limit: i64) -> sqlx::Result<Vec<Notice>> {
    list(pool, true, limit).await
}
pub async fn by_id(pool: &PgPool, id: i64) -> sqlx::Result<Option<Notice>> {
    let record=sqlx::query_as("SELECT * FROM notices WHERE id=$1 AND status IN ('published','archived','withdrawn') AND (status='archived' OR published_on IS NULL OR published_on<=$2)").bind(id).bind(crate::backend::today(OffsetDateTime::now_utc())).fetch_optional(pool).await?;
    match record {
        Some(row) => Ok(Some(public_row(pool, row).await?)),
        None => Ok(None),
    }
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
    let day = crate::backend::today(OffsetDateTime::now_utc());
    sqlx::query_as("SELECT count(*) FILTER(WHERE status='published' AND (withdraw_on IS NULL OR withdraw_on>$1)) AS published,count(*) FILTER(WHERE status IN ('archived','withdrawn') AND extract(year FROM withdrawn_at::timestamptz)=extract(year FROM $2::date)) AS withdrawn_this_year,count(*) FILTER(WHERE status IN ('archived','withdrawn') OR (status='published' AND withdraw_on<=$3)) AS archived FROM notices")
        .bind(day).bind(day).bind(day).fetch_one(pool).await
}
pub async fn next_withdrawal(pool: &PgPool) -> sqlx::Result<Option<Date>> {
    sqlx::query_scalar(
        "SELECT min(withdraw_on) FROM notices WHERE status='published' AND withdraw_on>$1",
    )
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
