// SPDX-License-Identifier: AGPL-3.0-only
use std::io;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_MAX_CLIENTS: usize = 10_000;

/// A short read timeout lets a blocked command observe FIN/RST without a
/// second thread or consuming bytes from the client's command stream.
pub struct DisconnectProbe(TcpStream);

impl DisconnectProbe {
    pub fn new(stream: &TcpStream) -> io::Result<Self> {
        let probe = stream.try_clone()?;
        probe.set_read_timeout(Some(Duration::from_millis(1)))?;
        Ok(Self(probe))
    }

    pub fn disconnected(&self) -> bool {
        match self.0.peek(&mut [0; 1]) {
            Ok(0) => true,
            Ok(_) => false,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                false
            }
            Err(_) => true,
        }
    }
}

impl Drop for DisconnectProbe {
    fn drop(&mut self) {
        // Clones share the socket option; restore blocking reads before the
        // command loop reads another request on this connection.
        let _ = self.0.set_read_timeout(None);
    }
}

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

    pub fn reset_stats(&self) {
        self.received.store(0, Ordering::Relaxed);
        self.rejected.store(0, Ordering::Relaxed);
    }
}

// Covers QUIT, malformed input, disconnected clients and failed thread spawns.
pub struct Permit(Arc<Limits>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}
