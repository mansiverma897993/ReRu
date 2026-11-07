// SPDX-License-Identifier: AGPL-3.0-only
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

pub const DEFAULT_MAX_CLIENTS: usize = 10_000;

pub struct Limits {
    maximum: AtomicUsize,
    active: AtomicUsize,
    received: AtomicU64,
    rejected: AtomicU64,
}

impl Limits {
    pub fn new(maximum: usize) -> Self {
        Self {
            maximum: AtomicUsize::new(maximum),
            active: AtomicUsize::new(0),
            received: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
        }
    }

    pub fn maximum(&self) -> usize {
        self.maximum.load(Ordering::Relaxed)
    }
    pub fn set_maximum(&self, maximum: usize) {
        self.maximum.store(maximum, Ordering::Relaxed);
    }

    pub fn acquire(self: &Arc<Self>) -> Option<Permit> {
        self.received.fetch_add(1, Ordering::Relaxed);
        let maximum = self.maximum();
        let mut active = self.active.load(Ordering::Relaxed);
        loop {
            if active >= maximum {
                self.rejected.fetch_add(1, Ordering::Relaxed);
                return None;
            }
            match self.active.compare_exchange_weak(
                active,
                active + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(Permit(Arc::clone(self))),
                Err(current) => active = current,
            }
        }
    }

    pub fn info(&self) -> String {
        format!(
            "# Clients\r\nconnected_clients:{}\r\nmaxclients:{}\r\n\r\n",
            self.active.load(Ordering::Relaxed),
            self.maximum()
        )
    }

    pub fn stats(&self) -> String {
        format!(
            "total_connections_received:{}\r\nrejected_connections:{}\r\n",
            self.received.load(Ordering::Relaxed),
            self.rejected.load(Ordering::Relaxed)
        )
    }
}

// Covers QUIT, malformed input, disconnected clients and failed thread spawns.
pub struct Permit(Arc<Limits>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}
