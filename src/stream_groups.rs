// SPDX-License-Identifier: AGPL-3.0-only
//! Consumer-group state and RESP2 command handling. All mutations use the server
//! command gate, including pending-entry ownership and blocking delivery.
use crate::{
    mark_modified, protocol, remove_expired,
    streams::{Stream, StreamEntry, StreamId},
    Entry, Store, Value,
};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct Consumer {
    pub seen_at: u64,
    pub active_at: Option<u64>,
}
#[derive(Clone, Debug)]
pub struct Pending {
    pub consumer: Vec<u8>,
    pub delivered_at: u64,
    pub deliveries: u64,
}
#[derive(Clone, Debug, Default)]
pub struct Group {
    pub last_delivered: StreamId,
    pub entries_read: Option<u64>,
    pub consumers: BTreeMap<Vec<u8>, Consumer>,
    pub pending: BTreeMap<StreamId, Pending>,
}
type Result<T> = std::result::Result<T, Vec<u8>>;
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn integer(n: u64) -> Vec<u8> {
    format!(":{}\r\n", n.min(i64::MAX as u64)).into_bytes()
}
fn array(items: Vec<Vec<u8>>) -> Vec<u8> {
    let mut out = format!("*{}\r\n", items.len()).into_bytes();
    for item in items {
        out.extend(item);
    }
    out
}
fn id(raw: &[u8]) -> Result<StreamId> {
    StreamId::parse_query(raw).map_err(protocol::error)
}
fn number(raw: &[u8]) -> Result<u64> {
    std::str::from_utf8(raw)
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n >= 0)
        .map(|n| n as u64)
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))
}
fn positive(raw: &[u8]) -> Result<usize> {
    let n = number(raw)?;
    if n == 0 {
        return Err(protocol::error("COUNT must be > 0"));
    }
    usize::try_from(n).map_err(|_| protocol::error("value is not an integer or out of range"))
}
fn syntax<T>() -> Result<T> {
    Err(protocol::error("syntax error"))
}
fn wrongtype() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}
fn nogroup(key: &[u8], group: &[u8], read: bool) -> Vec<u8> {
    let msg = format!(
        "No such key '{}' or consumer group '{}'{}",
        String::from_utf8_lossy(key),
        String::from_utf8_lossy(group),
        if read {
            " in XREADGROUP with GROUP option"
        } else {
            ""
        }
    )
    .replace(['\r', '\n'], " ");
    format!("-NOGROUP {msg}\r\n").into_bytes()
}
fn stream_mut<'a>(entries: &'a mut HashMap<Vec<u8>, Entry>, key: &[u8]) -> Result<&'a mut Stream> {
    remove_expired(entries, key);
    match entries.get_mut(key) {
        Some(Entry {
            value: Value::Stream(stream),
            ..
        }) => Ok(stream),
        Some(_) => Err(wrongtype()),
        None => Err(protocol::error("no such key")),
    }
}
fn group_mut<'a>(stream: &'a mut Stream, key: &[u8], name: &[u8]) -> Result<&'a mut Group> {
    stream
        .groups
        .get_mut(name)
        .ok_or_else(|| nogroup(key, name, false))
}
fn entry_reply(id: StreamId, entry: Option<&StreamEntry>) -> Vec<u8> {
    array(vec![
        protocol::bulk(id.to_string().as_bytes()),
        entry.map_or_else(|| b"*-1\r\n".to_vec(), |item| protocol::array(&item.fields)),
    ])
}
fn find_entry(stream: &[StreamEntry], id: StreamId) -> Option<&StreamEntry> {
    stream
        .binary_search_by_key(&id, |item| item.id)
        .ok()
        .map(|i| &stream[i])
}
fn touch_consumer(group: &mut Group, name: &[u8], now: u64, active: bool) {
    let consumer = group.consumers.entry(name.to_vec()).or_insert(Consumer {
        seen_at: now,
        active_at: None,
    });
    consumer.seen_at = now;
    if active {
        consumer.active_at = Some(now);
    }
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    let n = command.len();
    Some(match command[0].to_ascii_uppercase().as_slice() {
        b"XGROUP" => match command.get(1).map(|s| s.to_ascii_uppercase()).as_deref() {
            Some(b"CREATE" | b"SETID") => n >= 5,
            Some(b"DESTROY") => n == 4,
            Some(b"CREATECONSUMER" | b"DELCONSUMER") => n == 5,
            _ => false,
        },
        b"XREADGROUP" => n >= 7,
        b"XACK" => n >= 4,
        b"XPENDING" => n == 3 || n >= 6,
        b"XCLAIM" | b"XAUTOCLAIM" => n >= 6,
        b"XINFO" => match command.get(1).map(|s| s.to_ascii_uppercase()).as_deref() {
            Some(b"GROUPS") => n == 3,
            Some(b"CONSUMERS") => n == 4,
            Some(b"STREAM") => n >= 3,
            _ => false,
        },
        b"XSETID" => n >= 3,
        _ => return None,
    })
}
pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    valid_arity(command).is_some()
        && !command[0].eq_ignore_ascii_case(b"XPENDING")
        && !command[0].eq_ignore_ascii_case(b"XINFO")
}
pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let valid = valid_arity(command)?;
    if !valid {
        return Some(protocol::error(
            "wrong number of arguments or unsupported stream subcommand",
        ));
    }
    Some(execute_inner(command, store).unwrap_or_else(|error| error))
}
fn execute_inner(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    match c[0].to_ascii_uppercase().as_slice() {
        b"XGROUP" => group_command(c, store),
        b"XREADGROUP" => read_once(&parse_read(c)?, store),
        b"XACK" => acknowledge(c, store),
        b"XPENDING" => pending(c, store),
        b"XCLAIM" => claim(c, store),
        b"XAUTOCLAIM" => autoclaim(c, store),
        b"XINFO" => info(c, store),
        b"XSETID" => set_id(c, store),
        _ => unreachable!(),
    }
}

