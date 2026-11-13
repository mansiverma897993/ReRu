// SPDX-License-Identifier: AGPL-3.0-only
mod aof;
mod auth;
mod bitmaps;
mod cache;
mod clients;
mod collections;
mod command_table;
mod connection;
mod eviction;
mod expiry;
mod geo;
mod hyperloglog;
mod keyspace;
mod list_ops;
mod persistence;
mod protocol;
mod pubsub;
mod rdb;
mod replication;
mod sorted_sets;
mod stream_groups;
mod streams;

use std::collections::{HashMap, HashSet};
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
enum Value {
    String(Vec<u8>),
    List(Vec<Vec<u8>>),
    Stream(streams::Stream),
    Hash(HashMap<Vec<u8>, Vec<u8>>),
    Set(HashSet<Vec<u8>>),
    SortedSet(sorted_sets::SortedSet),
}

#[derive(Clone)]
struct Entry {
    value: Value,
    expires_at: Option<Instant>,
}

struct SharedStore {
    entries: Mutex<HashMap<Vec<u8>, Entry>>,
    changed: Condvar,
    command_gate: Mutex<()>,
    snapshot_path: Option<PathBuf>,
    aof_path: Option<PathBuf>,
    aof_rewrite: aof::Rewrite,
    auth: auth::Settings,
    rdb: rdb::Persistence,
    replicas: Mutex<Vec<SyncSender<Vec<u8>>>>,
    read_only: bool,
    watched_keys: Mutex<HashMap<Vec<u8>, WatchRegistration>>,
    pending_keys: Mutex<HashSet<Vec<u8>>>,
    staged_keys: Mutex<Option<HashSet<Vec<u8>>>>,
    eviction: eviction::Eviction,
    expiry: expiry::Expiry,
    clients: Arc<clients::Limits>,
    pubsub: pubsub::Hub,
}

#[derive(Default)]
struct WatchRegistration {
    revision: u64,
    clients: usize,
}

struct ClientWatches {
    store: Store,
    keys: HashMap<Vec<u8>, (u64, Option<Instant>)>,
}

impl ClientWatches {
    fn new(store: Store) -> Self {
        Self {
            store,
            keys: HashMap::new(),
        }
    }

    // Caller holds command_gate so registration and EXEC observe one command order.
    fn watch(&mut self, keys: &[Vec<u8>]) {
        let mut entries = self.store.entries.lock().unwrap();
        for key in keys {
            remove_expired(&mut entries, key);
        }
        let mut registry = self.store.watched_keys.lock().unwrap();
        for key in keys {
            if self.keys.contains_key(key) {
                continue;
            }
            let registration = registry.entry(key.clone()).or_default();
            registration.clients += 1;
            self.keys.insert(
                key.clone(),
                (
                    registration.revision,
                    entries.get(key).and_then(|entry| entry.expires_at),
                ),
            );
        }
    }

    fn changed(&self) -> bool {
        let registry = self.store.watched_keys.lock().unwrap();
        self.keys.iter().any(|(key, (revision, expiry))| {
            registry
                .get(key)
                .is_none_or(|item| item.revision != *revision)
                || expiry.is_some_and(|deadline| Instant::now() >= deadline)
        })
    }

    fn clear(&mut self) {
        let mut registry = self.store.watched_keys.lock().unwrap();
        for key in self.keys.keys() {
            if let Some(registration) = registry.get_mut(key) {
                registration.clients -= 1;
                if registration.clients == 0 {
                    registry.remove(key);
                }
            }
        }
        self.keys.clear();
    }
}

impl Drop for ClientWatches {
    fn drop(&mut self) {
        self.clear();
    }
}

fn mark_modified(store: &Store, key: &[u8]) {
    let mut staged = store.staged_keys.lock().unwrap();
    if let Some(keys) = staged.as_mut() {
        keys.insert(key.to_vec());
        return;
    }
    drop(staged);
    store.changed.notify_all();
    store.eviction.note_modified(key);
    mark_evicted(store, key);
}

fn mark_evicted(store: &Store, key: &[u8]) {
    let mut staged = store.staged_keys.lock().unwrap();
    if let Some(keys) = staged.as_mut() {
        keys.insert(key.to_vec());
        return;
    }
    drop(staged);
    store.pending_keys.lock().unwrap().insert(key.to_vec());
    invalidate_watch(store, key);
}

fn invalidate_watch(store: &Store, key: &[u8]) {
    if let Some(registration) = store.watched_keys.lock().unwrap().get_mut(key) {
        registration.revision = registration.revision.wrapping_add(1);
    }
}

type Store = Arc<SharedStore>;

fn connection_closed(probe: Option<&clients::DisconnectProbe>) -> bool {
    probe.is_some_and(clients::DisconnectProbe::disconnected)
}

// Keep the dataset lock in the Condvar wait, but wake often enough to detect
// a disconnected blocking client even when no key changes. No monitor thread
// or queued waiter survives the connection.
fn wait_for_change<'a>(
    store: &Store,
    entries: MutexGuard<'a, HashMap<Vec<u8>, Entry>>,
    deadline: Option<Instant>,
    probe: Option<&clients::DisconnectProbe>,
) -> Option<MutexGuard<'a, HashMap<Vec<u8>, Entry>>> {
    if connection_closed(probe) {
        return None;
    }
    let remaining = deadline.map(|end| end.saturating_duration_since(Instant::now()));
    if remaining.is_some_and(|duration| duration.is_zero()) {
        return None;
    }
    let timeout = if probe.is_some() {
        Some(
            remaining
                .unwrap_or(Duration::from_millis(20))
                .min(Duration::from_millis(20)),
        )
    } else {
        remaining
    };
    let entries = match timeout {
        Some(duration) => store.changed.wait_timeout(entries, duration).unwrap().0,
        None => store.changed.wait(entries).unwrap(),
    };
    (!connection_closed(probe)).then_some(entries)
}

fn is_mutating(command: &[Vec<u8>]) -> bool {
    (command[0].eq_ignore_ascii_case(b"CONFIG")
        && command
            .get(1)
            .is_some_and(|name| name.eq_ignore_ascii_case(b"SET")))
        || cache::is_mutating(command)
        || geo::is_mutating(command)
        || list_ops::is_mutating(command)
        || hyperloglog::is_mutating(command)
        || bitmaps::is_mutating(command)
        || sorted_sets::is_mutating(command)
        || collections::is_mutating(command)
        || stream_groups::is_mutating(command)
        || [
            b"SET".as_slice(),
            b"INCR",
            b"RPUSH",
            b"LPUSH",
            b"LPOP",
            b"BLPOP",
            b"BRPOP",
            b"BLMOVE",
            b"BRPOPLPUSH",
            b"LMPOP",
            b"BLMPOP",
            b"ZMPOP",
            b"BZMPOP",
            b"BZPOPMIN",
            b"BZPOPMAX",
            b"XADD",
            b"XDEL",
            b"XTRIM",
            b"DEL",
            b"UNLINK",
            b"RENAME",
            b"RENAMENX",
            b"COPY",
            b"EXPIRE",
            b"PEXPIRE",
            b"EXPIREAT",
            b"PEXPIREAT",
            b"PERSIST",
            b"FLUSHDB",
            b"FLUSHALL",
            b"RESTORE",
        ]
        .iter()
        .any(|name| command[0].eq_ignore_ascii_case(name))
}

