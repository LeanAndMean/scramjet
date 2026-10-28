use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Event {
    Text(String),
    Paste(String),
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Tab,
    Follow,
    CancelAll,
    NextChild,
    CancelChild,
    Approve,
    Deny,
    ApprovalNext,
    ApprovalPrevious,
    HistoryPrevious,
    HistoryJourney,
    Copy,
    Submit,
    Quit,
    Focus(bool),
    Mouse {
        button: u16,
        x: u16,
        y: u16,
        release: bool,
    },
    Unknown,
}

pub(super) struct Decoder {
    escape: Vec<u8>,
    utf8: Vec<u8>,
    paste: Option<Vec<u8>>,
    started: Option<Instant>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self {
            escape: Vec::with_capacity(128),
            utf8: Vec::with_capacity(4),
            paste: None,
            started: None,
        }
    }
}

impl Decoder {
    pub fn idle(&self) -> bool {
        self.escape.is_empty() && self.utf8.is_empty() && self.paste.is_none()
    }

    pub fn check_deadline(&self, now: Instant) -> Result<(), &'static str> {
        if self
            .started
            .is_some_and(|start| now.duration_since(start) >= DEADLINE)
        {
            Err("incomplete input exceeded two-second deadline")
        } else {
            Ok(())
        }
    }

    pub fn feed(&mut self, byte: u8, now: Instant) -> Result<Option<Event>, &'static str> {
        self.check_deadline(now)?;
        if let Some(paste) = &mut self.paste {
            paste.push(byte);
            if paste.ends_with(b"\x1b[201~") {
                paste.truncate(paste.len() - 6);
                if paste.len() > 8192 {
                    return Err("paste exceeds 8192 raw bytes");
                }
                let text = String::from_utf8(self.paste.take().unwrap())
                    .map_err(|_| "invalid UTF-8 in paste")?;
                self.started = None;
                return Ok(Some(Event::Paste(text)));
            }
            if paste.len() > 8192 + 5 {
                return Err("paste exceeds 8192 raw bytes");
            }
            return Ok(None);
        }
        if !self.escape.is_empty() {
            if self.escape.len() == 128 {
                return Err("escape sequence exceeds 128 bytes");
            }
            self.escape.push(byte);
            let complete = match self.escape.get(1) {
                Some(b'[') => self.escape.len() > 2 && (0x40..=0x7e).contains(&byte),
                Some(b'O') => self.escape.len() == 3,
                _ => true,
            };
            if !complete {
                return Ok(None);
            }
            if self.escape == b"\x1b[200~" {
                self.escape.clear();
                self.paste = Some(Vec::with_capacity(8198));
                return Ok(None);
            }
            let event = sequence(&self.escape);
            self.escape.clear();
            self.started = None;
            return Ok(Some(event));
        }
        if !self.utf8.is_empty() || byte >= 0x80 {
            if self.utf8.is_empty() {
                self.started = Some(now);
            }
            self.utf8.push(byte);
            match std::str::from_utf8(&self.utf8) {
                Ok(text) => {
                    let event = Event::Text(text.to_owned());
                    self.utf8.clear();
                    self.started = None;
                    return Ok(Some(event));
                }
                Err(error) if error.error_len().is_none() && self.utf8.len() < 4 => {
                    return Ok(None);
                }
                Err(_) => return Err("invalid UTF-8 input"),
            }
        }
        Ok(match byte {
            0x1b => {
                self.escape.push(byte);
                self.started = Some(now);
                None
            }
            3 | 17 | 26 => Some(Event::Quit),
            9 => Some(Event::Tab),
            10 | 13 => Some(Event::Text("\n".into())),
            8 | 127 => Some(Event::Backspace),
            0x20..=0x7e => Some(Event::Text((byte as char).to_string())),
            _ => Some(Event::Unknown),
        })
    }
}

