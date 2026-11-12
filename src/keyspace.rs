// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, pubsub::glob_matches, remove_expired, Store, Value};
use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    let name = &command[0];
    if name.eq_ignore_ascii_case(b"DEL")
        || name.eq_ignore_ascii_case(b"UNLINK")
        || name.eq_ignore_ascii_case(b"EXISTS")
        || name.eq_ignore_ascii_case(b"TOUCH")
    {
        Some(command.len() >= 2)
    } else if name.eq_ignore_ascii_case(b"DBSIZE") {
        Some(command.len() == 1)
    } else if name.eq_ignore_ascii_case(b"KEYS") {
        Some(command.len() == 2)
    } else if name.eq_ignore_ascii_case(b"RANDOMKEY") {
        Some(command.len() == 1)
    } else if name.eq_ignore_ascii_case(b"SCAN") {
        Some(command.len() >= 2 && command.len().is_multiple_of(2))
    } else if name.eq_ignore_ascii_case(b"RENAME") || name.eq_ignore_ascii_case(b"RENAMENX") {
        Some(command.len() == 3)
    } else if name.eq_ignore_ascii_case(b"COPY") {
        Some(command.len() == 3 || command.len() == 4)
    } else if [
        b"TTL".as_slice(),
        b"PTTL",
        b"EXPIRETIME",
        b"PEXPIRETIME",
        b"PERSIST",
    ]
    .iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
    {
        Some(command.len() == 2)
    } else if [b"EXPIRE".as_slice(), b"PEXPIRE", b"EXPIREAT", b"PEXPIREAT"]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
    {
        Some(command.len() >= 3)
    } else {
        None
    }
}

fn kind(value: &Value) -> &'static [u8] {
    match value {
        Value::String(_) => b"string",
        Value::List(_) => b"list",
        Value::Stream(_) => b"stream",
        Value::Hash(_) => b"hash",
        Value::Set(_) => b"set",
        Value::SortedSet(_) => b"zset",
    }
}

fn scan(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let cursor = match std::str::from_utf8(&command[1])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(value) => value,
        None => return protocol::error("invalid cursor"),
    };
    let mut pattern = None;
    let mut count = 10usize;
    let mut value_type = None;
    for pair in command[2..].as_chunks::<2>().0 {
        if pair[0].eq_ignore_ascii_case(b"MATCH") {
            pattern = Some(pair[1].as_slice());
        } else if pair[0].eq_ignore_ascii_case(b"COUNT") {
            count = match std::str::from_utf8(&pair[1])
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
            {
                Some(value) => value,
                None => return protocol::error("value is not an integer or out of range"),
            };
        } else if pair[0].eq_ignore_ascii_case(b"TYPE") {
            value_type = Some(pair[1].as_slice());
        } else {
            return protocol::error("syntax error");
        }
    }
    let now = Instant::now();
    let entries = store.entries.lock().unwrap();
    let mut keys: Vec<_> = entries
        .iter()
        .filter(|(_, entry)| entry.expires_at.is_none_or(|deadline| deadline > now))
        .map(|(key, _)| key.clone())
        .collect();
    keys.sort();
    let start = cursor.min(keys.len());
    let end = start.saturating_add(count).min(keys.len());
    let page: Vec<_> = keys[start..end]
        .iter()
        .filter(|key| {
            pattern.is_none_or(|pattern| glob_matches(pattern, key))
                && value_type.is_none_or(|value_type| {
                    kind(&entries.get(*key).unwrap().value).eq_ignore_ascii_case(value_type)
                })
        })
        .cloned()
        .collect();
    let next = if end == keys.len() { 0 } else { end };
    let mut reply = b"*2\r\n".to_vec();
    reply.extend(protocol::bulk(next.to_string().as_bytes()));
    reply.extend(protocol::array(&page));
    reply
}

#[derive(Default)]
struct ExpiryConditions {
    absent: bool,
    present: bool,
    longer: bool,
    shorter: bool,
}

