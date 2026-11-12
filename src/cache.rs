// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
enum Command {
    Set,
    GetEx,
    GetDel,
    MGet,
    MSet,
    MSetNx,
    Append,
    StrLen,
    SetNx,
    SetEx,
    PSetEx,
    GetSet,
    IncrBy,
    IncrByFloat,
    Decr,
    DecrBy,
    GetRange,
    SubStr,
    SetRange,
}

fn parse(name: &[u8]) -> Option<Command> {
    use Command::*;
    [
        (b"SET".as_slice(), Set),
        (b"GETEX", GetEx),
        (b"GETDEL", GetDel),
        (b"MGET", MGet),
        (b"MSET", MSet),
        (b"MSETNX", MSetNx),
        (b"APPEND", Append),
        (b"STRLEN", StrLen),
        (b"SETNX", SetNx),
        (b"SETEX", SetEx),
        (b"PSETEX", PSetEx),
        (b"GETSET", GetSet),
        (b"INCRBY", IncrBy),
        (b"INCRBYFLOAT", IncrByFloat),
        (b"DECR", Decr),
        (b"DECRBY", DecrBy),
        (b"GETRANGE", GetRange),
        (b"SUBSTR", SubStr),
        (b"SETRANGE", SetRange),
    ]
    .iter()
    .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
    .map(|(_, kind)| *kind)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| match kind {
        Command::Set => command.len() >= 3,
        Command::GetEx => command.len() == 2 || command.len() == 3 || command.len() == 4,
        Command::GetDel | Command::StrLen => command.len() == 2,
        Command::MGet => command.len() >= 2,
        Command::MSet | Command::MSetNx => command.len() >= 3 && command.len() % 2 == 1,
        Command::Append => command.len() == 3,
        Command::SetNx
        | Command::GetSet
        | Command::IncrBy
        | Command::IncrByFloat
        | Command::DecrBy => command.len() == 3,
        Command::SetEx
        | Command::PSetEx
        | Command::GetRange
        | Command::SubStr
        | Command::SetRange => command.len() == 4,
        Command::Decr => command.len() == 2,
    })
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    parse(&command[0]).is_some_and(|kind| {
        matches!(
            kind,
            Command::Set
                | Command::GetDel
                | Command::MSet
                | Command::MSetNx
                | Command::Append
                | Command::SetNx
                | Command::SetEx
                | Command::PSetEx
                | Command::GetSet
                | Command::IncrBy
                | Command::IncrByFloat
                | Command::Decr
                | Command::DecrBy
                | Command::SetRange
        ) || matches!(kind, Command::GetEx) && command.len() > 2
    })
}

fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}
fn optional_bulk(value: Option<&[u8]>) -> Vec<u8> {
    value.map_or_else(|| b"$-1\r\n".to_vec(), protocol::bulk)
}

#[derive(Clone, Copy)]
enum Expiration {
    Reset,
    Keep,
    At(Instant),
}

fn deadline(option: &[u8], bytes: &[u8], command: &str) -> Result<Instant, Vec<u8>> {
    let invalid = || protocol::error(&format!("invalid expire time in '{command}' command"));
    let amount = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(invalid)?;
    let milliseconds = if option.eq_ignore_ascii_case(b"EX") || option.eq_ignore_ascii_case(b"EXAT")
    {
        amount.checked_mul(1000).ok_or_else(invalid)?
    } else {
        amount
    };
    if option.eq_ignore_ascii_case(b"EX") || option.eq_ignore_ascii_case(b"PX") {
        return Instant::now()
            .checked_add(Duration::from_millis(milliseconds))
            .ok_or_else(invalid);
    }
    let absolute = UNIX_EPOCH
        .checked_add(Duration::from_millis(milliseconds))
        .ok_or_else(invalid)?;
    let remaining = absolute
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    Instant::now().checked_add(remaining).ok_or_else(invalid)
}