fn group_command(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let sub = c[1].to_ascii_uppercase();
    let key = &c[2];
    let name = &c[3];
    let create = sub == b"CREATE";
    let setid = sub == b"SETID";
    let mut mkstream = false;
    let mut entries_read = None;
    let mut parsed_id = None;
    if create || setid {
        if c[4] != b"$" {
            parsed_id = Some(id(&c[4])?);
        }
        let mut i = 5;
        while i < c.len() {
            if create && c[i].eq_ignore_ascii_case(b"MKSTREAM") {
                mkstream = true;
                i += 1;
            } else if c[i].eq_ignore_ascii_case(b"ENTRIESREAD") && i + 1 < c.len() {
                entries_read = Some(if c[i + 1] == b"-1" {
                    None
                } else {
                    Some(number(&c[i + 1])?)
                });
                i += 2;
            } else {
                return syntax();
            }
        }
    }
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    if !entries.contains_key(key) {
        if sub == b"DESTROY" {
            return Ok(integer(0));
        }
        if create && mkstream {
            entries.insert(
                key.clone(),
                Entry {
                    value: Value::Stream(Stream::default()),
                    expires_at: None,
                },
            );
        } else {
            return Err(protocol::error("The XGROUP subcommand requires the key to exist. Use MKSTREAM to create an empty stream."));
        }
    }
    let stream = stream_mut(&mut entries, key)?;
    let reply = if create || setid {
        if create && stream.groups.contains_key(name) {
            return Err(b"-BUSYGROUP Consumer Group name already exists\r\n".to_vec());
        }
        let last = parsed_id.unwrap_or(stream.last_generated_id);
        if create {
            stream.groups.insert(
                name.clone(),
                Group {
                    last_delivered: last,
                    entries_read: entries_read.unwrap_or(None),
                    ..Group::default()
                },
            );
        } else {
            let group = group_mut(stream, key, name)?;
            group.last_delivered = last;
            group.entries_read = entries_read.unwrap_or(None);
        }
        b"+OK\r\n".to_vec()
    } else if sub == b"DESTROY" {
        let removed = stream.groups.remove(name).is_some();
        if !removed {
            return Ok(integer(0));
        }
        integer(1)
    } else {
        let group = group_mut(stream, key, name)?;
        if sub == b"CREATECONSUMER" {
            if group.consumers.contains_key(&c[4]) {
                return Ok(integer(0));
            }
            touch_consumer(group, &c[4], now_ms(), false);
            integer(1)
        } else {
            if group.consumers.remove(&c[4]).is_none() {
                return Ok(integer(0));
            }
            let before = group.pending.len();
            group.pending.retain(|_, pending| pending.consumer != c[4]);
            integer((before - group.pending.len()) as u64)
        }
    };
    mark_modified(store, key);
    Ok(reply)
}