fn expiry_arguments(command: &[Vec<u8>]) -> Result<(i64, ExpiryConditions), Vec<u8>> {
    let mut conditions = ExpiryConditions::default();
    for option in &command[3..] {
        if option.eq_ignore_ascii_case(b"NX") {
            conditions.absent = true;
        } else if option.eq_ignore_ascii_case(b"XX") {
            conditions.present = true;
        } else if option.eq_ignore_ascii_case(b"GT") {
            conditions.longer = true;
        } else if option.eq_ignore_ascii_case(b"LT") {
            conditions.shorter = true;
        } else {
            return Err(protocol::error(&format!(
                "Unsupported option {}",
                String::from_utf8_lossy(option)
            )));
        }
    }
    if conditions.absent && (conditions.present || conditions.longer || conditions.shorter) {
        return Err(protocol::error(
            "NX and XX, GT or LT options at the same time are not compatible",
        ));
    }
    if conditions.longer && conditions.shorter {
        return Err(protocol::error(
            "GT and LT options at the same time are not compatible",
        ));
    }
    let raw = &command[2];
    let number = std::str::from_utf8(raw)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .filter(|number| number.to_string().as_bytes() == raw)
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))?;
    let invalid_expiry = || {
        protocol::error(&format!(
            "invalid expire time in '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        ))
    };
    let milliseconds = if command[0].eq_ignore_ascii_case(b"EXPIRE")
        || command[0].eq_ignore_ascii_case(b"EXPIREAT")
    {
        number.checked_mul(1000).ok_or_else(invalid_expiry)?
    } else {
        number
    };
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .ok_or_else(invalid_expiry)?;
    if command[0].eq_ignore_ascii_case(b"EXPIREAT") || command[0].eq_ignore_ascii_case(b"PEXPIREAT")
    {
        Ok((milliseconds.saturating_sub(epoch_ms), conditions))
    } else {
        epoch_ms
            .checked_add(milliseconds)
            .ok_or_else(invalid_expiry)?;
        Ok((milliseconds, conditions))
    }
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let valid = valid_arity(command)?;
    let name = &command[0];
    if !valid {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(name).to_ascii_lowercase()
        )));
    }
    if name.eq_ignore_ascii_case(b"DBSIZE") {
        let entries = store.entries.lock().unwrap();
        let now = Instant::now();
        let count = entries
            .values()
            .filter(|entry| entry.expires_at.is_none_or(|deadline| deadline > now))
            .count();
        return Some(format!(":{count}\r\n").into_bytes());
    }
    if name.eq_ignore_ascii_case(b"KEYS") {
        let entries = store.entries.lock().unwrap();
        let now = Instant::now();
        let mut keys: Vec<_> = entries
            .iter()
            .filter(|(key, entry)| {
                entry.expires_at.is_none_or(|deadline| deadline > now)
                    && glob_matches(&command[1], key)
            })
            .map(|(key, _)| key.clone())
            .collect();
        keys.sort();
        return Some(protocol::array(&keys));
    }
    if name.eq_ignore_ascii_case(b"SCAN") {
        return Some(scan(command, store));
    }
    if name.eq_ignore_ascii_case(b"RANDOMKEY") {
        let random = RandomState::new();
        let now = Instant::now();
        let entries = store.entries.lock().unwrap();
        let key = entries
            .iter()
            .filter(|(_, entry)| entry.expires_at.is_none_or(|deadline| deadline > now))
            .min_by_key(|(key, _)| random.hash_one(key))
            .map(|(key, _)| key);
        return Some(key.map_or_else(|| b"$-1\r\n".to_vec(), |key| protocol::bulk(key)));
    }
    if name.eq_ignore_ascii_case(b"DEL")
        || name.eq_ignore_ascii_case(b"UNLINK")
        || name.eq_ignore_ascii_case(b"EXISTS")
        || name.eq_ignore_ascii_case(b"TOUCH")
    {
        let mut entries = store.entries.lock().unwrap();
        let mut count = 0;
        for key in &command[1..] {
            remove_expired(&mut entries, key);
            if name.eq_ignore_ascii_case(b"DEL") || name.eq_ignore_ascii_case(b"UNLINK") {
                if entries.remove(key).is_some() {
                    mark_modified(store, key);
                    count += 1;
                }
            } else if entries.contains_key(key) {
                count += 1;
            }
        }
        return Some(format!(":{count}\r\n").into_bytes());
    }
    if name.eq_ignore_ascii_case(b"RENAME") || name.eq_ignore_ascii_case(b"RENAMENX") {
        let mut entries = store.entries.lock().unwrap();
        let source = &command[1];
        let destination = &command[2];
        remove_expired(&mut entries, source);
        remove_expired(&mut entries, destination);
        if !entries.contains_key(source) {
            return Some(protocol::error("no such key"));
        }
        let nx = name.eq_ignore_ascii_case(b"RENAMENX");
        if source == destination || (nx && entries.contains_key(destination)) {
            return Some(if nx {
                b":0\r\n".to_vec()
            } else {
                b"+OK\r\n".to_vec()
            });
        }
        let entry = entries.remove(source).unwrap();
        entries.insert(destination.clone(), entry);
        mark_modified(store, source);
        mark_modified(store, destination);
        store.changed.notify_all();
        return Some(if nx {
            b":1\r\n".to_vec()
        } else {
            b"+OK\r\n".to_vec()
        });
    }
    if name.eq_ignore_ascii_case(b"COPY") {
        if command.len() == 4 && !command[3].eq_ignore_ascii_case(b"REPLACE") {
            return Some(protocol::error("syntax error"));
        }
        let mut entries = store.entries.lock().unwrap();
        let source = &command[1];
        let destination = &command[2];
        remove_expired(&mut entries, source);
        remove_expired(&mut entries, destination);
        if source == destination || (command.len() == 3 && entries.contains_key(destination)) {
            return Some(b":0\r\n".to_vec());
        }
        let Some(entry) = entries.get(source).cloned() else {
            return Some(b":0\r\n".to_vec());
        };
        entries.insert(destination.clone(), entry);
        mark_modified(store, destination);
        store.changed.notify_all();
        return Some(b":1\r\n".to_vec());
    }
    if [b"TTL".as_slice(), b"PTTL", b"EXPIRETIME", b"PEXPIRETIME"]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
    {
        let mut entries = store.entries.lock().unwrap();
        remove_expired(&mut entries, &command[1]);
        let value = match entries.get(&command[1]) {
            None => -2,
            Some(entry) => match entry.expires_at {
                None => -1,
                Some(deadline) => {
                    let milliseconds = deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis();
                    if name.eq_ignore_ascii_case(b"EXPIRETIME")
                        || name.eq_ignore_ascii_case(b"PEXPIRETIME")
                    {
                        let absolute = SystemTime::now()
                            .checked_add(deadline.saturating_duration_since(Instant::now()));
                        let absolute = absolute
                            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                            .and_then(|duration| i64::try_from(duration.as_millis()).ok());
                        match absolute {
                            Some(milliseconds) if name.eq_ignore_ascii_case(b"PEXPIRETIME") => {
                                milliseconds
                            }
                            Some(milliseconds) => milliseconds / 1000,
                            None => return Some(protocol::error("expire time out of range")),
                        }
                    } else if name.eq_ignore_ascii_case(b"PTTL") {
                        milliseconds as i64
                    } else {
                        ((milliseconds + 500) / 1000) as i64
                    }
                }
            },
        };
        return Some(format!(":{value}\r\n").into_bytes());
    }
    if name.eq_ignore_ascii_case(b"PERSIST") {
        let mut entries = store.entries.lock().unwrap();
        remove_expired(&mut entries, &command[1]);
        let changed = entries
            .get_mut(&command[1])
            .is_some_and(|entry| entry.expires_at.take().is_some());
        return Some(if changed {
            b":1\r\n".to_vec()
        } else {
            b":0\r\n".to_vec()
        });
    }
    let (milliseconds, conditions) = match expiry_arguments(command) {
        Ok(arguments) => arguments,
        Err(error) => return Some(error),
    };
    let now = Instant::now();
    let deadline = if milliseconds <= 0 {
        now
    } else {
        match now.checked_add(Duration::from_millis(milliseconds as u64)) {
            Some(deadline) => deadline,
            None => return Some(protocol::error("invalid expire time")),
        }
    };
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &command[1]);
    let Some(entry) = entries.get_mut(&command[1]) else {
        return Some(b":0\r\n".to_vec());
    };
    let old = entry.expires_at;
    let skipped = (conditions.absent && old.is_some())
        || (conditions.present && old.is_none())
        || (conditions.longer && old.is_none_or(|previous| deadline <= previous))
        || (conditions.shorter && old.is_some_and(|previous| deadline >= previous));
    if skipped {
        return Some(b":0\r\n".to_vec());
    }
    if milliseconds <= 0 {
        entries.remove(&command[1]);
    } else {
        entry.expires_at = Some(deadline);
    }
    Some(b":1\r\n".to_vec())
}
