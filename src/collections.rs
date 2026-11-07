// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, pubsub::glob_matches, remove_expired, Entry, Store, Value};
use std::collections::hash_map::RandomState;
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

#[derive(Clone, Copy)]
enum Command {
    Hset,
    Hmset,
    Hsetnx,
    Hget,
    Hmget,
    Hgetall,
    Hdel,
    Hexists,
    Hlen,
    Hkeys,
    Hvals,
    Hincrby,
    Hincrbyfloat,
    Hstrlen,
    Hrandfield,
    Hscan,
    Sadd,
    Srem,
    Sismember,
    Smismember,
    Smembers,
    Scard,
    Sunion,
    Sinter,
    Sintercard,
    Sdiff,
    Sunionstore,
    Sinterstore,
    Sdiffstore,
    Smove,
    Spop,
    Srandmember,
    Sscan,
}

const COMMANDS: &[(&[u8], Command)] = &[
    (b"HSET", Command::Hset),
    (b"HMSET", Command::Hmset),
    (b"HSETNX", Command::Hsetnx),
    (b"HGET", Command::Hget),
    (b"HMGET", Command::Hmget),
    (b"HGETALL", Command::Hgetall),
    (b"HDEL", Command::Hdel),
    (b"HEXISTS", Command::Hexists),
    (b"HLEN", Command::Hlen),
    (b"HKEYS", Command::Hkeys),
    (b"HVALS", Command::Hvals),
    (b"HINCRBY", Command::Hincrby),
    (b"HINCRBYFLOAT", Command::Hincrbyfloat),
    (b"HSTRLEN", Command::Hstrlen),
    (b"HRANDFIELD", Command::Hrandfield),
    (b"HSCAN", Command::Hscan),
    (b"SADD", Command::Sadd),
    (b"SREM", Command::Srem),
    (b"SISMEMBER", Command::Sismember),
    (b"SMISMEMBER", Command::Smismember),
    (b"SMEMBERS", Command::Smembers),
    (b"SCARD", Command::Scard),
    (b"SUNION", Command::Sunion),
    (b"SINTER", Command::Sinter),
    (b"SINTERCARD", Command::Sintercard),
    (b"SDIFF", Command::Sdiff),
    (b"SUNIONSTORE", Command::Sunionstore),
    (b"SINTERSTORE", Command::Sinterstore),
    (b"SDIFFSTORE", Command::Sdiffstore),
    (b"SMOVE", Command::Smove),
    (b"SPOP", Command::Spop),
    (b"SRANDMEMBER", Command::Srandmember),
    (b"SSCAN", Command::Sscan),
];

fn parse(name: &[u8]) -> Option<Command> {
    COMMANDS
        .iter()
        .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
        .map(|(_, command)| *command)
}

impl Command {
    fn valid_arity(self, count: usize) -> bool {
        use Command::*;
        match self {
            Hset | Hmset => count >= 4 && count.is_multiple_of(2),
            Hsetnx | Hincrby | Hincrbyfloat => count == 4,
            Hget | Hexists | Hstrlen | Sismember => count == 3,
            Hmget | Hdel | Sadd | Srem | Smismember => count >= 3,
            Hgetall | Hlen | Hkeys | Hvals | Smembers | Scard => count == 2,
            Sunion | Sinter | Sdiff => count >= 2,
            Sintercard => count >= 3,
            Sunionstore | Sinterstore | Sdiffstore => count >= 3,
            Smove => count == 4,
            Spop | Srandmember => count == 2 || count == 3,
            Hrandfield => (2..=4).contains(&count),
            Hscan | Sscan => count >= 3,
        }
    }

    fn mutating(self) -> bool {
        matches!(
            self,
            Self::Hset
                | Self::Hmset
                | Self::Hsetnx
                | Self::Hdel
                | Self::Hincrby
                | Self::Hincrbyfloat
                | Self::Sadd
                | Self::Srem
                | Self::Sunionstore
                | Self::Sinterstore
                | Self::Sdiffstore
                | Self::Smove
                | Self::Spop
        )
    }
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| kind.valid_arity(command.len()))
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    parse(&command[0]).is_some_and(Command::mutating)
}

fn integer(value: i64) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}

fn optional_bulk(value: Option<&Vec<u8>>) -> Vec<u8> {
    value.map_or_else(|| b"$-1\r\n".to_vec(), |value| protocol::bulk(value))
}

