// SPDX-License-Identifier: AGPL-3.0-only
use std::borrow::Cow;

use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};

const MAX_BITMAP_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy)]
enum Command {
    Set,
    Get,
    Count,
    Op,
    Pos,
    Field,
    FieldReadOnly,
}

fn parse(name: &[u8]) -> Option<Command> {
    use Command::*;
    [
        (b"SETBIT".as_slice(), Set),
        (b"GETBIT", Get),
        (b"BITCOUNT", Count),
        (b"BITOP", Op),
        (b"BITPOS", Pos),
        (b"BITFIELD", Field),
        (b"BITFIELD_RO", FieldReadOnly),
    ]
    .iter()
    .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
    .map(|(_, kind)| *kind)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| match kind {
        Command::Set => command.len() == 4,
        Command::Get => command.len() == 3,
        Command::Count => matches!(command.len(), 2 | 4 | 5),
        Command::Op => command.len() >= 4,
        Command::Pos => (3..=6).contains(&command.len()),
        Command::Field | Command::FieldReadOnly => command.len() >= 2,
    })
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    parse(&command[0])
        .is_some_and(|kind| matches!(kind, Command::Set | Command::Op | Command::Field))
}

#[derive(Clone, Copy)]
struct FieldSpec {
    width: u32,
    signed: bool,
    position: usize,
}

#[derive(Clone, Copy)]
enum Overflow {
    Wrap,
    Saturate,
    Fail,
}

enum FieldAction {
    Get(FieldSpec),
    Set(FieldSpec, i64, Overflow),
    Increment(FieldSpec, i64, Overflow),
}

fn field_encoding(bytes: &[u8]) -> Result<(u32, bool), Vec<u8>> {
    let (signed, digits) = match bytes.split_first() {
        Some((b'i', digits)) => (true, digits),
        Some((b'u', digits)) => (false, digits),
        _ => return Err(protocol::error("invalid bitfield type")),
    };
    let width = std::str::from_utf8(digits)
        .ok()
        .and_then(|text| text.parse::<u32>().ok())
        .filter(|width| *width > 0 && *width <= if signed { 64 } else { 63 })
        .ok_or_else(|| protocol::error("invalid bitfield type"))?;
    Ok((width, signed))
}

fn field_position(bytes: &[u8], width: u32, writing: bool) -> Result<usize, Vec<u8>> {
    let (number, multiplier) = if bytes.first() == Some(&b'#') {
        (&bytes[1..], width as u64)
    } else {
        (bytes, 1)
    };
    let index = std::str::from_utf8(number)
        .ok()
        .and_then(|text| text.parse::<u64>().ok())
        .and_then(|value| value.checked_mul(multiplier))
        .ok_or_else(|| protocol::error("bit offset is not an integer or out of range"))?;
    let last = index
        .checked_add(u64::from(width) - 1)
        .ok_or_else(|| protocol::error("bit offset is not an integer or out of range"))?;
    let maximum = if writing {
        (MAX_BITMAP_BYTES as u64) * 8 - 1
    } else {
        u32::MAX as u64
    };
    if last > maximum {
        return Err(protocol::error(
            "bit offset is not an integer or out of range",
        ));
    }
    Ok(index as usize)
}

fn field_actions(command: &[Vec<u8>], read_only: bool) -> Result<Vec<FieldAction>, Vec<u8>> {
    let mut actions = Vec::new();
    let mut at = 2;
    let mut overflow = Overflow::Wrap;
    while at < command.len() {
        let name = &command[at];
        if name.eq_ignore_ascii_case(b"OVERFLOW") && !read_only {
            let Some(mode) = command.get(at + 1) else {
                return Err(protocol::error("syntax error"));
            };
            overflow = if mode.eq_ignore_ascii_case(b"WRAP") {
                Overflow::Wrap
            } else if mode.eq_ignore_ascii_case(b"SAT") {
                Overflow::Saturate
            } else if mode.eq_ignore_ascii_case(b"FAIL") {
                Overflow::Fail
            } else {
                return Err(protocol::error("syntax error"));
            };
            at += 2;
            continue;
        }
        let get = name.eq_ignore_ascii_case(b"GET");
        let set = name.eq_ignore_ascii_case(b"SET") && !read_only;
        let increment = name.eq_ignore_ascii_case(b"INCRBY") && !read_only;
        if !get && !set && !increment {
            return Err(protocol::error("syntax error"));
        }
        let needed = if get { 3 } else { 4 };
        if at + needed > command.len() {
            return Err(protocol::error("syntax error"));
        }
        let (width, is_signed) = field_encoding(&command[at + 1])?;
        let position = field_position(&command[at + 2], width, !get)?;
        let spec = FieldSpec {
            width,
            signed: is_signed,
            position,
        };
        actions.push(if get {
            FieldAction::Get(spec)
        } else if set {
            FieldAction::Set(spec, signed(&command[at + 3])?, overflow)
        } else {
            FieldAction::Increment(spec, signed(&command[at + 3])?, overflow)
        });
        at += needed;
    }
    Ok(actions)
}

