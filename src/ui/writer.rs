use rustix::fs::{Mode, OFlags};
use std::{
    fs::File,
    io::{self, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) enum Output {
    Stdout,
    Tty {
        file: Arc<File>,
        deadline: Arc<Mutex<Instant>>,
        quiesced: Arc<AtomicBool>,
    },
}
impl Output {
    pub fn tty() -> io::Result<Self> {
        let fd = rustix::fs::open(
            "/dev/tty",
            OFlags::WRONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(Self::Tty {
            file: Arc::new(File::from(fd)),
            deadline: Arc::new(Mutex::new(Instant::now() + Duration::from_millis(100))),
            quiesced: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn quiesce(&mut self) {
        if let Self::Tty { quiesced, .. } = self {
            quiesced.store(true, Ordering::Relaxed);
        }
    }
    pub fn begin(&mut self) {
        if let Self::Tty { deadline, .. } = self {
            *deadline.lock().unwrap() = Instant::now() + Duration::from_millis(100);
        }
    }
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout => io::stdout().write(bytes),
            Self::Tty {
                file,
                deadline,
                quiesced,
            } => {
                if quiesced.load(Ordering::Relaxed) {
                    return Ok(bytes.len());
                }
                let deadline = *deadline.lock().unwrap();
                loop {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "terminal operation exceeded 100ms",
                        ));
                    }
                    match (&**file).write(bytes) {
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) =>
                        {
                            std::thread::sleep(Duration::from_millis(1))
                        }
                        result => return result,
                    }
                }
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout => io::stdout().flush(),
            Self::Tty {
                deadline, quiesced, ..
            } => {
                if quiesced.load(Ordering::Relaxed) {
                    return Ok(());
                }
                if Instant::now() >= *deadline.lock().unwrap() {
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "terminal operation exceeded 100ms",
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn writes_and_flush_share_an_operation_deadline() {
        let file = File::options().write(true).open("/dev/null").unwrap();
        let mut writer = Output::Tty {
            file: Arc::new(file),
            deadline: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1))),
            quiesced: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(
            writer.write(b"x").unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(writer.flush().unwrap_err().kind(), io::ErrorKind::TimedOut);
        writer.begin();
        writer.write_all(b"x").unwrap();
        writer.clone().flush().unwrap();
    }
}
