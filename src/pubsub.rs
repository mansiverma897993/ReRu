// SPDX-License-Identifier: AGPL-3.0-only
use crate::protocol;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const REPLY_IDLE: Duration = Duration::from_secs(30);
// A subscriber's output is buffered per connection, so a short pause never
// drops it mid-frame. Redis-like limits apply: hard cap, or staying above the
// soft cap for a minute.
const OUTPUT_HARD_LIMIT: usize = 32 * 1024 * 1024;
const OUTPUT_SOFT_LIMIT: usize = 8 * 1024 * 1024;
const OUTPUT_SOFT_DURATION: Duration = Duration::from_secs(60);
// How often an idle or blocked writer re-checks the soft limit.
const OUTPUT_TICK: Duration = Duration::from_secs(1);

// A socket write timeout reports WouldBlock after a partial write. Retry the
// remaining bytes while writes make progress, but give up after `idle` without
// progress or when the absolute deadline passes.
fn write_frame(
    writer: &mut TcpStream,
    bytes: &[u8],
    idle: Duration,
    deadline: Instant,
) -> io::Result<()> {
    let mut offset = 0;
    let mut idle_deadline = Instant::now() + idle;
    while offset < bytes.len() {
        let now = Instant::now();
        if now >= deadline || now >= idle_deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "write stalled"));
        }
        match writer.write(&bytes[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "write returned zero bytes",
                ))
            }
            Ok(written) => {
                offset += written;
                idle_deadline = Instant::now() + idle;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

type Writer = Arc<Mutex<TcpStream>>;

#[derive(Default)]
struct OutputQueue {
    frames: VecDeque<Arc<[u8]>>,
    // Queued plus in-flight bytes.
    bytes: usize,
    closed: bool,
    soft_since: Option<Instant>,
}

enum Next {
    Frame(Arc<[u8]>),
    Expired,
    Closed,
}

/// Per-subscriber output buffer. Frames are queued whole and written by the
/// subscriber's own writer thread, so publishing never blocks on a slow peer
/// and never leaves a half-written frame.
#[derive(Default)]
struct Output {
    queue: Mutex<OutputQueue>,
    available: Condvar,
}

impl Output {
    /// Queue one frame. Returns false when the client must be evicted.
    fn push(&self, frame: Arc<[u8]>) -> bool {
        let mut queue = self.queue.lock().unwrap();
        if queue.closed {
            return false;
        }
        queue.bytes = queue.bytes.saturating_add(frame.len());
        queue.frames.push_back(frame);
        if queue.bytes > OUTPUT_HARD_LIMIT {
            return false;
        }
        if queue.bytes > OUTPUT_SOFT_LIMIT {
            queue.soft_since.get_or_insert_with(Instant::now);
        } else {
            queue.soft_since = None;
        }
        self.available.notify_one();
        true
    }

    /// True when the client stayed above the soft limit for too long.
    fn soft_expired(&self) -> bool {
        let queue = self.queue.lock().unwrap();
        queue.bytes > OUTPUT_SOFT_LIMIT
            && queue
                .soft_since
                .is_some_and(|since| since.elapsed() >= OUTPUT_SOFT_DURATION)
    }

    /// Wait for a frame, waking at least once per tick to enforce the limits.
    fn next(&self) -> Next {
        let mut queue = self.queue.lock().unwrap();
        loop {
            if let Some(frame) = queue.frames.pop_front() {
                return Next::Frame(frame);
            }
            if queue.closed {
                return Next::Closed;
            }
            if queue.bytes > OUTPUT_SOFT_LIMIT
                && queue
                    .soft_since
                    .is_some_and(|since| since.elapsed() >= OUTPUT_SOFT_DURATION)
            {
                return Next::Expired;
            }
            let (next, _) = self.available.wait_timeout(queue, OUTPUT_TICK).unwrap();
            queue = next;
        }
    }

    fn completed(&self, len: usize) {
        let mut queue = self.queue.lock().unwrap();
        queue.bytes = queue.bytes.saturating_sub(len);
        if queue.bytes <= OUTPUT_SOFT_LIMIT {
            queue.soft_since = None;
        }
        self.available.notify_all();
    }

    fn close(&self) {
        self.queue.lock().unwrap().closed = true;
        self.available.notify_all();
    }

    /// Give the writer thread a bounded chance to flush the final replies
    /// before the connection closes.
    fn finish(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        let mut queue = self.queue.lock().unwrap();
        while queue.bytes > 0 && !queue.closed {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            let (next, _) = self.available.wait_timeout(queue, deadline - now).unwrap();
            queue = next;
        }
        queue.closed = true;
        self.available.notify_all();
    }
}

/// Write a queued frame, waiting as long as the peer needs. Memory stays
/// bounded by the output limits, which the loop also enforces while blocked.
fn write_buffered(stream: &mut TcpStream, frame: &[u8], output: &Output) -> io::Result<()> {
    let mut offset = 0;
    let mut backoff = Duration::from_millis(2);
    while offset < frame.len() {
        if output.soft_expired() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "output buffer limit exceeded",
            ));
        }
        match stream.write(&frame[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "write returned zero bytes",
                ))
            }
            Ok(written) => {
                offset += written;
                backoff = Duration::from_millis(2);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn writer_loop(mut stream: TcpStream, output: Arc<Output>) {
    loop {
        match output.next() {
            Next::Frame(frame) => match write_buffered(&mut stream, &frame, &output) {
                Ok(()) => output.completed(frame.len()),
                Err(_) => {
                    output.close();
                    let _ = stream.shutdown(Shutdown::Both);
                    return;
                }
            },
            Next::Expired => {
                output.close();
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            Next::Closed => return,
        }
    }
}

#[derive(Default)]
pub struct Hub {
    next_id: AtomicU64,
    listeners: Mutex<HashMap<u64, Listener>>,
    pattern_counts: Mutex<HashMap<Vec<u8>, usize>>,
}

struct Listener {
    writer: Writer,
    output: Option<Arc<Output>>,
    channels: HashSet<Vec<u8>>,
    patterns: HashSet<Vec<u8>>,
    held: bool,
    deferred: Vec<u8>,
}

/// Start the writer thread for a listener the first time it needs buffering.
fn ensure_output(listener: &mut Listener) -> io::Result<Arc<Output>> {
    if listener.output.is_none() {
        let stream = listener.writer.lock().unwrap().try_clone()?;
        let output = Arc::new(Output::default());
        let background = Arc::clone(&output);
        std::thread::Builder::new()
            .name("reru-subscriber".to_string())
            .spawn(move || writer_loop(stream, background))?;
        listener.output = Some(output);
    }
    Ok(Arc::clone(listener.output.as_ref().unwrap()))
}

fn decrement_pattern(counts: &mut HashMap<Vec<u8>, usize>, pattern: &[u8]) {
    if let Some(count) = counts.get_mut(pattern) {
        *count -= 1;
        if *count == 0 {
            counts.remove(pattern);
        }
    }
}

fn disconnected() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "subscriber disconnected")
}

