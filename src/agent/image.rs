//! Images the `read` tool sends to the model, recognized and fitted as Scramjet's are: PNG, JPEG,
//! GIF and WebP, sent as they are when they fit in 2000x2000 pixels and 4.5MB of base64 (below
//! Anthropic's 5MB per image), else scaled down and encoded again as PNG or JPEG.
use image::{
    DynamicImage, ImageDecoder, ImageFormat, ImageReader,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::FilterType,
};
use std::io::Cursor;

const MAX_SIDE: u32 = 2000;
const MAX_BASE64: usize = 4_718_592;
/// Scramjet's JPEG qualities after its default 80, without the 85 that only helps a lower default.
const JPEG_QUALITIES: [u8; 4] = [80, 70, 55, 40];

#[derive(Debug, PartialEq)]
pub(super) struct Image {
    pub mime: &'static str,
    pub data: String,
}

impl Image {
    /// As a Responses content part.
    pub fn part(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "input_image",
            "detail": "auto",
            "image_url": format!("data:{};base64,{}", self.mime, self.data),
        })
    }
}

#[derive(Debug)]
pub(super) struct Fitted {
    pub image: Image,
    original: (u32, u32),
    sent: (u32, u32),
}

impl Fitted {
    /// Scramjet's note on a scaled image, so that the model can map coordinates back.
    pub fn note(&self) -> Option<String> {
        let ((width, height), (sent_width, sent_height)) = (self.original, self.sent);
        (self.original != self.sent).then(|| {
            format!(
                "[Image: original {width}x{height}, displayed at {sent_width}x{sent_height}. \
Multiply coordinates by {:.2} to map to original image.]",
                f64::from(width) / f64::from(sent_width)
            )
        })
    }
}

/// The type of a supported image from its first bytes, as Scramjet tells it: JPEG but not
/// JPEG-LS, PNG but not animated PNG, GIF and WebP.
pub(super) fn mime(bytes: &[u8]) -> Option<&'static str> {
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return (bytes.get(3) != Some(&0xf7)).then_some("image/jpeg");
    }
    if bytes.starts_with(PNG) {
        let header =
            bytes.len() >= 16 && bytes[8..12] == [0, 0, 0, 13] && &bytes[12..16] == b"IHDR";
        return (header && !animated_png(bytes)).then_some("image/png");
    }
    if bytes.starts_with(b"GIF") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return Some("image/webp");
    }
    None
}

/// Whether an animation control chunk comes before the image data, within the first 4100 bytes
/// Scramjet looks at.
fn animated_png(bytes: &[u8]) -> bool {
    let bytes = &bytes[..bytes.len().min(4100)];
    let mut offset = 8;
    while offset + 8 <= bytes.len() {
        match &bytes[offset + 4..offset + 8] {
            b"acTL" => return true,
            b"IDAT" => return false,
            _ => {}
        }
        let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        match (offset + 12).checked_add(length) {
            Some(next) if next <= bytes.len() => offset = next,
            _ => return false,
        }
    }
    false
}

/// The image of type `mime` in `bytes` as it can be sent; `None` when it does not decode or
/// cannot be made small enough.
pub(super) fn fit(bytes: &[u8], mime: &'static str) -> Option<Fitted> {
    let mut decoder =
        ImageReader::with_format(Cursor::new(bytes), ImageFormat::from_mime_type(mime)?)
            .into_decoder()
            .ok()?;
    let orientation = decoder.orientation().ok()?;
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    let original = (image.width(), image.height());
    if original.0 <= MAX_SIDE && original.1 <= MAX_SIDE && encoded_len(bytes.len()) < MAX_BASE64 {
        let image = Image {
            mime,
            data: base64(bytes),
        };
        return Some(Fitted {
            image,
            original,
            sent: original,
        });
    }
    let scaled = |side: u32, by: u32| {
        ((f64::from(side) * f64::from(MAX_SIDE) / f64::from(by)).round() as u32).max(1)
    };
    let (mut width, mut height) = original;
    if width > MAX_SIDE {
        height = scaled(height, width);
        width = MAX_SIDE;
    }
    if height > MAX_SIDE {
        width = scaled(width, height);
        height = MAX_SIDE;
    }
    loop {
        let resized = image.resize_exact(width, height, FilterType::Lanczos3);
        if let Some(image) = encoded(&resized) {
            return Some(Fitted {
                image,
                original,
                sent: (width, height),
            });
        }
        if (width, height) == (1, 1) {
            return None;
        }
        width = (width * 3 / 4).max(1);
        height = (height * 3 / 4).max(1);
    }
}

/// `image` as PNG, else as JPEG at the first quality that fits.
fn encoded(image: &DynamicImage) -> Option<Image> {
    let fits = |bytes: &[u8]| encoded_len(bytes.len()) < MAX_BASE64;
    let mut png = Vec::new();
    if image.write_with_encoder(PngEncoder::new(&mut png)).is_ok() && fits(&png) {
        return Some(Image {
            mime: "image/png",
            data: base64(&png),
        });
    }
    let rgb = image.to_rgb8();
    JPEG_QUALITIES.iter().find_map(|&quality| {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, quality)
            .encode_image(&rgb)
            .ok()?;
        fits(&jpeg).then(|| Image {
            mime: "image/jpeg",
            data: base64(&jpeg),
        })
    })
}

