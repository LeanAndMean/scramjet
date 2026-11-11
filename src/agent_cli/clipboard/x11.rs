//! The X11 clipboard, talking to the X server directly as `xclip` does, so that nothing needs
//! installing. A copy is served by a process of its own, scramjet-rs started again (`serve`),
//! as `xclip` forks one, so that it can be pasted after scramjet-rs exits; it ends once another
//! program copies. Every wait for the server or another client ends at a deadline, but for the
//! owner's wait for the next request.

use std::{
    cell::Cell,
    io::{self, BufRead, BufReader, IoSlice, Read, Write},
    net::{TcpStream, ToSocketAddrs},
    os::unix::process::CommandExt,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, Timespec};
use x11rb::{
    connection::{Connection, RequestConnection},
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, CreateWindowAux, EventMask,
            GetPropertyReply, PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
            Window, WindowClass,
        },
    },
    reexports::x11rb_protocol::{
        parse_display::{self, ConnectAddress},
        xauth::{self, Family},
    },
    rust_connection::{DefaultStream, PollMode, RustConnection, Stream},
    utils::RawFdContainer,
    wrapper::ConnectionExt as _,
};

/// The first argument that starts scramjet-rs as a selection's owner (`serve`), followed by the
/// selection's name.
pub(super) const SERVE: &str = "__x11-selection-owner";

/// The text types asked for, in order; `STRING` is Latin-1.
const TEXT: [&str; 3] = ["UTF8_STRING", "text/plain;charset=utf-8", "STRING"];

/// Copies `text` to the clipboard, owned by a process of its own.
pub(super) fn copy(text: &str) -> Result<(), String> {
    let mut owner = Command::new("/proc/self/exe");
    owner
        .arg0("scramjet")
        .args([SERVE, "CLIPBOARD"])
        .env_clear();
    // It may outlive scramjet-rs: what it needs to reach the X server, and nothing else.
    for name in ["DISPLAY", "XAUTHORITY", "HOME"] {
        if let Some(value) = std::env::var_os(name) {
            owner.env(name, value);
        }
    }
    start_owner(owner, text, super::TIMEOUT)
}

/// Starts `owner` in a session of its own, out of reach of the terminal's Ctrl-C and hangup,
/// gives it `text`, and waits at most `timeout` for it to say that it owns the selection.
fn start_owner(mut owner: Command, text: &str, timeout: Duration) -> Result<(), String> {
    owner.current_dir("/");
    owner
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // SAFETY: `setsid` is async-signal-safe, as what runs between `fork` and `exec` must be.
    unsafe {
        owner.pre_exec(|| Ok(rustix::process::setsid().map(drop)?));
    }
    let mut child =
        (owner.spawn()).map_err(|error| format!("cannot start the clipboard's owner: {error}"))?;
    let (stdin, stdout) = (child.stdin.take(), child.stdout.take());
    let (done, said) = mpsc::channel();
    let text = text.to_owned();
    let talking = std::thread::Builder::new()
        .name("clipboard owner".into())
        .spawn(move || {
            let (Some(mut stdin), Some(stdout)) = (stdin, stdout) else {
                return;
            };
            let _ = stdin.write_all(text.as_bytes());
            drop(stdin);
            let mut line = String::new();
            let _ = BufReader::new(stdout).take(1024).read_line(&mut line);
            let _ = done.send(line);
        });
    let said = talking.ok().and_then(|_| said.recv_timeout(timeout).ok());
    let said = said.as_deref().map(str::trim_end);
    if said == Some("ok") {
        // Waited for, not to linger as a zombie once another program copies.
        let reaper = std::thread::Builder::new().name("clipboard owner".into());
        let _ = reaper.spawn(move || child.wait());
        return Ok(());
    }
    let _ = child.kill();
    let _ = child.wait();
    Err(match said {
        None => "the clipboard's owner did not answer in time".to_owned(),
        Some("") => "the clipboard's owner ended".to_owned(),
        Some(why) => why.to_owned(),
    })
}

