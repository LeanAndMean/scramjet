//! How many children run at once in this process, across every `subagent` call and conversation
//! thread: each takes a slot before it starts and gives it back when it ends, in the order they
//! asked. Only children hold slots and children cannot start children, so no holder waits on
//! another. The bound is this process's: another scramjet-rs counts its own.

use std::{
    collections::VecDeque,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// How often a waiting child looks at its cancellation.
const CANCEL_POLL: Duration = Duration::from_millis(50);

pub(super) struct Slots {
    capacity: usize,
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    running: usize,
    waiting: VecDeque<u64>,
    next: u64,
}

/// A slot taken, given back when dropped.
pub(super) struct Slot<'a>(&'a Slots);

impl Slots {
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(State {
                running: 0,
                waiting: VecDeque::new(),
                next: 0,
            }),
            changed: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for a slot after those who asked before; `None` once `cancel` is set.
    pub(super) fn take(&self, cancel: &AtomicBool) -> Option<Slot<'_>> {
        let mut state = self.lock();
        let ticket = state.next;
        state.next += 1;
        state.waiting.push_back(ticket);
        loop {
            if cancel.load(Ordering::Relaxed) {
                state.waiting.retain(|waiting| *waiting != ticket);
                self.changed.notify_all();
                return None;
            }
            if state.running < self.capacity && state.waiting.front() == Some(&ticket) {
                state.waiting.pop_front();
                state.running += 1;
                self.changed.notify_all();
                return Some(Slot(self));
            }
            state = (self.changed.wait_timeout(state, CANCEL_POLL))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.lock().running -= 1;
        self.0.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, mpsc},
        thread,
    };

    #[test]
    fn no_more_than_the_capacity_run_and_the_rest_start_in_the_order_they_asked() {
        let slots = Arc::new(Slots::new(2));
        let never = AtomicBool::new(false);
        let first = slots.take(&never).unwrap();
        let second = slots.take(&never).unwrap();
        let (started, order) = mpsc::channel();
        let mut waiters = Vec::new();
        for name in ["third", "fourth"] {
            let (mine, started) = (slots.clone(), started.clone());
            waiters.push(thread::spawn(move || {
                let slot = mine.take(&AtomicBool::new(false)).unwrap();
                started.send(name).unwrap();
                thread::sleep(Duration::from_millis(20));
                drop(slot);
            }));
            // The next asks only once this one waits.
            while slots.lock().waiting.len() < waiters.len() {
                thread::yield_now();
            }
        }
        assert!(order.recv_timeout(Duration::from_millis(200)).is_err());
        drop(first);
        assert_eq!(order.recv_timeout(Duration::from_secs(5)), Ok("third"));
        assert_eq!(slots.lock().running, 2);
        drop(second);
        assert_eq!(order.recv_timeout(Duration::from_secs(5)), Ok("fourth"));
        for waiter in waiters {
            waiter.join().unwrap();
        }
        assert_eq!(slots.lock().running, 0);
    }

    #[test]
    fn a_child_cancelled_while_waiting_takes_no_slot_and_holds_up_no_one() {
        let slots = Arc::new(Slots::new(1));
        let held = slots.take(&AtomicBool::new(false)).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = {
            let (slots, cancel) = (slots.clone(), cancel.clone());
            thread::spawn(move || slots.take(&cancel).is_none())
        };
        while slots.lock().waiting.is_empty() {
            thread::yield_now();
        }
        let behind = {
            let slots = slots.clone();
            thread::spawn(move || slots.take(&AtomicBool::new(false)).map(drop).is_some())
        };
        while slots.lock().waiting.len() < 2 {
            thread::yield_now();
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(cancelled.join().unwrap());
        drop(held);
        assert!(behind.join().unwrap());
        assert_eq!(slots.lock().running, 0);
        assert!(slots.lock().waiting.is_empty());
    }
}
