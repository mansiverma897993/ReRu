// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_evicted, protocol, Entry, Store, Value};
use std::collections::{hash_map::RandomState, HashMap};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Policy {
    NoEviction,
    AllKeysLru,
    AllKeysLrm,
    AllKeysLfu,
    AllKeysRandom,
    VolatileLru,
    VolatileLrm,
    VolatileLfu,
    VolatileRandom,
    VolatileTtl,
}

impl Policy {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "noeviction" => Some(Self::NoEviction),
            "allkeys-lru" => Some(Self::AllKeysLru),
            "allkeys-lrm" => Some(Self::AllKeysLrm),
            "allkeys-lfu" => Some(Self::AllKeysLfu),
            "allkeys-random" => Some(Self::AllKeysRandom),
            "volatile-lru" => Some(Self::VolatileLru),
            "volatile-lrm" => Some(Self::VolatileLrm),
            "volatile-lfu" => Some(Self::VolatileLfu),
            "volatile-random" => Some(Self::VolatileRandom),
            "volatile-ttl" => Some(Self::VolatileTtl),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::NoEviction => "noeviction",
            Self::AllKeysLru => "allkeys-lru",
            Self::AllKeysLrm => "allkeys-lrm",
            Self::AllKeysLfu => "allkeys-lfu",
            Self::AllKeysRandom => "allkeys-random",
            Self::VolatileLru => "volatile-lru",
            Self::VolatileLrm => "volatile-lrm",
            Self::VolatileLfu => "volatile-lfu",
            Self::VolatileRandom => "volatile-random",
            Self::VolatileTtl => "volatile-ttl",
        }
    }

    pub fn is_allkeys(self) -> bool {
        matches!(
            self,
            Self::AllKeysLru | Self::AllKeysLrm | Self::AllKeysLfu | Self::AllKeysRandom
        )
    }

    fn is_volatile(self) -> bool {
        matches!(
            self,
            Self::VolatileLru
                | Self::VolatileLrm
                | Self::VolatileLfu
                | Self::VolatileRandom
                | Self::VolatileTtl
        )
    }
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub maxmemory: usize,
    pub policy: Policy,
}

pub fn parse_size(value: &str) -> Option<usize> {
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let count = value[..split].parse::<usize>().ok()?;
    let multiplier: usize = match value[split..].to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        _ => return None,
    };
    count.checked_mul(multiplier)
}

fn matches_pattern(pattern: &[u8], value: &[u8]) -> bool {
    let (mut pattern_at, mut value_at, mut star, mut retry) = (0, 0, None, 0);
    while value_at < value.len() {
        if pattern_at < pattern.len()
            && (pattern[pattern_at] == b'?'
                || pattern[pattern_at].eq_ignore_ascii_case(&value[value_at]))
        {
            pattern_at += 1;
            value_at += 1;
        } else if pattern_at < pattern.len() && pattern[pattern_at] == b'*' {
            star = Some(pattern_at);
            pattern_at += 1;
            retry = value_at;
        } else if let Some(position) = star {
            retry += 1;
            value_at = retry;
            pattern_at = position + 1;
        } else {
            return false;
        }
    }
    pattern[pattern_at..].iter().all(|byte| *byte == b'*')
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    if !command[0].eq_ignore_ascii_case(b"CONFIG") {
        return None;
    }
    Some(match command.get(1).map(Vec::as_slice) {
        Some(name) if name.eq_ignore_ascii_case(b"GET") => command.len() >= 3,
        Some(name) if name.eq_ignore_ascii_case(b"SET") => {
            command.len() >= 4 && command.len().is_multiple_of(2)
        }
        _ => false,
    })
}

pub fn info(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    if command.len() > 2 {
        return protocol::error("wrong number of arguments for 'info' command");
    }
    let section = command.get(1).map(Vec::as_slice);
    let memory = section.is_none_or(|name| name.eq_ignore_ascii_case(b"memory"));
    let stats = section.is_none_or(|name| name.eq_ignore_ascii_case(b"stats"));
    let mut report = String::new();
    if memory {
        let settings = store.eviction.settings();
        let entries = store.entries.lock().unwrap();
        let used = dataset_bytes(&entries);
        let retained_keys = entries.len();
        drop(entries);
        report.push_str(&format!(
            "# Memory\r\nused_memory:{used}\r\nmaxmemory:{}\r\nmaxmemory_policy:{}\r\nreru_estimated_dataset_bytes:{used}\r\n\r\n",
            settings.maxmemory,
            settings.policy.name()
        ));
        report.push_str(&format!(
            "reru_retained_keys:{retained_keys}\r\nreru_expiry_index_keys:{}\r\n\r\n",
            store.expiry.indexed_keys()
        ));
    }
    if stats {
        report.push_str(&format!(
            "# Stats\r\nevicted_keys:{}\r\n\r\n",
            store.eviction.evicted_keys.load(Ordering::Relaxed)
        ));
        report.push_str(&format!(
            "reru_active_expired_keys:{}\r\n\r\n",
            store.expiry.active_expired_keys()
        ));
        report.push_str(&store.clients.stats());
    }
    if section.is_none_or(|name| name.eq_ignore_ascii_case(b"clients")) {
        report.push_str(&store.clients.info());
    }
    if section.is_none_or(|name| name.eq_ignore_ascii_case(b"persistence")) {
        report.push_str(&crate::rdb::info(store));
        report.push_str(&crate::aof::info(store));
    }
    protocol::bulk(report.as_bytes())
}