fn record_write(store: &Store, command: &[Vec<u8>], response: &[u8]) {
    // Collection handlers record real changes themselves, including no-op checks.
    if cache::valid_arity(command).is_some()
        || stream_groups::valid_arity(command).is_some()
        || rdb::valid_arity(command).is_some()
        || geo::valid_arity(command).is_some()
        || list_ops::valid_arity(command).is_some()
        || hyperloglog::valid_arity(command).is_some()
        || bitmaps::valid_arity(command).is_some()
        || sorted_sets::valid_arity(command).is_some()
        || collections::valid_arity(command).is_some()
        || !is_mutating(command)
        || response.starts_with(b"-")
        || command[0].eq_ignore_ascii_case(b"DEL")
        || command[0].eq_ignore_ascii_case(b"UNLINK")
        || command[0].eq_ignore_ascii_case(b"RENAME")
        || command[0].eq_ignore_ascii_case(b"RENAMENX")
        || command[0].eq_ignore_ascii_case(b"COPY")
        || command[0].eq_ignore_ascii_case(b"CONFIG")
        || command[0].eq_ignore_ascii_case(b"BLPOP")
        || command[0].eq_ignore_ascii_case(b"BRPOP")
        || command[0].eq_ignore_ascii_case(b"BLMOVE")
        || command[0].eq_ignore_ascii_case(b"BRPOPLPUSH")
        || command[0].eq_ignore_ascii_case(b"LMPOP")
        || command[0].eq_ignore_ascii_case(b"BLMPOP")
        || command[0].eq_ignore_ascii_case(b"ZMPOP")
        || command[0].eq_ignore_ascii_case(b"BZMPOP")
        || command[0].eq_ignore_ascii_case(b"BZPOPMIN")
        || command[0].eq_ignore_ascii_case(b"BZPOPMAX")
        || command[0].eq_ignore_ascii_case(b"XADD")
        || command[0].eq_ignore_ascii_case(b"XDEL")
        || command[0].eq_ignore_ascii_case(b"XTRIM")
        || command[0].eq_ignore_ascii_case(b"FLUSHDB")
        || command[0].eq_ignore_ascii_case(b"FLUSHALL")
    {
        return;
    }
    if command[0].eq_ignore_ascii_case(b"LPOP")
        && [b"$-1\r\n".as_slice(), b"*-1\r\n", b"*0\r\n"].contains(&response)
    {
        return;
    }
    if [
        b"EXPIRE".as_slice(),
        b"PEXPIRE",
        b"EXPIREAT",
        b"PEXPIREAT",
        b"PERSIST",
    ]
    .iter()
    .any(|name| command[0].eq_ignore_ascii_case(name))
        && response != b":1\r\n"
    {
        return;
    }
    mark_modified(store, &command[1]);
}

// The caller holds command_gate. Bounded writes under noeviction and volatile
// policies are staged so an OOM cannot leave partial data or invalidate WATCH.
fn execute_checked(command: &[Vec<u8>], store: &Store, allow_blocking: bool) -> Vec<u8> {
    let settings = store.eviction.settings();
    if !is_mutating(command)
        || settings.maxmemory == 0
        || command[0].eq_ignore_ascii_case(b"CONFIG")
    {
        let response = execute_command(command, store, allow_blocking, None);
        record_write(store, command, &response);
        return response;
    }
    if settings.policy.is_allkeys() {
        let response = execute_command(command, store, allow_blocking, None);
        record_write(store, command, &response);
        if !response.starts_with(b"-") {
            store.eviction.evict(store);
        }
        return response;
    }

    let before = store.entries.lock().unwrap().clone();
    let before_bytes = eviction::dataset_bytes(&before);
    *store.staged_keys.lock().unwrap() = Some(HashSet::new());
    let response = execute_command(command, store, allow_blocking, None);
    record_write(store, command, &response);
    let after_bytes = eviction::dataset_bytes(&store.entries.lock().unwrap());
    let failed = response.starts_with(b"-");
    let overflow = !failed
        && after_bytes > settings.maxmemory
        && after_bytes > before_bytes
        && (settings.policy == eviction::Policy::NoEviction || !store.eviction.evict(store));
    let changed = store.staged_keys.lock().unwrap().take().unwrap();
    if overflow || failed {
        *store.entries.lock().unwrap() = before;
        return if overflow {
            b"-OOM command not allowed when used memory > 'maxmemory'.\r\n".to_vec()
        } else {
            response
        };
    }
    for key in changed {
        if store.entries.lock().unwrap().contains_key(&key) {
            mark_modified(store, &key);
        } else {
            store.eviction.forget(&key);
            mark_evicted(store, &key);
        }
    }
    response
}

fn save_and_publish(store: &Store) -> io::Result<()> {
    let changed_keys: Vec<_> = store.pending_keys.lock().unwrap().drain().collect();
    if changed_keys.is_empty() {
        return Ok(());
    }
    if !store.read_only {
        store
            .expiry
            .update(&store.entries.lock().unwrap(), &changed_keys);
    }
    if store.snapshot_path.is_none()
        && store.aof_path.is_none()
        && store.replicas.lock().unwrap().is_empty()
    {
        return Ok(());
    }
    let result = (|| {
        if let Some(path) = &store.aof_path {
            aof::append(path, &changed_keys, &store.entries.lock().unwrap())?;
        }
        if store.snapshot_path.is_some() || !store.replicas.lock().unwrap().is_empty() {
            let bytes = persistence::encode(&store.entries.lock().unwrap())?;
            if let Some(path) = &store.snapshot_path {
                persistence::save(path, &bytes)?;
            }
            let update = replication::encode(&bytes, Some(&changed_keys))?;
            store
                .replicas
                .lock()
                .unwrap()
                .retain(|sender| sender.try_send(update.clone()).is_ok());
        }
        Ok(())
    })();
    if result.is_err() {
        store.pending_keys.lock().unwrap().extend(changed_keys);
    }
    result
}

fn write_snapshot_frame(stream: &mut TcpStream, snapshot: &[u8]) -> io::Result<()> {
    stream.write_all(&(snapshot.len() as u64).to_be_bytes())?;
    stream.write_all(snapshot)
}

fn serve_replica(mut stream: TcpStream, store: &Store) -> io::Result<()> {
    let (sender, receiver) = mpsc::sync_channel(8);
    let initial = {
        let _gate = store.command_gate.lock().unwrap();
        let snapshot = persistence::encode(&store.entries.lock().unwrap())?;
        let bytes = replication::encode(&snapshot, None)?;
        store.replicas.lock().unwrap().push(sender);
        bytes
    };
    write_snapshot_frame(&mut stream, &initial)?;
    for snapshot in receiver {
        write_snapshot_frame(&mut stream, &snapshot)?;
    }
    Ok(())
}

fn follow_primary(address: String, store: Store, password: Option<Vec<u8>>) {
    loop {
        let result = (|| -> io::Result<()> {
            let mut stream = TcpStream::connect(&address)?;
            stream.set_nodelay(true)?;
            if let Some(password) = &password {
                stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                stream.set_write_timeout(Some(Duration::from_secs(3)))?;
                stream.write_all(&protocol::array(&[b"AUTH".to_vec(), password.clone()]))?;
                let mut reply = [0; 5];
                stream.read_exact(&mut reply)?;
                if &reply != b"+OK\r\n" {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "primary authentication failed",
                    ));
                }
                stream.set_read_timeout(None)?;
                stream.set_write_timeout(None)?;
            }
            stream.write_all(b"*1\r\n$8\r\nREPLSYNC\r\n")?;
            loop {
                let mut length = [0; 8];
                stream.read_exact(&mut length)?;
                let size = u64::from_be_bytes(length);
                if size > replication::MAX_FRAME as u64 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "replica snapshot too large",
                    ));
                }
                let mut bytes = vec![0; size as usize];
                stream.read_exact(&mut bytes)?;
                let update = replication::decode(&bytes)?;
                let entries = persistence::decode(update.snapshot)?;
                let _gate = store.command_gate.lock().unwrap();
                if let Some(path) = &store.snapshot_path {
                    persistence::save(path, update.snapshot)?;
                }
                if update.full_sync {
                    for item in store.watched_keys.lock().unwrap().values_mut() {
                        item.revision = item.revision.wrapping_add(1);
                    }
                } else {
                    for key in &update.changed_keys {
                        invalidate_watch(&store, key);
                    }
                }
                *store.entries.lock().unwrap() = entries;
                store.changed.notify_all();
            }
        })();
        if let Err(error) = result {
            eprintln!("replica link {address}: {error}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn remove_expired(store: &mut HashMap<Vec<u8>, Entry>, key: &[u8]) {
    if store.get(key).is_some_and(|entry| {
        entry
            .expires_at
            .is_some_and(|expiry| Instant::now() >= expiry)
    }) {
        store.remove(key);
    }
}

fn list_range(items: &[Vec<u8>], start: i64, stop: i64) -> Vec<u8> {
    let len = items.len() as i128;
    if len == 0 {
        return protocol::array(&[]);
    }
    let normalize = |index: i64| {
        if index < 0 {
            len + i128::from(index)
        } else {
            i128::from(index)
        }
    };
    let start = normalize(start).max(0);
    let stop = normalize(stop).min(len - 1);
    if start >= len || stop < start {
        protocol::array(&[])
    } else {
        protocol::array(&items[start as usize..=stop as usize])
    }
}

fn try_blocking_pop(
    entries: &mut HashMap<Vec<u8>, Entry>,
    keys: &[Vec<u8>],
    store: &Store,
    from_right: bool,
) -> Option<Vec<u8>> {
    for key in keys {
        remove_expired(entries, key);
        match entries.get_mut(key) {
            Some(Entry {
                value: Value::List(items),
                ..
            }) => {
                if items.is_empty() {
                    continue;
                }
                let value = if from_right {
                    items.pop().unwrap()
                } else {
                    items.remove(0)
                };
                if items.is_empty() {
                    entries.remove(key);
                }
                mark_modified(store, key);
                return Some(protocol::array(&[key.clone(), value]));
            }
            Some(_) => {
                return Some(
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec(),
                );
            }
            None => {}
        }
    }
    None
}

fn blpop_deadline(timeout: &[u8]) -> Result<Option<Instant>, Vec<u8>> {
    let seconds = std::str::from_utf8(timeout)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .ok_or_else(|| protocol::error("timeout is not a float or out of range"))?;
    if seconds == 0.0 {
        return Ok(None);
    }
    let duration = Duration::try_from_secs_f64(seconds)
        .map_err(|_| protocol::error("timeout is not a float or out of range"))?;
    Instant::now()
        .checked_add(duration)
        .map(Some)
        .ok_or_else(|| protocol::error("timeout is not a float or out of range"))
}

struct MultiPopOptions<'a> {
    keys: &'a [Vec<u8>],
    from_right: bool,
    count: usize,
    deadline: Option<Instant>,
}