fn sequence(bytes: &[u8]) -> Event {
    match bytes {
        b"\x1b[A" => Event::Up,
        b"\x1b[B" => Event::Down,
        b"\x1b[C" => Event::Right,
        b"\x1b[D" => Event::Left,
        b"\x1b[H" | b"\x1bOH" | b"\x1b[1~" => Event::Home,
        b"\x1b[F" | b"\x1bOF" | b"\x1b[4~" => Event::End,
        b"\x1b[3~" => Event::Delete,
        b"\x1b[5~" => Event::PageUp,
        b"\x1b[6~" => Event::PageDown,
        b"\x1bOP" | b"\x1b[11~" => Event::Submit,
        b"\x1bOQ" | b"\x1b[12~" => Event::Copy,
        b"\x1bOR" | b"\x1b[13~" => Event::Follow,
        b"\x1bOS" | b"\x1b[14~" => Event::CancelAll,
        b"\x1bOT" | b"\x1b[15~" => Event::NextChild,
        b"\x1bOU" | b"\x1b[17~" => Event::CancelChild,
        b"\x1b[18~" => Event::Approve,
        b"\x1b[19~" => Event::Deny,
        b"\x1b[20~" => Event::ApprovalNext,
        b"\x1b[21~" => Event::ApprovalPrevious,
        b"\x1b[23~" => Event::HistoryPrevious,
        b"\x1b[24~" => Event::HistoryJourney,
        b"\x1b[I" => Event::Focus(true),
        b"\x1b[O" => Event::Focus(false),
        _ => {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return Event::Unknown;
            };
            if let Some(parameters) = text.strip_prefix("\x1b[").and_then(|s| s.strip_suffix('u')) {
                let mut parts = parameters.split(';');
                let code = parts.next().and_then(|s| s.parse::<u16>().ok());
                let modifier = parts.next().and_then(|s| s.parse::<u16>().ok());
                if parts.next().is_none()
                    && matches!(code, Some(67 | 99))
                    && modifier.is_some_and(|m| m > 0 && ((m - 1) & 8 != 0 || (m - 1) & 5 == 5))
                {
                    return Event::Copy;
                }
            }
            if let Some(parameters) = text
                .strip_prefix("\x1b[<")
                .and_then(|s| s.strip_suffix(['M', 'm']))
            {
                let mut parts = parameters.split(';');
                let values = (parts.next(), parts.next(), parts.next(), parts.next());
                if let (Some(b), Some(x), Some(y), None) = values
                    && let (Ok(button), Ok(x), Ok(y)) =
                        (b.parse(), x.parse::<u16>(), y.parse::<u16>())
                    && x > 0
                    && y > 0
                {
                    return Event::Mouse {
                        button,
                        x: x - 1,
                        y: y - 1,
                        release: text.ends_with('m'),
                    };
                }
            }
            Event::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> Result<Vec<Event>, &'static str> {
        let mut decoder = Decoder::default();
        let now = Instant::now();
        let mut events = Vec::new();
        for byte in bytes {
            if let Some(event) = decoder.feed(*byte, now)? {
                events.push(event);
            }
        }
        Ok(events)
    }

    #[test]
    fn fragmented_unicode_paste_and_delimiters() {
        assert_eq!(
            decode("hé界🙂".as_bytes()).unwrap(),
            vec![
                Event::Text("h".into()),
                Event::Text("é".into()),
                Event::Text("界".into()),
                Event::Text("🙂".into())
            ]
        );
        assert_eq!(
            decode(b"\x1b[200~a\x1b[A\x03\n\x1b[201~").unwrap(),
            vec![Event::Paste("a\x1b[A\x03\n".into())]
        );
        assert_eq!(
            decode(b"\x1b[99;9u\x1b[67;6u\x1b[<32;5;8M\x1b[O").unwrap(),
            vec![
                Event::Copy,
                Event::Copy,
                Event::Mouse {
                    button: 32,
                    x: 4,
                    y: 7,
                    release: false
                },
                Event::Focus(false)
            ]
        );
    }

    #[test]
    fn both_f4_encodings_are_cancel_intent_not_quit_or_text() {
        assert_eq!(
            decode(b"\x1bOS\x1b[14~\x11").unwrap(),
            vec![Event::CancelAll, Event::CancelAll, Event::Quit]
        );
    }

    #[test]
    fn approval_keys_are_never_intent_inside_paste() {
        assert_eq!(
            decode(b"\x1b[18~\x1b[19~\x1b[20~\x1b[21~").unwrap(),
            vec![
                Event::Approve,
                Event::Deny,
                Event::ApprovalNext,
                Event::ApprovalPrevious
            ]
        );
        assert_eq!(
            decode(b"\x1b[200~\x1b[18~\x1b[19~\x1b[201~").unwrap(),
            vec![Event::Paste("\x1b[18~\x1b[19~".into())]
        );
    }

    #[test]
    fn ingress_bounds_and_deadlines() {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(vec![b'x'; 8192]);
        bytes.extend(b"\x1b[201~");
        assert!(decode(&bytes).is_ok());
        bytes.insert(6, b'x');
        assert_eq!(decode(&bytes).unwrap_err(), "paste exceeds 8192 raw bytes");
        let mut escape = b"\x1b[".to_vec();
        escape.extend(vec![b'1'; 127]);
        assert!(decode(&escape).is_err());
        assert!(decode(&[0xff]).is_err());
        for prefix in [b"\x1b[200~".as_slice(), b"\x1b[", &[0xe2]] {
            let mut decoder = Decoder::default();
            let now = Instant::now();
            for byte in prefix {
                decoder.feed(*byte, now).unwrap();
            }
            assert!(decoder.check_deadline(now + DEADLINE).is_err());
        }
    }
}