pub fn execute_config(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    if valid_arity(command) != Some(true) {
        return protocol::error("wrong number of arguments for 'config' command");
    }
    let settings = store.eviction.settings();
    if command[1].eq_ignore_ascii_case(b"GET") {
        let mut output = Vec::new();
        for (name, value) in [
            (
                b"appendonly".as_slice(),
                if store.aof_path.is_some() {
                    "yes"
                } else {
                    "no"
                }
                .to_owned(),
            ),
            (b"appendfsync".as_slice(), "always".to_owned()),
            (b"maxmemory".as_slice(), settings.maxmemory.to_string()),
            (
                b"maxclients".as_slice(),
                store.clients.maximum().to_string(),
            ),
            (
                b"maxmemory-policy".as_slice(),
                settings.policy.name().to_string(),
            ),
        ] {
            if command[2..]
                .iter()
                .any(|pattern| matches_pattern(pattern, name))
            {
                output.push(name.to_vec());
                output.push(value.into_bytes());
            }
        }
        if let Some(path) = &store.rdb.path {
            for (name, value) in [
                (
                    b"dir".as_slice(),
                    path.parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .to_string_lossy(),
                ),
                (
                    b"dbfilename".as_slice(),
                    path.file_name().unwrap_or_default().to_string_lossy(),
                ),
            ] {
                if command[2..]
                    .iter()
                    .any(|pattern| matches_pattern(pattern, name))
                {
                    output.push(name.to_vec());
                    output.push(value.as_bytes().to_vec());
                }
            }
        }
        return protocol::array(&output);
    }
    let mut updated = settings;
    let mut maxclients = store.clients.maximum();
    for pair in command[2..].as_chunks::<2>().0 {
        let value = match std::str::from_utf8(&pair[1]) {
            Ok(value) => value,
            Err(_) => return protocol::error("Invalid CONFIG value"),
        };
        if pair[0].eq_ignore_ascii_case(b"maxmemory") {
            updated.maxmemory = match parse_size(value) {
                Some(value) => value,
                None => return protocol::error("Invalid maxmemory value"),
            };
        } else if pair[0].eq_ignore_ascii_case(b"maxmemory-policy") {
            updated.policy = match Policy::parse(value) {
                Some(value) => value,
                None => return protocol::error("Invalid maxmemory-policy value"),
            };
        } else if pair[0].eq_ignore_ascii_case(b"maxclients") {
            maxclients = match value.parse::<usize>().ok().filter(|n| *n > 0) {
                Some(value) => value,
                None => return protocol::error("Invalid maxclients value"),
            };
        } else {
            return protocol::error("Unsupported CONFIG parameter");
        }
    }
    store.eviction.update(updated, store);
    store.clients.set_maximum(maxclients);
    b"+OK\r\n".to_vec()
}

pub struct Eviction {
    settings: Mutex<Settings>,
    accesses: Mutex<HashMap<Vec<u8>, Access>>,
    clock: AtomicU64,
    pub evicted_keys: AtomicU64,
}

#[derive(Clone, Copy)]
struct Access {
    last_access: u64,
    last_write: u64,
    frequency: u32,
    last_decay: Instant,
}

impl Access {
    fn decayed_frequency(self, now: Instant) -> u32 {
        let minutes = now.saturating_duration_since(self.last_decay).as_secs() / 60;
        self.frequency >> minutes.min(31)
    }
}

impl Eviction {
    pub fn new(maxmemory: usize, policy: Policy) -> Self {
        Self {
            settings: Mutex::new(Settings { maxmemory, policy }),
            accesses: Mutex::new(HashMap::new()),
            clock: AtomicU64::new(0),
            evicted_keys: AtomicU64::new(0),
        }
    }

    pub fn settings(&self) -> Settings {
        *self.settings.lock().unwrap()
    }

    pub fn update(&self, settings: Settings, store: &Store) {
        *self.settings.lock().unwrap() = settings;
        self.evict(store);
    }

