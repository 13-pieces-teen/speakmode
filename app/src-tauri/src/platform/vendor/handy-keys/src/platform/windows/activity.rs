//! Content-free interaction epochs shared with the existing hook owner.
//! No extra hook, key history, cursor coordinates, or input injection.
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputActivitySnapshot {
    pub generation: u64,
    pub interactions: u64,
}

struct Tracker {
    owners: AtomicU64,
    generation: AtomicU64,
    interactions: AtomicU64,
    heartbeat: AtomicU64,
}
impl Tracker {
    const fn new() -> Self {
        Self { owners: AtomicU64::new(0), generation: AtomicU64::new(1),
            interactions: AtomicU64::new(0), heartbeat: AtomicU64::new(0) }
    }
    fn invalidate(&self) { self.generation.fetch_add(1, Ordering::SeqCst); }
    fn enter(&self, now: u64) {
        self.invalidate();
        self.owners.fetch_add(1, Ordering::SeqCst);
        self.invalidate();
        self.heartbeat.store(now, Ordering::SeqCst);
    }
    fn leave(&self) {
        self.invalidate();
        self.owners.fetch_sub(1, Ordering::SeqCst);
        self.invalidate();
    }
    fn observe(&self, message: u32, own_injected: bool) {
        if meaningful_message(message) && !own_injected {
            self.interactions.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn snapshot(&self, now: u64) -> Option<InputActivitySnapshot> {
        let generation = self.generation.load(Ordering::SeqCst);
        if self.owners.load(Ordering::SeqCst) != 1 { return None; }
        let heartbeat = self.heartbeat.load(Ordering::SeqCst);
        if heartbeat == 0 || now.checked_sub(heartbeat)? > 1_000 { return None; }
        let interactions = self.interactions.load(Ordering::SeqCst);
        (self.owners.load(Ordering::SeqCst) == 1 && self.generation.load(Ordering::SeqCst) == generation)
            .then_some(InputActivitySnapshot { generation, interactions })
    }
}
static TRACKER: Tracker = Tracker::new();
fn now_ms() -> u64 {
    static CLOCK: OnceLock<Instant> = OnceLock::new();
    CLOCK.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
}

pub fn input_activity_snapshot() -> Option<InputActivitySnapshot> { TRACKER.snapshot(now_ms()) }
pub(super) fn invalidate() { TRACKER.invalidate(); }
pub(super) fn heartbeat() { TRACKER.heartbeat.store(now_ms(), Ordering::SeqCst); }
pub(super) struct Owner;
impl Owner {
    pub(super) fn enter() -> Self { TRACKER.enter(now_ms()); Self }
}
impl Drop for Owner { fn drop(&mut self) { TRACKER.leave(); } }

// Releases complete the stop gesture and cannot move a caret on their own.
// Pointer motion is ignored; a drag is vetoed by its button-down epoch and
// the held-button check at capture/validation. Wheel and all button downs count.
fn meaningful_message(message: u32) -> bool {
    matches!(message, 0x0100 | 0x0104 | 0x0201 | 0x0204 | 0x0207 | 0x020B | 0x020A | 0x020E)
}
pub(super) fn observe(message: u32, own_injected: bool) {
    TRACKER.observe(message, own_injected);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hover_and_releases_do_not_count_but_click_wheel_and_key_down_do() {
        let tracker=Tracker::new();tracker.enter(10);
        let stopped=tracker.snapshot(10).unwrap();
        for message in [0x0200, 0x0202, 0x0205, 0x0208, 0x020C, 0x0101, 0x0105] {
            tracker.observe(message,false);
            assert_eq!(tracker.snapshot(10),Some(stopped));
        }
        tracker.observe(0x0100,true);
        assert_eq!(tracker.snapshot(10),Some(stopped));
        for message in [0x0100, 0x0104, 0x0201, 0x0204, 0x0207, 0x020B, 0x020A, 0x020E] {
            let before=tracker.snapshot(10).unwrap();
            tracker.observe(message,false);
            assert_ne!(tracker.snapshot(10),Some(before));
        }
    }
    #[test]
    fn absent_stale_restarted_and_multiple_owners_never_reuse_an_epoch() {
        let tracker = Tracker::new();
        assert!(tracker.snapshot(10).is_none());
        tracker.enter(10);
        let first = tracker.snapshot(10).unwrap();
        assert!(tracker.snapshot(1011).is_none());
        tracker.enter(20);
        assert!(tracker.snapshot(20).is_none());
        tracker.leave();
        let second = tracker.snapshot(20).unwrap();
        assert_ne!(first, second);
        tracker.invalidate();
        assert_ne!(second, tracker.snapshot(20).unwrap());
        tracker.leave();
        assert!(tracker.snapshot(20).is_none());
    }
}
