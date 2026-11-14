// SPDX-License-Identifier: AGPL-3.0-only
use crate::{
    collections::{FieldDeadline, HashValue},
    streams, Entry, Value,
};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Cursor, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAGIC: &[u8; 8] = b"RERUSNP1";
const MAX_SNAPSHOT: usize = 256 * 1024 * 1024;
const MAX_ITEMS: usize = 1_000_000;

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid ReRu snapshot")
}

fn absolute_millis(
    deadline: Instant,
    now_instant: Instant,
    now_system: SystemTime,
) -> io::Result<u64> {
    let absolute = now_system
        .checked_add(deadline.saturating_duration_since(now_instant))
        .ok_or_else(invalid)?;
    u64::try_from(
        absolute
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid())?
            .as_millis(),
    )
    .map_err(|_| invalid())
}

pub(crate) fn put_u64(output: &mut Vec<u8>, value: u64) {
    output.extend(value.to_be_bytes());
}

pub(crate) fn put_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> io::Result<()> {
    put_u64(output, u64::try_from(bytes.len()).map_err(|_| invalid())?);
    output.extend(bytes);
    if output.len() > MAX_SNAPSHOT {
        return Err(invalid());
    }
    Ok(())
}

pub(crate) fn read_u64(cursor: &mut Cursor<&[u8]>) -> io::Result<u64> {
    let mut bytes = [0; 8];
    cursor.read_exact(&mut bytes)?;
    Ok(u64::from_be_bytes(bytes))
}