fn parse_multi_pop(command: &[Vec<u8>], blocking: bool) -> Result<MultiPopOptions<'_>, Vec<u8>> {
    let count_at = if blocking { 2 } else { 1 };
    let wrong_arity = || {
        protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        ))
    };
    if command.len() < count_at + 3 {
        return Err(wrong_arity());
    }
    let deadline = if blocking {
        blpop_deadline(&command[1])?
    } else {
        None
    };
    let numkeys = std::str::from_utf8(&command[count_at])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| protocol::error("numkeys should be greater than 0"))?;
    let start = count_at + 1;
    let end = start
        .checked_add(numkeys)
        .filter(|end| *end < command.len())
        .ok_or_else(wrong_arity)?;
    let from_right = if command[end].eq_ignore_ascii_case(b"RIGHT") {
        true
    } else if command[end].eq_ignore_ascii_case(b"LEFT") {
        false
    } else {
        return Err(protocol::error("syntax error"));
    };
    let count = if command.len() == end + 1 {
        1
    } else if command.len() == end + 3 && command[end + 1].eq_ignore_ascii_case(b"COUNT") {
        std::str::from_utf8(&command[end + 2])
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| protocol::error("COUNT should be greater than 0"))?
    } else {
        return Err(protocol::error("syntax error"));
    };
    Ok(MultiPopOptions {
        keys: &command[start..end],
        from_right,
        count,
        deadline,
    })
}

fn try_multi_pop(
    entries: &mut HashMap<Vec<u8>, Entry>,
    options: &MultiPopOptions<'_>,
    store: &Store,
) -> Option<Vec<u8>> {
    for key in options.keys {
        remove_expired(entries, key);
        match entries.get_mut(key) {
            Some(Entry {
                value: Value::List(items),
                ..
            }) => {
                if items.is_empty() {
                    continue;
                }
                let count = options.count.min(items.len());
                let values = if options.from_right {
                    (0..count).map(|_| items.pop().unwrap()).collect::<Vec<_>>()
                } else {
                    items.drain(..count).collect::<Vec<_>>()
                };
                if items.is_empty() {
                    entries.remove(key);
                }
                mark_modified(store, key);
                let mut reply = b"*2\r\n".to_vec();
                reply.extend(protocol::bulk(key));
                reply.extend(protocol::array(&values));
                return Some(reply);
            }
            Some(_) => {
                return Some(
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec(),
                );
            }
            None => {}
        }
    }
    None
}

struct ZPopOptions<'a> {
    keys: &'a [Vec<u8>],
    from_max: bool,
    count: usize,
    deadline: Option<Instant>,
    legacy: bool,
}

fn parse_zpop(command: &[Vec<u8>]) -> Result<ZPopOptions<'_>, Vec<u8>> {
    let name = &command[0];
    let legacy = name.eq_ignore_ascii_case(b"BZPOPMIN") || name.eq_ignore_ascii_case(b"BZPOPMAX");
    let blocking = legacy || name.eq_ignore_ascii_case(b"BZMPOP");
    let wrong_arity = || {
        protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(name).to_ascii_lowercase()
        ))
    };
    if legacy {
        if command.len() < 3 {
            return Err(wrong_arity());
        }
        return Ok(ZPopOptions {
            keys: &command[1..command.len() - 1],
            from_max: name.eq_ignore_ascii_case(b"BZPOPMAX"),
            count: 1,
            deadline: blpop_deadline(command.last().unwrap())?,
            legacy: true,
        });
    }
    let count_at = if blocking { 2 } else { 1 };
    if command.len() < count_at + 3 {
        return Err(wrong_arity());
    }
    let deadline = if blocking {
        blpop_deadline(&command[1])?
    } else {
        None
    };
    let numkeys = std::str::from_utf8(&command[count_at])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| protocol::error("numkeys should be greater than 0"))?;
    let start = count_at + 1;
    let end = start
        .checked_add(numkeys)
        .filter(|end| *end < command.len())
        .ok_or_else(wrong_arity)?;
    let from_max = if command[end].eq_ignore_ascii_case(b"MAX") {
        true
    } else if command[end].eq_ignore_ascii_case(b"MIN") {
        false
    } else {
        return Err(protocol::error("syntax error"));
    };
    let count = if command.len() == end + 1 {
        1
    } else if command.len() == end + 3 && command[end + 1].eq_ignore_ascii_case(b"COUNT") {
        std::str::from_utf8(&command[end + 2])
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| protocol::error("COUNT should be greater than 0"))?
    } else {
        return Err(protocol::error("syntax error"));
    };
    Ok(ZPopOptions {
        keys: &command[start..end],
        from_max,
        count,
        deadline,
        legacy: false,
    })
}

fn try_zpop(
    entries: &mut HashMap<Vec<u8>, Entry>,
    options: &ZPopOptions<'_>,
    store: &Store,
) -> Option<Vec<u8>> {
    for key in options.keys {
        remove_expired(entries, key);
        match entries.get_mut(key) {
            Some(Entry {
                value: Value::SortedSet(scores),
                ..
            }) => {
                if scores.is_empty() {
                    continue;
                }
                let selected: Vec<_> = if options.from_max {
                    scores
                        .entries()
                        .rev()
                        .take(options.count)
                        .map(|(member, score)| (member.clone(), score))
                        .collect()
                } else {
                    scores
                        .entries()
                        .take(options.count)
                        .map(|(member, score)| (member.clone(), score))
                        .collect()
                };
                for (member, _) in &selected {
                    scores.remove(member);
                }
                if scores.is_empty() {
                    entries.remove(key);
                }
                mark_modified(store, key);
                if options.legacy {
                    let (member, score) = &selected[0];
                    return Some(protocol::array(&[
                        key.clone(),
                        member.clone(),
                        sorted_sets::score_text(*score),
                    ]));
                }
                let mut reply = b"*2\r\n".to_vec();
                reply.extend(protocol::bulk(key));
                reply.extend(format!("*{}\r\n", selected.len()).as_bytes());
                for (member, score) in selected {
                    reply.extend(b"*2\r\n");
                    reply.extend(protocol::bulk(&member));
                    reply.extend(protocol::bulk(&sorted_sets::score_text(score)));
                }
                return Some(reply);
            }
            Some(_) => {
                return Some(
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec(),
                );
            }
            None => {}
        }
    }
    None
}

fn zpop_command(
    command: &[Vec<u8>],
    store: &Store,
    allow_blocking: bool,
    probe: Option<&clients::DisconnectProbe>,
) -> Vec<u8> {
    let options = match parse_zpop(command) {
        Ok(options) => options,
        Err(error) => return error,
    };
    let blocking = command[0].eq_ignore_ascii_case(b"BZMPOP") || options.legacy;
    loop {
        if connection_closed(probe) {
            return b"*-1\r\n".to_vec();
        }
        let gate = (blocking && allow_blocking).then(|| store.command_gate.lock().unwrap());
        let mut entries = store.entries.lock().unwrap();
        if let Some(response) = try_zpop(&mut entries, &options, store) {
            return response;
        }
        if !blocking || !allow_blocking {
            return b"*-1\r\n".to_vec();
        }
        drop(gate);
        entries = match wait_for_change(store, entries, options.deadline, probe) {
            Some(entries) => entries,
            None => return b"*-1\r\n".to_vec(),
        };
        drop(entries);
    }
}