pub struct ReadOptions<'a> {
    group: &'a [u8],
    consumer: &'a [u8],
    keys: &'a [Vec<u8>],
    ids: Vec<Option<StreamId>>,
    count: usize,
    block: Option<Duration>,
    noack: bool,
}
fn parse_read(c: &[Vec<u8>]) -> Result<ReadOptions<'_>> {
    let mut group = None;
    let mut consumer = None;
    let mut count = usize::MAX;
    let mut block = None;
    let mut noack = false;
    let mut i = 1;
    while i < c.len() && !c[i].eq_ignore_ascii_case(b"STREAMS") {
        if c[i].eq_ignore_ascii_case(b"GROUP") && i + 2 < c.len() {
            group = Some(c[i + 1].as_slice());
            consumer = Some(c[i + 2].as_slice());
            i += 3;
        } else if c[i].eq_ignore_ascii_case(b"COUNT") && i + 1 < c.len() {
            count = positive(&c[i + 1])?;
            i += 2;
        } else if c[i].eq_ignore_ascii_case(b"BLOCK") && i + 1 < c.len() {
            let duration = Duration::from_millis(number(&c[i + 1])?);
            if Instant::now().checked_add(duration).is_none() {
                return Err(protocol::error("timeout is out of range"));
            }
            block = Some(duration);
            i += 2;
        } else if c[i].eq_ignore_ascii_case(b"NOACK") {
            noack = true;
            i += 1;
        } else {
            return syntax();
        }
    }
    let group = group.ok_or_else(|| protocol::error("Missing GROUP option for XREADGROUP"))?;
    let rest = c
        .get(i + 1..)
        .ok_or_else(|| protocol::error("syntax error"))?;
    if rest.len() < 2 || !rest.len().is_multiple_of(2) {
        return Err(protocol::error("Unbalanced XREADGROUP list of streams"));
    }
    let half = rest.len() / 2;
    let ids = rest[half..]
        .iter()
        .map(|raw| {
            if raw == b">" {
                Ok(None)
            } else {
                id(raw).map(Some)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ReadOptions {
        group,
        consumer: consumer.unwrap(),
        keys: &rest[..half],
        ids,
        count,
        block,
        noack,
    })
}
fn read_once(options: &ReadOptions<'_>, store: &Store) -> Result<Vec<u8>> {
    let mut entries = store.entries.lock().unwrap();
    // Validate every stream before any delivery side effects.
    for key in options.keys {
        remove_expired(&mut entries, key);
        match entries.get(key) {
            Some(Entry {
                value: Value::Stream(stream),
                ..
            }) if stream.groups.contains_key(options.group) => {}
            Some(Entry {
                value: Value::Stream(_),
                ..
            })
            | None => return Err(nogroup(key, options.group, true)),
            _ => return Err(wrongtype()),
        }
    }
    let mut response = Vec::new();
    let now = now_ms();
    for (key, requested) in options.keys.iter().zip(&options.ids) {
        let stream = stream_mut(&mut entries, key)?;
        let group = stream.groups.get_mut(options.group).unwrap();
        let created_consumer = !group.consumers.contains_key(options.consumer);
        touch_consumer(group, options.consumer, now, false);
        let ids: Vec<StreamId> = if let Some(after) = requested {
            group
                .pending
                .range((
                    std::ops::Bound::Excluded(*after),
                    std::ops::Bound::Unbounded,
                ))
                .filter(|(_, pending)| pending.consumer == options.consumer)
                .take(options.count)
                .map(|(id, _)| *id)
                .collect()
        } else {
            stream
                .entries
                .iter()
                .filter(|item| item.id > group.last_delivered)
                .take(options.count)
                .map(|item| item.id)
                .collect()
        };
        let mut delivered = Vec::new();
        if requested.is_none() && !ids.is_empty() && group.entries_read.is_none() {
            group.entries_read = estimate_read(
                &stream.entries,
                stream.last_generated_id,
                stream.entries_added,
                group.last_delivered,
            );
        }
        for id in &ids {
            let entry = find_entry(&stream.entries, *id);
            if requested.is_none() {
                group.last_delivered = *id;
                if let Some(read) = &mut group.entries_read {
                    *read = read.saturating_add(1);
                }
                if !options.noack {
                    group.pending.insert(
                        *id,
                        Pending {
                            consumer: options.consumer.to_vec(),
                            delivered_at: now,
                            deliveries: 1,
                        },
                    );
                }
            } else if entry.is_some() {
                let pending = group.pending.get_mut(id).unwrap();
                pending.delivered_at = now;
                pending.deliveries = pending.deliveries.saturating_add(1);
            }
            delivered.push(entry_reply(*id, entry));
        }
        if !ids.is_empty() {
            touch_consumer(group, options.consumer, now, true);
            if requested.is_none()
                && group.entries_read.is_none()
                && group.last_delivered == stream.last_generated_id
            {
                group.entries_read = Some(stream.entries_added);
            }
        }
        if created_consumer || !ids.is_empty() {
            mark_modified(store, key);
        }
        if !delivered.is_empty() || requested.is_some() {
            response.push(array(vec![protocol::bulk(key), array(delivered)]));
        }
    }
    Ok(if response.is_empty() {
        b"*-1\r\n".to_vec()
    } else {
        array(response)
    })
}
pub fn blocking_read(
    command: &[Vec<u8>],
    store: &Store,
    probe: &crate::clients::DisconnectProbe,
) -> io::Result<Vec<u8>> {
    let options = match parse_read(command) {
        Ok(options) => options,
        Err(error) => return Ok(error),
    };
    let deadline = options
        .block
        .filter(|duration| !duration.is_zero())
        .and_then(|duration| Instant::now().checked_add(duration));
    loop {
        if probe.disconnected() {
            return Ok(b"*-1\r\n".to_vec());
        }
        let gate = store.command_gate.lock().unwrap();
        let response = crate::execute_checked(command, store, false);
        crate::save_and_publish(store)?;
        if response != b"*-1\r\n"
            || options.block.is_none()
            || options.ids.iter().any(Option::is_some)
        {
            return Ok(response);
        }
        let entries = store.entries.lock().unwrap();
        drop(gate);
        let Some(entries) = crate::wait_for_change(store, entries, deadline, Some(probe)) else {
            return Ok(response);
        };
        drop(entries);
    }
}

fn acknowledge(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let ids = c[3..]
        .iter()
        .map(|raw| id(raw))
        .collect::<Result<Vec<_>>>()?;
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &c[1]);
    if !entries.contains_key(&c[1]) {
        return Ok(integer(0));
    }
    let stream = stream_mut(&mut entries, &c[1])?;
    let Some(group) = stream.groups.get_mut(&c[2]) else {
        return Ok(integer(0));
    };
    let removed = ids
        .into_iter()
        .filter(|id| group.pending.remove(id).is_some())
        .count();
    if removed > 0 {
        mark_modified(store, &c[1]);
    }
    Ok(integer(removed as u64))
}

