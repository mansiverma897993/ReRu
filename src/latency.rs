// SPDX-License-Identifier: AGPL-3.0-only
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_SAMPLES: usize = 160;
const COMMAND_EVENT: &[u8] = b"command";

#[derive(Clone, Copy)]
struct Sample {
    timestamp: u64,
    latency_ms: u64,
}

pub struct Monitor {
    threshold_ms: AtomicU64,
    data: Mutex<MonitorData>,
}

#[derive(Default)]
struct MonitorData {
    samples: VecDeque<Sample>,
    max_latency_ms: u64,
}

impl Default for Monitor {
    fn default() -> Self {
        Self {
            threshold_ms: AtomicU64::new(0),
            data: Mutex::new(MonitorData::default()),
        }
    }
}

impl Monitor {
    pub fn threshold_ms(&self) -> u64 {
        self.threshold_ms.load(Ordering::Relaxed)
    }

    pub fn set_threshold_ms(&self, threshold_ms: u64) {
        self.threshold_ms.store(threshold_ms, Ordering::Relaxed);
    }

    // The disabled path only loads the threshold; it does not read the clock or lock.
    pub fn start(&self, command: &[Vec<u8>]) -> Option<(Instant, u64)> {
        // Avoid self-generated samples from the commands that manage them.
        if command[0].eq_ignore_ascii_case(b"LATENCY") {
            return None;
        }
        let threshold_ms = self.threshold_ms();
        (threshold_ms > 0).then(|| (Instant::now(), threshold_ms))
    }

    pub fn record(&self, started: Option<(Instant, u64)>) {
        let Some((started, threshold_ms)) = started else {
            return;
        };
        if self.threshold_ms() == 0 {
            return;
        }
        let elapsed = started.elapsed();
        if elapsed <= Duration::from_millis(threshold_ms) {
            return;
        }
        let latency_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut data = self.data.lock().unwrap();
        if data.samples.len() == MAX_SAMPLES {
            data.samples.pop_front();
        }
        data.samples.push_back(Sample {
            timestamp,
            latency_ms,
        });
        data.max_latency_ms = data.max_latency_ms.max(latency_ms);
    }

    pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
        if !command[0].eq_ignore_ascii_case(b"LATENCY") {
            return None;
        }
        Some(match command.get(1).map(Vec::as_slice) {
            Some(subcommand) if subcommand.eq_ignore_ascii_case(b"LATEST") => command.len() == 2,
            Some(subcommand) if subcommand.eq_ignore_ascii_case(b"HISTORY") => command.len() == 3,
            Some(subcommand) if subcommand.eq_ignore_ascii_case(b"RESET") => command.len() >= 2,
            Some(_) => command.len() >= 2,
            None => false,
        })
    }

    pub fn execute(&self, command: &[Vec<u8>]) -> Vec<u8> {
        if Self::valid_arity(command) != Some(true) {
            return crate::protocol::error("wrong number of arguments for 'latency' command");
        }
        if command[1].eq_ignore_ascii_case(b"LATEST") {
            return self.latest();
        }
        if command[1].eq_ignore_ascii_case(b"HISTORY") {
            return self.history(&command[2]);
        }
        if command[1].eq_ignore_ascii_case(b"RESET") {
            return self.reset(&command[2..]);
        }
        crate::protocol::error(&format!(
            "unknown subcommand '{}' for 'latency' command",
            String::from_utf8_lossy(&command[1]).to_ascii_lowercase()
        ))
    }

    fn latest(&self) -> Vec<u8> {
        let data = self.data.lock().unwrap();
        if data.samples.is_empty() {
            return resp_array(&[]);
        }
        let latest = data.samples.back().unwrap();
        resp_array(&[resp_array(&[
            crate::protocol::bulk(COMMAND_EVENT),
            resp_integer(latest.timestamp),
            resp_integer(latest.latency_ms),
            resp_integer(data.max_latency_ms),
        ])])
    }

    fn history(&self, event: &[u8]) -> Vec<u8> {
        if !event.eq_ignore_ascii_case(COMMAND_EVENT) {
            return resp_array(&[]);
        }
        let data = self.data.lock().unwrap();
        let replies: Vec<_> = data
            .samples
            .iter()
            .map(|sample| {
                resp_array(&[
                    resp_integer(sample.timestamp),
                    resp_integer(sample.latency_ms),
                ])
            })
            .collect();
        resp_array(&replies)
    }

    fn reset(&self, events: &[Vec<u8>]) -> Vec<u8> {
        let mut data = self.data.lock().unwrap();
        if events.is_empty() {
            let reset = usize::from(!data.samples.is_empty());
            data.samples.clear();
            data.max_latency_ms = 0;
            return resp_integer(reset as u64);
        }
        let requested = events
            .iter()
            .any(|event| event.eq_ignore_ascii_case(COMMAND_EVENT));
        let reset = usize::from(requested && !data.samples.is_empty());
        if requested {
            data.samples.clear();
            data.max_latency_ms = 0;
        }
        resp_integer(reset as u64)
    }
}

fn resp_integer(value: u64) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

fn resp_array(values: &[Vec<u8>]) -> Vec<u8> {
    let mut response = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        response.extend_from_slice(value);
    }
    response
}
