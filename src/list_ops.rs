// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};

#[derive(Clone, Copy)]
enum Command {
    Index,
    Set,
    Trim,
    Rpop,
    Lpushx,
    Rpushx,
    Insert,
    Move,
    Rpoplpush,
    Pos,
    Rem,
}

fn parse(name: &[u8]) -> Option<Command> {
    use Command::*;
    [
        (b"LINDEX".as_slice(), Index),
        (b"LSET", Set),
        (b"LTRIM", Trim),
        (b"RPOP", Rpop),
        (b"LPUSHX", Lpushx),
        (b"RPUSHX", Rpushx),
        (b"LINSERT", Insert),
        (b"LMOVE", Move),
        (b"RPOPLPUSH", Rpoplpush),
        (b"LPOS", Pos),
        (b"LREM", Rem),
    ]
    .iter()
    .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
    .map(|(_, kind)| *kind)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| match kind {
        Command::Index => command.len() == 3,
        Command::Set | Command::Trim | Command::Rem => command.len() == 4,
        Command::Rpop => command.len() == 2 || command.len() == 3,
        Command::Lpushx | Command::Rpushx => command.len() >= 3,
        Command::Insert | Command::Move => command.len() == 5,
        Command::Rpoplpush => command.len() == 3,
        Command::Pos => command.len() >= 3 && command.len() % 2 == 1,
    })
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    parse(&command[0]).is_some_and(|kind| !matches!(kind, Command::Index | Command::Pos))
}

fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}

fn integer(value: usize) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

fn parse_i64(bytes: &[u8]) -> Result<i64, Vec<u8>> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))
}

fn index(number: i64, len: usize) -> Option<usize> {
    let at = if number < 0 {
        len as i128 + i128::from(number)
    } else {
        i128::from(number)
    };
    (0..len as i128).contains(&at).then_some(at as usize)
}

fn pop(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let count = if command.len() == 3 {
        match parse_i64(&command[2]) {
            Ok(value) if value >= 0 => value as usize,
            _ => return protocol::error("value is not an integer or out of range"),
        }
    } else {
        1
    };
    let mut entries = store.entries.lock().unwrap();
    let key = &command[1];
    remove_expired(&mut entries, key);
    let Some(entry) = entries.get_mut(key) else {
        return if command.len() == 3 {
            b"*-1\r\n".to_vec()
        } else {
            b"$-1\r\n".to_vec()
        };
    };
    let Value::List(items) = &mut entry.value else {
        return wrong_type();
    };
    let mut values = Vec::new();
    for _ in 0..count.min(items.len()) {
        values.push(items.pop().unwrap());
    }
    if !values.is_empty() {
        if items.is_empty() {
            entries.remove(key);
        }
        mark_modified(store, key);
    }
    if command.len() == 2 {
        values
            .first()
            .map_or_else(|| b"$-1\r\n".to_vec(), |value| protocol::bulk(value))
    } else {
        protocol::array(&values)
    }
}