pub(crate) fn read_bytes(cursor: &mut Cursor<&[u8]>) -> io::Result<Vec<u8>> {
    let len = usize::try_from(read_u64(cursor)?).map_err(|_| invalid())?;
    if len > MAX_SNAPSHOT
        || len
            > cursor
                .get_ref()
                .len()
                .saturating_sub(cursor.position() as usize)
    {
        return Err(invalid());
    }
    let mut bytes = vec![0; len];
    cursor.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub fn encode(entries: &HashMap<Vec<u8>, Entry>) -> io::Result<Vec<u8>> {
    let mut output = MAGIC.to_vec();
    put_u64(
        &mut output,
        u64::try_from(entries.len()).map_err(|_| invalid())?,
    );
    let now_instant = Instant::now();
    let now_system = SystemTime::now();
    for (key, entry) in entries {
        put_bytes(&mut output, key)?;
        let expiry = entry
            .expires_at
            .map(|at| absolute_millis(at, now_instant, now_system))
            .transpose()?
            .unwrap_or(u64::MAX);
        put_u64(&mut output, expiry);
        match &entry.value {
            Value::String(value) => {
                output.push(1);
                put_bytes(&mut output, value)?;
            }
            Value::List(items) => {
                output.push(2);
                put_u64(
                    &mut output,
                    u64::try_from(items.len()).map_err(|_| invalid())?,
                );
                for item in items {
                    put_bytes(&mut output, item)?;
                }
            }
            Value::Stream(stream) => {
                output.push(8);
                put_u64(&mut output, stream.last_generated_id.milliseconds);
                put_u64(&mut output, stream.last_generated_id.sequence);
                put_u64(
                    &mut output,
                    u64::try_from(stream.entries.len()).map_err(|_| invalid())?,
                );
                for item in &stream.entries {
                    put_u64(&mut output, item.id.milliseconds);
                    put_u64(&mut output, item.id.sequence);
                    put_u64(
                        &mut output,
                        u64::try_from(item.fields.len()).map_err(|_| invalid())?,
                    );
                    for field in &item.fields {
                        put_bytes(&mut output, field)?;
                    }
                }
                crate::stream_groups::encode_metadata(stream, &mut output)?;
            }
            Value::Hash(fields) => {
                let with_expiry = !fields.deadlines().is_empty();
                output.push(if with_expiry { 9 } else { 4 });
                put_u64(&mut output, fields.len() as u64);
                for (field, value) in fields {
                    put_bytes(&mut output, field)?;
                    put_bytes(&mut output, value)?;
                    if with_expiry {
                        let deadline = fields
                            .deadline_record(field)
                            .map(|at| u64::try_from(at.unix_ms).map_err(|_| invalid()))
                            .transpose()?
                            .unwrap_or(u64::MAX);
                        put_u64(&mut output, deadline);
                    }
                }
            }
            Value::Set(members) => {
                output.push(5);
                put_u64(&mut output, members.len() as u64);
                for member in members {
                    put_bytes(&mut output, member)?;
                }
            }
            Value::SortedSet(members) => {
                output.push(6);
                put_u64(&mut output, members.len() as u64);
                for (member, score) in members.entries() {
                    put_bytes(&mut output, member)?;
                    put_u64(&mut output, score.to_bits());
                }
            }
        }
    }
    if output.len() > MAX_SNAPSHOT {
        return Err(invalid());
    }
    Ok(output)
}

pub fn decode(bytes: &[u8]) -> io::Result<HashMap<Vec<u8>, Entry>> {
    if bytes.len() > MAX_SNAPSHOT || !bytes.starts_with(MAGIC) {
        return Err(invalid());
    }
    let mut cursor = Cursor::new(bytes);
    cursor.set_position(MAGIC.len() as u64);
    let count = usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
    if count > MAX_ITEMS {
        return Err(invalid());
    }
    let now_system = SystemTime::now();
    let now_instant = Instant::now();
    let mut entries = HashMap::new();
    for _ in 0..count {
        let key = read_bytes(&mut cursor)?;
        let expiry = read_u64(&mut cursor)?;
        let mut kind = [0];
        cursor.read_exact(&mut kind)?;
        let value = match kind[0] {
            1 => Value::String(read_bytes(&mut cursor)?),
            2 => {
                let count = usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
                if count > MAX_ITEMS {
                    return Err(invalid());
                }
                let mut items = Vec::with_capacity(count);
                for _ in 0..count {
                    items.push(read_bytes(&mut cursor)?);
                }
                Value::List(items.into())
            }
            3 | 7 | 8 => {
                let last_generated_id = if kind[0] >= 7 {
                    Some(streams::StreamId {
                        milliseconds: read_u64(&mut cursor)?,
                        sequence: read_u64(&mut cursor)?,
                    })
                } else {
                    None
                };
                let count = usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
                if count > MAX_ITEMS {
                    return Err(invalid());
                }
                let mut stream = streams::Stream::default();
                for _ in 0..count {
                    let id = streams::StreamId {
                        milliseconds: read_u64(&mut cursor)?,
                        sequence: read_u64(&mut cursor)?,
                    };
                    let fields_count =
                        usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
                    if fields_count > MAX_ITEMS || fields_count % 2 != 0 {
                        return Err(invalid());
                    }
                    let mut fields = Vec::with_capacity(fields_count);
                    for _ in 0..fields_count {
                        fields.push(read_bytes(&mut cursor)?);
                    }
                    if stream.entries.last().is_some_and(|last| id <= last.id) {
                        return Err(invalid());
                    }
                    stream.entries.push(streams::StreamEntry { id, fields });
                }
                stream.last_generated_id = last_generated_id.unwrap_or_else(|| {
                    stream
                        .entries
                        .last()
                        .map_or(streams::StreamId::default(), |entry| entry.id)
                });
                if stream
                    .entries
                    .last()
                    .is_some_and(|entry| stream.last_generated_id < entry.id)
                {
                    return Err(invalid());
                }
                stream.entries_added = stream.entries.len() as u64;
                if kind[0] == 8 {
                    crate::stream_groups::decode_metadata(&mut stream, &mut cursor)?;
                }
                Value::Stream(stream)
            }
            4 | 9 => {
                let count = usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
                if count == 0 || count > MAX_ITEMS {
                    return Err(invalid());
                }
                let mut fields = HashValue::default();
                let mut seen = HashSet::new();
                for _ in 0..count {
                    let field = read_bytes(&mut cursor)?;
                    let value = read_bytes(&mut cursor)?;
                    let field_expiry = if kind[0] == 9 {
                        read_u64(&mut cursor)?
                    } else {
                        u64::MAX
                    };
                    if !seen.insert(field.clone()) {
                        return Err(invalid());
                    }
                    if field_expiry != u64::MAX {
                        let absolute = UNIX_EPOCH
                            .checked_add(Duration::from_millis(field_expiry))
                            .ok_or_else(invalid)?;
                        let Ok(remaining) = absolute.duration_since(now_system) else {
                            continue;
                        };
                        let at = now_instant.checked_add(remaining).ok_or_else(invalid)?;
                        let unix_ms = i64::try_from(field_expiry).map_err(|_| invalid())?;
                        fields.set_deadline(&field, Some(FieldDeadline { at, unix_ms }));
                    }
                    fields.insert(field, value);
                }
                Value::Hash(fields)
            }
            5 => {
                let count = usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
                if count == 0 || count > MAX_ITEMS {
                    return Err(invalid());
                }
                let mut members = HashSet::new();
                for _ in 0..count {
                    if !members.insert(read_bytes(&mut cursor)?) {
                        return Err(invalid());
                    }
                }
                Value::Set(members)
            }
            6 => {
                let count = usize::try_from(read_u64(&mut cursor)?).map_err(|_| invalid())?;
                if count == 0 || count > MAX_ITEMS {
                    return Err(invalid());
                }
                let mut members = crate::sorted_sets::SortedSet::default();
                for _ in 0..count {
                    let member = read_bytes(&mut cursor)?;
                    let score = f64::from_bits(read_u64(&mut cursor)?);
                    if score.is_nan() || members.insert(member, score).is_some() {
                        return Err(invalid());
                    }
                }
                Value::SortedSet(members)
            }
            _ => return Err(invalid()),
        };
        let expires_at = if expiry == u64::MAX {
            None
        } else {
            let deadline = UNIX_EPOCH
                .checked_add(Duration::from_millis(expiry))
                .ok_or_else(invalid)?;
            match deadline.duration_since(now_system) {
                Ok(remaining) => Some(now_instant.checked_add(remaining).ok_or_else(invalid)?),
                Err(_) => continue,
            }
        };
        if matches!(&value, Value::Hash(hash) if hash.is_empty()) {
            continue;
        }
        if entries.insert(key, Entry { value, expires_at }).is_some() {
            return Err(invalid());
        }
    }
    if cursor.position() as usize != bytes.len() {
        return Err(invalid());
    }
    Ok(entries)
}

pub fn load(path: &Path) -> io::Result<HashMap<Vec<u8>, Entry>> {
    let backup = path.with_extension("bak");
    for candidate in [path, backup.as_path()] {
        match fs::read(candidate) {
            Ok(bytes) => match decode(&bytes) {
                Ok(entries) => return Ok(entries),
                Err(error) if candidate == backup.as_path() => return Err(error),
                Err(_) => {}
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if path.exists() {
        return Err(invalid());
    }
    Ok(HashMap::new())
}

pub fn save(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() > MAX_SNAPSHOT {
        return Err(invalid());
    }
    let temporary = path.with_extension("tmp");
    let backup = path.with_extension("bak");
    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if backup.exists() && !path.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collection_snapshot(kind: u8, count: u64, values: &[&[u8]]) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        put_u64(&mut bytes, 1);
        put_bytes(&mut bytes, b"key").unwrap();
        put_u64(&mut bytes, u64::MAX);
        bytes.push(kind);
        put_u64(&mut bytes, count);
        for value in values {
            put_bytes(&mut bytes, value).unwrap();
        }
        bytes
    }

    #[test]
    fn validates_hash_and_set_snapshot_payloads() {
        let hash = collection_snapshot(4, 1, &[b"f\0", b"v\0"]);
        let set = collection_snapshot(5, 1, &[b"m\0"]);
        assert!(
            matches!(&decode(&hash).unwrap()[b"key".as_slice()].value, Value::Hash(fields) if fields.get(b"f\0".as_slice()).unwrap() == b"v\0")
        );
        assert!(
            matches!(&decode(&set).unwrap()[b"key".as_slice()].value, Value::Set(members) if members.contains(b"m\0".as_slice()))
        );
        for bytes in [
            collection_snapshot(4, 2, &[b"f", b"1", b"f", b"2"]),
            collection_snapshot(5, 2, &[b"m", b"m"]),
            collection_snapshot(4, 0, &[]),
            collection_snapshot(5, 0, &[]),
            collection_snapshot(5, MAX_ITEMS as u64 + 1, &[]),
            hash[..hash.len() - 1].to_vec(),
            set[..set.len() - 1].to_vec(),
        ] {
            assert!(decode(&bytes).is_err());
        }
    }

    #[test]
    fn rejects_invalid_sorted_set_snapshot_scores_and_duplicates() {
        let mut entries = HashMap::new();
        let mut set = crate::sorted_sets::SortedSet::default();
        set.insert(b"one".to_vec(), 1.5);
        set.insert(b"two".to_vec(), f64::INFINITY);
        entries.insert(
            b"scores".to_vec(),
            Entry {
                value: Value::SortedSet(set),
                expires_at: None,
            },
        );
        let valid = encode(&entries).unwrap();
        let decoded = decode(&valid).unwrap();
        let Value::SortedSet(restored) = &decoded[b"scores".as_slice()].value else {
            panic!("expected zset")
        };
        assert_eq!(restored.get(b"one"), Some(1.5));
        assert_eq!(restored.get(b"two"), Some(f64::INFINITY));

        let mut invalid = valid.clone();
        let score_offset = valid.len() - 8;
        invalid[score_offset..].copy_from_slice(&f64::NAN.to_bits().to_be_bytes());
        assert!(decode(&invalid).is_err());

        let mut duplicate = valid.clone();
        // Replace the second member's three bytes with the first member's bytes.
        let position = duplicate
            .windows(3)
            .position(|window| window == b"two")
            .unwrap();
        duplicate[position..position + 3].copy_from_slice(b"one");
        assert!(decode(&duplicate).is_err());
    }
}