pub struct ClientSubscription<'a> {
    id: u64,
    hub: &'a Hub,
}

impl Drop for ClientSubscription<'_> {
    fn drop(&mut self) {
        // A peer dropped by a failed publish may already have removed this
        // entry, and a poisoned lock must not panic during unwinding.
        let removed = self
            .hub
            .listeners
            .lock()
            .ok()
            .and_then(|mut listeners| listeners.remove(&self.id));
        // Flush and shut down outside the listeners lock so one slow client
        // cannot stall publishers.
        if let Some(listener) = removed {
            self.hub.remove_patterns(&listener.patterns);
            if let Some(output) = &listener.output {
                output.finish(Duration::from_millis(250));
            }
            if let Ok(writer) = listener.writer.lock() {
                let _ = writer.shutdown(Shutdown::Both);
            }
        }
    }
}

impl ClientSubscription<'_> {
    // RESET drops subscriptions without emitting unsubscribe acknowledgements.
    // Caller holds command_gate so no PUBLISH can race the RESET response.
    pub fn reset(&self) {
        if let Some(listener) = self.hub.listeners.lock().unwrap().get_mut(&self.id) {
            listener.channels.clear();
            self.hub.remove_patterns(&listener.patterns);
            listener.patterns.clear();
        }
    }

    pub fn active(&self) -> bool {
        self.hub
            .listeners
            .lock()
            .unwrap()
            .get(&self.id)
            .is_some_and(|listener| !listener.channels.is_empty() || !listener.patterns.is_empty())
    }

    // Apply a subscription change. The caller holds command_gate, so the
    // acknowledgements are queued before any later publish reaches this client.
    pub fn change(&self, command: &[Vec<u8>]) -> io::Result<()> {
        let mut listeners = self.hub.listeners.lock().unwrap();
        let Some(listener) = listeners.get_mut(&self.id) else {
            return Err(disconnected());
        };
        let frames = Self::apply_change(self.hub, listener, command)?;
        let output = ensure_output(listener)?;
        for frame in frames {
            if !output.push(Arc::from(frame.into_boxed_slice())) {
                self.hub.evict(&mut listeners, self.id);
                return Err(disconnected());
            }
        }
        Ok(())
    }

    /// Apply a subscription change and return the acknowledgement frames
    /// without writing them, so EXEC can nest them in its own reply.
    pub fn change_frame(&self, command: &[Vec<u8>]) -> io::Result<Vec<u8>> {
        let mut listeners = self.hub.listeners.lock().unwrap();
        let Some(listener) = listeners.get_mut(&self.id) else {
            return Err(disconnected());
        };
        let frames = Self::apply_change(self.hub, listener, command)?;
        let mut bytes = Vec::new();
        for frame in frames {
            bytes.extend_from_slice(&frame);
        }
        Ok(bytes)
    }

    fn apply_change(
        hub: &Hub,
        listener: &mut Listener,
        command: &[Vec<u8>],
    ) -> io::Result<Vec<Vec<u8>>> {
        let name = command[0].as_slice();
        let adding =
            name.eq_ignore_ascii_case(b"SUBSCRIBE") || name.eq_ignore_ascii_case(b"PSUBSCRIBE");
        let pattern =
            name.eq_ignore_ascii_case(b"PSUBSCRIBE") || name.eq_ignore_ascii_case(b"PUNSUBSCRIBE");
        if adding && command.len() == 1 {
            return Ok(vec![protocol::error(&format!(
                "wrong number of arguments for '{}' command",
                String::from_utf8_lossy(name).to_ascii_lowercase()
            ))]);
        }
        let kind = if adding {
            if pattern {
                b"psubscribe".as_slice()
            } else {
                b"subscribe"
            }
        } else if pattern {
            b"punsubscribe".as_slice()
        } else {
            b"unsubscribe".as_slice()
        };
        let items: Vec<Option<Vec<u8>>> = if !adding && command.len() == 1 {
            let subscriptions = if pattern {
                &listener.patterns
            } else {
                &listener.channels
            };
            if subscriptions.is_empty() {
                vec![None]
            } else {
                let mut items: Vec<_> = subscriptions.iter().cloned().map(Some).collect();
                items.sort();
                items
            }
        } else {
            command[1..].iter().cloned().map(Some).collect()
        };
        let mut frames = Vec::with_capacity(items.len());
        for item in items {
            if let Some(item) = &item {
                let subscriptions = if pattern {
                    &mut listener.patterns
                } else {
                    &mut listener.channels
                };
                if adding {
                    let inserted = subscriptions.insert(item.clone());
                    if pattern && inserted {
                        hub.add_pattern(item);
                    }
                } else if pattern {
                    if subscriptions.remove(item) {
                        hub.remove_pattern(item);
                    }
                } else {
                    subscriptions.remove(item);
                }
            }
            let count = listener.channels.len() + listener.patterns.len();
            frames.push(acknowledgement(kind, item.as_deref(), count));
        }
        Ok(frames)
    }

    /// Buffer publish deliveries until `release`, so a message published during
    /// the client's own EXEC cannot overtake the transaction reply.
    pub fn hold(&self) {
        if let Some(listener) = self.hub.listeners.lock().unwrap().get_mut(&self.id) {
            listener.held = true;
        }
    }

    pub fn release(&self) -> io::Result<()> {
        let mut listeners = self.hub.listeners.lock().unwrap();
        let Some(listener) = listeners.get_mut(&self.id) else {
            return Err(disconnected());
        };
        let deferred = std::mem::take(&mut listener.deferred);
        listener.held = false;
        if deferred.is_empty() {
            return Ok(());
        }
        let output = ensure_output(listener)?;
        if !output.push(Arc::from(deferred.into_boxed_slice())) {
            self.hub.evict(&mut listeners, self.id);
            return Err(disconnected());
        }
        Ok(())
    }

    /// Queue an EXEC reply so it precedes the deferred deliveries that follow.
    pub fn write_transaction(&self, reply: &[u8]) -> io::Result<()> {
        let mut listeners = self.hub.listeners.lock().unwrap();
        let Some(listener) = listeners.get_mut(&self.id) else {
            return Err(disconnected());
        };
        let output = ensure_output(listener)?;
        if output.push(Arc::from(reply)) {
            Ok(())
        } else {
            self.hub.evict(&mut listeners, self.id);
            Err(disconnected())
        }
    }

    pub fn write(&self, reply: &[u8]) -> io::Result<()> {
        let mut listeners = self.hub.listeners.lock().unwrap();
        let Some(listener) = listeners.get_mut(&self.id) else {
            return Err(disconnected());
        };
        if let Some(output) = &listener.output {
            if !output.push(Arc::from(reply)) {
                self.hub.evict(&mut listeners, self.id);
                return Err(disconnected());
            }
            return Ok(());
        }
        let writer = Arc::clone(&listener.writer);
        drop(listeners);
        let mut writer = writer.lock().unwrap();
        let deadline = Instant::now() + REPLY_IDLE;
        write_frame(&mut writer, reply, REPLY_IDLE, deadline)
    }
}

