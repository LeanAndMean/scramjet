//! The Wayland clipboard through the compositor's data-control protocol (`ext-data-control-v1`,
//! else wlroots' `wlr-data-control-unstable-v1`), as `wl-clipboard` uses it, so that nothing needs
//! installing. GNOME offers neither; there `wl-paste` and XWayland are tried after. A copy is
//! served by a process of its own (`serve`), as `wl-copy` forks one, so that it can be pasted
//! after scramjet-rs exits; it ends once another program copies. Every wait for the compositor
//! or another client ends at a deadline, but for the owner's wait for the next request.

use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, OwnedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use rustix::event::{PollFd, PollFlags, Timespec};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle,
    backend::{ObjectId, WaylandError},
    event_created_child,
    protocol::{wl_callback::WlCallback, wl_registry::WlRegistry, wl_seat::WlSeat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self as ext_device, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self as ext_offer, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self as ext_source, ExtDataControlSourceV1},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self as wlr_device, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self as wlr_offer, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self as wlr_source, ZwlrDataControlSourceV1},
};

/// The first argument that starts scramjet-rs as the clipboard's owner (`serve`).
pub(super) const SERVE: &str = "__wayland-selection-owner";

/// The text types asked for, in order; `STRING` is Latin-1.
const TEXT: [&str; 5] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
    "STRING",
];

/// Copies `text` to the clipboard, owned by a process of its own.
pub(super) fn copy(text: &str) -> Result<(), String> {
    let mut owner = Command::new("/proc/self/exe");
    owner.arg0("scramjet").arg(SERVE).env_clear();
    // It may outlive scramjet-rs: what it needs to reach the compositor, and nothing else.
    for name in ["WAYLAND_DISPLAY", "XDG_RUNTIME_DIR", "HOME"] {
        if let Some(value) = std::env::var_os(name) {
            owner.env(name, value);
        }
    }
    super::x11::start_owner(owner, text, super::TIMEOUT)
}

/// Owns the clipboard, offering the text read from the standard input, until another client
/// takes it; says `ok` on the standard output once it owns it, else why not.
pub(super) fn serve() -> Result<(), String> {
    let mut text = String::new();
    (io::stdin().read_to_string(&mut text)).map_err(|error| error.to_string())?;
    let say = |line: &str| {
        let _ = writeln!(io::stdout(), "{line}");
    };
    let offered = Clipboard::new(&socket(), super::TIMEOUT)
        .and_then(|clipboard| clipboard.offer(&offers(&text), || say("ok")));
    if let Err(Failed(why)) = &offered {
        say(why);
    }
    offered.map_err(|Failed(why)| why)
}

/// The clipboard's text, if it holds any of at most `max` bytes.
pub(super) fn text(max: usize) -> Result<Option<String>, String> {
    let read = || Clipboard::new(&socket(), super::TIMEOUT)?.text(max);
    read().map_err(|Failed(why)| why)
}

/// The clipboard's image in the first of `IMAGE_TYPES` it offers, and that type's extension.
pub(super) fn image(max: usize) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    let read = || Clipboard::new(&socket(), super::TIMEOUT)?.image(max);
    read().map_err(|Failed(why)| why)
}

/// The compositor's socket, as `WAYLAND_DISPLAY` names it.
fn socket() -> PathBuf {
    let display = std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default();
    match PathBuf::from(&display) {
        path if path.is_absolute() => path,
        _ => PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_default()).join(display),
    }
}

/// `text` in each of `TEXT`'s types: UTF-8, and Latin-1 when it has no other characters.
fn offers(text: &str) -> Vec<(&'static str, Vec<u8>)> {
    let in_latin1: Option<Vec<u8>> = text.chars().map(|c| u8::try_from(c).ok()).collect();
    (TEXT.into_iter())
        .filter_map(|kind| match kind {
            "STRING" => in_latin1.clone().map(|latin1| (kind, latin1)),
            _ => Some((kind, text.as_bytes().to_vec())),
        })
        .collect()
}

/// Why the clipboard could not be read or owned, as the error says it.
#[derive(Debug)]
struct Failed(String);