fn blocking_move(
    command: &[Vec<u8>],
    store: &Store,
    allow_blocking: bool,
    probe: Option<&clients::DisconnectProbe>,
) -> Vec<u8> {
    let legacy = command[0].eq_ignore_ascii_case(b"BRPOPLPUSH");
    if command.len() != if legacy { 4 } else { 6 } {
        return protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        ));
    }
    let deadline = match blpop_deadline(command.last().unwrap()) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let move_command = if legacy {
        vec![
            b"RPOPLPUSH".to_vec(),
            command[1].clone(),
            command[2].clone(),
        ]
    } else {
        for direction in [&command[3], &command[4]] {
            if !direction.eq_ignore_ascii_case(b"LEFT") && !direction.eq_ignore_ascii_case(b"RIGHT")
            {
                return protocol::error("syntax error");
            }
        }
        let mut move_command = vec![b"LMOVE".to_vec()];
        move_command.extend_from_slice(&command[1..5]);
        move_command
    };
    loop {
        if connection_closed(probe) {
            return b"$-1\r\n".to_vec();
        }
        let gate = allow_blocking.then(|| store.command_gate.lock().unwrap());
        let mut entries = store.entries.lock().unwrap();
        remove_expired(&mut entries, &command[1]);
        let available = match entries.get(&command[1]).map(|entry| &entry.value) {
            Some(Value::List(items)) => !items.is_empty(),
            Some(_) => true,
            None => false,
        };
        if available {
            drop(entries);
            let response = if allow_blocking {
                execute_checked(&move_command, store, false)
            } else {
                list_ops::execute(&move_command, store).expect("valid move command")
            };
            drop(gate);
            return response;
        }
        if !allow_blocking {
            return b"$-1\r\n".to_vec();
        }
        drop(gate);
        entries = match wait_for_change(store, entries, deadline, probe) {
            Some(entries) => entries,
            None => return b"$-1\r\n".to_vec(),
        };
        drop(entries);
    }
}

struct XReadOptions<'a> {
    keys: &'a [Vec<u8>],
    ids: &'a [Vec<u8>],
    count: usize,
    block: Option<Duration>,
}

fn parse_xread(command: &[Vec<u8>]) -> Result<XReadOptions<'_>, Vec<u8>> {
    let mut index = 1;
    let mut count = usize::MAX;
    let mut block = None;
    while index < command.len() && !command[index].eq_ignore_ascii_case(b"STREAMS") {
        if index + 1 >= command.len() {
            return Err(protocol::error("syntax error"));
        }
        let number = std::str::from_utf8(&command[index + 1])
            .ok()
            .and_then(|text| text.parse::<usize>().ok())
            .ok_or_else(|| protocol::error("value is not an integer or out of range"))?;
        if command[index].eq_ignore_ascii_case(b"COUNT") && number > 0 {
            count = number;
        } else if command[index].eq_ignore_ascii_case(b"BLOCK") {
            block = Some(Duration::from_millis(number as u64));
        } else {
            return Err(protocol::error("syntax error"));
        }
        index += 2;
    }
    if index == command.len() {
        return Err(protocol::error("syntax error"));
    }
    let rest = &command[index + 1..];
    if rest.is_empty() || !rest.len().is_multiple_of(2) {
        return Err(protocol::error("Unbalanced XREAD list of streams"));
    }
    let half = rest.len() / 2;
    Ok(XReadOptions {
        keys: &rest[..half],
        ids: &rest[half..],
        count,
        block,
    })
}

fn try_xread(
    entries: &mut HashMap<Vec<u8>, Entry>,
    keys: &[Vec<u8>],
    ids: &[streams::StreamId],
    count: usize,
) -> Result<Option<Vec<u8>>, Vec<u8>> {
    let mut groups = Vec::new();
    for (key, id) in keys.iter().zip(ids) {
        remove_expired(entries, key);
        match entries.get(key).map(|entry| &entry.value) {
            Some(Value::Stream(stream)) => {
                let found = stream.after(*id, count);
                if !found.is_empty() {
                    groups.push((key, streams::stream_entries(&found)));
                }
            }
            Some(_) => {
                return Err(
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec(),
                )
            }
            None => {}
        }
    }
    if groups.is_empty() {
        return Ok(None);
    }
    let mut response = format!("*{}\r\n", groups.len()).into_bytes();
    for (key, encoded_entries) in groups {
        response.extend_from_slice(b"*2\r\n");
        response.extend(protocol::bulk(key));
        response.extend(encoded_entries);
    }
    Ok(Some(response))
}

fn xread_ids(
    entries: &mut HashMap<Vec<u8>, Entry>,
    keys: &[Vec<u8>],
    raw_ids: &[Vec<u8>],
) -> Result<Vec<streams::StreamId>, Vec<u8>> {
    keys.iter()
        .zip(raw_ids)
        .map(|(key, raw)| {
            if raw != b"$" {
                return streams::StreamId::parse_query(raw).map_err(protocol::error);
            }
            remove_expired(entries, key);
            match entries.get(key).map(|entry| &entry.value) {
                Some(Value::Stream(stream)) => Ok(stream.last_id()),
                Some(_) => Err(
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec(),
                ),
                None => Ok(streams::StreamId {
                    milliseconds: 0,
                    sequence: 0,
                }),
            }
        })
        .collect()
}

