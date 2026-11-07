// SPDX-License-Identifier: AGPL-3.0-only
//! Redis RDB wire-format interoperability. Encodings are implemented from the
//! published format, with fixtures and cross-server tests as compatibility checks.
use crate::{mark_modified, protocol, remove_expired, sorted_sets::SortedSet, Entry, Store, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: u16 = 10;
const MAX_BYTES: usize = 256 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;
static SAVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub struct Persistence {
    pub path: Option<PathBuf>,
    last_save: AtomicU64,
    saving: AtomicBool,
    last_save_ok: AtomicBool,
}

impl Persistence {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            last_save: AtomicU64::new(epoch_millis() / 1000),
            saving: AtomicBool::new(false),
            last_save_ok: AtomicBool::new(true),
        }
    }
}

pub fn info(store: &Store) -> String {
    format!("# Persistence\r\nrdb_bgsave_in_progress:{}\r\nrdb_last_save_time:{}\r\nrdb_last_bgsave_status:{}\r\n\r\n",
        u8::from(store.rdb.saving.load(Ordering::Acquire)),
        store.rdb.last_save.load(Ordering::Relaxed),
        if store.rdb.last_save_ok.load(Ordering::Relaxed) { "ok" } else { "err" })
}

const fn crc_table() -> [u64; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u64;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1)
                ^ if value & 1 != 0 {
                    0x95ac_9329_ac4b_c9b5
                } else {
                    0
                };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

pub fn crc64(bytes: &[u8]) -> u64 {
    const TABLE: [u64; 256] = crc_table();
    bytes.iter().fold(0u64, |crc, byte| {
        (crc >> 8) ^ TABLE[((crc as u8) ^ byte) as usize]
    })
}

struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
    allocated: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            allocated: 0,
        }
    }
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| invalid("truncated RDB payload"))?;
        let bytes = &self.bytes[self.at..end];
        self.at = end;
        Ok(bytes)
    }
    fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn budget(&mut self, n: usize) -> io::Result<()> {
        self.allocated = self
            .allocated
            .checked_add(n)
            .filter(|n| *n <= MAX_BYTES)
            .ok_or_else(|| invalid("RDB decoded data exceeds limit"))?;
        Ok(())
    }
    fn length_from(&mut self, first: u8) -> io::Result<usize> {
        let n = match first >> 6 {
            0 => u64::from(first),
            1 => (u64::from(first & 63) << 8) | u64::from(self.byte()?),
            2 if first == 0x80 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            2 if first == 0x81 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            _ => return Err(invalid("invalid RDB length encoding")),
        };
        usize::try_from(n).map_err(|_| invalid("RDB length overflow"))
    }
    fn length(&mut self) -> io::Result<usize> {
        let first = self.byte()?;
        self.length_from(first)
    }
    fn count(&mut self) -> io::Result<usize> {
        let n = self.length()?;
        if n == 0 || n > MAX_ITEMS || n > self.bytes.len().saturating_sub(self.at) {
            return Err(invalid("invalid RDB collection size"));
        }
        self.budget(n.saturating_mul(32))?;
        Ok(n)
    }
    fn string(&mut self) -> io::Result<Vec<u8>> {
        let first = self.byte()?;
        if first >> 6 != 3 {
            let n = self.length_from(first)?;
            self.budget(n)?;
            return Ok(self.take(n)?.to_vec());
        }
        let value = match first & 63 {
            0 => i64::from(self.byte()? as i8),
            1 => i64::from(i16::from_le_bytes(self.take(2)?.try_into().unwrap())),
            2 => i64::from(i32::from_le_bytes(self.take(4)?.try_into().unwrap())),
            3 => {
                let compressed = self.length()?;
                let expanded = self.length()?;
                self.budget(expanded)?;
                return expand_lzf(self.take(compressed)?, expanded);
            }
            _ => return Err(invalid("unknown RDB string encoding")),
        };
        self.budget(20)?;
        Ok(value.to_string().into_bytes())
    }
    fn value(&mut self, kind: u8) -> io::Result<Value> {
        match kind {
            0 => Ok(Value::String(self.string()?)),
            1 | 2 | 4 | 5 => {
                let count = self.count()?;
                let mut items = Vec::new();
                for _ in 0..count {
                    items.push(self.string()?);
                    if kind == 4 {
                        items.push(self.string()?);
                    }
                    if kind == 5 {
                        let score = f64::from_le_bytes(self.take(8)?.try_into().unwrap());
                        if score.is_nan() {
                            return Err(invalid("NaN in RDB sorted set"));
                        }
                        items.push(crate::sorted_sets::score_text(score));
                    }
                }
                collection(kind, items)
            }
            11 => {
                let bytes = self.string()?;
                let mut packed = Decoder::new(&bytes);
                let width = u32::from_le_bytes(packed.take(4)?.try_into().unwrap()) as usize;
                let count = u32::from_le_bytes(packed.take(4)?.try_into().unwrap()) as usize;
                if ![2, 4, 8].contains(&width)
                    || count == 0
                    || count > MAX_ITEMS
                    || count.checked_mul(width) != Some(bytes.len().saturating_sub(8))
                {
                    return Err(invalid("invalid RDB intset"));
                }
                self.budget(count.saturating_mul(32))?;
                let mut items = Vec::new();
                let mut previous = None;
                for _ in 0..count {
                    let raw = packed.take(width)?;
                    let number = match width {
                        2 => i16::from_le_bytes(raw.try_into().unwrap()) as i64,
                        4 => i32::from_le_bytes(raw.try_into().unwrap()) as i64,
                        _ => i64::from_le_bytes(raw.try_into().unwrap()),
                    };
                    if previous.is_some_and(|old| old >= number) {
                        return Err(invalid("unordered RDB intset"));
                    }
                    previous = Some(number);
                    items.push(number.to_string().into_bytes());
                }
                collection(2, items)
            }
            16 | 17 | 20 => {
                let bytes = self.string()?;
                let items = listpack(&bytes)?;
                self.budget(items.len().saturating_mul(32))?;
                collection(
                    match kind {
                        16 => 4,
                        17 => 5,
                        _ => 2,
                    },
                    items,
                )
            }
            18 => {
                let count = self.count()?;
                let mut items = Vec::new();
                for _ in 0..count {
                    let container = self.length()?;
                    let bytes = self.string()?;
                    match container {
                        1 => items.push(bytes),
                        2 => items.extend(listpack(&bytes)?),
                        _ => return Err(invalid("invalid quicklist container")),
                    }
                    if items.len() > MAX_ITEMS {
                        return Err(invalid("RDB list exceeds limit"));
                    }
                }
                self.budget(items.len().saturating_mul(32))?;
                collection(1, items)
            }
            _ => Err(invalid(&format!("unsupported RDB value type {kind}"))),
        }
    }
}

