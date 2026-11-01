use image::{ImageEncoder, Luma};
use qrcode::{Color, QrCode};

pub(super) fn normalized_iban(iban: &str) -> Result<String, String> {
    let iban: String = iban
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let bytes = iban.as_bytes();
    if bytes.len() < 15
        || bytes.len() > 34
        || !bytes[..2].iter().all(u8::is_ascii_uppercase)
        || !bytes[2..4].iter().all(u8::is_ascii_digit)
        || !bytes.iter().all(u8::is_ascii_alphanumeric)
        || (iban.starts_with("CZ") && bytes.len() != 24)
    {
        return Err("Neplatný IBAN".into());
    }
    let mut remainder = 0_u32;
    for &c in bytes[4..].iter().chain(bytes[..4].iter()) {
        if c.is_ascii_digit() {
            remainder = (remainder * 10 + u32::from(c - b'0')) % 97;
        } else {
            let value = u32::from(c - b'A') + 10;
            remainder = (remainder * 100 + value) % 97;
        }
    }
    if remainder != 1 {
        return Err("Neplatný kontrolní součet IBAN".into());
    }
    Ok(iban)
}

fn payment_data(iban: &str, amount_kc: i64, variable_symbol: i64) -> Result<String, String> {
    let iban = normalized_iban(iban)?;
    if amount_kc <= 0 || variable_symbol <= 0 || variable_symbol > 9_999_999_999 {
        return Err("Neplatná částka nebo variabilní symbol".into());
    }
    Ok(format!(
        "SPD*1.0*ACC:{iban}*AM:{amount_kc}.00*CC:CZK*X-VS:{variable_symbol}"
    ))
}

pub(super) fn qr_png(iban: &str, amount_kc: i64, variable_symbol: i64) -> Result<Vec<u8>, String> {
    let data = payment_data(iban, amount_kc, variable_symbol)?;
    let code = QrCode::new(data.as_bytes()).map_err(|_| "Platební údaje jsou příliš dlouhé")?;
    let quiet = 4_usize;
    let scale = 6_usize;
    let width = code.width();
    let size = ((width + 2 * quiet) * scale) as u32;
    let mut image = image::GrayImage::from_pixel(size, size, Luma([255]));
    for y in 0..width {
        for x in 0..width {
            if code[(x, y)] == Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        image.put_pixel(
                            ((x + quiet) * scale + dx) as u32,
                            ((y + quiet) * scale + dy) as u32,
                            Luma([0]),
                        );
                    }
                }
            }
        }
    }
    let mut output = Vec::new();
    image::codecs::png::PngEncoder::new(&mut output)
        .write_image(image.as_raw(), size, size, image::ExtendedColorType::L8)
        .map_err(|_| "QR kód se nepodařilo vytvořit")?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_payment_and_checks_iban() {
        assert_eq!(
            payment_data("cz65 0800 0000 1920 0014 5399", 1234, 42).unwrap(),
            "SPD*1.0*ACC:CZ6508000000192000145399*AM:1234.00*CC:CZK*X-VS:42"
        );
        assert!(payment_data("CZ6508000000192000145398", 1234, 42).is_err());
        assert!(payment_data("invalid", 1234, 42).is_err());
        assert!(payment_data("CZ6508000000192000145399", 0, 42).is_err());
        assert!(payment_data("CZ6508000000192000145399", 1234, 10_000_000_000).is_err());
    }

    #[test]
    fn generates_a_png_with_a_quiet_border() {
        let png = qr_png("CZ6508000000192000145399", 1234, 42).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_luma8();
        assert!(image.width() > 100);
        assert_eq!(image.get_pixel(0, 0).0, [255]);
        assert!(image.pixels().any(|pixel| pixel.0 == [0]));
    }
}