fn move_item(command: &[Vec<u8>], store: &Store, legacy: bool) -> Vec<u8> {
    let (from_left, to_left) = if legacy {
        (false, true)
    } else {
        let direction = |value: &[u8]| {
            if value.eq_ignore_ascii_case(b"LEFT") {
                Some(true)
            } else if value.eq_ignore_ascii_case(b"RIGHT") {
                Some(false)
            } else {
                None
            }
        };
        match (direction(&command[3]), direction(&command[4])) {
            (Some(from), Some(to)) => (from, to),
            _ => return protocol::error("syntax error"),
        }
    };
    let source = &command[1];
    let destination = &command[2];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, source);
    remove_expired(&mut entries, destination);
    let Some(source_entry) = entries.get(source) else {
        return b"$-1\r\n".to_vec();
    };
    if !matches!(source_entry.value, Value::List(_))
        || entries
            .get(destination)
            .is_some_and(|entry| !matches!(entry.value, Value::List(_)))
    {
        return wrong_type();
    }
    if source == destination {
        let Some(Entry {
            value: Value::List(items),
            ..
        }) = entries.get_mut(source)
        else {
            return b"$-1\r\n".to_vec();
        };
        let value = if from_left {
            items.remove(0)
        } else {
            items.pop().unwrap()
        };
        if to_left {
            items.insert(0, value.clone());
        } else {
            items.push(value.clone());
        }
        mark_modified(store, source);
        return protocol::bulk(&value);
    }
    let value = match entries.get_mut(source) {
        Some(Entry {
            value: Value::List(items),
            ..
        }) => {
            if from_left {
                items.remove(0)
            } else {
                items.pop().unwrap()
            }
        }
        _ => return b"$-1\r\n".to_vec(),
    };
    if entries
        .get(source)
        .is_some_and(|entry| matches!(&entry.value, Value::List(items) if items.is_empty()))
    {
        entries.remove(source);
    }
    let destination_entry = entries.entry(destination.clone()).or_insert_with(|| Entry {
        value: Value::List(Vec::new()),
        expires_at: None,
    });
    let Value::List(items) = &mut destination_entry.value else {
        unreachable!()
    };
    if to_left {
        items.insert(0, value.clone());
    } else {
        items.push(value.clone());
    }
    mark_modified(store, source);
    mark_modified(store, destination);
    store.changed.notify_all();
    protocol::bulk(&value)
}