impl Hub {
    pub fn connect(&self, writer: Writer) -> ClientSubscription<'_> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.listeners.lock().unwrap().insert(
            id,
            Listener {
                writer,
                output: None,
                channels: HashSet::new(),
                patterns: HashSet::new(),
                held: false,
                deferred: Vec::new(),
            },
        );
        ClientSubscription { id, hub: self }
    }

    pub fn publish(&self, channel: &[u8], message: &[u8]) -> usize {
        let direct: Arc<[u8]> = Arc::from(message_reply(b"message", &[channel, message]));
        let mut listeners = self.listeners.lock().unwrap();
        let mut count = 0;
        let mut evicted = Vec::new();
        for (id, listener) in listeners.iter_mut() {
            let mut frames: Vec<Arc<[u8]>> = Vec::new();
            if listener.channels.contains(channel) {
                frames.push(Arc::clone(&direct));
            }
            for pattern in &listener.patterns {
                if glob_matches(pattern, channel) {
                    frames.push(Arc::from(message_reply(
                        b"pmessage",
                        &[pattern, channel, message],
                    )));
                }
            }
            if frames.is_empty() {
                continue;
            }
            count += frames.len();
            if listener.held {
                for frame in &frames {
                    listener.deferred.extend_from_slice(frame);
                }
                if listener.deferred.len() > OUTPUT_HARD_LIMIT {
                    evicted.push(*id);
                }
                continue;
            }
            let output = match &listener.output {
                Some(output) => Arc::clone(output),
                None => match ensure_output(listener) {
                    Ok(output) => output,
                    Err(_) => {
                        evicted.push(*id);
                        continue;
                    }
                },
            };
            for frame in frames {
                if !output.push(frame) {
                    evicted.push(*id);
                    break;
                }
            }
        }
        for id in evicted {
            self.evict(&mut listeners, id);
        }
        count
    }

    /// Drop a subscriber that exceeded its output limits. Its writer thread sees
    /// the closed buffer and the socket shutdown.
    fn evict(&self, listeners: &mut HashMap<u64, Listener>, id: u64) {
        if let Some(listener) = listeners.remove(&id) {
            self.remove_patterns(&listener.patterns);
            if let Some(output) = &listener.output {
                output.close();
            }
            let _ = listener.writer.lock().map(|w| w.shutdown(Shutdown::Both));
        }
    }

    fn channels(&self, pattern: Option<&[u8]>) -> Vec<Vec<u8>> {
        let listeners = self.listeners.lock().unwrap();
        let mut channels: Vec<_> = listeners
            .values()
            .flat_map(|listener| listener.channels.iter().cloned())
            .filter(|channel| pattern.is_none_or(|pattern| glob_matches(pattern, channel)))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        channels.sort();
        channels
    }

    fn numsub(&self, channels: &[Vec<u8>]) -> Vec<u8> {
        let listeners = self.listeners.lock().unwrap();
        let mut response = format!("*{}\r\n", channels.len() * 2).into_bytes();
        for channel in channels {
            response.extend(protocol::bulk(channel));
            let count = listeners
                .values()
                .filter(|listener| listener.channels.contains(channel))
                .count();
            response.extend(format!(":{count}\r\n").as_bytes());
        }
        response
    }

    fn add_pattern(&self, pattern: &[u8]) {
        let mut counts = self.pattern_counts.lock().unwrap();
        *counts.entry(pattern.to_vec()).or_insert(0) += 1;
    }

    fn remove_pattern(&self, pattern: &[u8]) {
        let mut counts = self.pattern_counts.lock().unwrap();
        decrement_pattern(&mut counts, pattern);
    }

    fn remove_patterns<'a>(&self, patterns: impl IntoIterator<Item = &'a Vec<u8>>) {
        // Drop tolerates a poisoned lock, so never panic while unwinding.
        let Ok(mut counts) = self.pattern_counts.lock() else {
            return;
        };
        for pattern in patterns {
            decrement_pattern(&mut counts, pattern);
        }
    }

    fn numpat(&self) -> usize {
        self.pattern_counts.lock().unwrap().len()
    }
}