impl<E: std::error::Error> From<E> for Failed {
    fn from(error: E) -> Self {
        Self(error.to_string())
    }
}

/// The data-control device of the compositor's first seat, in whichever protocol it offers.
enum Device {
    Ext(ExtDataControlManagerV1, ExtDataControlDeviceV1),
    Wlr(ZwlrDataControlManagerV1, ZwlrDataControlDeviceV1),
}

enum Offer {
    Ext(ExtDataControlOfferV1),
    Wlr(ZwlrDataControlOfferV1),
}

/// What the compositor has said.
#[derive(Default)]
struct State {
    /// The globals: name, interface and version.
    globals: Vec<(u32, String, u32)>,
    synced: bool,
    /// Each offer announced, with the types it said it has.
    offers: HashMap<ObjectId, (Offer, Vec<String>)>,
    /// The clipboard's offer, once the device has said (`Some(None)`: empty).
    selection: Option<Option<ObjectId>>,
    /// Requests for the copy offered: the type, and where to write it.
    sends: Vec<(String, OwnedFd)>,
    /// Whether the copy offered was replaced, or the device went.
    ended: bool,
}

struct Clipboard {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    device: Device,
    timeout: Duration,
}

impl Clipboard {
    /// Connects to the compositor at `path`, waiting at most `timeout` for each answer, and
    /// opens the first seat's data-control device.
    fn new(path: &Path, timeout: Duration) -> Result<Self, Failed> {
        let socket = UnixStream::connect(path)
            .map_err(|error| Failed(format!("cannot reach {}: {error}", path.display())))?;
        let connection = Connection::from_socket(socket)?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        let mut state = State::default();
        let registry = connection.display().get_registry(&handle, ());
        sync(&connection, &mut queue, &mut state, timeout)?;
        let global = |wanted: &str| {
            (state.globals.iter())
                .find(|(_, interface, _)| interface == wanted)
                .map(|&(name, ..)| name)
        };
        let Some(seat) = global(WlSeat::interface().name) else {
            return Err(Failed("the compositor has no seat".to_owned()));
        };
        let seat: WlSeat = registry.bind(seat, 1, &handle, ());
        let ext = global(ExtDataControlManagerV1::interface().name);
        let wlr = global(ZwlrDataControlManagerV1::interface().name);
        let device = match (ext, wlr) {
            (Some(name), _) => {
                let manager: ExtDataControlManagerV1 = registry.bind(name, 1, &handle, ());
                let device = manager.get_data_device(&seat, &handle, ());
                Device::Ext(manager, device)
            }
            (None, Some(name)) => {
                let manager: ZwlrDataControlManagerV1 = registry.bind(name, 1, &handle, ());
                let device = manager.get_data_device(&seat, &handle, ());
                Device::Wlr(manager, device)
            }
            (None, None) => {
                let none = "the compositor offers no data-control protocol";
                return Err(Failed(none.to_owned()));
            }
        };
        Ok(Self {
            connection,
            queue,
            state,
            device,
            timeout,
        })
    }

    /// The types the clipboard offers, once the device has said; `None` when it is empty.
    fn offered(&mut self) -> Result<Option<(ObjectId, Vec<String>)>, Failed> {
        let deadline = Instant::now() + self.timeout;
        until(&mut self.queue, &mut self.state, Some(deadline), |state| {
            state.selection.is_some()
        })?;
        let Some(Some(id)) = self.state.selection.clone() else {
            return Ok(None);
        };
        let types = (self.state.offers.get(&id)).map_or_else(Vec::new, |(_, types)| types.clone());
        Ok(Some((id, types)))
    }

    fn text(&mut self, max: usize) -> Result<Option<String>, Failed> {
        let Some((id, types)) = self.offered()? else {
            return Ok(None);
        };
        let Some(kind) = TEXT
            .into_iter()
            .find(|kind| types.iter().any(|t| t == kind))
        else {
            return Ok(None);
        };
        let bytes = self.receive(&id, kind, max)?;
        Ok(Some(match kind {
            "STRING" => bytes.into_iter().map(char::from).collect(),
            _ => String::from_utf8_lossy(&bytes).into_owned(),
        }))
    }