fn pending(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let mut range = None;
    if c.len() > 3 {
        let (offset, idle) = if c[3].eq_ignore_ascii_case(b"IDLE") && c.len() >= 8 {
            (5, number(&c[4])?)
        } else {
            (3, 0)
        };
        if c.len() != offset + 3 && c.len() != offset + 4 {
            return syntax();
        }
        let start = crate::streams::parse_bound(&c[offset], false).map_err(protocol::error)?;
        let end = crate::streams::parse_bound(&c[offset + 1], true).map_err(protocol::error)?;
        let count = usize::try_from(number(&c[offset + 2])?)
            .map_err(|_| protocol::error("count out of range"))?;
        range = Some((start, end, count, c.get(offset + 3), idle));
    }
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &c[1]);
    if !entries.contains_key(&c[1]) {
        return Err(nogroup(&c[1], &c[2], false));
    }
    let group = group_mut(stream_mut(&mut entries, &c[1])?, &c[1], &c[2])?;
    if let Some(((start, exclusive_start), (end, exclusive_end), count, consumer, idle)) = range {
        let now = now_ms();
        return Ok(array(
            group
                .pending
                .iter()
                .filter(|(id, pending)| {
                    (**id > start || (!exclusive_start && **id == start))
                        && (**id < end || (!exclusive_end && **id == end))
                        && consumer.is_none_or(|name| pending.consumer == *name)
                        && now.saturating_sub(pending.delivered_at) >= idle
                })
                .take(count)
                .map(|(id, pending)| {
                    array(vec![
                        protocol::bulk(id.to_string().as_bytes()),
                        protocol::bulk(&pending.consumer),
                        integer(now.saturating_sub(pending.delivered_at)),
                        integer(pending.deliveries),
                    ])
                })
                .collect(),
        ));
    }
    let first = group.pending.first_key_value().map_or_else(
        || b"$-1\r\n".to_vec(),
        |(id, _)| protocol::bulk(id.to_string().as_bytes()),
    );
    let last = group.pending.last_key_value().map_or_else(
        || b"$-1\r\n".to_vec(),
        |(id, _)| protocol::bulk(id.to_string().as_bytes()),
    );
    let mut counts = BTreeMap::<&[u8], u64>::new();
    for pending in group.pending.values() {
        *counts.entry(&pending.consumer).or_default() += 1;
    }
    let consumers = if counts.is_empty() {
        b"*-1\r\n".to_vec()
    } else {
        array(
            counts
                .into_iter()
                .map(|(name, count)| {
                    array(vec![
                        protocol::bulk(name),
                        protocol::bulk(count.to_string().as_bytes()),
                    ])
                })
                .collect(),
        )
    };
    Ok(array(vec![
        integer(group.pending.len() as u64),
        first,
        last,
        consumers,
    ]))
}

fn set_id(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let last = id(&c[2])?;
    let mut added = None;
    let mut deleted = None;
    let mut i = 3;
    while i < c.len() {
        if i + 1 >= c.len() {
            return syntax();
        }
        if c[i].eq_ignore_ascii_case(b"ENTRIESADDED") {
            added = Some(number(&c[i + 1])?);
        } else if c[i].eq_ignore_ascii_case(b"MAXDELETEDID") {
            deleted = Some(id(&c[i + 1])?);
        } else {
            return syntax();
        }
        i += 2;
    }
    let mut entries = store.entries.lock().unwrap();
    let stream = stream_mut(&mut entries, &c[1])?;
    if last < stream.max_deleted_id {
        return Err(protocol::error(
            "The ID specified in XSETID is smaller than current max_deleted_entry_id",
        ));
    }
    if stream.entries.last().is_some_and(|item| item.id > last)
        || deleted.is_some_and(|id| id > last)
    {
        return Err(protocol::error(
            "The ID specified in XSETID is smaller than the target stream top item",
        ));
    }
    if added.is_some_and(|n| n < stream.entries.len() as u64) {
        return Err(protocol::error(
            "entries_added is smaller than the stream length",
        ));
    }
    stream.last_generated_id = last;
    if let Some(added) = added {
        stream.entries_added = added;
    }
    if let Some(deleted) = deleted.filter(|id| *id != StreamId::default()) {
        stream.max_deleted_id = deleted;
    }
    mark_modified(store, &c[1]);
    Ok(b"+OK\r\n".to_vec())
}

