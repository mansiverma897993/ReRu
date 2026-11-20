// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};
use std::collections::HashMap;
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
    Lcs,
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
        (b"LCS", Lcs),
    ]
    .iter()
    .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
    .map(|(_, kind)| *kind)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| match kind {
        Command::Set => command.len() >= 3,
        Command::GetEx => command.len() >= 2,
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
        Command::Lcs => command.len() >= 3,
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

const MAX_LCS_CELLS: usize = 8_000_000;

// (first_start, first_end, second_start, second_end), all inclusive.
type LcsMatch = (usize, usize, usize, usize);

#[derive(Default)]
struct LcsOptions {
    length_only: bool,
    indexes: bool,
    min_match_len: usize,
    with_match_len: bool,
}

// Redis parses signed integers in their canonical decimal form.
fn lcs_integer(raw: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(raw).ok()?;
    let digits = text.strip_prefix('-').unwrap_or(text);
    if text == "0" {
        return Some(0);
    }
    if digits.is_empty() || digits.as_bytes()[0] == b'0' {
        return None;
    }
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok()
}

fn lcs_options(command: &[Vec<u8>]) -> Result<LcsOptions, Vec<u8>> {
    let mut options = LcsOptions::default();
    let mut index = 3;
    while index < command.len() {
        let option = &command[index];
        if option.eq_ignore_ascii_case(b"LEN") {
            options.length_only = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"IDX") {
            options.indexes = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"WITHMATCHLEN") {
            options.with_match_len = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"MINMATCHLEN") {
            let Some(raw) = command.get(index + 1) else {
                return Err(protocol::error("syntax error"));
            };
            let value = lcs_integer(raw)
                .ok_or_else(|| protocol::error("value is not an integer or out of range"))?;
            options.min_match_len = usize::try_from(value.max(0)).unwrap_or(usize::MAX);
            index += 2;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    if options.length_only && options.indexes {
        return Err(protocol::error(
            "If you want both the length and indexes, please just use IDX.",
        ));
    }
    Ok(options)
}

fn lcs_string<'a>(entries: &'a HashMap<Vec<u8>, Entry>, key: &[u8]) -> Result<&'a [u8], Vec<u8>> {
    match entries.get(key).map(|entry| &entry.value) {
        Some(Value::String(value)) => Ok(value),
        Some(_) => Err(protocol::error(
            "The specified keys must contain string values",
        )),
        None => Ok(&[]),
    }
}

// LEN only needs two rows of the dynamic-programming table.
fn lcs_length(first: &[u8], second: &[u8]) -> usize {
    let (short, long) = if second.len() < first.len() {
        (second, first)
    } else {
        (first, second)
    };
    let mut previous = vec![0u32; short.len() + 1];
    let mut current = vec![0u32; short.len() + 1];
    for &byte in long {
        for (index, &candidate) in short.iter().enumerate() {
            current[index + 1] = if candidate == byte {
                previous[index] + 1
            } else {
                previous[index + 1].max(current[index])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[short.len()] as usize
}

// Returns match ranges as (first_start, first_end, second_start, second_end),
// ordered from the end of both strings to the start, like Redis.
fn lcs_matches(first: &[u8], second: &[u8], cells: usize) -> (Vec<LcsMatch>, usize) {
    let width = second.len() + 1;
    let mut matrix = vec![0u32; cells];
    for i in 1..=first.len() {
        for j in 1..=second.len() {
            matrix[i * width + j] = if first[i - 1] == second[j - 1] {
                matrix[(i - 1) * width + j - 1] + 1
            } else {
                matrix[(i - 1) * width + j].max(matrix[i * width + j - 1])
            };
        }
    }
    let total = matrix[first.len() * width + second.len()] as usize;
    let mut matches: Vec<LcsMatch> = Vec::new();
    let (mut i, mut j) = (first.len(), second.len());
    while i > 0 && j > 0 {
        if first[i - 1] == second[j - 1] {
            match matches.last_mut() {
                Some(last) if last.0 == i && last.2 == j => {
                    last.0 -= 1;
                    last.2 -= 1;
                }
                _ => matches.push((i - 1, i - 1, j - 1, j - 1)),
            }
            i -= 1;
            j -= 1;
        } else if matrix[(i - 1) * width + j] > matrix[i * width + j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    (matches, total)
}

fn lcs(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &command[1]);
    remove_expired(&mut entries, &command[2]);
    let first = match lcs_string(&entries, &command[1]) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let second = match lcs_string(&entries, &command[2]) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let options = match lcs_options(command) {
        Ok(options) => options,
        Err(error) => return error,
    };
    let Some(cells) = (first.len() + 1)
        .checked_mul(second.len() + 1)
        .filter(|cells| *cells <= MAX_LCS_CELLS)
    else {
        return protocol::error("LCS keys are too large");
    };
    if options.length_only {
        return format!(":{}\r\n", lcs_length(first, second)).into_bytes();
    }
    let (matches, total) = lcs_matches(first, second, cells);
    if !options.indexes {
        let mut bytes = Vec::with_capacity(total);
        for found in matches.iter().rev() {
            bytes.extend_from_slice(&first[found.0..=found.1]);
        }
        return protocol::bulk(&bytes);
    }
    let shown: Vec<_> = matches
        .iter()
        .filter(|found| found.1 - found.0 + 1 >= options.min_match_len)
        .collect();
    let mut reply = b"*4\r\n".to_vec();
    reply.extend(protocol::bulk(b"matches"));
    reply.extend(format!("*{}\r\n", shown.len()).as_bytes());
    for found in shown {
        let length = found.1 - found.0 + 1;
        let fields = if options.with_match_len { 3 } else { 2 };
        reply.extend(format!("*{fields}\r\n").as_bytes());
        reply.extend(format!("*2\r\n:{}\r\n:{}\r\n", found.0, found.1).as_bytes());
        reply.extend(format!("*2\r\n:{}\r\n:{}\r\n", found.2, found.3).as_bytes());
        if options.with_match_len {
            reply.extend(format!(":{length}\r\n").as_bytes());
        }
    }
    reply.extend(protocol::bulk(b"len"));
    reply.extend(format!(":{total}\r\n").as_bytes());
    reply
}

fn optional_bulk(value: Option<&[u8]>) -> Vec<u8> {
    value.map_or_else(|| b"$-1\r\n".to_vec(), protocol::bulk)
}

#[derive(Clone, Copy)]
enum Expiration {
    Reset,
    Keep,
    At(crate::Deadline),
}

fn deadline(option: &[u8], bytes: &[u8], command: &str) -> Result<crate::Deadline, Vec<u8>> {
    let invalid = || protocol::error(&format!("invalid expire time in '{command}' command"));
    let amount = protocol::parse_int(bytes)
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))?;
    if amount <= 0 {
        return Err(invalid());
    }
    let milliseconds = if option.eq_ignore_ascii_case(b"EX") || option.eq_ignore_ascii_case(b"EXAT")
    {
        amount.checked_mul(1000).ok_or_else(invalid)?
    } else {
        amount
    };
    let milliseconds = u64::try_from(milliseconds).map_err(|_| invalid())?;
    let now = Instant::now();
    let now_unix = crate::unix_millis();
    if option.eq_ignore_ascii_case(b"EX") || option.eq_ignore_ascii_case(b"PX") {
        let at = now
            .checked_add(Duration::from_millis(milliseconds))
            .ok_or_else(invalid)?;
        let unix_ms = now_unix
            .checked_add(i64::try_from(milliseconds).map_err(|_| invalid())?)
            .ok_or_else(invalid)?;
        return Ok(crate::Deadline { at, unix_ms });
    }
    // Absolute deadlines keep the requested wall-clock millisecond exactly.
    let unix_ms = i64::try_from(milliseconds).map_err(|_| invalid())?;
    let absolute = UNIX_EPOCH
        .checked_add(Duration::from_millis(milliseconds))
        .ok_or_else(invalid)?;
    let remaining = absolute
        .duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO);
    let at = now.checked_add(remaining).ok_or_else(invalid)?;
    Ok(crate::Deadline { at, unix_ms })
}

struct SetOptions {
    nx: bool,
    xx: bool,
    get: bool,
    expiration: Expiration,
}

fn expiry_unit(option: &[u8]) -> Option<&'static [u8]> {
    [b"EX".as_slice(), b"PX", b"EXAT", b"PXAT"]
        .into_iter()
        .find(|candidate| option.eq_ignore_ascii_case(candidate))
}

fn set_options(command: &[Vec<u8>]) -> Result<SetOptions, Vec<u8>> {
    let mut nx = false;
    let mut xx = false;
    let mut get = false;
    let mut keep_ttl = false;
    let mut expiry: Option<(&'static [u8], &[u8])> = None;
    let mut index = 3;
    while index < command.len() {
        let option = &command[index];
        if option.eq_ignore_ascii_case(b"NX") {
            if xx {
                return Err(protocol::error("syntax error"));
            }
            nx = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"XX") {
            if nx {
                return Err(protocol::error("syntax error"));
            }
            xx = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"GET") {
            get = true;
            index += 1;
        } else if option.eq_ignore_ascii_case(b"KEEPTTL") {
            if expiry.is_some() {
                return Err(protocol::error("syntax error"));
            }
            keep_ttl = true;
            index += 1;
        } else if let Some(unit) = expiry_unit(option) {
            if keep_ttl || index + 1 >= command.len() {
                return Err(protocol::error("syntax error"));
            }
            if expiry.is_some_and(|(current, _)| current != unit) {
                return Err(protocol::error("syntax error"));
            }
            // Redis parses only the last value of a repeated unit.
            expiry = Some((unit, &command[index + 1]));
            index += 2;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    let expiration = match expiry {
        Some((unit, value)) => Expiration::At(deadline(unit, value, "set")?),
        None if keep_ttl => Expiration::Keep,
        None => Expiration::Reset,
    };
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
    if expires_at.is_some_and(|deadline| deadline.at <= Instant::now()) {
        entries.remove(key);
    }
    mark_modified(store, key);
    if options.get {
        optional_bulk(old.as_deref())
    } else {
        b"+OK\r\n".to_vec()
    }
}

struct GetexOptions<'a> {
    persist: bool,
    expiry: Option<(&'static [u8], &'a [u8])>,
}

/// Validate GETEX option syntax only. Redis looks up the key before parsing
/// the expiry value, so a missing key returns nil without value validation.
fn getex_options(command: &[Vec<u8>]) -> Result<GetexOptions<'_>, Vec<u8>> {
    let mut persist = false;
    let mut expiry: Option<(&'static [u8], &[u8])> = None;
    let mut index = 2;
    while index < command.len() {
        let option = &command[index];
        if option.eq_ignore_ascii_case(b"PERSIST") {
            if expiry.is_some() {
                return Err(protocol::error("syntax error"));
            }
            persist = true;
            index += 1;
        } else if let Some(unit) = expiry_unit(option) {
            if persist || index + 1 >= command.len() {
                return Err(protocol::error("syntax error"));
            }
            if expiry.is_some_and(|(current, _)| current != unit) {
                return Err(protocol::error("syntax error"));
            }
            // Redis parses only the last value of a repeated unit.
            expiry = Some((unit, &command[index + 1]));
            index += 2;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    Ok(GetexOptions { persist, expiry })
}

fn getex(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let options = match getex_options(command) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let Some(entry) = entries.get_mut(key) else {
        return b"$-1\r\n".to_vec();
    };
    let Value::String(value) = &entry.value else {
        return wrong_type();
    };
    let expiration = match options.expiry {
        Some((unit, raw)) => match deadline(unit, raw, "getex") {
            Ok(value) => Expiration::At(value),
            Err(error) => return error,
        },
        None if options.persist => Expiration::Keep,
        None => Expiration::Reset,
    };
    let reply = protocol::bulk(value);
    let changed = if command.len() == 2 {
        false
    } else if matches!(expiration, Expiration::Keep) {
        entry.expires_at.take().is_some()
    } else if let Expiration::At(deadline) = expiration {
        entry.expires_at = Some(deadline);
        if deadline.at <= Instant::now() {
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
        match protocol::parse_int(&command[2]) {
            Some(value) => value,
            None => return protocol::error("value is not an integer or out of range"),
        }
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let old = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::String(value)) => match protocol::parse_int(value) {
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
        protocol::parse_int(value)
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
    // Redis returns an empty range before converting when both indexes are
    // negative and start is past end.
    if start < 0 && end < 0 && start > end {
        return protocol::bulk(b"");
    }
    let len = bytes.len() as i128;
    let from = range_index(start, len).max(0).min(len) as usize;
    let to = range_index(end, len).max(0).min(len - 1);
    if to < from as i128 {
        protocol::bulk(b"")
    } else {
        protocol::bulk(&bytes[from..=to as usize])
    }
}

fn setrange(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let offset =
        match protocol::parse_int(&command[2]).and_then(|value| usize::try_from(value).ok()) {
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
        Command::Lcs => lcs(command, store),
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
