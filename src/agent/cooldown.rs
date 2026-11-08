//! The waits providers asked for with Retry-After, shared by every request this process sends
//! to the same endpoint and model: once one is told to wait, the others wait too rather than
//! being refused in turn. Another scramjet-rs keeps its own.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// An endpoint, by host, port and path, and a model there.
type Key = (String, u16, String, String);

static UNTIL: LazyLock<Mutex<HashMap<Key, Instant>>> = LazyLock::new(Mutex::default);

fn key(route: &super::providers::Route, model: &str) -> Key {
    let (host, path) = (route.host.clone(), route.path.clone());
    (host, route.port, path, model.to_owned())
}

/// Keeps `wait`, from now, as the one requests to `model` on `route` keep, unless a later end
/// is kept already.
pub(super) fn hold(route: &super::providers::Route, model: &str, wait: Duration) {
    let Some(end) = Instant::now().checked_add(wait) else {
        return;
    };
    let mut until = UNTIL.lock().unwrap_or_else(PoisonError::into_inner);
    until.retain(|_, kept| *kept > Instant::now());
    let kept = until.entry(key(route, model)).or_insert(end);
    *kept = (*kept).max(end);
}

/// How long requests to `model` on `route` have yet to wait, if at all.
pub(super) fn left(route: &super::providers::Route, model: &str) -> Option<Duration> {
    let until = UNTIL.lock().unwrap_or_else(PoisonError::into_inner);
    let end = *until.get(&key(route, model))?;
    Some(end.checked_duration_since(Instant::now())?).filter(|left| !left.is_zero())
}