fn estimate_read(
    entries: &[StreamEntry],
    last_id: StreamId,
    added: u64,
    delivered: StreamId,
) -> Option<u64> {
    if delivered == last_id {
        return Some(added);
    }
    let first = entries.first()?;
    if delivered <= first.id {
        return Some(
            added
                .saturating_sub(entries.len() as u64)
                .saturating_add(u64::from(delivered == first.id)),
        );
    }
    None
}

fn info(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    if c[1].eq_ignore_ascii_case(b"STREAM") {
        return stream_info(c, store);
    }
    let mut entries = store.entries.lock().unwrap();
    let stream = stream_mut(&mut entries, &c[2])?;
    if c[1].eq_ignore_ascii_case(b"GROUPS") {
        Ok(array(
            stream
                .groups
                .iter()
                .map(|(name, group)| {
                    let lag = group
                        .entries_read
                        .filter(|_| stream.max_deleted_id <= group.last_delivered)
                        .or_else(|| {
                            estimate_read(
                                &stream.entries,
                                stream.last_generated_id,
                                stream.entries_added,
                                group.last_delivered,
                            )
                        })
                        .and_then(|read| stream.entries_added.checked_sub(read));
                    array(vec![
                        protocol::bulk(b"name"),
                        protocol::bulk(name),
                        protocol::bulk(b"consumers"),
                        integer(group.consumers.len() as u64),
                        protocol::bulk(b"pending"),
                        integer(group.pending.len() as u64),
                        protocol::bulk(b"last-delivered-id"),
                        protocol::bulk(group.last_delivered.to_string().as_bytes()),
                        protocol::bulk(b"entries-read"),
                        group
                            .entries_read
                            .map_or_else(|| b"$-1\r\n".to_vec(), integer),
                        protocol::bulk(b"lag"),
                        lag.map_or_else(|| b"$-1\r\n".to_vec(), integer),
                    ])
                })
                .collect(),
        ))
    } else {
        let group = group_mut(stream, &c[2], &c[3])?;
        let now = now_ms();
        Ok(array(
            group
                .consumers
                .iter()
                .map(|(name, consumer)| {
                    array(vec![
                        protocol::bulk(b"name"),
                        protocol::bulk(name),
                        protocol::bulk(b"pending"),
                        integer(
                            group
                                .pending
                                .values()
                                .filter(|p| p.consumer == *name)
                                .count() as u64,
                        ),
                        protocol::bulk(b"idle"),
                        integer(now.saturating_sub(consumer.seen_at)),
                        protocol::bulk(b"inactive"),
                        consumer.active_at.map_or_else(
                            || b":-1\r\n".to_vec(),
                            |at| integer(now.saturating_sub(at)),
                        ),
                    ])
                })
                .collect(),
        ))
    }
}

fn group_lag(stream: &Stream, group: &Group) -> Option<u64> {
    group
        .entries_read
        .filter(|_| stream.max_deleted_id <= group.last_delivered)
        .or_else(|| {
            estimate_read(
                &stream.entries,
                stream.last_generated_id,
                stream.entries_added,
                group.last_delivered,
            )
        })
        .and_then(|read| stream.entries_added.checked_sub(read))
}

