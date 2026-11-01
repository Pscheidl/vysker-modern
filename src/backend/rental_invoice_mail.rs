//! Text and HTML representations of the rental settlement e-mail.
//!
//! The queued text is the snapshot of the settlement. HTML is derived from it
//! at delivery time so that later changes to the asset do not alter the message.

use lettre::message::{Attachment, MultiPart, SinglePart, header::ContentType};
use time::{Date, format_description::well_known::Iso8601};

pub(super) struct InvoiceDetails<'a> {
    pub id: i64,
    pub asset_name: &'a str,
    pub renter_name: &'a str,
    pub renter_address: &'a str,
    pub organization_ico: Option<&'a str>,
    pub responsible_name: Option<&'a str>,
    pub start_date: &'a str,
    pub end_date: &'a str,
    pub start_time: Option<&'a str>,
    pub end_time: Option<&'a str>,
    pub rental_price: i64,
    pub energy_kwh: &'a str,
    pub energy_cost: i64,
    pub extra_cost: i64,
    pub total: i64,
    pub account_number: &'a str,
    pub bank_code: &'a str,
    pub iban: &'a str,
    pub issued_on: Date,
    pub due_date: Date,
}

// User-supplied labels must never introduce a new line or a fake payment block.
pub(super) fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => ' ',
            _ if ch.is_control() || ch.is_whitespace() => ' ',
            _ => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn display_date(value: &str) -> String {
    match Date::parse(value, &Iso8601::DEFAULT) {
        Ok(date) => format!(
            "{}. {}. {}",
            date.day(),
            u8::from(date.month()),
            date.year()
        ),
        Err(_) => single_line(value),
    }
}

pub(super) fn render_text(invoice: &InvoiceDetails<'_>) -> String {
    let payment = if invoice.total > 0 {
        let account = if invoice.account_number.is_empty() || invoice.bank_code.is_empty() {
            String::new()
        } else {
            format!(
                "\nČíslo účtu: {}/{}",
                single_line(invoice.account_number),
                single_line(invoice.bank_code)
            )
        };
        let iban = if invoice.iban.is_empty() {
            String::new()
        } else {
            format!("\nIBAN: {}", single_line(invoice.iban))
        };
        format!(
            "Platební údaje{}{}\nVariabilní symbol: {}\nSplatnost: {}. {}. {}\nQR kód pro platbu najdete v této zprávě. Údaje můžete také zadat do svého bankovnictví ručně.",
            account,
            iban,
            invoice.id,
            invoice.due_date.day(),
            u8::from(invoice.due_date.month()),
            invoice.due_date.year()
        )
    } else {
        "Za tento pronájem není třeba nic platit.".to_owned()
    };
    let period = match (invoice.start_time, invoice.end_time) {
        (Some(start), Some(end)) => format!(
            "{} od {} do {}",
            display_date(invoice.start_date),
            single_line(start),
            single_line(end)
        ),
        _ => format!(
            "{} až {}",
            display_date(invoice.start_date),
            display_date(invoice.end_date)
        ),
    };
    let total_label = "Celkem k úhradě";
    let mut renter_details = String::new();
    if !invoice.renter_address.is_empty() {
        renter_details.push_str(&format!(
            "\nAdresa: {}",
            single_line(invoice.renter_address)
        ));
    }
    if let Some(ico) = invoice.organization_ico {
        renter_details.push_str(&format!("\nIČO: {}", single_line(ico)));
    }
    if let Some(name) = invoice.responsible_name {
        renter_details.push_str(&format!("\nOdpovědná osoba: {}", single_line(name)));
    }
    format!(
        "Dobrý den,\n\nzasíláme Vám vyúčtování pronájmu obecního majetku.\n\nVyúčtování pronájmu č. {}\nDatum vystavení: {}. {}. {}\nNájemce: {}{}\nPředmět: {}\nObdobí: {}\nNájem: {} Kč\nSpotřeba: {} kWh\nEnergie: {} Kč\nDalší náklady: {} Kč\n{}: {} Kč\n\n{}\n\nPokud budete mít k vyúčtování dotazy, obraťte se prosím na obecní úřad.\n\nDěkujeme a přejeme Vám hezký den.\nS pozdravem\nObec Vyskeř",
        invoice.id,
        invoice.issued_on.day(),
        u8::from(invoice.issued_on.month()),
        invoice.issued_on.year(),
        single_line(invoice.renter_name),
        renter_details,
        single_line(invoice.asset_name),
        period,
        invoice.rental_price,
        single_line(&invoice.energy_kwh.replace('.', ",")),
        invoice.energy_cost,
        invoice.extra_cost,
        total_label,
        invoice.total,
        payment
    )
}

