//! The X11 clipboard, read from the X server directly as `xclip -o` reads it, so that nothing
//! needs installing. Every wait, for the server and for the clipboard's owner, ends at a deadline.

use std::{
    cell::Cell,
    io::{self, IoSlice},
    net::{TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, Timespec};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ConnectionExt, CreateWindowAux, EventMask, GetPropertyReply, Property,
            Window, WindowClass,
        },
    },
    reexports::x11rb_protocol::{
        parse_display::{self, ConnectAddress},
        xauth::{self, Family},
    },
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
};

/// The text types asked for, in order; `STRING` is Latin-1.
const TEXT: [&str; 3] = ["UTF8_STRING", "text/plain;charset=utf-8", "STRING"];

/// The clipboard's text, if it holds any of at most `max` bytes.
pub(super) fn text(max: usize) -> Result<Option<String>, String> {
    let read = || Reader::new(None, "CLIPBOARD", super::TIMEOUT)?.text(max);
    read().map_err(|Failed(why)| why)
}

/// The clipboard's image in the first of `IMAGE_TYPES` it offers, and that type's extension.
pub(super) fn image(max: usize) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    let read = || Reader::new(None, "CLIPBOARD", super::TIMEOUT)?.image(max);
    read().map_err(|Failed(why)| why)
}

/// Why the clipboard could not be read, as the paste's error says it.
#[derive(Debug)]
struct Failed(String);

impl<E: std::error::Error> From<E> for Failed {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

/// A window of its own, to which a selection's owner hands the selection in `property`.
struct Reader {
    connection: RustConnection<Bounded>,
    window: Window,
    selection: Atom,
    property: Atom,
    incr: Atom,
    timeout: Duration,
}

impl Reader {
    /// Connects to `display` (`DISPLAY` if none) to read `selection`, waiting at most `timeout`
    /// for each answer.
    fn new(display: Option<&str>, selection: &str, timeout: Duration) -> Result<Self, Failed> {
        let (connection, screen) = connect(display, Instant::now() + timeout)?;
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id()?;
        let events = CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE);
        connection.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &events,
        )?;
        let atoms = atoms(&connection, &[selection, "SCRAMJET_RS_CLIPBOARD", "INCR"])?;
        Ok(Self {
            connection,
            window,
            selection: atoms[0],
            property: atoms[1],
            incr: atoms[2],
            timeout,
        })
    }

    fn text(&self, max: usize) -> Result<Option<String>, Failed> {
        if !self.owned()? {
            return Ok(None);
        }
        for (name, target) in TEXT.into_iter().zip(atoms(&self.connection, &TEXT)?) {
            if let Some(bytes) = self.convert(target, max)? {
                return Ok(Some(match name {
                    "STRING" => bytes.into_iter().map(char::from).collect(),
                    _ => String::from_utf8_lossy(&bytes).into_owned(),
                }));
            }
        }
        Ok(None)
    }

    fn image(&self, max: usize) -> Result<Option<(Vec<u8>, &'static str)>, Failed> {
        if !self.owned()? {
            return Ok(None);
        }
        let targets = atoms(&self.connection, &["TARGETS"])?[0];
        let Some(listed) = self.convert(targets, super::PASTE_MAX)? else {
            return Ok(None);
        };
        let cookies = (listed.as_chunks().0.iter())
            .map(|atom| self.connection.get_atom_name(u32::from_ne_bytes(*atom)))
            .collect::<Result<Vec<_>, _>>()?;
        let mut names = String::new();
        for cookie in cookies {
            names.push_str(&String::from_utf8_lossy(&cookie.reply()?.name));
            names.push('\n');
        }
        let Some((kind, extension)) = super::preferred(&names) else {
            return Ok(None);
        };
        let target = atoms(&self.connection, &[kind])?[0];
        Ok(self.convert(target, max)?.map(|bytes| (bytes, extension)))
    }

    fn owned(&self) -> Result<bool, Failed> {
        let owner = self.connection.get_selection_owner(self.selection)?;
        Ok(owner.reply()?.owner != x11rb::NONE)
    }

    /// The selection converted to `target`, unless its owner refuses; an error if it is longer
    /// than `max` bytes. A long one comes in pieces (ICCCM's `INCR`), each asked for by deleting
    /// the one before.
    fn convert(&self, target: Atom, max: usize) -> Result<Option<Vec<u8>>, Failed> {
        let connection = &self.connection;
        connection.delete_property(self.window, self.property)?;
        let (window, selection, property) = (self.window, self.selection, self.property);
        connection.convert_selection(window, selection, target, property, x11rb::CURRENT_TIME)?;
        connection.flush()?;
        self.wait_longer();
        let notified = loop {
            if let Event::SelectionNotify(event) = connection.wait_for_event()?
                && event.requestor == window
                && event.selection == selection
            {
                break event;
            }
        };
        if notified.property == x11rb::NONE {
            return Ok(None);
        }
        let first = self.take(max)?;
        if first.type_ != self.incr {
            return Ok((first.type_ != x11rb::NONE).then_some(first.value));
        }
        let mut bytes = Vec::new();
        loop {
            if let Event::PropertyNotify(event) = connection.wait_for_event()?
                && event.window == window
                && event.atom == property
                && event.state == Property::NEW_VALUE
            {
                let piece = self.take(max - bytes.len())?;
                if piece.value.is_empty() {
                    return Ok(Some(bytes));
                }
                bytes.extend_from_slice(&piece.value);
                self.wait_longer();
            }
        }
    }

    /// Reads and deletes the selection's property: an error if it holds more than `max` bytes.
    fn take(&self, max: usize) -> Result<GetPropertyReply, Failed> {
        let length = u32::try_from(max / 4 + 1).unwrap_or(u32::MAX);
        let (window, property) = (self.window, self.property);
        let cookie =
            (self.connection).get_property(true, window, property, AtomEnum::ANY, 0, length);
        let reply = cookie?.reply()?;
        match reply.bytes_after == 0 && reply.value.len() <= max {
            true => Ok(reply),
            false => Err(Failed(format!(
                "the clipboard holds more than {} MB",
                max.div_ceil(1 << 20)
            ))),
        }
    }

    fn wait_longer(&self) {
        (self.connection.stream().deadline).set(Instant::now() + self.timeout);
    }
}

