//! Publication rules are independent of visual categories.
use serde::{Deserialize, Serialize};
use time::{Date, Duration, Month, Weekday};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct NoticeReview {
    pub rule: String,
    pub legal_basis: String,
    pub not_before: Option<Date>,
    pub decision_on: Option<Date>,
    pub original_reference: String,
    pub reviewed: bool,
    pub archive_title: String,
    pub archive_basis: String,
    pub archive_until: Option<Date>,
}

impl NoticeReview {
    /// First day on which a normal withdrawal is allowed.
    pub fn earliest(&self, posted: Date) -> Result<Option<Date>, &'static str> {
        let after = |days| {
            posted
                .checked_add(Duration::days(days))
                .ok_or("Datum je mimo rozsah.")
        };
        if self.not_before.is_some_and(|d| d <= posted) {
            return Err("Nejdřívější sejmutí musí být po vyvěšení.");
        }
        let minimum = match self.rule.as_str() {
            "" | "informational" => None,
            "public_notice" => {
                let mut last = after(15)?;
                while non_working_day(last) {
                    last = last.next_day().ok_or("Datum je mimo rozsah.")?;
                }
                Some(last.next_day().ok_or("Datum je mimo rozsah.")?)
            }
            "property_intent" | "council_meeting" => {
                // Conservative full calendar days, excluding the posting day.
                let minimum = after(if self.rule == "property_intent" {
                    16
                } else {
                    8
                })?;
                let decision = self
                    .decision_on
                    .ok_or("Vyplňte datum projednání nebo zasedání.")?;
                if decision < minimum {
                    return Err("Do projednání nezbývá dost celých dní. Upravte datum jednání.");
                }
                Some(if self.rule == "council_meeting" {
                    decision.next_day().ok_or("Datum je mimo rozsah.")?
                } else {
                    minimum
                })
            }
            "custom" => Some(
                self.not_before
                    .ok_or("Pro jiný právní režim určete nejdřívější sejmutí.")?,
            ),
            _ => return Err("Neznámé pravidlo zveřejnění."),
        };
        Ok(match (minimum, self.not_before) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        })
    }
}

fn non_working_day(date: Date) -> bool {
    if matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday) {
        return true;
    }
    if matches!(
        (date.month() as u8, date.day()),
        (1, 1)
            | (5, 1)
            | (5, 8)
            | (7, 5)
            | (7, 6)
            | (9, 28)
            | (10, 28)
            | (11, 17)
            | (12, 24)
            | (12, 25)
            | (12, 26)
    ) {
        return true;
    }
    // Gregorian Easter, Meeus/Jones/Butcher algorithm.
    let y = date.year();
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let n = h + l - 7 * m + 114;
    let easter = Date::from_calendar_date(
        y,
        Month::try_from((n / 31) as u8).unwrap(),
        (n % 31 + 1) as u8,
    )
    .unwrap();
    date == easter - Duration::days(2) || date == easter + Duration::days(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;
    #[test]
    fn public_notice_includes_last_day_and_rolls_weekends_and_holidays() {
        let rule = NoticeReview {
            rule: "public_notice".into(),
            ..Default::default()
        };
        assert_eq!(
            rule.earliest(date!(2026 - 09 - 14)).unwrap(),
            Some(date!(2026 - 09 - 30))
        );
        // 15th day is Sunday 27 September, Monday is a Czech public holiday.
        assert_eq!(
            rule.earliest(date!(2026 - 09 - 12)).unwrap(),
            Some(date!(2026 - 09 - 30))
        );
        // Good Friday 3 April, followed by the Easter weekend and Monday.
        assert_eq!(
            rule.earliest(date!(2026 - 03 - 19)).unwrap(),
            Some(date!(2026 - 04 - 08))
        );
        assert_eq!(
            rule.earliest(date!(2026 - 12 - 10)).unwrap(),
            Some(date!(2026 - 12 - 29))
        );
    }
    #[test]
    fn meeting_needs_full_days_and_remains_visible_on_meeting_day() {
        let mut rule = NoticeReview {
            rule: "council_meeting".into(),
            decision_on: Some(date!(2026 - 10 - 08)),
            ..Default::default()
        };
        assert!(rule.earliest(date!(2026 - 10 - 01)).is_err());
        rule.decision_on = Some(date!(2026 - 10 - 09));
        assert_eq!(
            rule.earliest(date!(2026 - 10 - 01)).unwrap(),
            Some(date!(2026 - 10 - 10))
        );
    }
}