fn expiry_arguments(
    command: &[Vec<u8>],
    first: usize,
    allow_keep: bool,
    name: &str,
) -> Result<Expiration, Vec<u8>> {
    if first == command.len() {
        return Ok(Expiration::Reset);
    }
    let option = &command[first];
    if allow_keep && command.len() == first + 1 && option.eq_ignore_ascii_case(b"KEEPTTL") {
        return Ok(Expiration::Keep);
    }
    if command.len() != first + 2
        || ![b"EX".as_slice(), b"PX", b"EXAT", b"PXAT"]
            .iter()
            .any(|candidate| option.eq_ignore_ascii_case(candidate))
    {
        return Err(protocol::error("syntax error"));
    }
    Ok(Expiration::At(deadline(option, &command[first + 1], name)?))
}

struct SetOptions {
    nx: bool,
    xx: bool,
    get: bool,
    expiration: Expiration,
}

fn set_options(command: &[Vec<u8>]) -> Result<SetOptions, Vec<u8>> {
    let mut nx = false;
    let mut xx = false;
    let mut get = false;
    let mut expiration = Expiration::Reset;
    let mut has_expiry = false;
    let mut index = 3;
    while index < command.len() {
        let option = &command[index];
        if option.eq_ignore_ascii_case(b"NX") && !nx {
            nx = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"XX") && !xx {
            xx = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"GET") && !get {
            get = true;
            index += 1;
        } else if !has_expiry {
            let end = if option.eq_ignore_ascii_case(b"KEEPTTL") {
                index + 1
            } else {
                index + 2
            };
            if end > command.len() {
                return Err(protocol::error("syntax error"));
            }
            expiration = expiry_arguments(&command[..end], index, true, "set")?;
            has_expiry = true;
            index = end;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    if nx && xx {
        return Err(protocol::error("syntax error"));
    }
    Ok(SetOptions {
        nx,
        xx,
        get,
        expiration,
    })
}

fn set(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let options = match set_options(command) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let existing = entries.get(key);
    let old = if options.get {
        match existing.map(|entry| &entry.value) {
            Some(Value::String(value)) => Some(value.clone()),
            Some(_) => return wrong_type(),
            None => None,
        }
    } else {
        None
    };
    if (options.nx && existing.is_some()) || (options.xx && existing.is_none()) {
        return if options.get {
            optional_bulk(old.as_deref())
        } else {
            b"$-1\r\n".to_vec()
        };
    }
    let expires_at = match options.expiration {
        Expiration::Reset => None,
        Expiration::Keep => existing.and_then(|entry| entry.expires_at),
        Expiration::At(deadline) => Some(deadline),
    };
    entries.insert(
        key.clone(),
        Entry {
            value: Value::String(command[2].clone()),
            expires_at,
        },
    );
    if expires_at.is_some_and(|deadline| deadline <= Instant::now()) {
        entries.remove(key);
    }
    mark_modified(store, key);
    if options.get {
        optional_bulk(old.as_deref())
    } else {
        b"+OK\r\n".to_vec()
    }
}

fn getex(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let expiration = if command.len() == 2 {
        Expiration::Reset
    } else if command.len() == 3 && command[2].eq_ignore_ascii_case(b"PERSIST") {
        Expiration::Keep
    } else {
        match expiry_arguments(command, 2, false, "getex") {
            Ok(value) => value,
            Err(error) => return error,
        }
    };
    if command.len() == 3 && !command[2].eq_ignore_ascii_case(b"PERSIST") {
        return protocol::error("syntax error");
    }
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let Some(entry) = entries.get_mut(key) else {
        return b"$-1\r\n".to_vec();
    };
    let Value::String(value) = &entry.value else {
        return wrong_type();
    };
    let reply = protocol::bulk(value);
    let changed = if command.len() == 2 {
        false
    } else if matches!(expiration, Expiration::Keep) {
        entry.expires_at.take().is_some()
    } else if let Expiration::At(deadline) = expiration {
        entry.expires_at = Some(deadline);
        if deadline <= Instant::now() {
            entries.remove(key);
        }
        true
    } else {
        false
    };
    if changed {
        mark_modified(store, key);
    }
    reply
}

fn execute_multi(command: &[Vec<u8>], store: &Store, only_if_all_missing: bool) -> Vec<u8> {
    let mut entries = store.entries.lock().unwrap();
    for pair in command[1..].as_chunks::<2>().0 {
        remove_expired(&mut entries, &pair[0]);
    }
    if only_if_all_missing
        && command[1..]
            .as_chunks::<2>()
            .0
            .iter()
            .any(|pair| entries.contains_key(&pair[0]))
    {
        return b":0\r\n".to_vec();
    }
    for pair in command[1..].as_chunks::<2>().0 {
        entries.insert(
            pair[0].clone(),
            Entry {
                value: Value::String(pair[1].clone()),
                expires_at: None,
            },
        );
        mark_modified(store, &pair[0]);
    }
    if only_if_all_missing {
        b":1\r\n".to_vec()
    } else {
        b"+OK\r\n".to_vec()
    }
}

fn legacy_set(kind: Command, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let mut args = vec![b"SET".to_vec(), command[1].clone()];
    match kind {
        Command::SetNx => {
            args.push(command[2].clone());
            args.push(b"NX".to_vec());
        }
        Command::GetSet => {
            args.push(command[2].clone());
            args.push(b"GET".to_vec());
        }
        Command::SetEx | Command::PSetEx => {
            let (unit, name) = if matches!(kind, Command::SetEx) {
                (b"EX".as_slice(), "setex")
            } else {
                (b"PX".as_slice(), "psetex")
            };
            if let Err(error) = deadline(unit, &command[2], name) {
                return error;
            }
            args.push(command[3].clone());
            args.push(unit.to_vec());
            args.push(command[2].clone());
        }
        _ => unreachable!(),
    }
    let reply = set(&args, store);
    if matches!(kind, Command::SetNx) {
        if reply == b"+OK\r\n" {
            b":1\r\n".to_vec()
        } else if reply == b"$-1\r\n" {
            b":0\r\n".to_vec()
        } else {
            reply
        }
    } else {
        reply
    }
}

fn change_integer(kind: Command, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let amount = if matches!(kind, Command::Decr) {
        1
    } else {
        match std::str::from_utf8(&command[2])
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
        {
            Some(value) => value,
            None => return protocol::error("value is not an integer or out of range"),
        }
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let old = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::String(value)) => match std::str::from_utf8(value)
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
        {
            Some(value) => value,
            None => return protocol::error("value is not an integer or out of range"),
        },
        Some(_) => return wrong_type(),
        None => 0,
    };
    let next = if matches!(kind, Command::IncrBy) {
        old.checked_add(amount)
    } else {
        old.checked_sub(amount)
    };
    let Some(next) = next else {
        return protocol::error("increment or decrement would overflow");
    };
    let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
        value: Value::String(Vec::new()),
        expires_at: None,
    });
    let Value::String(value) = &mut entry.value else {
        unreachable!();
    };
    *value = next.to_string().into_bytes();
    mark_modified(store, key);
    format!(":{next}\r\n").into_bytes()
}