fn pos(command: &[Vec<u8>], items: Option<&[Vec<u8>]>) -> Vec<u8> {
    let mut rank = 1i64;
    let mut count = None;
    let mut maxlen = 0usize;
    for pair in command[3..].as_chunks::<2>().0 {
        if pair[0].eq_ignore_ascii_case(b"RANK") {
            rank = match parse_i64(&pair[1]) {
                Ok(value) if value != 0 => value,
                _ => return protocol::error("RANK can't be zero"),
            };
        } else if pair[0].eq_ignore_ascii_case(b"COUNT") {
            count = match parse_i64(&pair[1]) {
                Ok(value) if value >= 0 => Some(value as usize),
                _ => return protocol::error("COUNT must be non-negative"),
            };
        } else if pair[0].eq_ignore_ascii_case(b"MAXLEN") {
            maxlen = match parse_i64(&pair[1]) {
                Ok(value) if value >= 0 => value as usize,
                _ => return protocol::error("MAXLEN must be non-negative"),
            };
        } else {
            return protocol::error("syntax error");
        }
    }
    let Some(items) = items else {
        return if count.is_some() {
            protocol::array(&[])
        } else {
            b"$-1\r\n".to_vec()
        };
    };
    let limit = if maxlen == 0 {
        items.len()
    } else {
        maxlen.min(items.len())
    };
    let indexes: Box<dyn Iterator<Item = usize>> = if rank < 0 {
        Box::new((0..items.len()).rev().take(limit))
    } else {
        Box::new((0..items.len()).take(limit))
    };
    let skip = rank.unsigned_abs().saturating_sub(1);
    let mut found = Vec::new();
    for at in indexes
        .filter(|at| items[*at] == command[2])
        .skip(skip as usize)
    {
        found.push(at);
        if count.is_none_or(|count| count != 0 && found.len() >= count) {
            break;
        }
    }
    if count.is_some() {
        let mut reply = format!("*{}\r\n", found.len()).into_bytes();
        for at in found {
            reply.extend(integer(at));
        }
        reply
    } else {
        found
            .first()
            .map_or_else(|| b"$-1\r\n".to_vec(), |at| integer(*at))
    }
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = parse(&command[0])?;
    if valid_arity(command) != Some(true) {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    if matches!(kind, Command::Rpop) {
        return Some(pop(command, store));
    }
    if matches!(kind, Command::Move | Command::Rpoplpush) {
        return Some(move_item(
            command,
            store,
            matches!(kind, Command::Rpoplpush),
        ));
    }
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let items = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::List(items)) => Some(items),
        Some(_) => return Some(wrong_type()),
        None => None,
    };
    Some(match kind {
        Command::Index => {
            let at = match parse_i64(&command[2]) {
                Ok(at) => at,
                Err(error) => return Some(error),
            };
            items
                .and_then(|items| index(at, items.len()).map(|at| &items[at]))
                .map_or_else(|| b"$-1\r\n".to_vec(), |value| protocol::bulk(value))
        }
        Command::Pos => pos(command, items.map(Vec::as_slice)),
        Command::Lpushx | Command::Rpushx => {
            let Some(Entry {
                value: Value::List(items),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(integer(0));
            };
            if matches!(kind, Command::Lpushx) {
                items.splice(0..0, command[2..].iter().rev().cloned());
            } else {
                items.extend_from_slice(&command[2..]);
            }
            let len = items.len();
            mark_modified(store, key);
            store.changed.notify_all();
            integer(len)
        }
        Command::Set => {
            let at = match parse_i64(&command[2]) {
                Ok(at) => at,
                Err(error) => return Some(error),
            };
            let Some(Entry {
                value: Value::List(items),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(protocol::error("no such key"));
            };
            let Some(at) = index(at, items.len()) else {
                return Some(protocol::error("index out of range"));
            };
            items[at] = command[3].clone();
            mark_modified(store, key);
            b"+OK\r\n".to_vec()
        }
        Command::Trim => {
            let start = match parse_i64(&command[2]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let stop = match parse_i64(&command[3]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let Some(Entry {
                value: Value::List(items),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(b"+OK\r\n".to_vec());
            };
            let len = items.len() as i128;
            let normalize = |value: i64| {
                if value < 0 {
                    len + i128::from(value)
                } else {
                    i128::from(value)
                }
            };
            let from = normalize(start).max(0).min(len) as usize;
            let to = normalize(stop).max(-1).min(len - 1);
            if to < from as i128 {
                entries.remove(key);
            } else {
                items.truncate(to as usize + 1);
                items.drain(..from);
            }
            mark_modified(store, key);
            b"+OK\r\n".to_vec()
        }
        Command::Insert => {
            let before = if command[2].eq_ignore_ascii_case(b"BEFORE") {
                true
            } else if command[2].eq_ignore_ascii_case(b"AFTER") {
                false
            } else {
                return Some(protocol::error("syntax error"));
            };
            let Some(Entry {
                value: Value::List(items),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(integer(0));
            };
            let Some(at) = items.iter().position(|item| item == &command[3]) else {
                return Some(b":-1\r\n".to_vec());
            };
            items.insert(at + usize::from(!before), command[4].clone());
            let len = items.len();
            mark_modified(store, key);
            store.changed.notify_all();
            integer(len)
        }
        Command::Rem => {
            let count = match parse_i64(&command[2]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let Some(Entry {
                value: Value::List(items),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(integer(0));
            };
            let limit = count.unsigned_abs();
            let indexes: Box<dyn Iterator<Item = usize>> = if count < 0 {
                Box::new((0..items.len()).rev())
            } else {
                Box::new(0..items.len())
            };
            let mut remove = Vec::new();
            for at in indexes {
                if items[at] == command[3] {
                    remove.push(at);
                    if count != 0 && remove.len() as u64 >= limit {
                        break;
                    }
                }
            }
            remove.sort_unstable_by(|left, right| right.cmp(left));
            for at in &remove {
                items.remove(*at);
            }
            let empty = items.is_empty();
            if empty {
                entries.remove(key);
            }
            if !remove.is_empty() {
                mark_modified(store, key);
            }
            integer(remove.len())
        }
        Command::Rpop | Command::Move | Command::Rpoplpush => unreachable!(),
    })
}