pub fn is_subscription_command(command: &[Vec<u8>]) -> bool {
    [
        b"SUBSCRIBE".as_slice(),
        b"UNSUBSCRIBE",
        b"PSUBSCRIBE",
        b"PUNSUBSCRIBE",
    ]
    .iter()
    .any(|name| command[0].eq_ignore_ascii_case(name))
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    let name = &command[0];
    if name.eq_ignore_ascii_case(b"PUBLISH") {
        Some(command.len() == 3)
    } else if name.eq_ignore_ascii_case(b"PUBSUB") {
        Some(match command.get(1).map(Vec::as_slice) {
            Some(sub) if sub.eq_ignore_ascii_case(b"CHANNELS") => command.len() <= 3,
            Some(sub) if sub.eq_ignore_ascii_case(b"NUMSUB") => true,
            Some(sub) if sub.eq_ignore_ascii_case(b"NUMPAT") => command.len() == 2,
            _ => false,
        })
    } else {
        None
    }
}

pub fn execute(command: &[Vec<u8>], hub: &Hub) -> Option<Vec<u8>> {
    let name = &command[0];
    if name.eq_ignore_ascii_case(b"PUBLISH") {
        return Some(if command.len() == 3 {
            format!(":{}\r\n", hub.publish(&command[1], &command[2])).into_bytes()
        } else {
            protocol::error("wrong number of arguments for 'publish' command")
        });
    }
    if !name.eq_ignore_ascii_case(b"PUBSUB") {
        return None;
    }
    if valid_arity(command) != Some(true) {
        return Some(protocol::error(
            "wrong number of arguments for 'pubsub' command",
        ));
    }
    let sub = &command[1];
    Some(if sub.eq_ignore_ascii_case(b"CHANNELS") {
        protocol::array(&hub.channels(command.get(2).map(Vec::as_slice)))
    } else if sub.eq_ignore_ascii_case(b"NUMSUB") {
        hub.numsub(&command[2..])
    } else {
        format!(":{}\r\n", hub.numpat()).into_bytes()
    })
}