fn stream_info(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let full = c.get(3).is_some_and(|v| v.eq_ignore_ascii_case(b"FULL"));
    let count = match c.len() {
        3 => 10,
        4 if full => 10,
        6 if full && c[4].eq_ignore_ascii_case(b"COUNT") => number(&c[5])? as usize,
        _ => return syntax(),
    };
    let limit = if count == 0 { usize::MAX } else { count };
    let mut entries = store.entries.lock().unwrap();
    let stream = stream_mut(&mut entries, &c[2])?;
    let mut fields = vec![
        protocol::bulk(b"length"),
        integer(stream.entries.len() as u64),
        // ReRu stores entries in a vector, not Redis's radix tree. Zero means
        // no radix allocation; these implementation metrics are not emulated.
        protocol::bulk(b"radix-tree-keys"),
        integer(0),
        protocol::bulk(b"radix-tree-nodes"),
        integer(0),
        protocol::bulk(b"last-generated-id"),
        protocol::bulk(stream.last_generated_id.to_string().as_bytes()),
        protocol::bulk(b"max-deleted-entry-id"),
        protocol::bulk(stream.max_deleted_id.to_string().as_bytes()),
        protocol::bulk(b"entries-added"),
        integer(stream.entries_added),
        protocol::bulk(b"recorded-first-entry-id"),
        protocol::bulk(
            stream
                .entries
                .first()
                .map_or(StreamId::default(), |entry| entry.id)
                .to_string()
                .as_bytes(),
        ),
    ];
    if !full {
        fields.extend([
            protocol::bulk(b"groups"),
            integer(stream.groups.len() as u64),
            protocol::bulk(b"first-entry"),
            stream.entries.first().map_or_else(
                || b"*-1\r\n".to_vec(),
                |entry| entry_reply(entry.id, Some(entry)),
            ),
            protocol::bulk(b"last-entry"),
            stream.entries.last().map_or_else(
                || b"*-1\r\n".to_vec(),
                |entry| entry_reply(entry.id, Some(entry)),
            ),
        ]);
    } else {
        fields.extend([
            protocol::bulk(b"entries"),
            array(
                stream
                    .entries
                    .iter()
                    .take(limit)
                    .map(|entry| entry_reply(entry.id, Some(entry)))
                    .collect(),
            ),
            protocol::bulk(b"groups"),
            array(
                stream
                    .groups
                    .iter()
                    .map(|(name, group)| {
                        let pending = array(
                            group
                                .pending
                                .iter()
                                .take(limit)
                                .map(|(id, pending)| {
                                    array(vec![
                                        protocol::bulk(id.to_string().as_bytes()),
                                        protocol::bulk(&pending.consumer),
                                        integer(pending.delivered_at),
                                        integer(pending.deliveries),
                                    ])
                                })
                                .collect(),
                        );
                        let consumers = array(
                            group
                                .consumers
                                .iter()
                                .map(|(name, consumer)| {
                                    let owned: Vec<_> = group
                                        .pending
                                        .iter()
                                        .filter(|(_, pending)| pending.consumer == *name)
                                        .collect();
                                    array(vec![
                                        protocol::bulk(b"name"),
                                        protocol::bulk(name),
                                        protocol::bulk(b"seen-time"),
                                        integer(consumer.seen_at),
                                        protocol::bulk(b"active-time"),
                                        consumer
                                            .active_at
                                            .map_or_else(|| b":-1\r\n".to_vec(), integer),
                                        protocol::bulk(b"pel-count"),
                                        integer(owned.len() as u64),
                                        protocol::bulk(b"pending"),
                                        array(
                                            owned
                                                .into_iter()
                                                .take(limit)
                                                .map(|(id, pending)| {
                                                    array(vec![
                                                        protocol::bulk(id.to_string().as_bytes()),
                                                        integer(pending.delivered_at),
                                                        integer(pending.deliveries),
                                                    ])
                                                })
                                                .collect(),
                                        ),
                                    ])
                                })
                                .collect(),
                        );
                        array(vec![
                            protocol::bulk(b"name"),
                            protocol::bulk(name),
                            protocol::bulk(b"last-delivered-id"),
                            protocol::bulk(group.last_delivered.to_string().as_bytes()),
                            protocol::bulk(b"entries-read"),
                            group
                                .entries_read
                                .map_or_else(|| b"$-1\r\n".to_vec(), integer),
                            protocol::bulk(b"lag"),
                            group_lag(stream, group).map_or_else(|| b"$-1\r\n".to_vec(), integer),
                            protocol::bulk(b"pel-count"),
                            integer(group.pending.len() as u64),
                            protocol::bulk(b"pending"),
                            pending,
                            protocol::bulk(b"consumers"),
                            consumers,
                        ])
                    })
                    .collect(),
            ),
        ]);
    }
    Ok(array(fields))
}