fn signed_integer(bytes: &[u8]) -> Option<i64> {
    std::str::from_utf8(bytes)
        .ok()?
        .parse::<i64>()
        .ok()
        .filter(|value| value.to_string().as_bytes() == bytes)
}

fn finite_float(bytes: &[u8]) -> Option<f64> {
    std::str::from_utf8(bytes)
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

struct ScanOptions<'a> {
    cursor: usize,
    count: usize,
    pattern: Option<&'a [u8]>,
    novalues: bool,
}

fn scan_options(command: &[Vec<u8>], hash: bool) -> Result<ScanOptions<'_>, Vec<u8>> {
    let cursor = std::str::from_utf8(&command[2])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| protocol::error("invalid cursor"))?;
    let mut options = ScanOptions {
        cursor,
        count: 10,
        pattern: None,
        novalues: false,
    };
    let mut at = 3;
    while at < command.len() {
        if command[at].eq_ignore_ascii_case(b"NOVALUES") && hash && !options.novalues {
            options.novalues = true;
            at += 1;
        } else if at + 1 < command.len() && command[at].eq_ignore_ascii_case(b"MATCH") {
            options.pattern = Some(&command[at + 1]);
            at += 2;
        } else if at + 1 < command.len() && command[at].eq_ignore_ascii_case(b"COUNT") {
            options.count = std::str::from_utf8(&command[at + 1])
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| protocol::error("value is not an integer or out of range"))?;
            at += 2;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    Ok(options)
}