    pub fn note_modified(&self, key: &[u8]) {
        let settings = self.settings();
        if settings.maxmemory == 0 || settings.policy == Policy::NoEviction {
            return;
        }
        let tick = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
        let now = Instant::now();
        let mut accesses = self.accesses.lock().unwrap();
        let access = accesses.entry(key.to_vec()).or_insert(Access {
            last_access: tick,
            last_write: tick,
            frequency: 1,
            last_decay: now,
        });
        access.last_write = tick;
    }

    pub fn forget(&self, key: &[u8]) {
        self.accesses.lock().unwrap().remove(key);
    }

    pub fn touch(&self, command: &[Vec<u8>], store: &Store) {
        let settings = self.settings();
        if settings.maxmemory == 0 || settings.policy == Policy::NoEviction || command.len() < 2 {
            return;
        }
        let name = &command[0];
        let keys: Vec<&Vec<u8>> = if [
            b"MGET".as_slice(),
            b"DEL",
            b"UNLINK",
            b"EXISTS",
            b"TOUCH",
            b"SUNION",
            b"SINTER",
            b"SDIFF",
            b"SUNIONSTORE",
            b"SINTERSTORE",
            b"SDIFFSTORE",
        ]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
        {
            command[1..].iter().collect()
        } else if name.eq_ignore_ascii_case(b"MSET") || name.eq_ignore_ascii_case(b"MSETNX") {
            command[1..].iter().step_by(2).collect()
        } else if name.eq_ignore_ascii_case(b"SINTERCARD") {
            let count = std::str::from_utf8(&command[1])
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            command[2..command.len().min(2usize.saturating_add(count))]
                .iter()
                .collect()
        } else if name.eq_ignore_ascii_case(b"LMPOP")
            || name.eq_ignore_ascii_case(b"BLMPOP")
            || name.eq_ignore_ascii_case(b"ZMPOP")
            || name.eq_ignore_ascii_case(b"BZMPOP")
        {
            let count_at =
                if name.eq_ignore_ascii_case(b"BLMPOP") || name.eq_ignore_ascii_case(b"BZMPOP") {
                    2
                } else {
                    1
                };
            let count = command
                .get(count_at)
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let from = count_at + 1;
            command[from.min(command.len())..command.len().min(from.saturating_add(count))]
                .iter()
                .collect()
        } else if [
            b"ZUNION".as_slice(),
            b"ZINTER",
            b"ZDIFF",
            b"ZINTERCARD",
            b"ZUNIONSTORE",
            b"ZINTERSTORE",
            b"ZDIFFSTORE",
        ]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
        {
            let stored = name.to_ascii_uppercase().ends_with(b"STORE");
            let count_at = if stored { 2 } else { 1 };
            let count = command
                .get(count_at)
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(0);
            let from = count_at + 1;
            command[from.min(command.len())..command.len().min(from.saturating_add(count))]
                .iter()
                .collect()
        } else if name.eq_ignore_ascii_case(b"BITOP") {
            command[2..].iter().collect()
        } else if name.eq_ignore_ascii_case(b"RENAME")
            || name.eq_ignore_ascii_case(b"RENAMENX")
            || name.eq_ignore_ascii_case(b"COPY")
            || name.eq_ignore_ascii_case(b"ZRANGESTORE")
            || name.eq_ignore_ascii_case(b"GEOSEARCHSTORE")
            || name.eq_ignore_ascii_case(b"LMOVE")
            || name.eq_ignore_ascii_case(b"RPOPLPUSH")
            || name.eq_ignore_ascii_case(b"BLMOVE")
            || name.eq_ignore_ascii_case(b"BRPOPLPUSH")
            || name.eq_ignore_ascii_case(b"SMOVE")
        {
            command[1..command.len().min(3)].iter().collect()
        } else if name.eq_ignore_ascii_case(b"XREAD") {
            crate::parse_xread(command)
                .map(|options| options.keys.iter().collect())
                .unwrap_or_default()
        } else if name.eq_ignore_ascii_case(b"BLPOP")
            || name.eq_ignore_ascii_case(b"BRPOP")
            || name.eq_ignore_ascii_case(b"BZPOPMIN")
            || name.eq_ignore_ascii_case(b"BZPOPMAX")
        {
            command[1..command.len() - 1].iter().collect()
        } else if name.eq_ignore_ascii_case(b"PING")
            || name.eq_ignore_ascii_case(b"ECHO")
            || name.eq_ignore_ascii_case(b"TIME")
            || name.eq_ignore_ascii_case(b"SAVE")
            || name.eq_ignore_ascii_case(b"BGSAVE")
            || name.eq_ignore_ascii_case(b"LASTSAVE")
            || name.eq_ignore_ascii_case(b"FLUSHDB")
            || name.eq_ignore_ascii_case(b"FLUSHALL")
            || name.eq_ignore_ascii_case(b"CONFIG")
        {
            Vec::new()
        } else {
            vec![&command[1]]
        };
        let entries = store.entries.lock().unwrap();
        let mut accesses = self.accesses.lock().unwrap();
        let now = Instant::now();
        for key in keys {
            if entries
                .get(key)
                .is_some_and(|entry| entry.expires_at.is_none_or(|at| at > now))
            {
                let tick = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
                let access = accesses.entry(key.clone()).or_insert(Access {
                    last_access: tick,
                    last_write: 0,
                    frequency: 0,
                    last_decay: now,
                });
                access.frequency = access.decayed_frequency(now).saturating_add(1);
                access.last_decay = now;
                access.last_access = tick;
            }
        }
    }