fn atoms(connection: &impl Connection, names: &[&str]) -> Result<Vec<Atom>, Failed> {
    let cookies = (names.iter())
        .map(|name| connection.intern_atom(false, name.as_bytes()))
        .collect::<Result<Vec<_>, _>>()?;
    (cookies.into_iter())
        .map(|cookie| Ok(cookie.reply()?.atom))
        .collect()
}

/// Connects as `x11rb::connect` does, the connection's waits ending at `deadline`; also
/// returns the screen's number.
fn connect(
    display: Option<&str>,
    deadline: Instant,
) -> Result<(RustConnection<Bounded>, usize), Failed> {
    let parsed = parse_display::parse_display(display)?;
    for address in parsed.connect_instruction() {
        let connected = match address {
            ConnectAddress::Hostname(host, port) => tcp(host, port, deadline),
            address => DefaultStream::connect(&address),
        };
        let Ok((stream, (family, peer))) = connected else {
            continue;
        };
        // As x11rb's: without the authority file's cookie, the server may still let us in.
        let (name, data) = (xauth::get_auth(family, &peer, parsed.display).ok())
            .flatten()
            .unwrap_or_default();
        let stream = Bounded {
            stream,
            deadline: Cell::new(deadline),
        };
        let screen = usize::from(parsed.screen);
        let connection =
            RustConnection::connect_to_stream_with_auth_info(stream, screen, name, data)?;
        return Ok((connection, screen));
    }
    Err(Failed("the X server cannot be reached".to_owned()))
}

fn tcp(host: &str, port: u16, deadline: Instant) -> io::Result<(DefaultStream, (Family, Vec<u8>))> {
    for address in (host, port).to_socket_addrs()? {
        let left = deadline.saturating_duration_since(Instant::now());
        if let Ok(stream) = TcpStream::connect_timeout(&address, left.max(Duration::from_millis(1)))
        {
            return DefaultStream::from_tcp_stream(stream);
        }
    }
    Err(io::ErrorKind::NotConnected.into())
}

/// The stream to the X server, whose waits end at `deadline`.
struct Bounded {
    stream: DefaultStream,
    deadline: Cell<Instant>,
}