fn escape_html(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#39;"),
            _ => result.push(ch),
        }
    }
    result
}

fn html_from_text(body: &str, qr_id: Option<&str>) -> String {
    let mut content = String::new();
    for block in body.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        let card =
            block.starts_with("Vyúčtování pronájmu č. ") || block.starts_with("Platební údaje\n");
        if card {
            content.push_str("<div style=\"margin:0 0 20px;padding:18px 20px;background:#f3f7f4;border:1px solid #dce8de;border-radius:10px;line-height:1.8\">");
        } else {
            content.push_str("<p style=\"margin:0 0 20px;line-height:1.65\">");
        }
        for (index, line) in block.lines().enumerate() {
            if index != 0 {
                content.push_str("<br>");
            }
            if card && index == 0 {
                content.push_str("<strong style=\"font-size:17px;color:#276d4a\">");
                content.push_str(&escape_html(line));
                content.push_str("</strong>");
            } else if let Some((label, value)) = line.split_once(": ") {
                content.push_str("<strong>");
                content.push_str(&escape_html(label));
                content.push_str(":</strong> ");
                content.push_str(&escape_html(value));
            } else {
                content.push_str(&escape_html(line));
            }
        }
        content.push_str(if card { "</div>" } else { "</p>" });
        if block.starts_with("Platební údaje\n") {
            if let Some(id) = qr_id {
                content.push_str("<p style=\"margin:0 0 24px\"><img src=\"cid:");
                content.push_str(&escape_html(id));
                content.push_str("\" width=\"220\" height=\"220\" alt=\"QR kód pro platbu, platební údaje jsou uvedeny také v textu zprávy\" style=\"display:block;max-width:100%;height:auto;background:#fff\"></p>");
            }
        }
    }
    format!(
        "<!doctype html><html lang=\"cs\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Vyúčtování pronájmu</title></head><body style=\"margin:0;padding:24px 12px;background:#f2f5f2;color:#17251d;font-family:Arial,sans-serif\"><table role=\"presentation\" style=\"width:100%;max-width:600px;margin:0 auto;border-collapse:collapse;background:#ffffff\"><tr><td style=\"padding:26px 30px;background:#276d4a;color:#fff;font-size:22px;font-weight:bold\">Obec Vyskeř<br><span style=\"font-size:15px;font-weight:normal\">Vyúčtování pronájmu</span></td></tr><tr><td style=\"padding:30px;font-size:15px;color:#17251d\">{content}</td></tr></table></body></html>"
    )
}