/// Owns `selection`, offering the text read from the standard input, until another client takes
/// it; says `ok` on the standard output once it owns it, else why not.
pub(super) fn serve(selection: &str) -> Result<(), String> {
    let mut text = String::new();
    (io::stdin().read_to_string(&mut text)).map_err(|error| error.to_string())?;
    let say = |line: &str| {
        let _ = writeln!(io::stdout(), "{line}");
    };
    let offered = Selection::new(None, selection, super::TIMEOUT)
        .and_then(|owner| owner.offer(&text, || say("ok")));
    if let Err(Failed(why)) = &offered {
        say(why);
    }
    offered.map_err(|Failed(why)| why)
}

/// The clipboard's text, if it holds any of at most `max` bytes.
pub(super) fn text(max: usize) -> Result<Option<String>, String> {
    let read = || Selection::new(None, "CLIPBOARD", super::TIMEOUT)?.text(max);
    read().map_err(|Failed(why)| why)
}

/// The clipboard's image in the first of `IMAGE_TYPES` it offers, and that type's extension.
pub(super) fn image(max: usize) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    let read = || Selection::new(None, "CLIPBOARD", super::TIMEOUT)?.image(max);
    read().map_err(|Failed(why)| why)
}

/// Why the clipboard could not be read or owned, as the error says it.
#[derive(Debug)]
struct Failed(String);

impl<E: std::error::Error> From<E> for Failed {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

/// A window of its own, which owns the selection or to which its owner hands it in `property`.
struct Selection {
    connection: RustConnection<Bounded>,
    window: Window,
    selection: Atom,
    property: Atom,
    incr: Atom,
    timeout: Duration,
}

impl Selection {
    /// Connects to `display` (`DISPLAY` if none) for `selection`, waiting at most `timeout` for
    /// each answer.
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
        let [selection, property, incr] =
            atoms(&connection, [selection, "SCRAMJET_RS_CLIPBOARD", "INCR"])?;
        Ok(Self {
            connection,
            window,
            selection,
            property,
            incr,
            timeout,
        })
    }

    /// Owns the selection and offers `text`, as UTF-8 and, when it has no other characters,
    /// Latin-1, until another client takes it; `owned` once it owns it.
    fn offer(&self, text: &str, owned: impl FnOnce()) -> Result<(), Failed> {
        let [utf8, mime, any, latin1] = atoms(
            &self.connection,
            ["UTF8_STRING", "text/plain;charset=utf-8", "TEXT", "STRING"],
        )?;
        let bytes = text.as_bytes();
        let mut offers = vec![(utf8, utf8, bytes), (mime, mime, bytes), (any, utf8, bytes)];
        let in_latin1: Option<Vec<u8>> = text.chars().map(|c| u8::try_from(c).ok()).collect();
        if let Some(in_latin1) = &in_latin1 {
            offers.push((latin1, latin1, in_latin1));
        }
        // In pieces only when longer than a request can carry, as ICCCM asks: WSLg's bridge to
        // Windows loses pieced text, and its X server crashed taking it (2026-11-04).
        let piece = self.connection.maximum_request_bytes().saturating_sub(64);
        self.own()?;
        owned();
        self.serve(&offers, piece)
    }

    /// Takes the selection: an error if another client keeps it.
    fn own(&self) -> Result<(), Failed> {
        let (connection, window, selection) = (&self.connection, self.window, self.selection);
        connection.set_selection_owner(window, selection, x11rb::CURRENT_TIME)?;
        match connection.get_selection_owner(selection)?.reply()?.owner == window {
            true => Ok(()),
            false => Err(Failed("another program kept the clipboard".to_owned())),
        }
    }