impl Stream for Bounded {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        let mut flags = PollFlags::empty();
        if mode.readable() {
            flags |= PollFlags::IN;
        }
        if mode.writable() {
            flags |= PollFlags::OUT;
        }
        loop {
            let left = self
                .deadline
                .get()
                .saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"));
            }
            let timeout = Timespec::try_from(left).map_err(io::Error::other)?;
            let mut polled = [PollFd::new(&self.stream, flags)];
            match rustix::event::poll(&mut polled, Some(&timeout)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => {}
                Ok(_) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn read(&self, buffer: &mut [u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.stream.read(buffer, fds)
    }

    fn write(&self, buffer: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.stream.write(buffer, fds)
    }

    fn write_vectored(
        &self,
        buffers: &[IoSlice<'_>],
        fds: &mut Vec<RawFdContainer>,
    ) -> io::Result<usize> {
        self.stream.write_vectored(buffers, fds)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use x11rb::{
        protocol::xproto::{
            ChangeWindowAttributesAux, PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
        },
        wrapper::ConnectionExt as _,
    };

    use super::*;

    /// Owns a selection of the tests' own, never the clipboard, on the X server of `DISPLAY`,
    /// and offers each target's bytes, in `INCR` pieces of `piece` bytes when longer; an owner
    /// that does not `answer` ignores every request. `None` when no X server can be reached,
    /// as in a sandbox or a terminal without a display.
    fn own(
        case: &str,
        offers: &[(&'static str, &[u8])],
        piece: usize,
        answer: bool,
    ) -> Option<String> {
        let selection = format!("SCRAMJET_RS_TEST_{}_{case}", std::process::id());
        let offers: Vec<_> = (offers.iter())
            .map(|(target, bytes)| (*target, bytes.to_vec()))
            .collect();
        let (ready, owning) = mpsc::channel();
        let name = selection.clone();
        std::thread::spawn(move || {
            let Ok(owner) = Reader::new(None, &name, Duration::from_secs(5)) else {
                let _ = ready.send(false);
                return;
            };
            let connection = &owner.connection;
            let window = owner.window;
            let _ = connection.set_selection_owner(window, owner.selection, x11rb::CURRENT_TIME);
            let _ = ready.send(connection.sync().is_ok());
            let names: Vec<&str> = ["TARGETS"]
                .into_iter()
                .chain(offers.iter().map(|(target, _)| *target))
                .collect();
            let Ok(targets) = atoms(connection, &names) else {
                return;
            };
            // Each `INCR` transfer under way: the requestor's window and property, the type, and
            // what is left to send.
            let mut sending: Vec<(Window, Atom, Atom, Vec<u8>)> = Vec::new();
            while let Ok(event) = connection.wait_for_event() {
                owner.wait_longer();
                match event {
                    Event::SelectionRequest(request) if answer => {
                        let (requestor, target) = (request.requestor, request.target);
                        let offer = (targets[1..].iter().zip(&offers))
                            .find(|(atom, _)| **atom == target)
                            .map(|(_, (_, bytes))| bytes);
                        let mut property = request.property;
                        let replace = PropMode::REPLACE;
                        if target == targets[0] {
                            let _ = connection.change_property32(
                                replace,
                                requestor,
                                property,
                                AtomEnum::ATOM,
                                &targets,
                            );
                        } else if let Some(bytes) = offer.filter(|bytes| bytes.len() > piece) {
                            let events = ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE);
                            let _ = connection.change_window_attributes(requestor, &events);
                            let length = [bytes.len() as u32];
                            let _ = connection.change_property32(
                                replace, requestor, property, owner.incr, &length,
                            );
                            // The server reuses a window's number after its reader went.
                            sending.retain(|sent| (sent.0, sent.1) != (requestor, property));
                            sending.push((requestor, property, target, bytes.clone()));
                        } else if let Some(bytes) = offer {
                            let _ = connection
                                .change_property8(replace, requestor, property, target, bytes);
                        } else {
                            property = x11rb::NONE;
                        }
                        let notify = SelectionNotifyEvent {
                            response_type: SELECTION_NOTIFY_EVENT,
                            sequence: 0,
                            time: request.time,
                            requestor,
                            selection: request.selection,
                            target,
                            property,
                        };
                        let _ =
                            connection.send_event(false, requestor, EventMask::NO_EVENT, notify);
                    }
                    Event::PropertyNotify(event) if event.state == Property::DELETE => {
                        let Some(index) = (sending.iter())
                            .position(|sent| (sent.0, sent.1) == (event.window, event.atom))
                        else {
                            continue;
                        };
                        let (requestor, property, kind, left) = &mut sending[index];
                        let next: Vec<u8> = left.drain(..piece.min(left.len())).collect();
                        let _ = connection.change_property8(
                            PropMode::REPLACE,
                            *requestor,
                            *property,
                            *kind,
                            &next,
                        );
                        if next.is_empty() {
                            sending.remove(index);
                        }
                    }
                    _ => {}
                }
                let _ = connection.flush();
            }
        });
        match owning.recv() {
            Ok(true) => Some(selection),
            _ => {
                eprintln!("no X server reached: the X11 clipboard is not tested");
                None
            }
        }
    }

    fn read(selection: &str) -> Reader {
        Reader::new(None, selection, Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn text_is_read_as_utf8_else_latin1_and_long_text_in_pieces() {
        let text = "one\r\ntwo \u{2014} \u{754c}\n";
        let Some(utf8) = own("utf8", &[("UTF8_STRING", text.as_bytes())], 1 << 20, true) else {
            return;
        };
        assert_eq!(read(&utf8).text(1000).unwrap().as_deref(), Some(text));
        let latin1 = own("latin1", &[("STRING", b"caf\xe9\n")], 1 << 20, true).unwrap();
        assert_eq!(
            read(&latin1).text(1000).unwrap().as_deref(),
            Some("caf\u{e9}\n")
        );
        let long: String = (0..100_000).map(|line| format!("{line}\n")).collect();
        let pieces = own(
            "pieces",
            &[("UTF8_STRING", long.as_bytes())],
            64 << 10,
            true,
        )
        .unwrap();
        assert_eq!(read(&pieces).text(1 << 20).unwrap(), Some(long));
    }

    #[test]
    fn text_longer_than_allowed_is_refused_whole_or_in_pieces() {
        let long = [b'a'; 2000];
        let Some(whole) = own("whole", &[("UTF8_STRING", &long)], 1 << 20, true) else {
            return;
        };
        let pieces = own("toolong", &[("UTF8_STRING", &long)], 300, true).unwrap();
        for selection in [whole, pieces] {
            let refused = read(&selection).text(1000).unwrap_err();
            assert_eq!(refused.0, "the clipboard holds more than 1 MB");
            assert_eq!(read(&selection).text(2000).unwrap().unwrap().len(), 2000);
        }
    }

    #[test]
    fn an_image_is_read_in_the_first_type_read_sends_that_the_owner_offers() {
        let offers: [(&str, &[u8]); 3] = [
            ("image/bmp", b"BM"),
            ("image/gif", b"GIF89a"),
            ("image/jpeg", b"\xff\xd8\xff"),
        ];
        let Some(images) = own("image", &offers, 1 << 20, true) else {
            return;
        };
        let image = read(&images).image(1000).unwrap();
        assert_eq!(image, Some((b"\xff\xd8\xff".to_vec(), "jpg")));
        assert_eq!(read(&images).text(1000).unwrap(), None);
        let text = own("noimage", &[("UTF8_STRING", b"text")], 1 << 20, true).unwrap();
        assert_eq!(read(&text).image(1000).unwrap(), None);
    }

    #[test]
    fn nothing_is_read_from_a_selection_nobody_owns_and_a_silent_owner_times_out() {
        let Some(silent) = own("silent", &[("UTF8_STRING", b"text")], 1 << 20, false) else {
            return;
        };
        let unowned = format!("SCRAMJET_RS_TEST_{}_unowned", std::process::id());
        assert_eq!(read(&unowned).text(1000).unwrap(), None);
        assert_eq!(read(&unowned).image(1000).unwrap(), None);
        let started = Instant::now();
        let waiting = Reader::new(None, &silent, Duration::from_millis(200)).unwrap();
        assert_eq!(waiting.text(1000).unwrap_err().0, "timed out");
        assert!(started.elapsed() < Duration::from_secs(2));
        let absent = Reader::new(Some(":9999"), "CLIPBOARD", Duration::from_secs(5));
        assert_eq!(absent.err().unwrap().0, "the X server cannot be reached");
    }
}