fn field_read(bytes: &[u8], spec: FieldSpec) -> i64 {
    let mut raw = 0u64;
    for index in spec.position..spec.position + spec.width as usize {
        let bit = bytes
            .get(index / 8)
            .map_or(0, |byte| (byte >> (7 - index % 8)) & 1);
        raw = (raw << 1) | u64::from(bit);
    }
    if spec.signed && spec.width < 64 && raw & (1u64 << (spec.width - 1)) != 0 {
        (i128::from(raw) - (1i128 << spec.width)) as i64
    } else {
        raw as i64
    }
}

fn field_write(bytes: &mut Vec<u8>, spec: FieldSpec, value: i64) {
    let end = spec.position + spec.width as usize;
    bytes.resize(bytes.len().max(end.div_ceil(8)), 0);
    let raw = value as u64;
    for bit in 0..spec.width as usize {
        let index = spec.position + bit;
        let mask = 0x80 >> (index % 8);
        if raw & (1u64 << (spec.width as usize - bit - 1)) != 0 {
            bytes[index / 8] |= mask;
        } else {
            bytes[index / 8] &= !mask;
        }
    }
}

fn field_overflow(value: i128, spec: FieldSpec, overflow: Overflow) -> Option<i64> {
    let minimum = if spec.signed {
        -(1i128 << (spec.width - 1))
    } else {
        0
    };
    let maximum = if spec.signed {
        (1i128 << (spec.width - 1)) - 1
    } else {
        (1i128 << spec.width) - 1
    };
    if (minimum..=maximum).contains(&value) {
        return Some(value as i64);
    }
    match overflow {
        Overflow::Fail => None,
        Overflow::Saturate => Some(value.clamp(minimum, maximum) as i64),
        Overflow::Wrap => {
            let modulus = 1i128 << spec.width;
            let wrapped = value.rem_euclid(modulus);
            Some(if spec.signed && wrapped > maximum {
                (wrapped - modulus) as i64
            } else {
                wrapped as i64
            })
        }
    }
}

fn bitfield(command: &[Vec<u8>], store: &Store, read_only: bool) -> Vec<u8> {
    let actions = match field_actions(command, read_only) {
        Ok(actions) => actions,
        Err(error) => return error,
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let mut bytes: Cow<'_, [u8]> = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::String(bytes)) => Cow::Borrowed(bytes),
        Some(_) => return wrong_type(),
        None => Cow::Borrowed(&[]),
    };
    let mut modified = false;
    let mut replies = Vec::new();
    for action in actions {
        match action {
            FieldAction::Get(spec) => replies.push(Some(field_read(&bytes, spec))),
            FieldAction::Set(spec, value, overflow) => {
                let old = field_read(&bytes, spec);
                if let Some(value) = field_overflow(i128::from(value), spec, overflow) {
                    field_write(bytes.to_mut(), spec, value);
                    modified = true;
                    replies.push(Some(old));
                } else {
                    replies.push(None);
                }
            }
            FieldAction::Increment(spec, increment, overflow) => {
                let old = field_read(&bytes, spec);
                let next = i128::from(old) + i128::from(increment);
                if let Some(value) = field_overflow(next, spec, overflow) {
                    field_write(bytes.to_mut(), spec, value);
                    modified = true;
                    replies.push(Some(value));
                } else {
                    replies.push(None);
                }
            }
        }
    }
    if modified {
        let bytes = bytes.into_owned();
        let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
            value: Value::String(Vec::new()),
            expires_at: None,
        });
        entry.value = Value::String(bytes);
        mark_modified(store, key);
    }
    let mut response = format!("*{}\r\n", replies.len()).into_bytes();
    for reply in replies {
        match reply {
            Some(value) => response.extend(integer(value)),
            None => response.extend_from_slice(b"$-1\r\n"),
        }
    }
    response
}

