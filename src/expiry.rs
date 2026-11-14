// SPDX-License-Identifier: AGPL-3.0-only
//! Deadline-indexed expiry on primaries. One index record per expiring key keeps
//! repeated TTL updates from accumulating stale timer records.
use crate::{mark_evicted, save_and_publish, Entry, Store};
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CYCLE_INTERVAL: Duration = Duration::from_millis(100);
const MAX_KEYS_PER_CYCLE: usize = 256;
const INDEX_TIME_BUDGET: Duration = Duration::from_millis(5);

#[derive(Default)]
struct Index {
    deadlines: BTreeSet<(Instant, Vec<u8>)>,
    by_key: HashMap<Vec<u8>, Instant>,
}

impl Index {
    fn set(&mut self, key: &[u8], deadline: Option<Instant>) {
        if let Some(old) = self.by_key.remove(key) {
            self.deadlines.remove(&(old, key.to_vec()));
        }
        if let Some(deadline) = deadline {
            self.by_key.insert(key.to_vec(), deadline);
            self.deadlines.insert((deadline, key.to_vec()));
        }
    }
}

#[derive(Default)]
pub struct Expiry {
    index: Mutex<Index>,
    active_expired_keys: AtomicU64,
}

impl Expiry {
    // Caller holds the command gate and entries lock, after memory-limit rollback.
    pub fn update(&self, entries: &HashMap<Vec<u8>, Entry>, keys: &[Vec<u8>]) {
        let mut index = self.index.lock().unwrap();
        for key in keys {
            index.set(key, entries.get(key).and_then(|entry| entry.expires_at));
        }
    }

    pub fn rebuild(&self, entries: &HashMap<Vec<u8>, Entry>) {
        let mut index = self.index.lock().unwrap();
        *index = Index::default();
        for (key, entry) in entries {
            index.set(key, entry.expires_at);
        }
    }

    pub fn indexed_keys(&self) -> usize {
        self.index.lock().unwrap().by_key.len()
    }

    pub fn active_expired_keys(&self) -> u64 {
        self.active_expired_keys.load(Ordering::Relaxed)
    }

    pub fn reset_stats(&self) {
        self.active_expired_keys.store(0, Ordering::Relaxed);
    }
}

// The caller holds command_gate. Deletion and persistence follow the same
// serialization order as client writes and EXEC.
fn expire_cycle(store: &Store) {
    let started = Instant::now();
    let mut removed = 0;
    let mut changed = Vec::new();
    {
        let mut entries = store.entries.lock().unwrap();
        let mut index = store.expiry.index.lock().unwrap();
        for _ in 0..MAX_KEYS_PER_CYCLE {
            if started.elapsed() >= INDEX_TIME_BUDGET
                || index.deadlines.first().is_none_or(|(at, _)| *at > started)
            {
                break;
            }
            let (deadline, key) = index.deadlines.pop_first().unwrap();
            index.by_key.remove(&key);
            match entries.get(&key) {
                Some(entry) if entry.expires_at == Some(deadline) => {
                    entries.remove(&key);
                    removed += 1;
                }
                Some(entry) => {
                    // Defensive reconciliation: a new value must never be deleted
                    // by the deadline belonging to an older value.
                    index.set(&key, entry.expires_at);
                    continue;
                }
                // A passive read already removed this key; still persist and
                // propagate its deletion and discard eviction bookkeeping.
                None => {}
            }
            changed.push(key);
        }
    }
    store
        .expiry
        .active_expired_keys
        .fetch_add(removed, Ordering::Relaxed);
    for key in changed {
        store.eviction.forget(&key);
        mark_evicted(store, &key);
    }
}

pub fn start(store: &Store) -> io::Result<()> {
    if store.read_only {
        return Ok(());
    }
    store.expiry.rebuild(&store.entries.lock().unwrap());
    let weak = Arc::downgrade(store);
    std::thread::Builder::new()
        .name("active-expiry".into())
        .spawn(move || {
            let mut reported_error = false;
            loop {
                std::thread::sleep(CYCLE_INTERVAL);
                let Some(store) = weak.upgrade() else {
                    break;
                };
                let Ok(_gate) = store.command_gate.try_lock() else {
                    continue;
                };
                expire_cycle(&store);
                // Also retry changes retained after an earlier persistence failure.
                match save_and_publish(&store) {
                    Ok(()) => reported_error = false,
                    Err(error) if !reported_error => {
                        eprintln!("expiry persistence failed; will retry: {error}");
                        reported_error = true;
                    }
                    Err(_) => {}
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_updates_and_removal_do_not_leave_stale_deadlines() {
        let mut index = Index::default();
        let start = Instant::now();
        for i in 0..10_000 {
            index.set(b"key", Some(start + Duration::from_millis(i)));
        }
        assert_eq!(index.deadlines.len(), 1);
        assert_eq!(index.by_key.len(), 1);
        assert_eq!(
            index.deadlines.first().unwrap().0,
            start + Duration::from_millis(9_999)
        );
        index.set(b"key", None);
        assert!(index.by_key.is_empty());
        assert!(index.deadlines.is_empty());
    }
}