fn finite_float(bytes: &[u8]) -> Option<f64> {
    std::str::from_utf8(bytes)
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn change_float(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let Some(amount) = finite_float(&command[2]) else {
        return protocol::error("value is not a valid float");
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let old = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::String(value)) => match finite_float(value) {
            Some(value) => value,
            None => return protocol::error("value is not a valid float"),
        },
        Some(_) => return wrong_type(),
        None => 0.0,
    };
    let next = old + amount;
    if !next.is_finite() {
        return protocol::error("increment would produce NaN or Infinity");
    }
    let bytes = if next == 0.0 {
        b"0".to_vec()
    } else {
        next.to_string().into_bytes()
    };
    let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
        value: Value::String(Vec::new()),
        expires_at: None,
    });
    let Value::String(value) = &mut entry.value else {
        unreachable!();
    };
    *value = bytes.clone();
    mark_modified(store, key);
    protocol::bulk(&bytes)
}

fn range_index(index: i64, len: i128) -> i128 {
    if index < 0 {
        len + i128::from(index)
    } else {
        i128::from(index)
    }
}

fn getrange(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let parse = |value: &[u8]| {
        std::str::from_utf8(value)
            .ok()
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or_else(|| protocol::error("value is not an integer or out of range"))
    };
    let start = match parse(&command[2]) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let end = match parse(&command[3]) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &command[1]);
    let bytes = match entries.get(&command[1]).map(|entry| &entry.value) {
        Some(Value::String(value)) => value.as_slice(),
        Some(_) => return wrong_type(),
        None => return protocol::bulk(b""),
    };
    let len = bytes.len() as i128;
    let from = range_index(start, len).max(0).min(len) as usize;
    let to = range_index(end, len).max(-1).min(len - 1);
    if to < from as i128 {
        protocol::bulk(b"")
    } else {
        protocol::bulk(&bytes[from..=to as usize])
    }
}