fn scan_reply(cursor: usize, values: &[Vec<u8>]) -> Vec<u8> {
    let mut reply = b"*2\r\n".to_vec();
    reply.extend(protocol::bulk(cursor.to_string().as_bytes()));
    reply.extend(protocol::array(values));
    reply
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = parse(&command[0])?;
    if !kind.valid_arity(command.len()) {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    use Command::*;
    Some(match kind {
        Sadd | Srem | Sismember | Smismember | Smembers | Scard | Sunion | Sinter | Sdiff
        | Sunionstore | Sinterstore | Sdiffstore | Smove | Spop | Srandmember | Sscan
        | Sintercard => execute_set(kind, command, store),
        _ => execute_hash(kind, command, store),
    })
}

fn execute_hash(kind: Command, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    use Command::*;
    // Validate the increment before creating a key or changing a field.
    let increment = if matches!(kind, Hincrby) {
        match signed_integer(&command[3]) {
            Some(value) => value,
            None => return protocol::error("value is not an integer or out of range"),
        }
    } else {
        0
    };
    let float_increment = if matches!(kind, Hincrbyfloat) {
        match finite_float(&command[3]) {
            Some(value) => value,
            None => return protocol::error("value is not a valid float"),
        }
    } else {
        0.0
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let hash = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::Hash(hash)) => Some(hash),
        Some(_) => return wrong_type(),
        None => None,
    };
    match kind {
        Hget => optional_bulk(hash.and_then(|hash| hash.get(&command[2]))),
        Hmget => {
            let mut response = format!("*{}\r\n", command.len() - 2).into_bytes();
            for field in &command[2..] {
                response.extend(optional_bulk(hash.and_then(|hash| hash.get(field))));
            }
            response
        }
        Hexists => integer(i64::from(
            hash.is_some_and(|hash| hash.contains_key(&command[2])),
        )),
        Hlen => integer(hash.map_or(0, |hash| hash.len()) as i64),
        Hstrlen => integer(
            hash.and_then(|hash| hash.get(&command[2]))
                .map_or(0, |value| value.len()) as i64,
        ),
        Hscan => {
            let options = match scan_options(command, true) {
                Ok(options) => options,
                Err(error) => return error,
            };
            let mut fields: Vec<_> = hash.into_iter().flat_map(|hash| hash.iter()).collect();
            fields.sort_unstable_by_key(|(field, _)| *field);
            let start = options.cursor.min(fields.len());
            let end = start.saturating_add(options.count).min(fields.len());
            let mut values = Vec::new();
            for (field, value) in &fields[start..end] {
                if options
                    .pattern
                    .is_none_or(|pattern| glob_matches(pattern, field))
                {
                    values.push((*field).clone());
                    if !options.novalues {
                        values.push((*value).clone());
                    }
                }
            }
            scan_reply(if end == fields.len() { 0 } else { end }, &values)
        }
        Hrandfield => {
            if command.len() == 4 && !command[3].eq_ignore_ascii_case(b"WITHVALUES") {
                return protocol::error("syntax error");
            }
            let requested = if command.len() >= 3 {
                match signed_integer(&command[2]) {
                    Some(value) => value,
                    None => return protocol::error("value is not an integer or out of range"),
                }
            } else {
                1
            };
            if requested.unsigned_abs() > 1_000_000 {
                return protocol::error("count exceeds ReRu limit");
            }
            let Some(hash) = hash else {
                return if command.len() == 2 {
                    b"$-1\r\n".to_vec()
                } else {
                    protocol::array(&[])
                };
            };
            let random = RandomState::new();
            let mut fields: Vec<_> = hash.keys().collect();
            fields.sort_unstable_by_key(|field| random.hash_one(field));
            let selected: Vec<_> = if requested < 0 && !fields.is_empty() {
                (0..requested.unsigned_abs() as usize)
                    .map(|index| fields[(random.hash_one(index) as usize) % fields.len()])
                    .collect()
            } else {
                fields.into_iter().take(requested as usize).collect()
            };
            if command.len() == 2 {
                return selected
                    .first()
                    .map_or_else(|| b"$-1\r\n".to_vec(), |field| protocol::bulk(field));
            }
            let mut values = Vec::new();
            for field in selected {
                values.push(field.clone());
                if command.len() == 4 {
                    values.push(hash.get(field).unwrap().clone());
                }
            }
            protocol::array(&values)
        }
        Hgetall | Hkeys | Hvals => {
            let mut fields: Vec<_> = hash.into_iter().flat_map(|hash| hash.iter()).collect();
            fields.sort_unstable_by_key(|(field, _)| *field);
            let mut values = Vec::new();
            for (field, value) in fields {
                if matches!(kind, Hgetall | Hkeys) {
                    values.push(field.clone());
                }
                if matches!(kind, Hgetall | Hvals) {
                    values.push(value.clone());
                }
            }
            protocol::array(&values)
        }
        Hdel => {
            let Some(Entry {
                value: Value::Hash(hash),
                ..
            }) = entries.get_mut(key)
            else {
                return integer(0);
            };
            let mut removed = 0;
            for field in &command[2..] {
                removed += i64::from(hash.remove(field).is_some());
            }
            if hash.is_empty() {
                entries.remove(key);
            }
            if removed != 0 {
                mark_modified(store, key);
            }
            integer(removed)
        }
        Hset | Hmset | Hsetnx | Hincrby | Hincrbyfloat => {
            if matches!(kind, Hsetnx) && hash.is_some_and(|hash| hash.contains_key(&command[2])) {
                return integer(0);
            }
            let updated_number = if matches!(kind, Hincrby) {
                let old = match hash.and_then(|hash| hash.get(&command[2])) {
                    None => 0,
                    Some(bytes) => match signed_integer(bytes) {
                        Some(value) => value,
                        None => return protocol::error("hash value is not an integer"),
                    },
                };
                match old.checked_add(increment) {
                    Some(value) => value,
                    None => return protocol::error("increment or decrement would overflow"),
                }
            } else {
                0
            };
            let updated_float = if matches!(kind, Hincrbyfloat) {
                let old = match hash.and_then(|hash| hash.get(&command[2])) {
                    None => 0.0,
                    Some(bytes) => match finite_float(bytes) {
                        Some(value) => value,
                        None => return protocol::error("hash value is not a float"),
                    },
                };
                let value = old + float_increment;
                if !value.is_finite() {
                    return protocol::error("increment would produce NaN or Infinity");
                }
                value
            } else {
                0.0
            };
            let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
                value: Value::Hash(HashMap::new()),
                expires_at: None,
            });
            let Value::Hash(hash) = &mut entry.value else {
                unreachable!()
            };
            let response = if matches!(kind, Hincrby) {
                hash.insert(command[2].clone(), updated_number.to_string().into_bytes());
                integer(updated_number)
            } else if matches!(kind, Hincrbyfloat) {
                let bytes = if updated_float == 0.0 {
                    b"0".to_vec()
                } else {
                    updated_float.to_string().into_bytes()
                };
                hash.insert(command[2].clone(), bytes.clone());
                protocol::bulk(&bytes)
            } else {
                let mut added = 0;
                for pair in command[2..].as_chunks::<2>().0 {
                    added += i64::from(hash.insert(pair[0].clone(), pair[1].clone()).is_none());
                }
                if matches!(kind, Hmset) {
                    b"+OK\r\n".to_vec()
                } else {
                    integer(added)
                }
            };
            mark_modified(store, key);
            response
        }
        _ => unreachable!(),
    }
}