    fn image(&mut self, max: usize) -> Result<Option<(Vec<u8>, &'static str)>, Failed> {
        let Some((id, types)) = self.offered()? else {
            return Ok(None);
        };
        let Some((kind, extension)) = super::preferred(&types.join("\n")) else {
            return Ok(None);
        };
        Ok(Some((self.receive(&id, kind, max)?, extension)))
    }

    /// The bytes the offer `id` gives in the type `kind`: an error if more than `max`.
    fn receive(&mut self, id: &ObjectId, kind: &str, max: usize) -> Result<Vec<u8>, Failed> {
        let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
        match self.state.offers.get(id) {
            Some((Offer::Ext(offer), _)) => offer.receive(kind.to_owned(), write.as_fd()),
            Some((Offer::Wlr(offer), _)) => offer.receive(kind.to_owned(), write.as_fd()),
            None => return Ok(Vec::new()),
        }
        self.connection.flush()?;
        drop(write);
        let mut file = File::from(read);
        let mut bytes = Vec::new();
        let mut chunk = vec![0; 64 * 1024];
        loop {
            let timeout = Timespec::try_from(self.timeout)?;
            let mut polled = [PollFd::new(&file, PollFlags::IN)];
            match rustix::event::poll(&mut polled, Some(&timeout)) {
                Ok(0) => {
                    let late = "the clipboard's owner did not answer in time";
                    return Err(Failed(late.to_owned()));
                }
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
            match file.read(&mut chunk) {
                Ok(0) => return Ok(bytes),
                Ok(read) => bytes.extend_from_slice(&chunk[..read]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.into()),
            }
            if bytes.len() > max {
                return Err(Failed(format!(
                    "the clipboard holds more than {} MB",
                    max.div_ceil(1 << 20)
                )));
            }
        }
    }

    /// Owns the clipboard and offers each type's bytes among `offers` until another client
    /// takes it; `owned` once it owns it.
    fn offer(mut self, offers: &[(&str, Vec<u8>)], owned: impl FnOnce()) -> Result<(), Failed> {
        let types = offers.iter().map(|(kind, _)| *kind);
        let handle = self.queue.handle();
        match &self.device {
            Device::Ext(manager, device) => {
                let source = manager.create_data_source(&handle, ());
                types.for_each(|kind| source.offer(kind.to_owned()));
                device.set_selection(Some(&source));
            }
            Device::Wlr(manager, device) => {
                let source = manager.create_data_source(&handle, ());
                types.for_each(|kind| source.offer(kind.to_owned()));
                device.set_selection(Some(&source));
            }
        }
        sync(
            &self.connection,
            &mut self.queue,
            &mut self.state,
            self.timeout,
        )?;
        if self.state.ended {
            return Err(Failed(
                "another program took the clipboard at once".to_owned(),
            ));
        }
        owned();
        let writing = Arc::new(AtomicUsize::new(0));
        loop {
            until(&mut self.queue, &mut self.state, None, |state| {
                !state.sends.is_empty() || state.ended
            })?;
            for (kind, fd) in self.state.sends.drain(..) {
                let Some((_, bytes)) = offers.iter().find(|(offered, _)| *offered == kind) else {
                    continue;
                };
                let bytes = bytes.clone();
                writing.fetch_add(1, Ordering::SeqCst);
                let writing = writing.clone();
                // Apart, so that a reader that stops reading holds up no other.
                std::thread::spawn(move || {
                    let _ = File::from(fd).write_all(&bytes);
                    writing.fetch_sub(1, Ordering::SeqCst);
                });
            }
            if self.state.ended {
                let deadline = Instant::now() + self.timeout;
                while writing.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return Ok(());
            }
        }
    }
}

/// Waits at most `timeout` for the compositor to have answered every request sent.
fn sync(
    connection: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    timeout: Duration,
) -> Result<(), Failed> {
    state.synced = false;
    connection.display().sync(&queue.handle(), ());
    until(queue, state, Some(Instant::now() + timeout), |state| {
        state.synced
    })
}

/// Dispatches what the compositor sends until `done`, if need be at most until `deadline`.
fn until(
    queue: &mut EventQueue<State>,
    state: &mut State,
    deadline: Option<Instant>,
    done: impl Fn(&State) -> bool,
) -> Result<(), Failed> {
    loop {
        queue.dispatch_pending(state)?;
        if done(state) {
            return Ok(());
        }
        queue.flush()?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let left = deadline.map(|until| until.saturating_duration_since(Instant::now()));
        if left.is_some_and(|left| left.is_zero()) {
            return Err(Failed("the compositor did not answer in time".to_owned()));
        }
        let timeout = left.map(Timespec::try_from).transpose()?;
        let ready = {
            let fd = guard.connection_fd();
            let mut polled = [PollFd::new(&fd, PollFlags::IN)];
            match rustix::event::poll(&mut polled, timeout.as_ref()) {
                Ok(0) | Err(rustix::io::Errno::INTR) => false,
                Ok(_) => true,
                Err(error) => return Err(error.into()),
            }
        };
        if !ready {
            continue;
        }
        match guard.read() {
            Ok(_) => {}
            Err(WaylandError::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wayland_client::protocol::wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_registry::Event;
        match event {
            Event::Global {
                name,
                interface,
                version,
            } => state.globals.push((name, interface, version)),
            Event::GlobalRemove { name } => state.globals.retain(|global| global.0 != name),
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        _: wayland_client::protocol::wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.synced = true;
    }
}

/// Events ignored: a seat's, and the manager's (it has none).
macro_rules! ignored {
    ($($interface:ty),*) => {$(
        impl Dispatch<$interface, ()> for State {
            fn event(
                _: &mut Self,
                _: &$interface,
                _: <$interface as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignored!(WlSeat, ExtDataControlManagerV1, ZwlrDataControlManagerV1);

/// The same events from either protocol's device, source and offer.
macro_rules! data_control {
    ($variant:ident, $device:ident, $Device:ty, $source:ident, $Source:ty, $offer:ident, $Offer:ty) => {
        impl Dispatch<$Device, ()> for State {
            fn event(
                state: &mut Self,
                _: &$Device,
                event: $device::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $device::Event::DataOffer { id } => {
                        let offer = (Offer::$variant(id.clone()), Vec::new());
                        state.offers.insert(id.id(), offer);
                    }
                    $device::Event::Selection { id } => {
                        state.selection = Some(id.map(|offer| offer.id()));
                    }
                    $device::Event::Finished => state.ended = true,
                    _ => {}
                }
            }

            event_created_child!(State, $Device, [
                $device::EVT_DATA_OFFER_OPCODE => ($Offer, ()),
            ]);
        }

        impl Dispatch<$Source, ()> for State {
            fn event(
                state: &mut Self,
                _: &$Source,
                event: $source::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $source::Event::Send { mime_type, fd } => state.sends.push((mime_type, fd)),
                    $source::Event::Cancelled => state.ended = true,
                    _ => {}
                }
            }
        }

        impl Dispatch<$Offer, ()> for State {
            fn event(
                state: &mut Self,
                offer: &$Offer,
                event: $offer::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let $offer::Event::Offer { mime_type } = event
                    && let Some((_, types)) = state.offers.get_mut(&offer.id())
                {
                    types.push(mime_type);
                }
            }
        }
    };
}

data_control!(
    Ext,
    ext_device,
    ExtDataControlDeviceV1,
    ext_source,
    ExtDataControlSourceV1,
    ext_offer,
    ExtDataControlOfferV1
);
data_control!(
    Wlr,
    wlr_device,
    ZwlrDataControlDeviceV1,
    wlr_source,
    ZwlrDataControlSourceV1,
    wlr_offer,
    ZwlrDataControlOfferV1
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::{process::Stdio, sync::mpsc};

    /// The scratch compositor the tests may use: never a desktop's, whose seat has one clipboard.
    fn compositor() -> Option<PathBuf> {
        let path = std::env::var_os("SCRAMJET_RS_TEST_WAYLAND").map(PathBuf::from);
        if path.is_none() {
            eprintln!("no SCRAMJET_RS_TEST_WAYLAND: the Wayland clipboard is not tested");
        }
        path
    }

    /// Offers `offers` on `path`'s clipboard from a thread, once it owns it; the receiver says
    /// how its offer ended.
    fn own(
        path: &Path,
        offers: Vec<(&'static str, Vec<u8>)>,
    ) -> mpsc::Receiver<Result<(), String>> {
        let (owned, owning) = mpsc::channel();
        let (ended, ending) = mpsc::channel();
        let path = path.to_owned();
        std::thread::spawn(move || {
            let offered = Clipboard::new(&path, Duration::from_secs(5))
                .and_then(|clipboard| clipboard.offer(&offers, || owned.send(()).unwrap()));
            let _ = ended.send(offered.map_err(|Failed(why)| why));
        });
        owning.recv_timeout(Duration::from_secs(5)).unwrap();
        ending
    }

    fn read(path: &Path) -> Clipboard {
        Clipboard::new(path, Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn the_clipboard_is_read_in_its_types_offered_and_given_up() {
        let Some(path) = compositor() else {
            return;
        };
        let text = "one\r\ntwo \u{2014} \u{754c}\n";
        let first = own(&path, offers(text));
        assert_eq!(read(&path).text(1 << 20).unwrap().as_deref(), Some(text));
        assert_eq!(read(&path).image(1 << 20).unwrap(), None);

        let second = own(&path, offers("caf\u{e9}"));
        assert_eq!(first.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        let mut latin1 = read(&path);
        let (id, types) = latin1.offered().unwrap().unwrap();
        assert_eq!(types, TEXT);
        assert_eq!(latin1.receive(&id, "STRING", 16).unwrap(), b"caf\xe9");
        assert_eq!(read(&path).text(16).unwrap().as_deref(), Some("caf\u{e9}"));
        let Err(Failed(why)) = read(&path).text(3) else {
            panic!("five bytes read within three");
        };
        assert_eq!(why, "the clipboard holds more than 1 MB");

        let long = "long text, ".repeat(500_000);
        let third = own(&path, offers(&long));
        assert_eq!(second.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        assert_eq!(read(&path).text(10 << 20).unwrap(), Some(long));

        let images = vec![
            ("text/html", b"<img>".to_vec()),
            ("image/jpeg", b"jpeg".to_vec()),
            ("image/png", b"png".to_vec()),
        ];
        let fourth = own(&path, images);
        assert_eq!(third.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        let image = read(&path).image(1 << 20).unwrap();
        assert_eq!(image, Some((b"png".to_vec(), "png")));
        assert_eq!(read(&path).text(1 << 20).unwrap(), None);

        // As wl-clipboard reads and writes it, when it is installed.
        let wl = |program: &str, arguments: &[&str]| {
            let mut command = Command::new(program);
            command.args(arguments).env("WAYLAND_DISPLAY", &path);
            command.stdin(Stdio::piped()).stdout(Stdio::piped());
            command.spawn().ok()
        };
        let Some(pasted) = wl("wl-paste", &["--list-types"]) else {
            eprintln!("no wl-paste: the Wayland clipboard is not checked against it");
            return;
        };
        let listed = String::from_utf8(pasted.wait_with_output().unwrap().stdout).unwrap();
        assert_eq!(listed, "text/html\nimage/jpeg\nimage/png\n");
        let fifth = own(&path, offers(text));
        assert_eq!(fourth.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        let pasted = wl("wl-paste", &["--no-newline"])
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert_eq!(String::from_utf8(pasted.stdout).unwrap(), text);
        let mut copying = wl("wl-copy", &[]).unwrap();
        copying
            .stdin
            .take()
            .unwrap()
            .write_all(b"from wl-copy")
            .unwrap();
        assert!(copying.wait().unwrap().success());
        assert_eq!(fifth.recv_timeout(Duration::from_secs(5)).unwrap(), Ok(()));
        assert_eq!(
            read(&path).text(1 << 20).unwrap().as_deref(),
            Some("from wl-copy")
        );
    }
}
