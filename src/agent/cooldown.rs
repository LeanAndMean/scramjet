//! The waits providers asked for with Retry-After, shared by every request this process sends
//! to the same endpoint and model: once one is told to wait, the others wait too rather than
//! being refused in turn. They then start one after another, `GAP` apart, rather than all at
//! the wait's end. Another scramjet-rs keeps its own.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// Between two requests' starts after a shared wait.
const GAP: Duration = Duration::from_millis(250);

/// An endpoint, by host, port and path, and a model there.
type Key = (String, u16, String, String);

/// The wait's end, and the first start not yet given to a request.
struct Held {
    end: Instant,
    next: Instant,
}

static HELD: LazyLock<Mutex<HashMap<Key, Held>>> = LazyLock::new(Mutex::default);

fn key(route: &super::providers::Route, model: &str) -> Key {
    let (host, path) = (route.host.clone(), route.path.clone());
    (host, route.port, path, model.to_owned())
}

/// Keeps `wait`, from now, as the one requests to `model` on `route` keep, unless a later end
/// is kept already. A refused request that `retries` at the end takes its first start.
pub(super) fn hold(route: &super::providers::Route, model: &str, wait: Duration, retries: bool) {
    let Some(end) = Instant::now().checked_add(wait) else {
        return;
    };
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    held.retain(|_, held| held.end > Instant::now());
    let held = held
        .entry(key(route, model))
        .or_insert(Held { end, next: end });
    held.end = held.end.max(end);
    held.next = held.next.max(held.end);
    if retries {
        held.next += GAP;
    }
}

/// How long a request to `model` on `route` is to wait before it starts, if at all, taking
/// that start; none is taken when the wait left is over `max_wait`.
pub(super) fn take(
    route: &super::providers::Route,
    model: &str,
    max_wait: Duration,
) -> Option<Duration> {
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    let held = held.get_mut(&key(route, model))?;
    let now = Instant::now();
    if held.end <= now || held.end - now > max_wait {
        return None;
    }
    let start = held.next.max(held.end);
    held.next = start + GAP;
    Some(start - now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> super::super::providers::Route {
        super::super::providers::Route {
            provider: "local".into(),
            tls: false,
            host: "127.0.0.1".into(),
            port: 9,
            authority: "127.0.0.1:9".into(),
            path: "/v1".into(),
            headers: Vec::new(),
            commanded: Vec::new(),
            login: None,
        }
    }

    #[test]
    fn requests_held_by_one_wait_start_a_gap_apart_after_the_refused_one() {
        let (route, long) = (route(), Duration::from_secs(60));
        hold(&route, "spaced", Duration::from_secs(1), true);
        let starts: Vec<Duration> = (0..3)
            .map(|_| take(&route, "spaced", long).unwrap())
            .collect();
        let near = |start: Duration, millis: u64| {
            let expected = Duration::from_millis(millis);
            start <= expected && expected - start < Duration::from_millis(100)
        };
        assert!(near(starts[0], 1250), "{starts:?}");
        assert!(near(starts[1], 1500), "{starts:?}");
        assert!(near(starts[2], 1750), "{starts:?}");
        // One that will not retry leaves the first start to the others.
        hold(&route, "unclaimed", Duration::from_secs(1), false);
        assert!(near(take(&route, "unclaimed", long).unwrap(), 1000));
        // A wait over the settings' limit is not waited for, and takes no start.
        assert_eq!(take(&route, "unclaimed", Duration::from_millis(500)), None);
        assert!(near(take(&route, "unclaimed", long).unwrap(), 1250));
        assert_eq!(take(&route, "never held", long), None);
    }

    #[test]
    fn a_longer_wait_given_meanwhile_moves_the_starts_after_it() {
        let (route, long) = (route(), Duration::from_secs(60));
        hold(&route, "extended", Duration::from_secs(1), false);
        hold(&route, "extended", Duration::from_secs(3), true);
        let start = take(&route, "extended", long).unwrap();
        assert!(start > Duration::from_millis(3150) && start <= Duration::from_millis(3250));
        // A shorter one does not shorten it.
        hold(&route, "extended", Duration::from_secs(1), false);
        assert!(take(&route, "extended", long).unwrap() > Duration::from_millis(3400));
    }
}