fn acknowledgement(kind: &[u8], item: Option<&[u8]>, count: usize) -> Vec<u8> {
    let mut reply = b"*3\r\n".to_vec();
    reply.extend(protocol::bulk(kind));
    reply.extend(item.map_or_else(|| b"$-1\r\n".to_vec(), protocol::bulk));
    reply.extend(format!(":{count}\r\n").as_bytes());
    reply
}

fn message_reply(kind: &[u8], items: &[&[u8]]) -> Vec<u8> {
    let mut reply = format!("*{}\r\n", items.len() + 1).into_bytes();
    reply.extend(protocol::bulk(kind));
    for item in items {
        reply.extend(protocol::bulk(item));
    }
    reply
}

pub(crate) fn glob_matches(pattern: &[u8], value: &[u8]) -> bool {
    let mut current = vec![false; value.len() + 1];
    current[0] = true;
    let mut index = 0;
    while index < pattern.len() {
        let mut next = vec![false; value.len() + 1];
        match pattern[index] {
            b'*' => {
                next[0] = current[0];
                for at in 1..=value.len() {
                    next[at] = current[at] || next[at - 1];
                }
            }
            b'?' => {
                next[1..].copy_from_slice(&current[..value.len()]);
            }
            b'[' if class_end(pattern, index).is_some() => {
                let end = class_end(pattern, index).unwrap();
                for at in 0..value.len() {
                    next[at + 1] =
                        current[at] && class_matches(&pattern[index + 1..end], value[at]);
                }
                index = end;
            }
            byte => {
                let literal = if byte == b'\\' && index + 1 < pattern.len() {
                    index += 1;
                    pattern[index]
                } else {
                    byte
                };
                for at in 0..value.len() {
                    next[at + 1] = current[at] && value[at] == literal;
                }
            }
        }
        current = next;
        index += 1;
    }
    current[value.len()]
}