fn setrange(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let offset = match std::str::from_utf8(&command[2])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(offset) => offset,
        None => return protocol::error("value is not an integer or out of range"),
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    if entries
        .get(key)
        .is_some_and(|entry| !matches!(entry.value, Value::String(_)))
    {
        return wrong_type();
    }
    if command[3].is_empty() {
        return format!(
            ":{}\r\n",
            entries.get(key).map_or(0, |entry| match &entry.value {
                Value::String(value) => value.len(),
                _ => unreachable!(),
            })
        )
        .into_bytes();
    }
    let Some(end) = offset.checked_add(command[3].len()) else {
        return protocol::error("string exceeds ReRu 64 MiB limit");
    };
    if end > 64 * 1024 * 1024 {
        return protocol::error("string exceeds ReRu 64 MiB limit");
    }
    let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
        value: Value::String(Vec::new()),
        expires_at: None,
    });
    let Value::String(value) = &mut entry.value else {
        unreachable!();
    };
    value.resize(value.len().max(end), 0);
    value[offset..end].copy_from_slice(&command[3]);
    let length = value.len();
    mark_modified(store, key);
    format!(":{length}\r\n").into_bytes()
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = parse(&command[0])?;
    if valid_arity(command) == Some(false) {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    Some(match kind {
        Command::Set => set(command, store),
        Command::SetNx | Command::SetEx | Command::PSetEx | Command::GetSet => {
            legacy_set(kind, command, store)
        }
        Command::IncrBy | Command::Decr | Command::DecrBy => change_integer(kind, command, store),
        Command::IncrByFloat => change_float(command, store),
        Command::GetRange | Command::SubStr => getrange(command, store),
        Command::SetRange => setrange(command, store),
        Command::MSet => execute_multi(command, store, false),
        Command::MSetNx => execute_multi(command, store, true),
        Command::MGet => {
            let mut entries = store.entries.lock().unwrap();
            let mut reply = format!("*{}\r\n", command.len() - 1).into_bytes();
            for key in &command[1..] {
                remove_expired(&mut entries, key);
                let value = match entries.get(key).map(|entry| &entry.value) {
                    Some(Value::String(value)) => Some(value.as_slice()),
                    _ => None,
                };
                reply.extend(optional_bulk(value));
            }
            reply
        }
        Command::GetDel => {
            let key = &command[1];
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, key);
            match entries.get(key).map(|entry| &entry.value) {
                Some(Value::String(value)) => {
                    let reply = protocol::bulk(value);
                    entries.remove(key);
                    mark_modified(store, key);
                    reply
                }
                Some(_) => wrong_type(),
                None => b"$-1\r\n".to_vec(),
            }
        }
        Command::GetEx => getex(command, store),
        Command::StrLen => {
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get(&command[1]).map(|entry| &entry.value) {
                Some(Value::String(value)) => format!(":{}\r\n", value.len()).into_bytes(),
                Some(_) => wrong_type(),
                None => b":0\r\n".to_vec(),
            }
        }
        Command::Append => {
            let key = &command[1];
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, key);
            match entries.get_mut(key) {
                Some(Entry {
                    value: Value::String(value),
                    ..
                }) => {
                    value.extend_from_slice(&command[2]);
                    let len = value.len();
                    mark_modified(store, key);
                    format!(":{len}\r\n").into_bytes()
                }
                Some(_) => wrong_type(),
                None => {
                    entries.insert(
                        key.clone(),
                        Entry {
                            value: Value::String(command[2].clone()),
                            expires_at: None,
                        },
                    );
                    mark_modified(store, key);
                    format!(":{}\r\n", command[2].len()).into_bytes()
                }
            }
        }
    })
}