    /// Answers each request for the selection with the bytes of its target among `offers`
    /// (target, type, bytes), in `INCR` pieces of `piece` bytes when longer, until another client
    /// takes it and the pieces under way have been taken.
    fn serve(&self, offers: &[(Atom, Atom, &[u8])], piece: usize) -> Result<(), Failed> {
        let connection = &self.connection;
        let [targets] = atoms(connection, ["TARGETS"])?;
        let listed: Vec<Atom> = std::iter::once(targets)
            .chain(offers.iter().map(|(target, ..)| *target))
            .collect();
        let mut sending: Vec<Sending> = Vec::new();
        let mut owned = true;
        loop {
            connection.flush()?;
            let now = Instant::now();
            sending.retain(|sent| sent.until > now);
            if !owned && sending.is_empty() {
                return Ok(());
            }
            let Some(event) = connection.poll_for_event()? else {
                self.wait(sending.iter().map(|sent| sent.until).min())?;
                continue;
            };
            self.wait_longer();
            match event {
                Event::SelectionClear(event) if event.selection == self.selection => owned = false,
                Event::SelectionRequest(request) => {
                    let (requestor, target) = (request.requestor, request.target);
                    // Obsolete requestors name no property, meaning the target's.
                    let property = match request.property {
                        x11rb::NONE => target,
                        property => property,
                    };
                    // A window's number is reused once its client went.
                    sending.retain(|sent| (sent.requestor, sent.property) != (requestor, property));
                    let offer = offers.iter().find(|(offered, ..)| *offered == target);
                    let replace = PropMode::REPLACE;
                    let answered = match offer {
                        _ if !owned => false,
                        _ if target == targets => {
                            let atom = AtomEnum::ATOM;
                            connection
                                .change_property32(replace, requestor, property, atom, &listed)?;
                            true
                        }
                        Some(&(_, kind, bytes)) if bytes.len() > piece => {
                            let events = ChangeWindowAttributesAux::new()
                                .event_mask(EventMask::PROPERTY_CHANGE);
                            connection.change_window_attributes(requestor, &events)?;
                            let length = [u32::try_from(bytes.len()).unwrap_or(u32::MAX)];
                            connection.change_property32(
                                replace, requestor, property, self.incr, &length,
                            )?;
                            sending.push(Sending {
                                requestor,
                                property,
                                kind,
                                left: bytes,
                                until: Instant::now() + self.timeout,
                            });
                            true
                        }
                        Some(&(_, kind, bytes)) => {
                            connection
                                .change_property8(replace, requestor, property, kind, bytes)?;
                            true
                        }
                        None => false,
                    };
                    let notify = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: request.time,
                        requestor,
                        selection: request.selection,
                        target,
                        property: if answered { property } else { x11rb::NONE },
                    };
                    connection.send_event(false, requestor, EventMask::NO_EVENT, notify)?;
                }
                Event::PropertyNotify(event) if event.state == Property::DELETE => {
                    let Some(index) = (sending.iter()).position(|sent| {
                        (sent.requestor, sent.property) == (event.window, event.atom)
                    }) else {
                        continue;
                    };
                    let sent = &mut sending[index];
                    let (next, left) = sent.left.split_at(piece.min(sent.left.len()));
                    let (requestor, property, kind) = (sent.requestor, sent.property, sent.kind);
                    connection.change_property8(
                        PropMode::REPLACE,
                        requestor,
                        property,
                        kind,
                        next,
                    )?;
                    (sent.left, sent.until) = (left, Instant::now() + self.timeout);
                    if next.is_empty() {
                        sending.remove(index);
                    }
                }
                _ => {}
            }
        }
    }

    /// Waits until the server sends something, if need be at most until `until`.
    fn wait(&self, until: Option<Instant>) -> Result<(), Failed> {
        let stream = &self.connection.stream().stream;
        loop {
            let left = until.map(|until| until.saturating_duration_since(Instant::now()));
            let timeout = left.map(Timespec::try_from).transpose()?;
            let mut polled = [PollFd::new(stream, PollFlags::IN)];
            match rustix::event::poll(&mut polled, timeout.as_ref()) {
                Err(rustix::io::Errno::INTR) => {}
                polled => return Ok(polled.map(drop)?),
            }
        }
    }

    fn text(&self, max: usize) -> Result<Option<String>, Failed> {
        if !self.owned()? {
            return Ok(None);
        }
        for (name, target) in TEXT.into_iter().zip(atoms(&self.connection, TEXT)?) {
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
        let [targets] = atoms(&self.connection, ["TARGETS"])?;
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
        let [target] = atoms(&self.connection, [kind])?;
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

/// An `INCR` transfer under way: the requestor's window and property, the type, what is left
/// to send, and until when the requestor may take the next piece.
struct Sending<'a> {
    requestor: Window,
    property: Atom,
    kind: Atom,
    left: &'a [u8],
    until: Instant,
}

fn atoms<const N: usize>(
    connection: &impl Connection,
    names: [&str; N],
) -> Result<[Atom; N], Failed> {
    let cookies = names.map(|name| connection.intern_atom(false, name.as_bytes()));
    let mut atoms = [x11rb::NONE; N];
    for (atom, cookie) in atoms.iter_mut().zip(cookies) {
        *atom = cookie?.reply()?.atom;
    }
    Ok(atoms)
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
    use super::*;

    /// A selection of the tests' own, never the clipboard, named after `case`.
    fn name(case: &str) -> String {
        format!("SCRAMJET_RS_TEST_{}_{case}", std::process::id())
    }

    /// Owns `name(case)` on the X server of `DISPLAY` and offers each target's bytes, in `INCR`
    /// pieces of `piece` bytes when longer; an owner that does not `answer` ignores every
    /// request. `None` when no X server can be reached, as in a sandbox or a terminal without a
    /// display.
    fn own(
        case: &str,
        offers: &[(&'static str, &[u8])],
        piece: usize,
        answer: bool,
    ) -> Option<String> {
        let selection = name(case);
        let offers: Vec<_> = (offers.iter())
            .map(|(target, bytes)| (*target, bytes.to_vec()))
            .collect();
        let (ready, owning) = mpsc::channel();
        let name = selection.clone();
        std::thread::spawn(move || {
            let owner = Selection::new(None, &name, Duration::from_secs(5));
            let Ok(owner) = owner.and_then(|owner| owner.own().map(|()| owner)) else {
                let _ = ready.send(false);
                return;
            };
            let _ = ready.send(true);
            if !answer {
                loop {
                    std::thread::park();
                }
            }
            let offered = (offers.iter())
                .map(|(target, bytes)| {
                    let [atom] = atoms(&owner.connection, [*target])?;
                    Ok((atom, atom, bytes.as_slice()))
                })
                .collect::<Result<Vec<_>, Failed>>();
            let _ = owner.serve(&offered.unwrap(), piece);
        });
        match owning.recv() {
            Ok(true) => Some(selection),
            _ => {
                eprintln!("no X server reached: the X11 clipboard is not tested");
                None
            }
        }
    }

    fn read(selection: &str) -> Selection {
        Selection::new(None, selection, Duration::from_secs(5)).unwrap()
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
        let unowned = name("unowned");
        assert_eq!(read(&unowned).text(1000).unwrap(), None);
        assert_eq!(read(&unowned).image(1000).unwrap(), None);
        let started = Instant::now();
        let waiting = Selection::new(None, &silent, Duration::from_millis(200)).unwrap();
        assert_eq!(waiting.text(1000).unwrap_err().0, "timed out");
        assert!(started.elapsed() < Duration::from_secs(2));
        let absent = Selection::new(Some(":9999"), "CLIPBOARD", Duration::from_secs(5));
        assert_eq!(absent.err().unwrap().0, "the X server cannot be reached");
    }

    #[test]
    fn a_copy_is_offered_as_text_in_latin1_when_it_fits_until_another_takes_it() {
        let offer = |case: &str, text: &'static str| {
            let selection = name(case);
            let owner = Selection::new(None, &selection, Duration::from_secs(5)).ok()?;
            let (owning, owned) = mpsc::channel();
            let (ending, ended) = mpsc::channel();
            std::thread::spawn(move || {
                let offered = owner.offer(text, || owning.send(()).unwrap());
                let _ = ending.send(offered.map_err(|Failed(why)| why));
            });
            owned.recv().unwrap();
            Some((selection, ended))
        };
        let Some((latin1, _)) = offer("copied", "caf\u{e9}\n") else {
            eprintln!("no X server reached: the X11 clipboard is not tested");
            return;
        };
        let reader = read(&latin1);
        let [utf8, mime, any, string, targets] = atoms(
            &reader.connection,
            [
                "UTF8_STRING",
                "text/plain;charset=utf-8",
                "TEXT",
                "STRING",
                "TARGETS",
            ],
        )
        .unwrap();
        let converted = |target| reader.convert(target, 1000).unwrap();
        for target in [utf8, mime, any] {
            assert_eq!(converted(target).as_deref(), Some("caf\u{e9}\n".as_bytes()));
        }
        assert_eq!(converted(string).as_deref(), Some(&b"caf\xe9\n"[..]));
        let listed = |reader: &Selection| -> Vec<u32> {
            let listed = reader.convert(targets, 1000).unwrap().unwrap();
            (listed.as_chunks().0.iter())
                .map(|atom| u32::from_ne_bytes(*atom))
                .collect()
        };
        assert_eq!(listed(&reader), [targets, utf8, mime, any, string]);
        let (other, ended) = offer("wider", "\u{754c}").unwrap();
        let reader = read(&other);
        assert_eq!(reader.text(1000).unwrap().as_deref(), Some("\u{754c}"));
        assert_eq!(reader.convert(string, 1000).unwrap(), None);
        assert_eq!(listed(&reader), [targets, utf8, mime, any]);
        assert_eq!(ended.try_recv(), Err(mpsc::TryRecvError::Empty));
        let taker = Selection::new(None, &other, Duration::from_secs(5)).unwrap();
        taker.own().unwrap();
        assert_eq!(ended.recv_timeout(Duration::from_secs(5)), Ok(Ok(())));
    }

    #[test]
    fn an_owner_is_started_in_a_session_of_its_own_and_waited_for_until_it_owns() {
        let script = |script: &str| {
            let mut owner = Command::new("sh");
            owner.args(["-c", script]);
            owner
        };
        // Says `ok` only once it has the text, in a session of its own in `/`.
        let owning = script(
            "read -r text; read -r _ _ _ _ _ session _ < /proc/$$/stat; \
             [ \"$text\" = copied ] && [ \"$session\" = $$ ] && [ \"$(pwd -P)\" = / ] && echo ok; \
             exec sleep 1",
        );
        assert_eq!(
            start_owner(owning, "copied\n", Duration::from_secs(5)),
            Ok(())
        );
        let refusing = script("cat > /dev/null; echo 'another program kept the clipboard'; exit 1");
        assert_eq!(
            start_owner(refusing, "text", Duration::from_secs(5)).unwrap_err(),
            "another program kept the clipboard"
        );
        let ending = script("exit 1");
        assert_eq!(
            start_owner(ending, "text", Duration::from_secs(5)).unwrap_err(),
            "the clipboard's owner ended"
        );
        let started = Instant::now();
        let silent = script("cat > /dev/null; exec sleep 30");
        assert_eq!(
            start_owner(silent, "text", Duration::from_millis(300)).unwrap_err(),
            "the clipboard's owner did not answer in time"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = Command::new("scramjet-rs-no-such-tool");
        let error = start_owner(missing, "text", Duration::from_secs(5)).unwrap_err();
        assert!(
            error.starts_with("cannot start the clipboard's owner: "),
            "{error}"
        );
    }
}