fn integer(value: i64) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}
fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}

fn signed(bytes: &[u8]) -> Result<i64, Vec<u8>> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))
}

fn offset(bytes: &[u8], writing: bool) -> Result<usize, Vec<u8>> {
    let value = signed(bytes)?;
    let maximum = if writing {
        MAX_BITMAP_BYTES as u64 * 8 - 1
    } else {
        u32::MAX as u64
    };
    if value < 0 || value as u64 > maximum {
        return Err(protocol::error(
            "bit offset is not an integer or out of range",
        ));
    }
    Ok(value as usize)
}

fn bit(bytes: &[u8]) -> Result<u8, Vec<u8>> {
    match bytes {
        b"0" => Ok(0),
        b"1" => Ok(1),
        _ => Err(protocol::error("bit is not an integer or out of range")),
    }
}

fn unit(bytes: &[u8]) -> Result<usize, Vec<u8>> {
    if bytes.eq_ignore_ascii_case(b"BYTE") {
        Ok(8)
    } else if bytes.eq_ignore_ascii_case(b"BIT") {
        Ok(1)
    } else {
        Err(protocol::error("syntax error"))
    }
}

fn span(length: usize, start: i64, end: i64) -> Option<(usize, usize)> {
    if length == 0 {
        return None;
    }
    let length = length as i128;
    let index = |n: i64| {
        if n < 0 {
            length + i128::from(n)
        } else {
            i128::from(n)
        }
    };
    let from = index(start).max(0).min(length);
    let to = index(end).max(-1).min(length - 1);
    (from <= to).then_some((from as usize, (to + 1) as usize))
}

fn bit_span(
    length: usize,
    command: &[Vec<u8>],
    first: usize,
    last: usize,
    width: usize,
) -> Result<Option<(usize, usize)>, Vec<u8>> {
    let from = signed(&command[first])?;
    let to = signed(&command[last])?;
    Ok(span(length.saturating_mul(8) / width, from, to)
        .map(|(from, to)| (from * width, to * width)))
}

fn bit_mask(first_bit: usize, end_bit: usize, byte_index: usize) -> u8 {
    let base = byte_index * 8;
    let first = first_bit.saturating_sub(base).min(8);
    let last = end_bit.saturating_sub(base).min(8);
    let mut mask = 0;
    for position in first..last {
        mask |= 0x80 >> position;
    }
    mask
}

fn count_bits(bytes: &[u8], from: usize, to: usize) -> i64 {
    let mut count = 0;
    for (index, byte) in bytes.iter().enumerate().take(to.div_ceil(8)).skip(from / 8) {
        count += (byte & bit_mask(from, to, index)).count_ones() as i64;
    }
    count
}

fn find_bit(bytes: &[u8], target: u8, from: usize, to: usize) -> i64 {
    for (index, byte) in bytes.iter().enumerate().take(to.div_ceil(8)).skip(from / 8) {
        let candidate = (if target == 1 { *byte } else { !byte }) & bit_mask(from, to, index);
        if candidate != 0 {
            return (index * 8 + candidate.leading_zeros() as usize) as i64;
        }
    }
    -1
}