fn claim(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let min_idle = number(&c[4])?;
    let now = now_ms();
    let mut ids = Vec::new();
    let mut i = 5;
    while i < c.len() {
        if [
            b"IDLE".as_slice(),
            b"TIME",
            b"RETRYCOUNT",
            b"FORCE",
            b"JUSTID",
            b"LASTID",
        ]
        .iter()
        .any(|option| c[i].eq_ignore_ascii_case(option))
        {
            break;
        }
        ids.push(id(&c[i])?);
        i += 1;
    }
    if ids.is_empty() {
        return syntax();
    }
    let mut delivered_at = now;
    let mut retry = None;
    let mut force = false;
    let mut justid = false;
    let mut lastid = None;
    while i < c.len() {
        if c[i].eq_ignore_ascii_case(b"FORCE") {
            force = true;
            i += 1;
        } else if c[i].eq_ignore_ascii_case(b"JUSTID") {
            justid = true;
            i += 1;
        } else if i + 1 < c.len() {
            if c[i].eq_ignore_ascii_case(b"IDLE") {
                delivered_at = now.saturating_sub(number(&c[i + 1])?);
            } else if c[i].eq_ignore_ascii_case(b"TIME") {
                delivered_at = number(&c[i + 1])?.min(now);
            } else if c[i].eq_ignore_ascii_case(b"RETRYCOUNT") {
                retry = Some(number(&c[i + 1])?);
            } else if c[i].eq_ignore_ascii_case(b"LASTID") {
                lastid = Some(id(&c[i + 1])?);
            } else {
                return syntax();
            }
            i += 2;
        } else {
            return syntax();
        }
    }
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &c[1]);
    if !entries.contains_key(&c[1]) {
        return Err(nogroup(&c[1], &c[2], false));
    }
    let stream = stream_mut(&mut entries, &c[1])?;
    let group = stream
        .groups
        .get_mut(&c[2])
        .ok_or_else(|| nogroup(&c[1], &c[2], false))?;
    let mut changed = false;
    if group.consumers.contains_key(&c[3]) {
        touch_consumer(group, &c[3], now, false);
    }
    if let Some(last) = lastid {
        if last > group.last_delivered {
            group.last_delivered = last;
            group.entries_read = None;
            changed = true;
        }
    }
    let mut result = Vec::new();
    for id in ids {
        let Some(item) = find_entry(&stream.entries, id) else {
            changed |= group.pending.remove(&id).is_some();
            continue;
        };
        let previous = group.pending.get(&id);
        if previous.is_none() && !force {
            continue;
        }
        if previous.is_some_and(|p| now.saturating_sub(p.delivered_at) < min_idle) {
            continue;
        }
        let deliveries = retry.unwrap_or_else(|| {
            previous.map_or(1, |p| p.deliveries.saturating_add(u64::from(!justid)))
        });
        group.pending.insert(
            id,
            Pending {
                consumer: c[3].clone(),
                delivered_at,
                deliveries,
            },
        );
        result.push(if justid {
            protocol::bulk(id.to_string().as_bytes())
        } else {
            entry_reply(id, Some(item))
        });
        changed = true;
    }
    if !result.is_empty() {
        touch_consumer(group, &c[3], now, true);
    }
    if changed {
        mark_modified(store, &c[1]);
    }
    Ok(array(result))
}
fn autoclaim(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let min_idle = number(&c[4])?;
    let start = id(&c[5])?;
    let mut count = 100;
    let mut justid = false;
    let mut i = 6;
    while i < c.len() {
        if c[i].eq_ignore_ascii_case(b"COUNT") && i + 1 < c.len() {
            count = positive(&c[i + 1])?;
            i += 2;
        } else if c[i].eq_ignore_ascii_case(b"JUSTID") {
            justid = true;
            i += 1;
        } else {
            return syntax();
        }
    }
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &c[1]);
    if !entries.contains_key(&c[1]) {
        return Err(nogroup(&c[1], &c[2], false));
    }
    let stream = stream_mut(&mut entries, &c[1])?;
    let group = stream
        .groups
        .get_mut(&c[2])
        .ok_or_else(|| nogroup(&c[1], &c[2], false))?;
    let now = now_ms();
    let mut changed = false;
    if group.consumers.contains_key(&c[3]) {
        touch_consumer(group, &c[3], now, false);
    }
    let budget = count.saturating_mul(10);
    let ids: Vec<_> = group
        .pending
        .range(start..)
        .take(budget.saturating_add(1))
        .map(|(id, _)| *id)
        .collect();
    let mut claimed = Vec::new();
    let mut deleted = Vec::new();
    let mut scanned = 0;
    for id in &ids {
        if scanned >= budget || claimed.len() >= count {
            break;
        }
        scanned += 1;
        let Some(item) = find_entry(&stream.entries, *id) else {
            group.pending.remove(id);
            deleted.push(protocol::bulk(id.to_string().as_bytes()));
            changed = true;
            continue;
        };
        let pending = group.pending.get_mut(id).unwrap();
        if now.saturating_sub(pending.delivered_at) < min_idle {
            continue;
        }
        pending.consumer = c[3].clone();
        pending.delivered_at = now;
        pending.deliveries = pending.deliveries.saturating_add(u64::from(!justid));
        claimed.push(if justid {
            protocol::bulk(id.to_string().as_bytes())
        } else {
            entry_reply(*id, Some(item))
        });
        changed = true;
    }
    let next = ids.get(scanned).copied().unwrap_or_default();
    if !claimed.is_empty() {
        touch_consumer(group, &c[3], now, true);
    }
    if changed {
        mark_modified(store, &c[1]);
    }
    Ok(array(vec![
        protocol::bulk(next.to_string().as_bytes()),
        array(claimed),
        array(deleted),
    ]))
}

pub fn metadata_bytes(stream: &Stream) -> usize {
    stream
        .groups
        .iter()
        .map(|(name, group)| {
            name.len()
                + 96
                + group
                    .consumers
                    .keys()
                    .map(|name| name.len() + 48)
                    .sum::<usize>()
                + group
                    .pending
                    .values()
                    .map(|pending| pending.consumer.len() + 64)
                    .sum::<usize>()
        })
        .sum()
}

