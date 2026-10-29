use crate::CodingError;
pub(super) struct Pause {
    pub point: Point,
    pub entered: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
}
impl Pause {
    pub(super) fn hit(&self, point: Point) {
        if self.point == point {
            self.entered
                .store(true, std::sync::atomic::Ordering::Release);
            let (lock, changed) = &*self.gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = changed.wait(released).unwrap();
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Point {
    Reserve,
    Append,
    ShortAppend,
    Sync,
    Authorize,
    Authorized,
    Dispatch,
    Dispatched,
    CreateOpen,
    PartialWrite,
    FileSync,
    DirectorySync,
    Created,
    Start,
    Reaped,
    ProcessAck,
    Result,
}
#[derive(Clone, Copy)]
pub(super) struct Fault {
    pub point: Point,
    pub exit: bool,
}
impl Fault {
    pub(super) fn hit(self, point: Point) -> Result<(), CodingError> {
        if self.point == point {
            if self.exit {
                std::process::exit(29);
            }
            return Err(
                std::io::Error::other("injected haiku persistence/effect interruption").into(),
            );
        }
        Ok(())
    }
}