fn collection(kind: u8, items: Vec<Vec<u8>>) -> io::Result<Value> {
    if items.is_empty() || items.len() > MAX_ITEMS * 2 {
        return Err(invalid("invalid empty or oversized RDB collection"));
    }
    match kind {
        1 => Ok(Value::List(items)),
        2 => {
            let count = items.len();
            let set: HashSet<_> = items.into_iter().collect();
            if set.len() != count {
                return Err(invalid("duplicate RDB set member"));
            }
            Ok(Value::Set(set))
        }
        4 | 5 => {
            if !items.len().is_multiple_of(2) {
                return Err(invalid("invalid RDB key/value pairs"));
            }
            let mut hash = HashMap::new();
            let mut sorted = SortedSet::default();
            let mut items = items.into_iter();
            while let Some(key) = items.next() {
                let value = items.next().unwrap();
                let duplicate = if kind == 4 {
                    hash.insert(key, value).is_some()
                } else {
                    let score = std::str::from_utf8(&value)
                        .ok()
                        .and_then(|s| s.parse::<f64>().ok())
                        .filter(|s| !s.is_nan())
                        .ok_or_else(|| invalid("invalid RDB score"))?;
                    sorted.insert(key, score).is_some()
                };
                if duplicate {
                    return Err(invalid("duplicate RDB collection key"));
                }
            }
            Ok(if kind == 4 {
                Value::Hash(hash)
            } else {
                Value::SortedSet(sorted)
            })
        }
        _ => unreachable!(),
    }
}

