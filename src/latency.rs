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
        // Avoid self-generated samples and time spent waiting in blocking commands.
        if command[0].eq_ignore_ascii_case(b"LATENCY") || is_blocking(command) {
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
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.record_elapsed(elapsed, threshold_ms, timestamp);
    }

    fn record_elapsed(&self, elapsed: Duration, threshold_ms: u64, timestamp: u64) {
        if elapsed <= Duration::from_millis(threshold_ms) {
            return;
        }
        let latency_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        let mut data = self.data.lock().unwrap();
        data.max_latency_ms = data.max_latency_ms.max(latency_ms);
        if let Some(latest) = data.samples.back_mut() {
            if latest.timestamp == timestamp {
                latest.latency_ms = latest.latency_ms.max(latency_ms);
                return;
            }
        }
        if data.samples.len() == MAX_SAMPLES {
            data.samples.pop_front();
        }
        data.samples.push_back(Sample {
            timestamp,
            latency_ms,
        });
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
            let name = command
                .get(1)
                .filter(|name| {
                    [b"LATEST".as_slice(), b"HISTORY", b"RESET"]
                        .iter()
                        .any(|known| name.eq_ignore_ascii_case(known))
                })
                .map(|name| {
                    format!(
                        "latency|{}",
                        String::from_utf8_lossy(name).to_ascii_lowercase()
                    )
                })
                .unwrap_or_else(|| "latency".to_owned());
            return crate::protocol::error(&format!(
                "wrong number of arguments for '{name}' command"
            ));
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
        crate::protocol::unknown_subcommand("LATENCY", &command[1])
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

fn is_blocking(command: &[Vec<u8>]) -> bool {
    let name = command[0].as_slice();
    if [
        b"BLPOP".as_slice(),
        b"BRPOP",
        b"BLMOVE",
        b"BRPOPLPUSH",
        b"BLMPOP",
        b"BZPOPMIN",
        b"BZPOPMAX",
        b"BZMPOP",
    ]
    .iter()
    .any(|blocking| name.eq_ignore_ascii_case(blocking))
    {
        return true;
    }
    (name.eq_ignore_ascii_case(b"XREAD") || name.eq_ignore_ascii_case(b"XREADGROUP"))
        && command
            .iter()
            .any(|argument| argument.eq_ignore_ascii_case(b"BLOCK"))
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

#[cfg(test)]
mod tests {
    use super::{is_blocking, Monitor};
    use std::time::Duration;

    fn command(parts: &[&[u8]]) -> Vec<Vec<u8>> {
        parts.iter().map(|part| part.to_vec()).collect()
    }

    #[test]
    fn blocking_wait_commands_are_not_timed() {
        let monitor = Monitor::default();
        monitor.set_threshold_ms(1);
        for parts in [
            &[b"BLPOP".as_slice(), b"key", b"1"][..],
            &[b"BRPOP", b"key", b"1"],
            &[b"BLMOVE", b"src", b"dst", b"LEFT", b"RIGHT", b"1"],
            &[b"BRPOPLPUSH", b"source", b"dest", b"1"],
            &[b"BLMPOP", b"1", b"1", b"key", b"LEFT", b"COUNT", b"1"],
            &[b"BZPOPMIN", b"key", b"1"],
            &[b"BZPOPMAX", b"key", b"1"],
            &[b"BZMPOP", b"1", b"1", b"key", b"MIN", b"COUNT", b"1"],
            &[b"XREAD", b"BLOCK", b"100", b"STREAMS", b"s", b"0"],
            &[b"XREADGROUP", b"GROUP", b"g", b"c", b"BLOCK", b"100"],
        ] {
            assert!(is_blocking(&command(parts)));
            assert!(monitor.start(&command(parts)).is_none());
        }
        assert!(!is_blocking(&command(&[b"XREAD", b"STREAMS", b"s", b"0"])));
    }

    #[test]
    fn samples_in_the_same_second_merge_using_the_maximum() {
        let monitor = Monitor::default();
        monitor.record_elapsed(Duration::from_millis(8), 1, 123);
        monitor.record_elapsed(Duration::from_millis(4), 1, 123);
        monitor.record_elapsed(Duration::from_millis(12), 1, 123);

        assert_eq!(
            monitor.latest(),
            b"*1\r\n*4\r\n$7\r\ncommand\r\n:123\r\n:12\r\n:12\r\n"
        );
        assert_eq!(monitor.history(b"command"), b"*1\r\n*2\r\n:123\r\n:12\r\n");
        assert_eq!(
            monitor.execute(&command(&[b"LATENCY", b"RESET", b"command"])),
            b":1\r\n"
        );
        assert_eq!(monitor.latest(), b"*0\r\n");
        assert_eq!(monitor.history(b"command"), b"*0\r\n");
    }

    #[test]
    fn latency_arity_and_unknown_subcommand_errors_match_protocol_helpers() {
        let monitor = Monitor::default();
        assert_eq!(
            monitor.execute(&command(&[b"LATENCY", b"LATEST", b"extra"])),
            b"-ERR wrong number of arguments for 'latency|latest' command\r\n"
        );
        assert_eq!(
            monitor.execute(&command(&[b"LATENCY", b"HISTORY"])),
            b"-ERR wrong number of arguments for 'latency|history' command\r\n"
        );
        assert_eq!(
            monitor.execute(&command(&[b"LATENCY", b"BOGUS"])),
            b"-ERR unknown subcommand 'BOGUS'. Try LATENCY HELP.\r\n"
        );
    }
}
