use serde::Serialize;
use std::io::{self, Write};

pub(super) const LIMIT: usize = 524288;
const PAGE_BYTES: usize = 512;
pub(super) const LEGEND: &str = "ASCII JSON: concatenate payload bytes WITHOUT gutters/newlines. Decode JSON escapes once; \\uXXXX uses UTF-16, surrogate pairs represent one scalar. Coverage is display, not proof of reading.";

pub(super) struct Document {
    bytes: Vec<u8>,
    page: usize,
    coverage: [u8; 128],
}
struct Encoder {
    bytes: Vec<u8>,
    pending: [u8; 4],
    used: usize,
}
impl Encoder {
    fn push(&mut self, bytes: &[u8]) -> io::Result<()> {
        let capacity = LIMIT - LEGEND.len();
        if bytes.len() > capacity - self.bytes.len() {
            return Err(io::Error::other("presentation resource limit"));
        }
        let needed = self.bytes.len() + bytes.len();
        if needed > self.bytes.capacity() {
            let target = needed
                .max(self.bytes.capacity().saturating_mul(2))
                .min(capacity);
            self.bytes.reserve_exact(target - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn scalar(&mut self, scalar: char) -> io::Result<()> {
        let mut utf16 = [0; 2];
        for unit in scalar.encode_utf16(&mut utf16) {
            let digits = b"0123456789ABCDEF";
            self.push(&[
                b'\\',
                b'u',
                digits[(*unit >> 12) as usize],
                digits[((*unit >> 8) & 15) as usize],
                digits[((*unit >> 4) & 15) as usize],
                digits[(*unit & 15) as usize],
            ])?;
        }
        Ok(())
    }
}
impl Write for Encoder {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for &byte in bytes {
            if self.used == 0 && byte == 0x7f {
                self.push(b"\\u007F")?;
                continue;
            }
            if self.used == 0 && byte.is_ascii() {
                self.push(&[byte])?;
                continue;
            }
            if self.used == self.pending.len() {
                return Err(io::Error::other("invalid serializer UTF8"));
            }
            self.pending[self.used] = byte;
            self.used += 1;
            match std::str::from_utf8(&self.pending[..self.used]) {
                Ok(text) => {
                    let scalar = text
                        .chars()
                        .next()
                        .ok_or_else(|| io::Error::other("empty scalar"))?;
                    self.scalar(scalar)?;
                    self.used = 0;
                }
                Err(error) if error.error_len().is_none() => {}
                Err(_) => return Err(io::Error::other("invalid serializer UTF8")),
            }
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Document {
    pub fn new(value: &impl Serialize) -> Result<Self, &'static str> {
        let mut encoder = Encoder {
            bytes: Vec::new(),
            pending: [0; 4],
            used: 0,
        };
        serde_json::to_writer(&mut encoder, value)
            .map_err(|_| "presentation resource limit or serialization failure")?;
        if encoder.used != 0 || encoder.bytes.is_empty() {
            return Err("incomplete presentation serialization");
        }
        Ok(Self {
            bytes: encoder.bytes,
            page: 0,
            coverage: [0; 128],
        })
    }
    pub fn pages(&self) -> usize {
        self.bytes.len().div_ceil(PAGE_BYTES)
    }
    pub fn page(&self) -> usize {
        self.page
    }
    pub fn payload(&self) -> &[u8] {
        &self.bytes[self.page * PAGE_BYTES..((self.page + 1) * PAGE_BYTES).min(self.bytes.len())]
    }
    pub fn next(&mut self) {
        self.page = (self.page + 1).min(self.pages() - 1);
    }
    pub fn previous(&mut self) {
        self.page = self.page.saturating_sub(1);
    }
    pub fn painted(&mut self) {
        self.coverage[self.page / 8] |= 1 << (self.page % 8);
    }
    pub fn covered(&self) -> bool {
        (0..self.pages()).all(|page| self.coverage[page / 8] & (1 << (page % 8)) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn complete_ascii_json_round_trips_and_covers_more_than_64_pages() {
        let value = json!({"opaque": "\"\\\0\r\n\t界😀".repeat(4000), "optional": null});
        let mut document = Document::new(&value).unwrap();
        assert!(document.bytes.is_ascii());
        assert!(document.pages() > 64);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&document.bytes).unwrap(),
            value
        );
        let mut joined = Vec::new();
        for page in 0..document.pages() {
            assert_eq!(document.page(), page);
            joined.extend_from_slice(document.payload());
            assert!(!document.covered());
            document.painted();
            document.next();
        }
        assert!(document.covered());
        assert_eq!(joined, document.bytes);
        for _ in 1..document.pages() {
            document.previous();
        }
        assert_eq!(document.page(), 0);
        assert!(document.covered());
    }
    #[test]
    fn byte_split_unicode_and_existing_json_escapes_are_not_double_encoded() {
        let text = "\"\\\0\r\n界😀\u{7f}";
        let raw = serde_json::to_vec(text).unwrap();
        let mut writer = Encoder {
            bytes: Vec::new(),
            pending: [0; 4],
            used: 0,
        };
        for byte in raw {
            writer.write_all(&[byte]).unwrap();
        }
        assert_eq!(writer.used, 0);
        assert_eq!(
            serde_json::from_slice::<String>(&writer.bytes).unwrap(),
            text
        );
        assert_eq!(
            std::str::from_utf8(&writer.bytes).unwrap(),
            "\"\\\"\\\\\\u0000\\r\\n\\u754C\\uD83D\\uDE00\\u007F\""
        );
    }
    #[test]
    fn maximum_admitted_unicode_record_fits_and_excess_is_refused_intact() {
        let value = "😀".repeat(32767);
        assert!(serde_json::to_vec(&value).unwrap().len() <= 131072);
        let document = Document::new(&value).unwrap();
        assert!(document.bytes.len() <= 393216);
        assert!(document.pages() <= 1024);
        let oversized = "\0".repeat(LIMIT / 6);
        assert!(Document::new(&oversized).is_err());
        assert_eq!(oversized.len(), LIMIT / 6);
        let del = "\u{7f}".repeat(131000);
        assert!(serde_json::to_vec(&del).unwrap().len() < 131072);
        assert!(Document::new(&del).is_err());
        assert_eq!(del.len(), 131000);
    }
}