fn expand_lzf(bytes: &[u8], size: usize) -> io::Result<Vec<u8>> {
    if size > MAX_BYTES {
        return Err(invalid("RDB LZF output exceeds limit"));
    }
    let mut input = Decoder::new(bytes);
    let mut output = Vec::new();
    while input.at < bytes.len() {
        let control = input.byte()?;
        if control < 32 {
            let n = usize::from(control) + 1;
            if output.len() + n > size {
                return Err(invalid("invalid LZF literal"));
            }
            output.extend(input.take(n)?);
        } else {
            let mut n = usize::from(control >> 5);
            if n == 7 {
                n += usize::from(input.byte()?);
            }
            n += 2;
            let offset = (usize::from(control & 31) << 8) + usize::from(input.byte()?) + 1;
            if offset > output.len() || output.len() + n > size {
                return Err(invalid("invalid LZF back reference"));
            }
            for _ in 0..n {
                output.push(output[output.len() - offset]);
            }
        }
    }
    if output.len() != size {
        return Err(invalid("invalid LZF output length"));
    }
    Ok(output)
}

fn listpack(bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
    let mut input = Decoder::new(bytes);
    let size = u32::from_le_bytes(input.take(4)?.try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(input.take(2)?.try_into().unwrap());
    if size != bytes.len() {
        return Err(invalid("invalid listpack size"));
    }
    let mut items = Vec::new();
    loop {
        let start = input.at;
        let first = input.byte()?;
        if first == 255 {
            break;
        }
        let item = if first < 128 {
            first.to_string().into_bytes()
        } else if first < 192 {
            input.take(usize::from(first & 63))?.to_vec()
        } else if first < 224 {
            let n = (i16::from(first & 31) << 8) | i16::from(input.byte()?);
            let n = if n & 4096 != 0 { n - 8192 } else { n };
            n.to_string().into_bytes()
        } else if first < 240 {
            let n = (usize::from(first & 15) << 8) | usize::from(input.byte()?);
            input.take(n)?.to_vec()
        } else if first == 240 {
            let n = u32::from_le_bytes(input.take(4)?.try_into().unwrap()) as usize;
            input.take(n)?.to_vec()
        } else {
            let n = match first {
                241 => i16::from_le_bytes(input.take(2)?.try_into().unwrap()) as i64,
                242 => {
                    let raw = input.take(3)?;
                    let n = i32::from_le_bytes([raw[0], raw[1], raw[2], 0]);
                    ((n << 8) >> 8) as i64
                }
                243 => i32::from_le_bytes(input.take(4)?.try_into().unwrap()) as i64,
                244 => i64::from_le_bytes(input.take(8)?.try_into().unwrap()),
                _ => return Err(invalid("unknown listpack entry")),
            };
            n.to_string().into_bytes()
        };
        let length = input.at - start;
        let mut encoded = vec![(length & 127) as u8];
        let mut rest = length >> 7;
        while rest > 0 {
            encoded.push((rest & 127) as u8);
            rest >>= 7;
        }
        encoded.reverse();
        for byte in encoded.iter_mut().skip(1) {
            *byte |= 128;
        }
        if input.take(encoded.len())? != encoded {
            return Err(invalid("invalid listpack back length"));
        }
        input.budget(item.len() + 32)?;
        items.push(item);
        if items.len() > MAX_ITEMS {
            return Err(invalid("listpack item limit exceeded"));
        }
    }
    if input.at != bytes.len() || (count != u16::MAX && items.len() != count as usize) {
        return Err(invalid("invalid listpack item count or terminator"));
    }
    Ok(items)
}

fn put_length(out: &mut Vec<u8>, n: usize) {
    if n < 64 {
        out.push(n as u8);
    } else if n < 16384 {
        out.extend([0x40 | (n >> 8) as u8, n as u8]);
    } else if n <= u32::MAX as usize {
        out.push(0x80);
        out.extend((n as u32).to_be_bytes());
    } else {
        out.push(0x81);
        out.extend((n as u64).to_be_bytes());
    }
}

fn put_string(out: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    if out.len().saturating_add(value.len()).saturating_add(9) > MAX_BYTES {
        return Err(invalid("RDB output exceeds limit"));
    }
    put_length(out, value.len());
    out.extend(value);
    Ok(())
}

fn put_value(out: &mut Vec<u8>, key: Option<&[u8]>, value: &Value) -> io::Result<()> {
    let count = match value {
        Value::List(items) => Some(items.len()),
        Value::Set(items) => Some(items.len()),
        Value::Hash(items) => Some(items.len()),
        Value::SortedSet(items) => Some(items.len()),
        _ => None,
    };
    if count.is_some_and(|n| n == 0 || n > MAX_ITEMS) {
        return Err(invalid("RDB collection size exceeds supported range"));
    }
    let kind = match value {
        Value::String(_) => 0,
        Value::List(_) => 1,
        Value::Set(_) => 2,
        Value::Hash(_) => 4,
        Value::SortedSet(_) => 5,
        Value::Stream(_) => return Err(invalid("RDB streams are not supported yet")),
    };
    out.push(kind);
    if let Some(key) = key {
        put_string(out, key)?;
    }
    match value {
        Value::String(value) => put_string(out, value)?,
        Value::List(items) => {
            put_length(out, items.len());
            for item in items {
                put_string(out, item)?;
            }
        }
        Value::Set(items) => {
            put_length(out, items.len());
            let mut items: Vec<_> = items.iter().collect();
            items.sort();
            for item in items {
                put_string(out, item)?;
            }
        }
        Value::Hash(items) => {
            put_length(out, items.len());
            let mut items: Vec<_> = items.iter().collect();
            items.sort_by_key(|(k, _)| *k);
            for (field, value) in items {
                put_string(out, field)?;
                put_string(out, value)?;
            }
        }
        Value::SortedSet(items) => {
            put_length(out, items.len());
            for (member, score) in items.entries() {
                put_string(out, member)?;
                out.extend(score.to_le_bytes());
            }
        }
        Value::Stream(_) => unreachable!(),
    }
    Ok(())
}

pub fn encode(entries: &HashMap<Vec<u8>, Entry>) -> io::Result<Vec<u8>> {
    if entries.len() > MAX_ITEMS {
        return Err(invalid("RDB key count exceeds limit"));
    }
    let mut out = format!("REDIS{VERSION:04}").into_bytes();
    out.extend([254, 0]);
    let now = Instant::now();
    let epoch = epoch_millis();
    let mut items: Vec<_> = entries.iter().collect();
    items.sort_by_key(|(k, _)| *k);
    for (key, entry) in items {
        if let Some(deadline) = entry.expires_at {
            if deadline <= now {
                continue;
            }
            let millis = u64::try_from(deadline.duration_since(now).as_millis())
                .ok()
                .and_then(|n| epoch.checked_add(n))
                .ok_or_else(|| invalid("RDB expiry overflow"))?;
            out.push(252);
            out.extend(millis.to_le_bytes());
        }
        put_value(&mut out, Some(key), &entry.value)?;
    }
    out.push(255);
    out.extend(crc64(&out).to_le_bytes());
    if out.len() > MAX_BYTES {
        return Err(invalid("RDB output exceeds limit"));
    }
    Ok(out)
}

fn checked_body(bytes: &[u8], dump: bool) -> io::Result<&[u8]> {
    if bytes.len() > MAX_BYTES || bytes.len() < if dump { 11 } else { 18 } {
        return Err(invalid("invalid RDB size"));
    }
    let split = bytes.len() - 8;
    let expected = u64::from_le_bytes(bytes[split..].try_into().unwrap());
    if (dump || expected != 0) && crc64(&bytes[..split]) != expected {
        return Err(invalid("RDB checksum mismatch"));
    }
    Ok(&bytes[..split])
}

pub fn decode(bytes: &[u8]) -> io::Result<HashMap<Vec<u8>, Entry>> {
    let body = checked_body(bytes, false)?;
    let mut input = Decoder::new(body);
    if input.take(5)? != b"REDIS" {
        return Err(invalid("invalid RDB header"));
    }
    let version = std::str::from_utf8(input.take(4)?)
        .ok()
        .and_then(|s| s.parse::<u16>().ok());
    if !version.is_some_and(|n| (6..=15).contains(&n)) {
        return Err(invalid("unsupported RDB version"));
    }
    let mut entries = HashMap::new();
    let mut seen = HashSet::new();
    let mut expiry = None;
    let epoch = epoch_millis();
    let now = Instant::now();
    loop {
        let opcode = input.byte()?;
        match opcode {
            255 => {
                if expiry.is_some() {
                    return Err(invalid("orphan RDB expiry"));
                }
                break;
            }
            254 => {
                if input.length()? != 0 {
                    return Err(invalid("only RDB database 0 is supported"));
                }
            }
            250 => {
                input.string()?;
                input.string()?;
            }
            251 => {
                input.length()?;
                input.length()?;
            }
            252 | 253 => {
                if expiry.is_some() {
                    return Err(invalid("duplicate RDB expiry"));
                }
                expiry = Some(if opcode == 252 {
                    u64::from_le_bytes(input.take(8)?.try_into().unwrap())
                } else {
                    u32::from_le_bytes(input.take(4)?.try_into().unwrap()) as u64 * 1000
                });
            }
            248 => {
                input.length()?;
            }
            249 => {
                input.byte()?;
            }
            kind => {
                let key = input.string()?;
                let value = input.value(kind)?;
                if !seen.insert(key.clone()) || seen.len() > MAX_ITEMS {
                    return Err(invalid("duplicate or excessive RDB keys"));
                }
                let expires_at = match expiry.take() {
                    Some(time) if time <= epoch => continue,
                    Some(time) => Some(
                        now.checked_add(Duration::from_millis(time - epoch))
                            .ok_or_else(|| invalid("RDB expiry overflow"))?,
                    ),
                    None => None,
                };
                entries.insert(key, Entry { value, expires_at });
            }
        }
    }
    if input.at != body.len() {
        return Err(invalid("trailing RDB data"));
    }
    Ok(entries)
}

pub fn load(path: &Path) -> io::Result<HashMap<Vec<u8>, Entry>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    decode(&bytes)
}