fn execute_command(
    command: &[Vec<u8>],
    store: &Store,
    allow_blocking: bool,
    probe: Option<&clients::DisconnectProbe>,
) -> Vec<u8> {
    if command[0].eq_ignore_ascii_case(b"COMMAND") {
        return command_table::execute(command);
    }
    if !command_table::supports(command) {
        return protocol::error("unknown command");
    }
    if let Some(response) = stream_groups::execute(command, store) {
        return response;
    }
    if command[0].eq_ignore_ascii_case(b"ACL") {
        return auth::acl(command);
    }
    if command[0].eq_ignore_ascii_case(b"BGREWRITEAOF") {
        return aof::bgrewrite(command, store);
    }
    store.eviction.touch(command, store);
    if command[0].eq_ignore_ascii_case(b"INFO") {
        eviction::info(command, store)
    } else if command[0].eq_ignore_ascii_case(b"CONFIG") {
        eviction::execute_config(command, store)
    } else if let Some(response) = pubsub::execute(command, &store.pubsub) {
        response
    } else if let Some(response) = cache::execute(command, store) {
        response
    } else if let Some(response) = rdb::execute(command, store) {
        response
    } else if let Some(response) = geo::execute(command, store) {
        response
    } else if let Some(response) = list_ops::execute(command, store) {
        response
    } else if let Some(response) = hyperloglog::execute(command, store) {
        response
    } else if let Some(response) = bitmaps::execute(command, store) {
        response
    } else if let Some(response) = sorted_sets::execute(command, store) {
        response
    } else if let Some(response) = collections::execute(command, store) {
        response
    } else if let Some(response) = keyspace::execute(command, store) {
        response
    } else if command[0].eq_ignore_ascii_case(b"UNWATCH") {
        b"+OK\r\n".to_vec()
    } else if command[0].eq_ignore_ascii_case(b"PING") {
        match command.len() {
            1 => b"+PONG\r\n".to_vec(),
            2 => protocol::bulk(&command[1]),
            _ => protocol::error("wrong number of arguments for 'ping' command"),
        }
    } else if command[0].eq_ignore_ascii_case(b"ECHO") {
        if command.len() != 2 {
            protocol::error("wrong number of arguments for 'echo' command")
        } else {
            protocol::bulk(&command[1])
        }
    } else if command[0].eq_ignore_ascii_case(b"TIME") {
        if command.len() != 1 {
            protocol::error("wrong number of arguments for 'time' command")
        } else {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            protocol::array(&[
                now.as_secs().to_string().into_bytes(),
                now.subsec_micros().to_string().into_bytes(),
            ])
        }
    } else if command[0].eq_ignore_ascii_case(b"FLUSHDB")
        || command[0].eq_ignore_ascii_case(b"FLUSHALL")
    {
        if command.len() > 2 {
            protocol::error(&format!(
                "wrong number of arguments for '{}' command",
                String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
            ))
        } else if command.len() == 2
            && !command[1].eq_ignore_ascii_case(b"SYNC")
            && !command[1].eq_ignore_ascii_case(b"ASYNC")
        {
            protocol::error("syntax error")
        } else {
            let mut entries = store.entries.lock().unwrap();
            let keys: Vec<_> = entries.keys().cloned().collect();
            entries.clear();
            drop(entries);
            for key in keys {
                store.eviction.forget(&key);
                mark_evicted(store, &key);
            }
            for registration in store.watched_keys.lock().unwrap().values_mut() {
                registration.revision = registration.revision.wrapping_add(1);
            }
            b"+OK\r\n".to_vec()
        }
    } else if command[0].eq_ignore_ascii_case(b"GET") {
        if command.len() != 2 {
            protocol::error("wrong number of arguments for 'get' command")
        } else {
            let mut store = store.entries.lock().unwrap();
            remove_expired(&mut store, &command[1]);
            match store.get(&command[1]).map(|entry| &entry.value) {
                Some(Value::String(value)) => protocol::bulk(value),
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => b"$-1\r\n".to_vec(),
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"INCR") {
        if command.len() != 2 {
            protocol::error("wrong number of arguments for 'incr' command")
        } else {
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get_mut(&command[1]) {
                Some(Entry {
                    value: Value::String(value),
                    ..
                }) => match std::str::from_utf8(value)
                    .ok()
                    .and_then(|text| text.parse::<i64>().ok())
                    .and_then(|number| number.checked_add(1))
                {
                    Some(number) => {
                        *value = number.to_string().into_bytes();
                        format!(":{number}\r\n").into_bytes()
                    }
                    None => protocol::error("value is not an integer or out of range"),
                },
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => {
                    entries.insert(
                        command[1].clone(),
                        Entry {
                            value: Value::String(b"1".to_vec()),
                            expires_at: None,
                        },
                    );
                    b":1\r\n".to_vec()
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"RPUSH") {
        if command.len() < 3 {
            protocol::error("wrong number of arguments for 'rpush' command")
        } else {
            let changed = &store.changed;
            let mut store = store.entries.lock().unwrap();
            remove_expired(&mut store, &command[1]);
            match store.get_mut(&command[1]) {
                Some(Entry {
                    value: Value::List(items),
                    ..
                }) => {
                    items.extend_from_slice(&command[2..]);
                    changed.notify_all();
                    format!(":{}\r\n", items.len()).into_bytes()
                }
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => {
                    store.insert(
                        command[1].clone(),
                        Entry {
                            value: Value::List(command[2..].to_vec()),
                            expires_at: None,
                        },
                    );
                    changed.notify_all();
                    format!(":{}\r\n", command.len() - 2).into_bytes()
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"LRANGE") {
        if command.len() != 4 {
            protocol::error("wrong number of arguments for 'lrange' command")
        } else {
            let indexes = [command[2].as_slice(), command[3].as_slice()].map(|bytes| {
                std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
            });
            match indexes {
                [Some(start), Some(stop)] => {
                    let mut store = store.entries.lock().unwrap();
                    remove_expired(&mut store, &command[1]);
                    match store.get(&command[1]).map(|entry| &entry.value) {
                            Some(Value::List(items)) => list_range(items, start, stop),
                            Some(_) => b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec(),
                            None => protocol::array(&[]),
                        }
                }
                _ => protocol::error("value is not an integer or out of range"),
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"LPUSH") {
        if command.len() < 3 {
            protocol::error("wrong number of arguments for 'lpush' command")
        } else {
            let changed = &store.changed;
            let mut store = store.entries.lock().unwrap();
            remove_expired(&mut store, &command[1]);
            match store.get_mut(&command[1]) {
                Some(Entry {
                    value: Value::List(items),
                    ..
                }) => {
                    items.splice(0..0, command[2..].iter().rev().cloned());
                    changed.notify_all();
                    format!(":{}\r\n", items.len()).into_bytes()
                }
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => {
                    store.insert(
                        command[1].clone(),
                        Entry {
                            value: Value::List(command[2..].iter().rev().cloned().collect()),
                            expires_at: None,
                        },
                    );
                    changed.notify_all();
                    format!(":{}\r\n", command.len() - 2).into_bytes()
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"LLEN") {
        if command.len() != 2 {
            protocol::error("wrong number of arguments for 'llen' command")
        } else {
            let mut store = store.entries.lock().unwrap();
            remove_expired(&mut store, &command[1]);
            match store.get(&command[1]).map(|entry| &entry.value) {
                Some(Value::List(items)) => format!(":{}\r\n", items.len()).into_bytes(),
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => b":0\r\n".to_vec(),
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"LPOP") {
        if command.len() != 2 && command.len() != 3 {
            protocol::error("wrong number of arguments for 'lpop' command")
        } else {
            let count = if command.len() == 3 {
                std::str::from_utf8(&command[2])
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
            } else {
                Some(1)
            };
            match count {
                None => protocol::error("value is not an integer or out of range"),
                Some(count) => {
                    let mut store = store.entries.lock().unwrap();
                    remove_expired(&mut store, &command[1]);
                    match store.get_mut(&command[1]) {
                            Some(Entry { value: Value::List(items), .. }) => {
                                if count == 0 {
                                    b"*0\r\n".to_vec()
                                } else {
                                    let values: Vec<_> = items.drain(..count.min(items.len())).collect();
                                    let empty = items.is_empty();
                                    if empty {
                                        store.remove(&command[1]);
                                    }
                                    if command.len() == 2 {
                                        protocol::bulk(&values[0])
                                    } else {
                                        protocol::array(&values)
                                    }
                                }
                            }
                            Some(_) => b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec(),
                            None if command.len() == 3 => b"*-1\r\n".to_vec(),
                            None => b"$-1\r\n".to_vec(),
                        }
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"BLPOP") || command[0].eq_ignore_ascii_case(b"BRPOP")
    {
        if command.len() < 3 {
            protocol::error(&format!(
                "wrong number of arguments for '{}' command",
                String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
            ))
        } else {
            match blpop_deadline(command.last().unwrap()) {
                Err(error) => error,
                Ok(deadline) => {
                    let keys = &command[1..command.len() - 1];
                    loop {
                        if connection_closed(probe) {
                            break b"*-1\r\n".to_vec();
                        }
                        let gate = allow_blocking.then(|| store.command_gate.lock().unwrap());
                        let mut entries = store.entries.lock().unwrap();
                        if let Some(response) = try_blocking_pop(
                            &mut entries,
                            keys,
                            store,
                            command[0].eq_ignore_ascii_case(b"BRPOP"),
                        ) {
                            break response;
                        }
                        if !allow_blocking {
                            break b"*-1\r\n".to_vec();
                        }
                        drop(gate);
                        entries = match wait_for_change(store, entries, deadline, probe) {
                            Some(entries) => entries,
                            None => break b"*-1\r\n".to_vec(),
                        };
                        drop(entries);
                    }
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"BLMOVE")
        || command[0].eq_ignore_ascii_case(b"BRPOPLPUSH")
    {
        blocking_move(command, store, allow_blocking, probe)
    } else if command[0].eq_ignore_ascii_case(b"LMPOP")
        || command[0].eq_ignore_ascii_case(b"BLMPOP")
    {
        let blocking = command[0].eq_ignore_ascii_case(b"BLMPOP");
        match parse_multi_pop(command, blocking) {
            Err(error) => error,
            Ok(options) => loop {
                if connection_closed(probe) {
                    break b"*-1\r\n".to_vec();
                }
                let gate = (blocking && allow_blocking).then(|| store.command_gate.lock().unwrap());
                let mut entries = store.entries.lock().unwrap();
                if let Some(response) = try_multi_pop(&mut entries, &options, store) {
                    break response;
                }
                if !blocking || !allow_blocking {
                    break b"*-1\r\n".to_vec();
                }
                drop(gate);
                entries = match wait_for_change(store, entries, options.deadline, probe) {
                    Some(entries) => entries,
                    None => break b"*-1\r\n".to_vec(),
                };
                drop(entries);
            },
        }
    } else if command[0].eq_ignore_ascii_case(b"ZMPOP")
        || command[0].eq_ignore_ascii_case(b"BZMPOP")
        || command[0].eq_ignore_ascii_case(b"BZPOPMIN")
        || command[0].eq_ignore_ascii_case(b"BZPOPMAX")
    {
        zpop_command(command, store, allow_blocking, probe)
    } else if command[0].eq_ignore_ascii_case(b"TYPE") {
        if command.len() != 2 {
            protocol::error("wrong number of arguments for 'type' command")
        } else {
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get(&command[1]).map(|entry| &entry.value) {
                Some(Value::String(_)) => b"+string\r\n".to_vec(),
                Some(Value::List(_)) => b"+list\r\n".to_vec(),
                Some(Value::Stream(_)) => b"+stream\r\n".to_vec(),
                Some(Value::Hash(_)) => b"+hash\r\n".to_vec(),
                Some(Value::Set(_)) => b"+set\r\n".to_vec(),
                Some(Value::SortedSet(_)) => b"+zset\r\n".to_vec(),
                None => b"+none\r\n".to_vec(),
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"XADD") {
        streams::add_command(command, store)
    } else if command[0].eq_ignore_ascii_case(b"XRANGE")
        || command[0].eq_ignore_ascii_case(b"XREVRANGE")
    {
        if command.len() != 4 && (command.len() != 6 || !command[4].eq_ignore_ascii_case(b"COUNT"))
        {
            protocol::error(&format!(
                "wrong number of arguments for '{}' command",
                String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
            ))
        } else {
            let count = if command.len() == 6 {
                std::str::from_utf8(&command[5])
                    .ok()
                    .and_then(|s| s.parse::<usize>().ok())
            } else {
                Some(usize::MAX)
            };
            match count {
                None => protocol::error("value is not an integer or out of range"),
                Some(count) => {
                    let mut entries = store.entries.lock().unwrap();
                    remove_expired(&mut entries, &command[1]);
                    match entries.get(&command[1]).map(|entry| &entry.value) {
                            Some(Value::Stream(stream)) => match if command[0].eq_ignore_ascii_case(b"XREVRANGE") {
                                stream.range_reverse(&command[2], &command[3], Some(count))
                            } else {
                                stream.range(&command[2], &command[3], Some(count))
                            } {
                                Ok(found) => streams::stream_entries(&found),
                                Err(message) => protocol::error(message),
                            },
                            Some(_) => b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec(),
                            None => protocol::array(&[]),
                        }
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"XDEL") {
        if command.len() < 3 {
            protocol::error("wrong number of arguments for 'xdel' command")
        } else {
            let mut ids = HashSet::new();
            for raw in &command[2..] {
                match streams::StreamId::parse_query(raw) {
                    Ok(id) => {
                        ids.insert(id);
                    }
                    Err(message) => return protocol::error(message),
                }
            }
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get_mut(&command[1]) {
                Some(Entry {
                    value: Value::Stream(stream),
                    ..
                }) => {
                    let removed = stream.delete(&ids);
                    if removed != 0 {
                        mark_modified(store, &command[1]);
                    }
                    format!(":{removed}\r\n").into_bytes()
                }
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => b":0\r\n".to_vec(),
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"XTRIM") {
        if !(command.len() == 4 || (command.len() == 5 && command[3] == b"=")) {
            protocol::error("syntax error")
        } else {
            let threshold = &command[command.len() - 1];
            let maxlen = if command[2].eq_ignore_ascii_case(b"MAXLEN") {
                match std::str::from_utf8(threshold)
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                {
                    Some(value) => Some(value),
                    None => return protocol::error("value is not an integer or out of range"),
                }
            } else {
                None
            };
            let minid = if command[2].eq_ignore_ascii_case(b"MINID") {
                match streams::StreamId::parse_query(threshold) {
                    Ok(value) => Some(value),
                    Err(message) => return protocol::error(message),
                }
            } else {
                None
            };
            if maxlen.is_none() && minid.is_none() {
                return protocol::error("syntax error");
            }
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get_mut(&command[1]) {
                Some(Entry {
                    value: Value::Stream(stream),
                    ..
                }) => {
                    let removed = if let Some(maxlen) = maxlen {
                        stream.trim_maxlen(maxlen)
                    } else {
                        stream.trim_minid(minid.unwrap())
                    };
                    if removed != 0 {
                        mark_modified(store, &command[1]);
                    }
                    format!(":{removed}\r\n").into_bytes()
                }
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => b":0\r\n".to_vec(),
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"XREAD") {
        match parse_xread(command) {
            Err(error) => error,
            Ok(options) => {
                let mut gate = allow_blocking.then(|| store.command_gate.lock().unwrap());
                let mut entries = store.entries.lock().unwrap();
                match xread_ids(&mut entries, options.keys, options.ids) {
                    Err(error) => error,
                    Ok(ids) => {
                        let deadline = match options.block {
                            Some(duration) if !duration.is_zero() => {
                                Instant::now().checked_add(duration)
                            }
                            _ => None,
                        };
                        loop {
                            if connection_closed(probe) {
                                break b"*-1\r\n".to_vec();
                            }
                            match try_xread(&mut entries, options.keys, &ids, options.count) {
                                Err(error) => break error,
                                Ok(Some(response)) => break response,
                                Ok(None) => {}
                            }
                            if !allow_blocking {
                                break b"*-1\r\n".to_vec();
                            }
                            if options.block.is_some() {
                                drop(gate.take());
                            }
                            match options.block {
                                None => break b"*-1\r\n".to_vec(),
                                Some(duration) if duration.is_zero() => {
                                    entries = match wait_for_change(store, entries, None, probe) {
                                        Some(entries) => entries,
                                        None => break b"*-1\r\n".to_vec(),
                                    }
                                }
                                Some(_) => {
                                    entries = match wait_for_change(store, entries, deadline, probe)
                                    {
                                        Some(entries) => entries,
                                        None => break b"*-1\r\n".to_vec(),
                                    };
                                }
                            }
                            drop(entries);
                            gate = allow_blocking.then(|| store.command_gate.lock().unwrap());
                            entries = store.entries.lock().unwrap();
                        }
                    }
                }
            }
        }
    } else if command[0].eq_ignore_ascii_case(b"XLEN") {
        if command.len() != 2 {
            protocol::error("wrong number of arguments for 'xlen' command")
        } else {
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get(&command[1]).map(|entry| &entry.value) {
                Some(Value::Stream(stream)) => {
                    format!(":{}\r\n", stream.entries.len()).into_bytes()
                }
                Some(_) => {
                    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n"
                        .to_vec()
                }
                None => b":0\r\n".to_vec(),
            }
        }
    } else {
        protocol::error("unknown command")
    }
}

fn validate_queued(command: &[Vec<u8>]) -> Result<(), Vec<u8>> {
    let name = &command[0];
    let arity = command.len();
    let valid = if let Some(valid) = connection::valid_arity(command) {
        valid
    } else if let Some(valid) = stream_groups::valid_arity(command) {
        valid
    } else if let Some(valid) = auth::valid_arity(command) {
        valid
    } else if let Some(valid) = command_table::valid_arity(command) {
        valid
    } else if let Some(valid) = eviction::valid_arity(command) {
        valid
    } else if let Some(valid) = pubsub::valid_arity(command) {
        valid
    } else if let Some(valid) = cache::valid_arity(command) {
        valid
    } else if name.eq_ignore_ascii_case(b"SAVE")
        || name.eq_ignore_ascii_case(b"BGSAVE")
        || name.eq_ignore_ascii_case(b"BGREWRITEAOF")
    {
        return Err(protocol::error("Command not allowed inside a transaction"));
    } else if let Some(valid) = rdb::valid_arity(command) {
        valid
    } else if let Some(valid) = geo::valid_arity(command) {
        valid
    } else if let Some(valid) = list_ops::valid_arity(command) {
        valid
    } else if let Some(valid) = hyperloglog::valid_arity(command) {
        valid
    } else if let Some(valid) = bitmaps::valid_arity(command) {
        valid
    } else if let Some(valid) = sorted_sets::valid_arity(command) {
        valid
    } else if let Some(valid) = collections::valid_arity(command) {
        valid
    } else if let Some(valid) = keyspace::valid_arity(command) {
        valid
    } else if name.eq_ignore_ascii_case(b"INFO") || name.eq_ignore_ascii_case(b"PING") {
        arity <= 2
    } else if name.eq_ignore_ascii_case(b"UNWATCH") || name.eq_ignore_ascii_case(b"TIME") {
        arity == 1
    } else if name.eq_ignore_ascii_case(b"FLUSHDB") || name.eq_ignore_ascii_case(b"FLUSHALL") {
        arity == 1 || arity == 2
    } else if name.eq_ignore_ascii_case(b"ECHO")
        || name.eq_ignore_ascii_case(b"GET")
        || name.eq_ignore_ascii_case(b"INCR")
        || name.eq_ignore_ascii_case(b"LLEN")
        || name.eq_ignore_ascii_case(b"TYPE")
        || name.eq_ignore_ascii_case(b"XLEN")
    {
        arity == 2
    } else if name.eq_ignore_ascii_case(b"RPUSH")
        || name.eq_ignore_ascii_case(b"LPUSH")
        || name.eq_ignore_ascii_case(b"BLPOP")
        || name.eq_ignore_ascii_case(b"BRPOP")
    {
        arity >= 3
    } else if name.eq_ignore_ascii_case(b"LRANGE") {
        arity == 4
    } else if name.eq_ignore_ascii_case(b"LPOP") {
        arity == 2 || arity == 3
    } else if name.eq_ignore_ascii_case(b"BLMOVE") {
        arity == 6
    } else if name.eq_ignore_ascii_case(b"BRPOPLPUSH") {
        arity == 4
    } else if name.eq_ignore_ascii_case(b"LMPOP") {
        arity >= 4
    } else if name.eq_ignore_ascii_case(b"BLMPOP") {
        arity >= 5
    } else if name.eq_ignore_ascii_case(b"ZMPOP") {
        arity >= 4
    } else if name.eq_ignore_ascii_case(b"BZMPOP") {
        arity >= 5
    } else if name.eq_ignore_ascii_case(b"BZPOPMIN") || name.eq_ignore_ascii_case(b"BZPOPMAX") {
        arity >= 3
    } else if name.eq_ignore_ascii_case(b"XADD") {
        arity >= 5
    } else if name.eq_ignore_ascii_case(b"XDEL") {
        arity >= 3
    } else if name.eq_ignore_ascii_case(b"XTRIM") {
        arity == 4 || arity == 5
    } else if name.eq_ignore_ascii_case(b"XRANGE") || name.eq_ignore_ascii_case(b"XREVRANGE") {
        arity == 4 || (arity == 6 && command[4].eq_ignore_ascii_case(b"COUNT"))
    } else if name.eq_ignore_ascii_case(b"XREAD") {
        return parse_xread(command).map(|_| ());
    } else {
        return Err(protocol::error("unknown command"));
    };
    if valid {
        Ok(())
    } else {
        let lower = String::from_utf8_lossy(name).to_ascii_lowercase();
        Err(protocol::error(&format!(
            "wrong number of arguments for '{lower}' command"
        )))
    }
}

fn handle_client(mut stream: TcpStream, store: Store) -> io::Result<()> {
    let mut session = connection::Session::new(&stream)?;
    let mut reader = BufReader::new(stream.try_clone()?);
    stream.set_write_timeout(Some(Duration::from_millis(500)))?;
    let subscription = store
        .pubsub
        .connect(Arc::new(Mutex::new(stream.try_clone()?)));
    let mut transaction: Option<Vec<Vec<Vec<u8>>>> = None;
    let mut transaction_error = false;
    let mut authenticated = !store.auth.required();
    let mut watches = ClientWatches::new(Arc::clone(&store));
    while let Some(command) = protocol::read_command(&mut reader)? {
        if !authenticated
            && !command[0].eq_ignore_ascii_case(b"AUTH")
            && !command[0].eq_ignore_ascii_case(b"QUIT")
            && !command[0].eq_ignore_ascii_case(b"HELLO")
            && !command[0].eq_ignore_ascii_case(b"RESET")
        {
            subscription.write(b"-NOAUTH Authentication required.\r\n")?;
            continue;
        }
        if command[0].eq_ignore_ascii_case(b"RESET") {
            if command.len() != 1 {
                transaction_error |= transaction.is_some();
                subscription.write(&protocol::error(
                    "wrong number of arguments for 'reset' command",
                ))?;
                continue;
            }
            let _gate = store.command_gate.lock().unwrap();
            transaction = None;
            transaction_error = false;
            watches.clear();
            subscription.reset();
            session.reset();
            authenticated = !store.auth.required();
            subscription.write(b"+RESET\r\n")?;
            continue;
        }
        if command[0].eq_ignore_ascii_case(b"REPLSYNC") && !store.read_only {
            if command.len() != 1 {
                stream.write_all(&protocol::error(
                    "wrong number of arguments for 'replsync' command",
                ))?;
                continue;
            }
            return serve_replica(stream, &store);
        }
        if command[0].eq_ignore_ascii_case(b"QUIT") {
            if command.len() != 1 {
                subscription.write(&protocol::error(
                    "wrong number of arguments for 'quit' command",
                ))?;
                continue;
            }
            subscription.write(b"+OK\r\n")?;
            return Ok(());
        }
        if pubsub::is_subscription_command(&command) {
            if transaction.is_some() {
                transaction_error = true;
                subscription.write(&protocol::error(
                    "Pub/Sub command inside MULTI is not allowed",
                ))?;
            } else {
                let _gate = store.command_gate.lock().unwrap();
                subscription.change(&command)?;
            }
            continue;
        }
        if subscription.active() {
            if command[0].eq_ignore_ascii_case(b"PING") {
                let reply = if command.len() > 2 {
                    protocol::error("wrong number of arguments for 'ping' command")
                } else {
                    let payload = command.get(1).map_or(b"".as_slice(), Vec::as_slice);
                    let mut reply = b"*2\r\n".to_vec();
                    reply.extend(protocol::bulk(b"pong"));
                    reply.extend(protocol::bulk(payload));
                    reply
                };
                let _gate = store.command_gate.lock().unwrap();
                subscription.write(&reply)?;
            } else {
                subscription.write(&protocol::error(
                    "Can't execute command while subscribed; only (P|S)SUBSCRIBE, (P|S)UNSUBSCRIBE, PING, RESET and QUIT are allowed",
                ))?;
            }
            continue;
        }
        let response = if command[0].eq_ignore_ascii_case(b"AUTH") && transaction.is_none() {
            store.auth.authenticate(&command, &mut authenticated)
        } else if store.read_only && is_mutating(&command) {
            if transaction.is_some() {
                transaction_error = true;
            }
            b"-READONLY You can't write against a read only replica.\r\n".to_vec()
        } else if command[0].eq_ignore_ascii_case(b"WATCH") {
            if command.len() < 2 {
                protocol::error("wrong number of arguments for 'watch' command")
            } else if transaction.is_some() {
                protocol::error("WATCH inside MULTI is not allowed")
            } else {
                let _gate = store.command_gate.lock().unwrap();
                watches.watch(&command[1..]);
                b"+OK\r\n".to_vec()
            }
        } else if command[0].eq_ignore_ascii_case(b"UNWATCH") && transaction.is_none() {
            if command.len() != 1 {
                protocol::error("wrong number of arguments for 'unwatch' command")
            } else {
                watches.clear();
                b"+OK\r\n".to_vec()
            }
        } else if command[0].eq_ignore_ascii_case(b"MULTI") {
            if command.len() != 1 {
                protocol::error("wrong number of arguments for 'multi' command")
            } else if transaction.is_some() {
                protocol::error("MULTI calls can not be nested")
            } else {
                transaction = Some(Vec::new());
                transaction_error = false;
                b"+OK\r\n".to_vec()
            }
        } else if command[0].eq_ignore_ascii_case(b"DISCARD") {
            if command.len() != 1 {
                protocol::error("wrong number of arguments for 'discard' command")
            } else if transaction.take().is_some() {
                transaction_error = false;
                watches.clear();
                b"+OK\r\n".to_vec()
            } else {
                protocol::error("DISCARD without MULTI")
            }
        } else if command[0].eq_ignore_ascii_case(b"EXEC") {
            if command.len() != 1 {
                protocol::error("wrong number of arguments for 'exec' command")
            } else if let Some(queued) = transaction.take() {
                if transaction_error {
                    transaction_error = false;
                    watches.clear();
                    b"-EXECABORT Transaction discarded because of previous errors.\r\n".to_vec()
                } else {
                    let _gate = store.command_gate.lock().unwrap();
                    let changed = watches.changed();
                    watches.clear();
                    if changed {
                        b"*-1\r\n".to_vec()
                    } else {
                        let mut response = format!("*{}\r\n", queued.len()).into_bytes();
                        let writes = queued.iter().any(|command| {
                            is_mutating(command) || hyperloglog::may_update_cache(command)
                        });
                        for queued_command in queued {
                            let result = if queued_command[0].eq_ignore_ascii_case(b"AUTH") {
                                store.auth.authenticate(&queued_command, &mut authenticated)
                            } else if let Some(response) =
                                session.execute(&queued_command, &store, &mut authenticated)
                            {
                                response
                            } else {
                                execute_checked(&queued_command, &store, false)
                            };
                            response.extend(result);
                        }
                        if writes {
                            save_and_publish(&store)?;
                        }
                        response
                    }
                }
            } else {
                protocol::error("EXEC without MULTI")
            }
        } else if let Some(queued) = transaction.as_mut() {
            match validate_queued(&command) {
                Ok(()) => {
                    queued.push(command);
                    b"+QUEUED\r\n".to_vec()
                }
                Err(error) => {
                    transaction_error = true;
                    error
                }
            }
        } else if command[0].eq_ignore_ascii_case(b"XREADGROUP") {
            let probe = clients::DisconnectProbe::new(&stream)?;
            stream_groups::blocking_read(&command, &store, &probe)?
        } else if command[0].eq_ignore_ascii_case(b"BLPOP")
            || command[0].eq_ignore_ascii_case(b"BRPOP")
            || command[0].eq_ignore_ascii_case(b"BLMOVE")
            || command[0].eq_ignore_ascii_case(b"BRPOPLPUSH")
            || command[0].eq_ignore_ascii_case(b"BLMPOP")
            || command[0].eq_ignore_ascii_case(b"BZMPOP")
            || command[0].eq_ignore_ascii_case(b"BZPOPMIN")
            || command[0].eq_ignore_ascii_case(b"BZPOPMAX")
            || command[0].eq_ignore_ascii_case(b"XREAD")
        {
            let probe = clients::DisconnectProbe::new(&stream)?;
            let response = execute_command(&command, &store, true, Some(&probe));
            record_write(&store, &command, &response);
            if is_mutating(&command) || hyperloglog::may_update_cache(&command) {
                let _gate = store.command_gate.lock().unwrap();
                save_and_publish(&store)?;
            }
            response
        } else {
            let _gate = store.command_gate.lock().unwrap();
            let response = session
                .execute(&command, &store, &mut authenticated)
                .unwrap_or_else(|| execute_checked(&command, &store, true));
            if is_mutating(&command) || hyperloglog::may_update_cache(&command) {
                save_and_publish(&store)?;
            }
            response
        };
        subscription.write(&response)?;
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    const USAGE: &str = "usage: reru [--port PORT] [--bind ADDRESS] [--snapshot FILE] [--appendonly FILE] [--dir DIR] [--dbfilename FILE] [--replicaof HOST PORT] [--requirepass PASSWORD | --requirepass-file FILE] [--masterauth PASSWORD | --masterauth-file FILE] [--maxmemory SIZE] [--maxmemory-policy POLICY] [--maxclients COUNT]";
    if args.len() == 2 && matches!(args[1].as_str(), "--version" | "-V") {
        println!("ReRu {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.len() == 2 && matches!(args[1].as_str(), "--help" | "-h") {
        println!("ReRu {}\n{USAGE}\nExperimental Redis-compatible server. See docs/COMPATIBILITY.md for supported behavior.", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let mut port = 6379;
    let mut bind_address = String::from("127.0.0.1");
    let mut snapshot_path = None;
    let mut aof_path = None;
    let mut rdb_dir = None;
    let mut rdb_filename = None;
    let mut replica_of = None;
    let mut requirepass = None;
    let mut masterauth = None;
    let mut maxmemory = 0;
    let mut maxclients = clients::DEFAULT_MAX_CLIENTS;
    let mut maxmemory_policy = eviction::Policy::NoEviction;
    let mut index = 1;
    while index < args.len() {
        let value = args
            .get(index + 1)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, USAGE))?;
        match args[index].as_str() {
            "--requirepass" => requirepass = Some(value.as_bytes().to_vec()),
            "--masterauth" => masterauth = Some(value.as_bytes().to_vec()),
            "--requirepass-file" => {
                requirepass = Some(auth::read_password_file(std::path::Path::new(value))?)
            }
            "--masterauth-file" => {
                masterauth = Some(auth::read_password_file(std::path::Path::new(value))?)
            }
            "--port" => {
                port = value.parse::<u16>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidInput, "--port must be a valid port")
                })?
            }
            "--bind" => {
                if value.parse::<std::net::IpAddr>().is_err() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--bind must be an IP address",
                    ));
                }
                bind_address = value.to_string();
            }
            "--snapshot" => snapshot_path = Some(PathBuf::from(value)),
            "--maxclients" => {
                maxclients = value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "--maxclients must be a positive integer",
                        )
                    })?;
            }
            "--appendonly" => aof_path = Some(PathBuf::from(value)),
            "--dir" => rdb_dir = Some(PathBuf::from(value)),
            "--dbfilename" => {
                let path = PathBuf::from(value);
                if path.components().count() != 1 || path.file_name().is_none() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "--dbfilename must be a filename",
                    ));
                }
                rdb_filename = Some(path);
            }
            "--maxmemory" => {
                maxmemory = eviction::parse_size(value).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid --maxmemory size")
                })?
            }
            "--maxmemory-policy" => {
                maxmemory_policy = eviction::Policy::parse(value).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "invalid --maxmemory-policy")
                })?
            }
            "--replicaof" => {
                let upstream_port = args
                    .get(index + 2)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "missing replica port")
                    })?
                    .parse::<u16>()
                    .map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "invalid replica port")
                    })?;
                replica_of = Some(format!("{value}:{upstream_port}"));
                index += 1;
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, USAGE)),
        }
        index += 2;
    }

    let rdb_path = if rdb_dir.is_some() || rdb_filename.is_some() {
        Some(
            rdb_dir
                .unwrap_or_else(|| PathBuf::from("."))
                .join(rdb_filename.unwrap_or_else(|| PathBuf::from("dump.rdb"))),
        )
    } else {
        None
    };
    if masterauth.is_some() && replica_of.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--masterauth requires --replicaof",
        ));
    }
    if rdb_path.is_some() && (snapshot_path.is_some() || aof_path.is_some()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RDB mode cannot be combined with --snapshot or --appendonly",
        ));
    }
    if aof_path.is_some() && (snapshot_path.is_some() || replica_of.is_some()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "--appendonly cannot be combined with --snapshot or --replicaof",
        ));
    }

    let listener = TcpListener::bind((bind_address.as_str(), port))?;
    let bound = listener.local_addr()?;
    let entries = if let Some(path) = &snapshot_path {
        persistence::load(path)?
    } else if let Some(path) = &rdb_path {
        rdb::load(path)?
    } else {
        HashMap::new()
    };
    let store: Store = Arc::new(SharedStore {
        entries: Mutex::new(entries),
        changed: Condvar::new(),
        command_gate: Mutex::new(()),
        snapshot_path,
        aof_path,
        rdb: rdb::Persistence::new(rdb_path),
        replicas: Mutex::new(Vec::new()),
        read_only: replica_of.is_some(),
        watched_keys: Mutex::new(HashMap::new()),
        pending_keys: Mutex::new(HashSet::new()),
        staged_keys: Mutex::new(None),
        eviction: eviction::Eviction::new(maxmemory, maxmemory_policy),
        expiry: expiry::Expiry::default(),
        clients: Arc::new(clients::Limits::new(maxclients)),
        aof_rewrite: aof::Rewrite::default(),
        auth: auth::Settings::new(requirepass),
        pubsub: pubsub::Hub::default(),
    });
    if let Some(path) = &store.aof_path {
        aof::replay(path, &store)?;
    }
    if !store.read_only {
        store.eviction.evict(&store);
        save_and_publish(&store)?;
    }
    expiry::start(&store)?;
    if let Some(address) = replica_of {
        let replica_store = Arc::clone(&store);
        std::thread::spawn(move || {
            follow_primary(
                address,
                replica_store,
                masterauth.filter(|password| !password.is_empty()),
            )
        });
    }
    eprintln!("ReRu listening on {}:{}", bound.ip(), bound.port());
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            // A single aborted or resource-limited connection must not stop the
            // listener. Back off briefly so a persistent error cannot spin.
            Err(error) => {
                eprintln!("accept error: {error}");
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        let Some(permit) = store.clients.acquire() else {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let _ = stream.write_all(b"-ERR max number of clients reached\r\n");
            continue;
        };
        let store = Arc::clone(&store);
        if let Err(error) = std::thread::Builder::new()
            .name("client".into())
            .spawn(move || {
                let _permit = permit;
                if let Err(error) = handle_client(stream, store) {
                    eprintln!("client error: {error}");
                }
            })
        {
            eprintln!("could not start client thread: {error}");
        }
    }
    Ok(())
}