fn bitop(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let op = &command[1];
    let not = op.eq_ignore_ascii_case(b"NOT");
    if !(not
        || op.eq_ignore_ascii_case(b"AND")
        || op.eq_ignore_ascii_case(b"OR")
        || op.eq_ignore_ascii_case(b"XOR"))
        || (not && command.len() != 4)
    {
        return protocol::error("syntax error");
    }
    let dest = &command[2];
    let mut entries = store.entries.lock().unwrap();
    let mut sources = Vec::with_capacity(command.len() - 3);
    for key in &command[3..] {
        remove_expired(&mut entries, key);
        match entries.get(key).map(|entry| &entry.value) {
            Some(Value::String(value)) => sources.push(value.clone()),
            Some(_) => return wrong_type(),
            None => sources.push(Vec::new()),
        }
    }
    let length = sources.iter().map(Vec::len).max().unwrap_or(0);
    if length > MAX_BITMAP_BYTES {
        return protocol::error("bitmap exceeds ReRu's 64 MiB limit");
    }
    let mut result = vec![0; length];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = if not {
            !sources[0][index]
        } else if op.eq_ignore_ascii_case(b"AND") {
            sources.iter().fold(0xff, |acc, source| {
                acc & source.get(index).copied().unwrap_or(0)
            })
        } else if op.eq_ignore_ascii_case(b"OR") {
            sources.iter().fold(0, |acc, source| {
                acc | source.get(index).copied().unwrap_or(0)
            })
        } else {
            sources.iter().fold(0, |acc, source| {
                acc ^ source.get(index).copied().unwrap_or(0)
            })
        };
    }
    remove_expired(&mut entries, dest);
    if result.is_empty() {
        if entries.remove(dest).is_some() {
            mark_modified(store, dest);
        }
    } else {
        entries.insert(
            dest.clone(),
            Entry {
                value: Value::String(result),
                expires_at: None,
            },
        );
        mark_modified(store, dest);
    }
    integer(length as i64)
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = parse(&command[0])?;
    if valid_arity(command) == Some(false) {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    if matches!(kind, Command::Op) {
        return Some(bitop(command, store));
    }
    if matches!(kind, Command::Field | Command::FieldReadOnly) {
        return Some(bitfield(
            command,
            store,
            matches!(kind, Command::FieldReadOnly),
        ));
    }
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let data = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::String(bytes)) => Some(bytes),
        Some(_) => return Some(wrong_type()),
        None => None,
    };
    Some(match kind {
        Command::Set => {
            let position = match offset(&command[2], true) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let new_bit = match bit(&command[3]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let old = data
                .and_then(|bytes| bytes.get(position / 8))
                .map_or(0, |byte| (byte >> (7 - position % 8)) & 1);
            let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
                value: Value::String(Vec::new()),
                expires_at: None,
            });
            let Value::String(bytes) = &mut entry.value else {
                unreachable!()
            };
            bytes.resize(bytes.len().max(position / 8 + 1), 0);
            let mask = 0x80 >> (position % 8);
            if new_bit == 1 {
                bytes[position / 8] |= mask;
            } else {
                bytes[position / 8] &= !mask;
            }
            mark_modified(store, key);
            integer(i64::from(old))
        }
        Command::Get => {
            let position = match offset(&command[2], false) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            integer(
                data.and_then(|bytes| bytes.get(position / 8))
                    .map_or(0, |byte| i64::from((byte >> (7 - position % 8)) & 1)),
            )
        }
        Command::Count => {
            let width = if command.len() == 5 {
                match unit(&command[4]) {
                    Ok(value) => value,
                    Err(error) => return Some(error),
                }
            } else {
                8
            };
            let bytes = data.map_or(&[][..], Vec::as_slice);
            let bounds = if command.len() >= 4 {
                match bit_span(bytes.len(), command, 2, 3, width) {
                    Ok(bounds) => bounds,
                    Err(error) => return Some(error),
                }
            } else {
                Some((0, bytes.len() * 8))
            };
            bounds.map_or_else(
                || integer(0),
                |(from, to)| integer(count_bits(bytes, from, to)),
            )
        }
        Command::Pos => {
            let target = match bit(&command[2]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let width = if command.len() == 6 {
                match unit(&command[5]) {
                    Ok(value) => value,
                    Err(error) => return Some(error),
                }
            } else {
                8
            };
            let bytes = data.map_or(&[][..], Vec::as_slice);
            let bounds = if command.len() >= 5 {
                match bit_span(bytes.len(), command, 3, 4, width) {
                    Ok(bounds) => bounds,
                    Err(error) => return Some(error),
                }
            } else if command.len() == 4 {
                let start = match signed(&command[3]) {
                    Ok(value) => value,
                    Err(error) => return Some(error),
                };
                span(bytes.len() * 8 / width, start, -1)
                    .map(|(from, to)| (from * width, to * width))
            } else {
                Some((0, bytes.len() * 8))
            };
            let found = bounds.map_or(-1, |(from, to)| find_bit(bytes, target, from, to));
            let implicit_zero = target == 0 && command.len() < 5 && bounds.is_some();
            integer(if found >= 0 {
                found
            } else if implicit_zero {
                (bytes.len() * 8) as i64
            } else {
                -1
            })
        }
        Command::Op | Command::Field | Command::FieldReadOnly => unreachable!(),
    })
}