fn set_array(set: &HashSet<Vec<u8>>) -> Vec<u8> {
    let mut members: Vec<_> = set.iter().cloned().collect();
    members.sort_unstable();
    protocol::array(&members)
}

fn random_members(set: &HashSet<Vec<u8>>, count: usize) -> Vec<Vec<u8>> {
    let random = RandomState::new();
    let mut members: Vec<_> = set.iter().cloned().collect();
    members.sort_unstable_by_key(|member| random.hash_one(member));
    members.truncate(count.min(members.len()));
    members
}

fn execute_set(kind: Command, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    use Command::*;
    let mut entries = store.entries.lock().unwrap();
    if matches!(kind, Sintercard) {
        let numkeys = match std::str::from_utf8(&command[1])
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
        {
            Some(value) => value,
            None => return protocol::error("numkeys should be greater than 0"),
        };
        let end = match 2usize.checked_add(numkeys) {
            Some(value) if value <= command.len() => value,
            _ => return protocol::error("syntax error"),
        };
        let limit = if end == command.len() {
            0
        } else if end + 2 == command.len() && command[end].eq_ignore_ascii_case(b"LIMIT") {
            match std::str::from_utf8(&command[end + 1])
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
            {
                Some(value) => value,
                None => return protocol::error("LIMIT can't be negative"),
            }
        } else {
            return protocol::error("syntax error");
        };
        let keys = &command[2..end];
        for key in keys {
            remove_expired(&mut entries, key);
        }
        let mut sets = Vec::new();
        let mut missing = false;
        for key in keys {
            match entries.get(key).map(|entry| &entry.value) {
                Some(Value::Set(set)) => sets.push(set),
                Some(_) => return wrong_type(),
                None => missing = true,
            }
        }
        if missing {
            return integer(0);
        }
        sets.sort_unstable_by_key(|set| set.len());
        let count = sets[0]
            .iter()
            .filter(|member| sets[1..].iter().all(|set| set.contains(*member)))
            .take(if limit == 0 { usize::MAX } else { limit })
            .count();
        return integer(count as i64);
    }
    if matches!(
        kind,
        Sunion | Sinter | Sdiff | Sunionstore | Sinterstore | Sdiffstore
    ) {
        let stored = matches!(kind, Sunionstore | Sinterstore | Sdiffstore);
        let source_keys = if stored { &command[2..] } else { &command[1..] };
        for key in source_keys {
            remove_expired(&mut entries, key);
        }
        let mut sets = Vec::new();
        for key in source_keys {
            match entries.get(key).map(|entry| &entry.value) {
                Some(Value::Set(set)) => sets.push(Some(set)),
                Some(_) => return wrong_type(),
                None => sets.push(None),
            }
        }
        let mut result = sets[0].cloned().unwrap_or_default();
        for set in &sets[1..] {
            match kind {
                Sunion | Sunionstore => {
                    if let Some(set) = set {
                        result.extend(set.iter().cloned());
                    }
                }
                Sinter | Sinterstore => {
                    result.retain(|member| set.is_some_and(|set| set.contains(member)))
                }
                Sdiff | Sdiffstore => {
                    result.retain(|member| !set.is_some_and(|set| set.contains(member)))
                }
                _ => unreachable!(),
            }
        }
        if !stored {
            return set_array(&result);
        }
        let destination = &command[1];
        let count = result.len();
        let previous = if count == 0 {
            entries.remove(destination)
        } else {
            entries.insert(
                destination.clone(),
                Entry {
                    value: Value::Set(result),
                    expires_at: None,
                },
            )
        };
        if previous.is_some() || count != 0 {
            mark_modified(store, destination);
        }
        return integer(count as i64);
    }
    if matches!(kind, Smove) {
        let source = &command[1];
        let destination = &command[2];
        let member = &command[3];
        remove_expired(&mut entries, source);
        remove_expired(&mut entries, destination);
        let source_set = match entries.get(source).map(|entry| &entry.value) {
            Some(Value::Set(set)) => set,
            Some(_) => return wrong_type(),
            None => return integer(0),
        };
        if !source_set.contains(member) {
            return integer(0);
        }
        if source == destination {
            return integer(1);
        }
        if entries
            .get(destination)
            .is_some_and(|entry| !matches!(entry.value, Value::Set(_)))
        {
            return wrong_type();
        }
        let Some(Entry {
            value: Value::Set(source_set),
            ..
        }) = entries.get_mut(source)
        else {
            unreachable!()
        };
        source_set.remove(member);
        if source_set.is_empty() {
            entries.remove(source);
        }
        let entry = entries.entry(destination.clone()).or_insert_with(|| Entry {
            value: Value::Set(HashSet::new()),
            expires_at: None,
        });
        let Value::Set(destination_set) = &mut entry.value else {
            unreachable!()
        };
        destination_set.insert(member.clone());
        mark_modified(store, source);
        mark_modified(store, destination);
        return integer(1);
    }
    let key = &command[1];
    remove_expired(&mut entries, key);
    let set = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::Set(set)) => Some(set),
        Some(_) => return wrong_type(),
        None => None,
    };
    match kind {
        Scard => integer(set.map_or(0, |set| set.len()) as i64),
        Sscan => {
            let options = match scan_options(command, false) {
                Ok(options) => options,
                Err(error) => return error,
            };
            let mut members: Vec<_> = set.into_iter().flat_map(|set| set.iter()).collect();
            members.sort_unstable();
            let start = options.cursor.min(members.len());
            let end = start.saturating_add(options.count).min(members.len());
            let values: Vec<_> = members[start..end]
                .iter()
                .filter(|member| {
                    options
                        .pattern
                        .is_none_or(|pattern| glob_matches(pattern, member))
                })
                .map(|member| (*member).clone())
                .collect();
            scan_reply(if end == members.len() { 0 } else { end }, &values)
        }
        Smembers => set.map_or_else(|| protocol::array(&[]), set_array),
        Sismember => integer(i64::from(set.is_some_and(|set| set.contains(&command[2])))),
        Smismember => {
            let mut response = format!("*{}\r\n", command.len() - 2).into_bytes();
            for member in &command[2..] {
                response.extend(integer(i64::from(
                    set.is_some_and(|set| set.contains(member)),
                )));
            }
            response
        }
        Sadd => {
            let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
                value: Value::Set(HashSet::new()),
                expires_at: None,
            });
            let Value::Set(set) = &mut entry.value else {
                unreachable!()
            };
            let mut added = 0;
            for member in &command[2..] {
                added += i64::from(set.insert(member.clone()));
            }
            if added != 0 {
                mark_modified(store, key);
            }
            integer(added)
        }
        Srem => {
            let Some(Entry {
                value: Value::Set(set),
                ..
            }) = entries.get_mut(key)
            else {
                return integer(0);
            };
            let mut removed = 0;
            for member in &command[2..] {
                removed += i64::from(set.remove(member));
            }
            if set.is_empty() {
                entries.remove(key);
            }
            if removed != 0 {
                mark_modified(store, key);
            }
            integer(removed)
        }
        Spop | Srandmember => {
            let requested = if command.len() == 3 {
                match signed_integer(&command[2]) {
                    Some(value) if matches!(kind, Srandmember) || value >= 0 => value,
                    _ => return protocol::error("value is not an integer or out of range"),
                }
            } else {
                1
            };
            if requested.unsigned_abs() > 1_000_000 {
                return protocol::error("count exceeds ReRu limit");
            }
            let Some(set) = set else {
                return if command.len() == 3 {
                    protocol::array(&[])
                } else {
                    b"$-1\r\n".to_vec()
                };
            };
            let selected = random_members(set, requested.unsigned_abs() as usize);
            let selected = if requested < 0 && !selected.is_empty() {
                let random = RandomState::new();
                (0..requested.unsigned_abs() as usize)
                    .map(|index| {
                        selected[(random.hash_one(index) as usize) % selected.len()].clone()
                    })
                    .collect::<Vec<_>>()
            } else {
                selected
            };
            if matches!(kind, Spop) && !selected.is_empty() {
                let Some(Entry {
                    value: Value::Set(set),
                    ..
                }) = entries.get_mut(key)
                else {
                    unreachable!()
                };
                for member in &selected {
                    set.remove(member);
                }
                if set.is_empty() {
                    entries.remove(key);
                }
                mark_modified(store, key);
            }
            if command.len() == 3 {
                protocol::array(&selected)
            } else {
                selected
                    .first()
                    .map_or_else(|| b"$-1\r\n".to_vec(), |member| protocol::bulk(member))
            }
        }
        _ => unreachable!(),
    }
}