fn class_end(pattern: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if pattern.get(index) == Some(&b'^') {
        index += 1;
    }
    let first = index;
    while index < pattern.len() {
        if pattern[index] == b'\\' && index + 1 < pattern.len() {
            index += 2;
        } else if pattern[index] == b']' && index > first {
            return Some(index);
        } else {
            index += 1;
        }
    }
    None
}

fn class_matches(class: &[u8], byte: u8) -> bool {
    let (negated, class) = if class.first() == Some(&b'^') {
        (true, &class[1..])
    } else {
        (false, class)
    };
    let mut index = 0;
    let mut matched = false;
    while index < class.len() {
        let first = if class[index] == b'\\' && index + 1 < class.len() {
            index += 1;
            class[index]
        } else {
            class[index]
        };
        index += 1;
        if index + 1 < class.len() && class[index] == b'-' {
            index += 1;
            let last = if class[index] == b'\\' && index + 1 < class.len() {
                index += 1;
                class[index]
            } else {
                class[index]
            };
            matched |= (first..=last).contains(&byte);
            index += 1;
        } else {
            matched |= first == byte;
        }
    }
    matched != negated
}

#[cfg(test)]
mod tests {
    use super::glob_matches;

    #[test]
    fn glob_is_binary_safe_and_case_sensitive() {
        assert!(glob_matches(b"cache:*", b"cache:one"));
        assert!(glob_matches(b"a?c", b"a\0c"));
        assert!(glob_matches(b"a\\*b", b"a*b"));
        assert!(glob_matches(b"key:[a-c]", b"key:b"));
        assert!(glob_matches(b"key:[^a-c]", b"key:z"));
        assert!(!glob_matches(b"key:[a-c]", b"key:z"));
        assert!(!glob_matches(b"Cache:*", b"cache:one"));
    }
}
