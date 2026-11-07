// SPDX-License-Identifier: AGPL-3.0-only
use crate::protocol;
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

type Writer = Arc<Mutex<TcpStream>>;

#[derive(Default)]
pub struct Hub {
    next_id: AtomicU64,
    listeners: Mutex<HashMap<u64, Listener>>,
}

struct Listener {
    writer: Writer,
    channels: HashSet<Vec<u8>>,
    patterns: HashSet<Vec<u8>>,
}

pub struct ClientSubscription<'a> {
    id: u64,
    hub: &'a Hub,
}

impl Drop for ClientSubscription<'_> {
    fn drop(&mut self) {
        self.hub.listeners.lock().unwrap().remove(&self.id);
    }
}

impl ClientSubscription<'_> {
    // RESET drops subscriptions without emitting unsubscribe acknowledgements.
    // Caller holds command_gate so no PUBLISH can race the RESET response.
    pub fn reset(&self) {
        if let Some(listener) = self.hub.listeners.lock().unwrap().get_mut(&self.id) {
            listener.channels.clear();
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

    // The caller holds command_gate while writing these replies. This keeps
    // subscription acknowledgements ahead of later PUBLISH deliveries.
    pub fn change(&self, command: &[Vec<u8>]) -> io::Result<()> {
        let name = command[0].as_slice();
        let adding =
            name.eq_ignore_ascii_case(b"SUBSCRIBE") || name.eq_ignore_ascii_case(b"PSUBSCRIBE");
        let pattern =
            name.eq_ignore_ascii_case(b"PSUBSCRIBE") || name.eq_ignore_ascii_case(b"PUNSUBSCRIBE");
        if adding && command.len() == 1 {
            return self.write(&protocol::error(&format!(
                "wrong number of arguments for '{}' command",
                String::from_utf8_lossy(name).to_ascii_lowercase()
            )));
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
        let mut listeners = self.hub.listeners.lock().unwrap();
        let listener = listeners.get_mut(&self.id).unwrap();
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
        for item in items {
            if let Some(item) = &item {
                let subscriptions = if pattern {
                    &mut listener.patterns
                } else {
                    &mut listener.channels
                };
                if adding {
                    subscriptions.insert(item.clone());
                } else {
                    subscriptions.remove(item);
                }
            }
            let count = listener.channels.len() + listener.patterns.len();
            listener.writer.lock().unwrap().write_all(&acknowledgement(
                kind,
                item.as_deref(),
                count,
            ))?;
        }
        Ok(())
    }

    pub fn write(&self, reply: &[u8]) -> io::Result<()> {
        let writer = self
            .hub
            .listeners
            .lock()
            .unwrap()
            .get(&self.id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "subscriber disconnected"))?
            .writer
            .clone();
        let result = writer.lock().unwrap().write_all(reply);
        result
    }
}

impl Hub {
    pub fn connect(&self, writer: Writer) -> ClientSubscription<'_> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.listeners.lock().unwrap().insert(
            id,
            Listener {
                writer,
                channels: HashSet::new(),
                patterns: HashSet::new(),
            },
        );
        ClientSubscription { id, hub: self }
    }

    pub fn publish(&self, channel: &[u8], message: &[u8]) -> usize {
        let mut listeners = self.listeners.lock().unwrap();
        let direct = message_reply(b"message", &[channel, message]);
        let mut count = 0;
        let mut failed = Vec::new();
        for (id, listener) in listeners.iter() {
            let mut writer = listener.writer.lock().unwrap();
            let mut delivered = true;
            if listener.channels.contains(channel) {
                if writer.write_all(&direct).is_err() {
                    delivered = false;
                } else {
                    count += 1;
                }
            }
            if delivered {
                for pattern in &listener.patterns {
                    if glob_matches(pattern, channel) {
                        let reply = message_reply(b"pmessage", &[pattern, channel, message]);
                        if writer.write_all(&reply).is_err() {
                            delivered = false;
                            break;
                        }
                        count += 1;
                    }
                }
            }
            if !delivered {
                failed.push(*id);
            }
        }
        for id in failed {
            listeners.remove(&id);
        }
        count
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

    fn numpat(&self) -> usize {
        self.listeners
            .lock()
            .unwrap()
            .values()
            .map(|listener| listener.patterns.len())
            .sum()
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