fn encoded_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(encoded_len(bytes.len()));
    for chunk in bytes.chunks(3) {
        let byte = |i: usize| u32::from(chunk.get(i).copied().unwrap_or(0));
        let bits = byte(0) << 16 | byte(1) << 8 | byte(2);
        for i in 0..4 {
            text.push(match i <= chunk.len() {
                true => ALPHABET[(bits >> (18 - 6 * i)) as usize & 63] as char,
                false => '=',
            });
        }
    }
    text
}

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// What base64 or base64url text, padded or not, holds; `None` for anything else.
pub(super) fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let value = |c: u8| match c {
        b'+' => Some(62),
        b'/' => Some(63),
        _ => BASE64URL
            .iter()
            .position(|known| *known == c)
            .map(|at| at as u32),
    };
    let text = text.trim_end_matches('=').as_bytes();
    let mut bytes = Vec::new();
    for chunk in text.chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let mut bits = 0u32;
        for c in chunk {
            bits = bits << 6 | value(*c)?;
        }
        let bits = bits << (6 * (4 - chunk.len()));
        for index in 0..chunk.len() - 1 {
            bytes.push((bits >> (16 - 8 * index)) as u8);
        }
    }
    Some(bytes)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use image::{ImageEncoder, RgbImage, RgbaImage};

    pub(in crate::agent) fn png(width: u32, height: u32) -> Vec<u8> {
        let image = RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([x as u8, y as u8, 7, 255])
        });
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes)
            .write_image(&image, width, height, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    #[test]
    fn base64_pads_as_the_standard_alphabet_does() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
    }

    #[test]
    fn images_are_told_by_their_first_bytes() {
        assert_eq!(mime(&png(2, 2)), Some("image/png"));
        assert_eq!(mime(b"\xff\xd8\xff\xe0rest"), Some("image/jpeg"));
        assert_eq!(mime(b"\xff\xd8\xff\xf7rest"), None, "JPEG-LS");
        assert_eq!(mime(b"GIF89a"), Some("image/gif"));
        assert_eq!(mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(mime(b"RIFF\0\0\0\0WAVE"), None);
        assert_eq!(mime(b"\x89PNG\r\n\x1a\n"), None, "no header chunk");
        assert_eq!(mime(b"fn main() {}"), None);
        // An animation control chunk before the image data.
        let mut animated = png(2, 2);
        let actl = [&[0, 0, 0, 8][..], b"acTL", &[0; 8], &[0; 4]].concat();
        animated.splice(33..33, actl);
        assert_eq!(mime(&animated), None);
    }

    #[test]
    fn an_image_that_fits_is_sent_as_it_is() {
        let bytes = png(40, 30);
        let fitted = fit(&bytes, "image/png").unwrap();
        assert_eq!(
            fitted.image,
            Image {
                mime: "image/png",
                data: base64(&bytes)
            }
        );
        assert_eq!(fitted.note(), None);
    }

    #[test]
    fn a_larger_image_is_scaled_to_fit_and_says_how_to_map_coordinates() {
        let fitted = fit(&png(2500, 10), "image/png").unwrap();
        assert_eq!(fitted.image.mime, "image/png");
        assert_eq!((fitted.original, fitted.sent), ((2500, 10), (2000, 8)));
        assert_eq!(
            fitted.note().unwrap(),
            "[Image: original 2500x10, displayed at 2000x8. Multiply coordinates by 1.25 to map \
to original image.]"
        );
        let mut decoded = image::load_from_memory(&decode(&fitted.image.data)).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2000, 8));
        // Taller than wide, the height is the one that is cut.
        let fitted = fit(&png(3, 4000), "image/png").unwrap();
        assert_eq!(fitted.sent, (2, 2000));
        decoded = image::load_from_memory(&decode(&fitted.image.data)).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2000));
    }

    #[test]
    fn noise_too_large_as_png_is_sent_as_jpeg() {
        // About 6MB of base64 as PNG: random pixels do not compress.
        let mut state = 1u32;
        let noise = RgbImage::from_fn(1400, 1100, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [r, g, b, _] = state.to_le_bytes();
            image::Rgb([r, g, b])
        });
        let fitted = encoded(&DynamicImage::ImageRgb8(noise)).unwrap();
        assert_eq!(fitted.mime, "image/jpeg");
        assert!(fitted.data.len() < MAX_BASE64);
    }

    #[test]
    fn what_does_not_decode_is_not_sent() {
        assert!(fit(b"\xff\xd8\xff\xe0 not really a jpeg", "image/jpeg").is_none());
    }

    fn decode(text: &str) -> Vec<u8> {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let values: Vec<u32> = (text.bytes().filter(|&b| b != b'='))
            .map(|b| ALPHABET.iter().position(|&a| a == b).unwrap() as u32)
            .collect();
        let mut bytes = Vec::new();
        for chunk in values.chunks(4) {
            let bits = chunk
                .iter()
                .enumerate()
                .fold(0, |bits, (i, v)| bits | v << (18 - 6 * i));
            bytes.extend(&bits.to_be_bytes()[1..chunk.len()]);
        }
        bytes
    }
}