    pub fn evict(&self, store: &Store) -> bool {
        let settings = self.settings();
        if settings.maxmemory == 0 || settings.policy == Policy::NoEviction {
            return true;
        }
        let mut entries = store.entries.lock().unwrap();
        let mut accesses = self.accesses.lock().unwrap();
        accesses.retain(|key, _| entries.contains_key(key));
        let mut used = dataset_bytes(&entries);
        if used <= settings.maxmemory {
            return true;
        }
        let now = Instant::now();
        let random = RandomState::new();
        let mut candidates: Vec<_> = entries
            .iter()
            .filter(|(_, entry)| {
                entry.expires_at.is_none_or(|at| at > now)
                    && (!settings.policy.is_volatile() || entry.expires_at.is_some())
            })
            .map(|(key, entry)| {
                let access = accesses.get(key).copied();
                let priority = match settings.policy {
                    Policy::AllKeysLru | Policy::VolatileLru => {
                        access.map_or(0, |access| access.last_access)
                    }
                    Policy::AllKeysLrm | Policy::VolatileLrm => {
                        access.map_or(0, |access| access.last_write)
                    }
                    Policy::AllKeysLfu | Policy::VolatileLfu => {
                        u64::from(access.map_or(0, |access| access.decayed_frequency(now)))
                    }
                    Policy::AllKeysRandom | Policy::VolatileRandom => random.hash_one(key),
                    Policy::VolatileTtl => entry
                        .expires_at
                        .unwrap()
                        .saturating_duration_since(now)
                        .as_millis()
                        .min(u128::from(u64::MAX))
                        as u64,
                    Policy::NoEviction => unreachable!(),
                };
                (key.clone(), entry_bytes(key, entry), priority)
            })
            .collect();
        candidates.sort_unstable_by(|left, right| {
            left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0))
        });
        let mut victims = Vec::new();
        for (key, bytes, _) in candidates {
            if used <= settings.maxmemory {
                break;
            }
            used = used.saturating_sub(bytes);
            victims.push(key);
        }
        if used > settings.maxmemory {
            return false;
        }
        for key in &victims {
            entries.remove(key);
            accesses.remove(key);
        }
        accesses.retain(|key, _| entries.contains_key(key));
        drop(accesses);
        drop(entries);
        self.evicted_keys
            .fetch_add(victims.len() as u64, Ordering::Relaxed);
        for key in victims {
            mark_evicted(store, &key);
        }
        true
    }
}

fn entry_bytes(key: &[u8], entry: &Entry) -> usize {
    let content = match &entry.value {
        Value::String(bytes) => bytes.len(),
        Value::List(items) => items.iter().map(|item| item.len() + 16).sum(),
        Value::Stream(stream) => {
            stream
                .entries
                .iter()
                .map(|item| {
                    16 + item
                        .fields
                        .iter()
                        .map(|field| field.len() + 16)
                        .sum::<usize>()
                })
                .sum::<usize>()
                + crate::stream_groups::metadata_bytes(stream)
        }
        Value::Hash(fields) => fields
            .iter()
            .map(|(field, value)| field.len() + value.len() + 32)
            .sum(),
        Value::Set(members) => members.iter().map(|member| member.len() + 16).sum(),
        Value::SortedSet(members) => members
            .entries()
            .map(|(member, _)| member.len() * 2 + 40)
            .sum(),
    };
    key.len().saturating_add(64).saturating_add(content)
}

pub fn dataset_bytes(entries: &HashMap<Vec<u8>, Entry>) -> usize {
    let now = Instant::now();
    entries
        .iter()
        .filter(|(_, entry)| entry.expires_at.is_none_or(|at| at > now))
        .fold(0usize, |sum, (key, entry)| {
            sum.saturating_add(entry_bytes(key, entry))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memory_sizes_without_overflow() {
        assert_eq!(parse_size("1kb"), Some(1024));
        assert_eq!(parse_size("2M"), Some(2 * 1024 * 1024));
        assert_eq!(parse_size("0"), Some(0));
        assert_eq!(parse_size("-1"), None);
        assert_eq!(parse_size("2tb"), None);
    }
}