fn save_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut name = path
        .file_name()
        .ok_or_else(|| invalid("invalid RDB filename"))?
        .to_os_string();
    name.push(format!(
        ".reru-{}-{}.tmp",
        std::process::id(),
        SAVE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = path.with_file_name(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()
    })();
    drop(file);
    let result = result.and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn dump(value: &Value) -> io::Result<Vec<u8>> {
    let mut out = Vec::new();
    put_value(&mut out, None, value)?;
    out.extend(VERSION.to_le_bytes());
    out.extend(crc64(&out).to_le_bytes());
    if out.len() > MAX_BYTES {
        return Err(invalid("DUMP output exceeds limit"));
    }
    Ok(out)
}

fn restore_value(bytes: &[u8]) -> io::Result<Value> {
    let body = checked_body(bytes, true)?;
    let split = body.len() - 2;
    let version = u16::from_le_bytes(body[split..].try_into().unwrap());
    if !(6..=15).contains(&version) {
        return Err(invalid("unsupported RDB version"));
    }
    let mut input = Decoder::new(&body[..split]);
    let kind = input.byte()?;
    let value = input.value(kind)?;
    if input.at != split {
        return Err(invalid("trailing DUMP data"));
    }
    Ok(value)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    match command[0].to_ascii_uppercase().as_slice() {
        b"DUMP" => Some(command.len() == 2),
        b"RESTORE" => Some(command.len() >= 4),
        b"SAVE" | b"LASTSAVE" => Some(command.len() == 1),
        b"BGSAVE" => Some(command.len() <= 2),
        _ => None,
    }
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let valid = valid_arity(command)?;
    if !valid {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    Some(match command[0].to_ascii_uppercase().as_slice() {
        b"DUMP" => {
            let mut entries = store.entries.lock().unwrap();
            remove_expired(&mut entries, &command[1]);
            match entries.get(&command[1]) {
                None => b"$-1\r\n".to_vec(),
                Some(entry) => match dump(&entry.value) {
                    Ok(bytes) => protocol::bulk(&bytes),
                    Err(error) => protocol::error(&error.to_string()),
                },
            }
        }
        b"RESTORE" => restore(command, store),
        b"LASTSAVE" => format!(":{}\r\n", store.rdb.last_save.load(Ordering::Relaxed)).into_bytes(),
        _ => save(command, store),
    })
}

fn restore(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let mut replace = false;
    let mut absolute = false;
    for option in &command[4..] {
        if option.eq_ignore_ascii_case(b"REPLACE") {
            replace = true;
        } else if option.eq_ignore_ascii_case(b"ABSTTL") {
            absolute = true;
        } else {
            return protocol::error("syntax error");
        }
    }
    let Some(ttl) = std::str::from_utf8(&command[2])
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|n| *n >= 0)
    else {
        return protocol::error("Invalid TTL value, must be >= 0");
    };
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &command[1]);
    if !replace && entries.contains_key(&command[1]) {
        return b"-BUSYKEY Target key name already exists.\r\n".to_vec();
    }
    let value = match restore_value(&command[3]) {
        Ok(value) => value,
        Err(_) => return protocol::error("DUMP payload version or checksum are wrong"),
    };
    let remaining = if absolute && ttl != 0 {
        (ttl as u64).saturating_sub(epoch_millis())
    } else {
        ttl as u64
    };
    if absolute && ttl != 0 && remaining == 0 {
        if entries.remove(&command[1]).is_some() {
            mark_modified(store, &command[1]);
        }
        return b"+OK\r\n".to_vec();
    }
    let expires_at = if ttl == 0 {
        None
    } else {
        match Instant::now().checked_add(Duration::from_millis(remaining)) {
            Some(time) => Some(time),
            None => return protocol::error("Invalid TTL value, must be >= 0"),
        }
    };
    entries.insert(command[1].clone(), Entry { value, expires_at });
    mark_modified(store, &command[1]);
    b"+OK\r\n".to_vec()
}