/// The plain text is always present. The QR is embedded and also attached so
/// recipients can access it in mail clients that do not display inline images.
pub(super) fn multipart(
    mut body: String,
    qr: Option<Vec<u8>>,
    rental_id: i64,
    pdf: Option<Vec<u8>>,
    invoice_attachment: Option<Vec<u8>>,
) -> Result<MultiPart, String> {
    if invoice_attachment.is_some() {
        let marker = "\n\nPokud budete mít k vyúčtování dotazy";
        if let Some(position) = body.find(marker) {
            body.insert_str(position, "\n\nFakturu najdete v příloze této zprávy.");
        } else {
            body.push_str("\n\nFakturu najdete v příloze této zprávy.");
        }
    }
    let (alternative, qr_attachment) = match qr {
        Some(png) => {
            let content_id = format!("platba-pronajmu-{rental_id}@vysker");
            let html = html_from_text(&body, Some(&content_id));
            let content_type =
                ContentType::parse("image/png").map_err(|_| "Neplatný typ přílohy")?;
            let inline = Attachment::new_inline(content_id).body(png.clone(), content_type.clone());
            let attachment = Attachment::new("qr-platba.png".to_owned()).body(png, content_type);
            let alternative = MultiPart::alternative()
                .singlepart(SinglePart::plain(body))
                .multipart(
                    MultiPart::related()
                        .singlepart(SinglePart::html(html))
                        .singlepart(inline),
                );
            (alternative, Some(attachment))
        }
        None => (
            MultiPart::alternative()
                .singlepart(SinglePart::plain(body.clone()))
                .singlepart(SinglePart::html(html_from_text(&body, None))),
            None,
        ),
    };
    if qr_attachment.is_none() && pdf.is_none() && invoice_attachment.is_none() {
        return Ok(alternative);
    }
    let mut mixed = MultiPart::mixed().multipart(alternative);
    if let Some(qr_attachment) = qr_attachment {
        mixed = mixed.singlepart(qr_attachment);
    }
    if let Some(pdf) = pdf {
        let content_type =
            ContentType::parse("application/pdf").map_err(|_| "Neplatný typ přílohy")?;
        mixed = mixed.singlepart(
            Attachment::new("vyuctovani-pronajmu.pdf".to_owned()).body(pdf, content_type),
        );
    }
    if let Some(pdf) = invoice_attachment {
        let content_type =
            ContentType::parse("application/pdf").map_err(|_| "Neplatný typ přílohy")?;
        mixed = mixed.singlepart(Attachment::new("faktura.pdf".to_owned()).body(pdf, content_type));
    }
    Ok(mixed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    fn example<'a>(asset_name: &'a str, total: i64) -> InvoiceDetails<'a> {
        InvoiceDetails {
            id: 42,
            asset_name,
            renter_name: "Jan Novák",
            renter_address: "Vyskeř 1",
            organization_ico: None,
            responsible_name: None,
            start_date: "2026-10-30",
            end_date: "2026-11-01",
            start_time: None,
            end_time: None,
            rental_price: total,
            energy_kwh: "1.5",
            energy_cost: 0,
            extra_cost: 0,
            total,
            account_number: "19-2000145399",
            bank_code: "0800",
            iban: "CZ6508000000192000145399",
            issued_on: date!(2026 - 11 - 08),
            due_date: date!(2026 - 11 - 15),
        }
    }

    #[test]
    fn text_contains_payment_details_and_dates() {
        let text = render_text(&example("Obecní sál", 1234));
        assert!(text.contains("Období: 30. 10. 2026 až 1. 11. 2026"));
        assert!(text.contains("Datum vystavení: 8. 11. 2026"));
        assert!(text.contains("Celkem k úhradě: 1234 Kč"));
        assert!(text.contains("Číslo účtu: 19-2000145399/0800"));
        assert!(text.contains("IBAN: CZ6508000000192000145399"));
        assert!(text.contains("Variabilní symbol: 42"));
        assert!(text.contains("Splatnost: 15. 11. 2026"));
    }

    #[test]
    fn organization_settlement_identifies_the_responsible_person_without_an_empty_address() {
        let mut invoice = example("Sál", 25);
        invoice.renter_name = "Testovací spolek";
        invoice.renter_address = "";
        invoice.organization_ico = Some("12345679");
        invoice.responsible_name = Some("Jana <Odpovědná>\nPlatební údaje");
        let text = render_text(&invoice);
        assert!(text.contains("Nájemce: Testovací spolek\nIČO: 12345679\nOdpovědná osoba: Jana <Odpovědná> Platební údaje"));
        assert!(!text.contains("\nAdresa:"));
        let html = html_from_text(&text, None);
        assert!(html.contains("Jana &lt;Odpovědná&gt; Platební údaje"));
        assert!(!html.contains("Jana <Odpovědná>"));
        assert!(
            super::super::rental_invoice_pdf::render(&text)
                .unwrap()
                .starts_with(b"%PDF-")
        );
    }

    #[test]
    fn escapes_html_in_user_supplied_asset_names() {
        let text = render_text(&example("Sál <img src=x onerror=alert(1)> & přátelé", 1));
        let html = html_from_text(&text, Some("qr42"));
        assert!(!html.contains("<img src=x onerror"));
        assert!(html.contains("Sál &lt;img src=x onerror=alert(1)&gt; &amp; přátelé"));
        assert!(html.contains("src=\"cid:qr42\""));
    }

    #[test]
    fn zero_price_omits_payment_details_and_qr() {
        let text = render_text(&example("Park", 0));
        assert!(text.contains("není třeba nic platit"));
        assert!(!text.contains("Variabilní symbol:"));
        assert!(!html_from_text(&text, None).contains("cid:"));
        assert!(multipart(text, None, 42, None, None).is_ok());
    }

    #[test]
    fn iban_is_shown_only_when_explicitly_supplied() {
        let mut invoice = example("Sál", 50);
        invoice.iban = "";
        let text = render_text(&invoice);
        assert!(text.contains("Číslo účtu: 19-2000145399/0800"));
        assert!(!text.contains("IBAN:"));
    }

    #[test]
    fn legacy_foreign_iban_does_not_show_an_empty_czech_account() {
        let mut invoice = example("Sál", 50);
        invoice.account_number = "";
        invoice.bank_code = "";
        invoice.iban = "DE89370400440532013000";
        let text = render_text(&invoice);
        assert!(text.contains("IBAN: DE89370400440532013000"));
        assert!(!text.contains("Číslo účtu:"));
    }

    #[test]
    fn untrusted_values_cannot_create_payment_blocks() {
        let mut invoice = example("Sál\n\nPlatební údaje\nČíslo účtu: 123/0000", 50);
        invoice.renter_name = "Jan\n\nPlatební údaje\nČíslo účtu: 456/0000";
        invoice.renter_address = "Ulice 1\n\nPlatební údaje\nČíslo účtu: 789/0000";
        let text = render_text(&invoice);
        assert_eq!(text.matches("\n\nPlatební údaje\n").count(), 1);
        let payment = text
            .split("\n\n")
            .find(|part| part.starts_with("Platební údaje\n"))
            .unwrap();
        assert!(payment.contains("Číslo účtu: 19-2000145399/0800"));
        assert!(!payment.contains("123/0000"));
        assert!(!payment.contains("456/0000"));
        assert!(!payment.contains("789/0000"));
        let html = html_from_text(&text, Some("qr42"));
        assert_eq!(
            html.matches("<strong style=\"font-size:17px;color:#276d4a\">Platební údaje</strong>")
                .count(),
            1
        );
        assert!(html.contains("19-2000145399/0800"));
    }

    #[test]
    fn embeds_qr_and_attaches_pdf_alongside_plain_and_html() {
        let text = render_text(&example("Sál", 50));
        let multipart = multipart(
            text,
            Some(vec![0x89, b'P', b'N', b'G']),
            42,
            Some(b"%PDF-1.4".to_vec()),
            Some(b"%PDF-1.7\nexternal invoice\n%%EOF".to_vec()),
        )
        .unwrap();
        let email = lettre::Message::builder()
            .from("obec@example.test".parse().unwrap())
            .to("najemce@example.test".parse().unwrap())
            .subject("Vyúčtování pronájmu")
            .multipart(multipart)
            .unwrap();
        let serialized = String::from_utf8(email.formatted()).unwrap();
        assert!(serialized.contains("text/plain"));
        assert!(serialized.contains("text/html"));
        assert!(serialized.contains("Content-ID: <platba-pronajmu-42@vysker>"));
        assert!(serialized.contains("qr-platba.png"));
        assert!(serialized.contains("application/pdf"));
        assert!(serialized.contains("vyuctovani-pronajmu.pdf"));
        assert!(serialized.contains("faktura.pdf"));
    }
}