pub fn encode_metadata(stream: &Stream, out: &mut Vec<u8>) -> io::Result<()> {
    use crate::persistence::{put_bytes, put_u64};
    put_u64(out, stream.entries_added);
    put_u64(out, stream.max_deleted_id.milliseconds);
    put_u64(out, stream.max_deleted_id.sequence);
    put_u64(out, stream.groups.len() as u64);
    for (name, group) in &stream.groups {
        put_bytes(out, name)?;
        put_u64(out, group.last_delivered.milliseconds);
        put_u64(out, group.last_delivered.sequence);
        put_u64(out, group.entries_read.unwrap_or(u64::MAX));
        put_u64(out, group.consumers.len() as u64);
        for (name, consumer) in &group.consumers {
            put_bytes(out, name)?;
            put_u64(out, consumer.seen_at);
            put_u64(out, consumer.active_at.unwrap_or(u64::MAX));
        }
        put_u64(out, group.pending.len() as u64);
        for (id, pending) in &group.pending {
            put_u64(out, id.milliseconds);
            put_u64(out, id.sequence);
            put_bytes(out, &pending.consumer)?;
            put_u64(out, pending.delivered_at);
            put_u64(out, pending.deliveries);
        }
    }
    Ok(())
}
pub fn decode_metadata(stream: &mut Stream, cursor: &mut io::Cursor<&[u8]>) -> io::Result<()> {
    use crate::persistence::{read_bytes, read_u64};
    fn invalid() -> io::Error {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid stream group snapshot metadata",
        )
    }
    fn count(cursor: &mut io::Cursor<&[u8]>) -> io::Result<u64> {
        let n = read_u64(cursor)?;
        if n > 1_000_000
            || n > cursor
                .get_ref()
                .len()
                .saturating_sub(cursor.position() as usize) as u64
        {
            return Err(invalid());
        }
        Ok(n)
    }
    fn stream_id(cursor: &mut io::Cursor<&[u8]>) -> io::Result<StreamId> {
        Ok(StreamId {
            milliseconds: read_u64(cursor)?,
            sequence: read_u64(cursor)?,
        })
    }
    stream.entries_added = read_u64(cursor)?;
    stream.max_deleted_id = stream_id(cursor)?;
    if stream.entries_added < stream.entries.len() as u64
        || stream.max_deleted_id > stream.last_generated_id
    {
        return Err(invalid());
    }
    for _ in 0..count(cursor)? {
        let name = read_bytes(cursor)?;
        let last_delivered = stream_id(cursor)?;
        let read = read_u64(cursor)?;
        let mut group = Group {
            last_delivered,
            entries_read: (read != u64::MAX).then_some(read),
            ..Group::default()
        };
        for _ in 0..count(cursor)? {
            let consumer = read_bytes(cursor)?;
            let seen_at = read_u64(cursor)?;
            let active = read_u64(cursor)?;
            if group
                .consumers
                .insert(
                    consumer,
                    Consumer {
                        seen_at,
                        active_at: (active != u64::MAX).then_some(active),
                    },
                )
                .is_some()
            {
                return Err(invalid());
            }
        }
        for _ in 0..count(cursor)? {
            let id = stream_id(cursor)?;
            let consumer = read_bytes(cursor)?;
            let pending = Pending {
                consumer,
                delivered_at: read_u64(cursor)?,
                deliveries: read_u64(cursor)?,
            };
            if id == StreamId::default()
                || !group.consumers.contains_key(&pending.consumer)
                || group.pending.insert(id, pending).is_some()
            {
                return Err(invalid());
            }
        }
        if stream.groups.insert(name, group).is_some() {
            return Err(invalid());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_rejects_orphaned_pending_owners_and_unbounded_group_counts() {
        let mut stream = Stream::default();
        let mut group = Group::default();
        group.pending.insert(
            StreamId {
                milliseconds: 1,
                sequence: 0,
            },
            Pending {
                consumer: b"missing".to_vec(),
                delivered_at: 0,
                deliveries: 1,
            },
        );
        stream.groups.insert(b"group".to_vec(), group);
        let mut bytes = Vec::new();
        encode_metadata(&stream, &mut bytes).unwrap();
        assert!(decode_metadata(
            &mut Stream::default(),
            &mut io::Cursor::new(bytes.as_slice())
        )
        .is_err());
        bytes.clear();
        encode_metadata(&Stream::default(), &mut bytes).unwrap();
        bytes[24..32].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(decode_metadata(
            &mut Stream::default(),
            &mut io::Cursor::new(bytes.as_slice())
        )
        .is_err());
    }
    #[test]
    fn legacy_stream_snapshot_without_groups_still_loads_and_preserves_last_id() {
        use crate::persistence::{put_bytes, put_u64};
        let mut bytes = b"RERUSNP1".to_vec();
        put_u64(&mut bytes, 1);
        put_bytes(&mut bytes, b"s").unwrap();
        put_u64(&mut bytes, u64::MAX);
        bytes.push(7);
        put_u64(&mut bytes, 2);
        put_u64(&mut bytes, 0);
        put_u64(&mut bytes, 1);
        put_u64(&mut bytes, 1);
        put_u64(&mut bytes, 0);
        put_u64(&mut bytes, 2);
        put_bytes(&mut bytes, b"f").unwrap();
        put_bytes(&mut bytes, b"v").unwrap();
        let mut entries = crate::persistence::decode(&bytes).unwrap();
        let Value::Stream(stream) = &mut entries.get_mut(b"s".as_slice()).unwrap().value else {
            panic!()
        };
        assert!(stream.groups.is_empty());
        assert_eq!(stream.entries.len(), 1);
        assert_eq!(
            stream
                .append(b"2-*", vec![b"f".to_vec(), b"new".to_vec()])
                .unwrap()
                .to_string(),
            "2-1"
        );
    }
}