fn save(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let background = command[0].eq_ignore_ascii_case(b"BGSAVE");
    if command.len() == 2 && !command[1].eq_ignore_ascii_case(b"SCHEDULE") {
        return protocol::error("syntax error");
    }
    let Some(path) = store.rdb.path.clone() else {
        return protocol::error("RDB persistence requires --dir or --dbfilename");
    };
    if store.rdb.saving.swap(true, Ordering::AcqRel) {
        return protocol::error("Background save already in progress");
    }
    // Clone while the command gate is held, so the worker saves one coherent point in time.
    let entries = store.entries.lock().unwrap().clone();
    let owned = store.clone();
    let write = move || {
        let result = encode(&entries).and_then(|bytes| save_file(&path, &bytes));
        if result.is_ok() {
            owned
                .rdb
                .last_save
                .store(epoch_millis() / 1000, Ordering::Relaxed);
        }
        owned
            .rdb
            .last_save_ok
            .store(result.is_ok(), Ordering::Relaxed);
        owned.rdb.saving.store(false, Ordering::Release);
        result
    };
    if background {
        if let Err(error) = std::thread::Builder::new()
            .name("rdb-save".into())
            .spawn(move || {
                if let Err(error) = write() {
                    eprintln!("RDB background save failed: {error}");
                }
            })
        {
            store.rdb.saving.store(false, Ordering::Release);
            return protocol::error(&error.to_string());
        }
        b"+Background saving started\r\n".to_vec()
    } else {
        match write() {
            Ok(()) => b"+OK\r\n".to_vec(),
            Err(error) => protocol::error(&error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_and_bounded_compression() {
        assert_eq!(crc64(b"123456789"), 0xe9c6_d914_c4b8_d9ca);
        assert_eq!(
            expand_lzf(&[2, b'a', b'b', b'c', 128, 2], 9).unwrap(),
            b"abcabcabc"
        );
        assert!(expand_lzf(&[32, 0], 3).is_err());
        assert!(expand_lzf(&[0, b'a'], 2).is_err());
    }
    #[test]
    fn rejects_truncation_corruption_and_trailing_payloads() {
        let mut entries = HashMap::new();
        entries.insert(
            b"key".to_vec(),
            Entry {
                value: Value::String(vec![0, 255]),
                expires_at: None,
            },
        );
        let bytes = encode(&entries).unwrap();
        for length in 0..bytes.len() {
            assert!(decode(&bytes[..length]).is_err());
        }
        let mut corrupt = bytes.clone();
        corrupt[12] ^= 1;
        assert!(decode(&corrupt).is_err());
        assert_eq!(decode(&bytes).unwrap().len(), 1);
        let mut payload = dump(&Value::String(b"hello".to_vec())).unwrap();
        payload[0] = 99;
        let split = payload.len() - 8;
        let crc = crc64(&payload[..split]);
        payload[split..].copy_from_slice(&crc.to_le_bytes());
        assert!(restore_value(&payload).is_err());
    }

    #[test]
    fn rejects_unsupported_databases_and_malformed_compact_values() {
        let mut bytes = encode(&HashMap::new()).unwrap();
        bytes[10] = 1;
        let end = bytes.len() - 8;
        // Zero is a permitted file checksum; semantic validation must still run.
        bytes[end..].fill(0);
        assert!(decode(&bytes).is_err());
        // One listpack integer with an invalid backward length.
        assert!(listpack(&[9, 0, 0, 0, 1, 0, 42, 2, 255]).is_err());
        // A small encoded input cannot request an unbounded output allocation.
        let compressed = [0xc3, 0, 0x80, 0xff, 0xff, 0xff, 0xff];
        assert!(Decoder::new(&compressed).string().is_err());
        // Declared collection counts are checked before reserving member storage.
        let oversized = [0x80, 0xff, 0xff, 0xff, 0xff];
        assert!(Decoder::new(&oversized).value(1).is_err());
    }
}
