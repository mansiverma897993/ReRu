// SPDX-License-Identifier: AGPL-3.0-only
use crate::{execute_command, protocol, Entry, Store, Value};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static REWRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub struct Rewrite {
    running: AtomicBool,
    last_ok: AtomicBool,
    completed: AtomicU64,
    base_size: AtomicU64,
}

impl Default for Rewrite {
    fn default() -> Self {
        Self {
            running: AtomicBool::new(false),
            last_ok: AtomicBool::new(true),
            completed: AtomicU64::new(0),
            base_size: AtomicU64::new(0),
        }
    }
}

pub fn info(store: &Store) -> String {
    let state = &store.aof_rewrite;
    let size = store
        .aof_path
        .as_ref()
        .and_then(|path| fs::metadata(path).ok())
        .map_or(0, |metadata| metadata.len());
    format!("aof_enabled:{}\r\naof_rewrite_in_progress:{}\r\naof_last_bgrewrite_status:{}\r\naof_rewrites:{}\r\naof_current_size:{size}\r\naof_base_size:{}\r\n\r\n",
        u8::from(store.aof_path.is_some()),
        u8::from(state.running.load(Ordering::Acquire)),
        if state.last_ok.load(Ordering::Relaxed) { "ok" } else { "err" },
        state.completed.load(Ordering::Relaxed),
        state.base_size.load(Ordering::Relaxed))
}

struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

// Caller holds command_gate so no append can race with copying the tail or
// replacing the file. Close all handles before rename for Windows compatibility.
fn install_rewrite(path: &Path, temporary: &Path, offset: u64) -> io::Result<u64> {
    let mut original = File::open(path)?;
    if original.metadata()?.len() < offset {
        return Err(invalid("AOF shrank during rewrite"));
    }
    original.seek(SeekFrom::Start(offset))?;
    let mut output = OpenOptions::new().append(true).open(temporary)?;
    io::copy(&mut original, &mut output)?;
    output.sync_all()?;
    let size = output.metadata()?.len();
    drop(original);
    drop(output);
    fs::rename(temporary, path)?;
    // Sync the directory entry after rename on Unix. Windows directory fsync is
    // unavailable through std; power-loss durability there is filesystem-dependent.
    #[cfg(unix)]
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()?;
    Ok(size)
}

fn rewrite_file(
    path: &Path,
    entries: HashMap<Vec<u8>, Entry>,
    offset: u64,
    store: &Store,
) -> io::Result<u64> {
    let mut name = path
        .file_name()
        .ok_or_else(|| invalid("invalid AOF filename"))?
        .to_os_string();
    name.push(format!(
        ".rewrite-{}-{}.tmp",
        std::process::id(),
        REWRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary_path = path.with_file_name(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary_path)?;
    let temporary = Temporary(temporary_path);
    let result = (|| {
        let mut output = Vec::new();
        command(&mut output, &[b"MULTI"]);
        file.write_all(&output)?;
        let now = Instant::now();
        for (key, entry) in entries {
            if entry.expires_at.is_some_and(|deadline| deadline <= now) {
                continue;
            }
            output.clear();
            append_value(&mut output, &key, &entry)?;
            file.write_all(&output)?;
        }
        output.clear();
        command(&mut output, &[b"EXEC"]);
        file.write_all(&output)?;
        file.sync_all()
    })();
    drop(file);
    result?;
    let _gate = store.command_gate.lock().unwrap();
    install_rewrite(path, &temporary.0, offset)
}

// Caller holds command_gate while taking the snapshot and its matching log offset.
pub fn bgrewrite(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    if command.len() != 1 {
        return protocol::error("wrong number of arguments for 'bgrewriteaof' command");
    }
    let Some(path) = store.aof_path.clone() else {
        return protocol::error("BGREWRITEAOF requires --appendonly FILE in ReRu");
    };
    if store.aof_rewrite.running.swap(true, Ordering::AcqRel) {
        return protocol::error("Background append only file rewriting already in progress");
    }
    let offset = match fs::metadata(&path) {
        Ok(metadata) => metadata.len(),
        Err(_) => {
            store.aof_rewrite.last_ok.store(false, Ordering::Relaxed);
            store.aof_rewrite.running.store(false, Ordering::Release);
            return protocol::error("Cannot read append only file");
        }
    };
    let entries = store.entries.lock().unwrap().clone();
    let worker_store = Arc::clone(store);
    let result = std::thread::Builder::new()
        .name("aof-rewrite".into())
        .spawn(move || {
            let result = rewrite_file(&path, entries, offset, &worker_store);
            let state = &worker_store.aof_rewrite;
            match result {
                Ok(size) => {
                    state.base_size.store(size, Ordering::Relaxed);
                    state.completed.fetch_add(1, Ordering::Relaxed);
                    state.last_ok.store(true, Ordering::Relaxed);
                }
                Err(error) => {
                    state.last_ok.store(false, Ordering::Relaxed);
                    eprintln!("AOF rewrite failed: {error}");
                }
            }
            state.running.store(false, Ordering::Release);
        });
    if result.is_err() {
        store.aof_rewrite.last_ok.store(false, Ordering::Relaxed);
        store.aof_rewrite.running.store(false, Ordering::Release);
        return protocol::error("Cannot start append only file rewrite");
    }
    b"+Background append only file rewriting started\r\n".to_vec()
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn command(output: &mut Vec<u8>, args: &[&[u8]]) {
    output.extend(format!("*{}\r\n", args.len()).as_bytes());
    for arg in args {
        output.extend(protocol::bulk(arg));
    }
}

fn append_value(output: &mut Vec<u8>, key: &[u8], entry: &Entry) -> io::Result<()> {
    match &entry.value {
        Value::String(value) => command(output, &[b"SET", key, value]),
        Value::List(items) => {
            for chunk in items.chunks(1022) {
                let mut args: Vec<&[u8]> = vec![b"RPUSH", key];
                args.extend(chunk.iter().map(|member| member.as_slice()));
                command(output, &args);
            }
        }
        Value::Hash(fields) => {
            let mut fields: Vec<_> = fields.iter().collect();
            fields.sort_by(|left, right| left.0.cmp(right.0));
            for chunk in fields.chunks(511) {
                let mut args: Vec<&[u8]> = vec![b"HSET", key];
                for (field, value) in chunk {
                    args.push(field);
                    args.push(value);
                }
                command(output, &args);
            }
        }
        Value::Set(members) => {
            let mut members: Vec<_> = members.iter().collect();
            members.sort();
            for chunk in members.chunks(1022) {
                let mut args: Vec<&[u8]> = vec![b"SADD", key];
                args.extend(chunk.iter().map(|member| member.as_slice()));
                command(output, &args);
            }
        }
        Value::SortedSet(members) => {
            let members: Vec<_> = members.entries().collect();
            for chunk in members.chunks(511) {
                let scores: Vec<_> = chunk.iter().map(|(_, score)| score.to_string()).collect();
                let mut args: Vec<&[u8]> = vec![b"ZADD", key];
                for ((member, _), score) in chunk.iter().zip(&scores) {
                    args.push(score.as_bytes());
                    args.push(member);
                }
                command(output, &args);
            }
        }
        Value::Stream(stream) => {
            append_stream(output, key, stream);
        }
    }
    if let Some(deadline) = entry.expires_at {
        let absolute = SystemTime::now()
            .checked_add(deadline.saturating_duration_since(Instant::now()))
            .ok_or_else(|| invalid("AOF expiry out of range"))?;
        let milliseconds = absolute
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("AOF expiry before epoch"))?
            .as_millis();
        let time = i64::try_from(milliseconds)
            .map_err(|_| invalid("AOF expiry out of range"))?
            .to_string();
        command(output, &[b"PEXPIREAT", key, time.as_bytes()]);
    }
    Ok(())
}

fn append_stream(output: &mut Vec<u8>, key: &[u8], stream: &crate::streams::Stream) {
    use std::collections::BTreeSet;
    // Create an empty stream even when it has no live entries or groups.
    command(
        output,
        &[
            b"XGROUP",
            b"CREATE",
            key,
            b"__reru_aof_empty__",
            b"0",
            b"MKSTREAM",
        ],
    );
    command(output, &[b"XGROUP", b"DESTROY", key, b"__reru_aof_empty__"]);
    // Restore PEL entries on temporary payloads. Trimming these (rather than
    // XDEL) preserves max-deleted-entry-id, including its zero sentinel. A
    // trimmed pending ID may exceed last-generated-id after an explicit reset.
    let pending_ids: BTreeSet<_> = stream
        .groups
        .values()
        .flat_map(|group| group.pending.keys().copied())
        .collect();
    for id in &pending_ids {
        command(
            output,
            &[
                b"XADD",
                key,
                id.to_string().as_bytes(),
                b"__reru_aof_pending__",
                b"",
            ],
        );
    }
    for (name, group) in &stream.groups {
        let last = group.last_delivered.to_string();
        let read = group
            .entries_read
            .map_or_else(|| "-1".to_owned(), |n| n.to_string());
        command(
            output,
            &[
                b"XGROUP",
                b"CREATE",
                key,
                name,
                last.as_bytes(),
                b"ENTRIESREAD",
                read.as_bytes(),
            ],
        );
        for consumer in group.consumers.keys() {
            command(output, &[b"XGROUP", b"CREATECONSUMER", key, name, consumer]);
        }
        for (id, pending) in &group.pending {
            let id = id.to_string();
            let at = pending.delivered_at.to_string();
            let deliveries = pending.deliveries.to_string();
            command(
                output,
                &[
                    b"XCLAIM",
                    key,
                    name,
                    &pending.consumer,
                    b"0",
                    id.as_bytes(),
                    b"TIME",
                    at.as_bytes(),
                    b"RETRYCOUNT",
                    deliveries.as_bytes(),
                    b"FORCE",
                    b"JUSTID",
                ],
            );
        }
    }
    if !pending_ids.is_empty() {
        command(output, &[b"XTRIM", key, b"MAXLEN", b"0"]);
        command(output, &[b"XSETID", key, b"0-0"]);
    }
    for item in &stream.entries {
        let id = item.id.to_string();
        let mut args: Vec<&[u8]> = vec![b"XADD", key, id.as_bytes()];
        args.extend(item.fields.iter().map(Vec::as_slice));
        command(output, &args);
    }
    command(
        output,
        &[
            b"XSETID",
            key,
            stream.last_generated_id.to_string().as_bytes(),
            b"ENTRIESADDED",
            stream.entries_added.to_string().as_bytes(),
            b"MAXDELETEDID",
            stream.max_deleted_id.to_string().as_bytes(),
        ],
    );
}

pub fn append(
    path: &Path,
    changed_keys: &[Vec<u8>],
    entries: &HashMap<Vec<u8>, Entry>,
) -> io::Result<()> {
    let mut output = Vec::new();
    command(&mut output, &[b"MULTI"]);
    let mut keys = changed_keys.to_vec();
    keys.sort();
    let now = Instant::now();
    for key in keys {
        command(&mut output, &[b"DEL", &key]);
        if let Some(entry) = entries
            .get(&key)
            .filter(|entry| entry.expires_at.is_none_or(|deadline| deadline > now))
        {
            append_value(&mut output, &key, entry)?;
        }
    }
    command(&mut output, &[b"EXEC"]);
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let original_len = file.metadata()?.len();
    if let Err(error) = file.write_all(&output).and_then(|_| file.sync_data()) {
        let _ = file.set_len(original_len);
        return Err(error);
    }
    Ok(())
}

pub fn replay(path: &Path, store: &Store) -> io::Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let mut reader = BufReader::new(file);
    let mut committed = 0;
    let mut transaction: Option<Vec<Vec<Vec<u8>>>> = None;
    let mut partial_tail = false;
    loop {
        let command = match protocol::read_command(&mut reader) {
            Ok(Some(command)) => command,
            Ok(None) => break,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
                partial_tail = true;
                break;
            }
            Err(error) => return Err(error),
        };
        if command.len() == 1 && command[0].eq_ignore_ascii_case(b"MULTI") {
            if transaction.replace(Vec::new()).is_some() {
                return Err(invalid("nested AOF transaction"));
            }
        } else if command.len() == 1 && command[0].eq_ignore_ascii_case(b"EXEC") {
            let batch = transaction
                .take()
                .ok_or_else(|| invalid("AOF EXEC without MULTI"))?;
            for item in batch {
                let reply = execute_command(&item, store, false, None);
                if reply.starts_with(b"-") {
                    return Err(invalid("AOF command failed during replay"));
                }
            }
            committed = reader.stream_position()?;
        } else if let Some(batch) = transaction.as_mut() {
            batch.push(command);
        } else {
            return Err(invalid("AOF command outside transaction"));
        }
    }
    if partial_tail || transaction.is_some() {
        reader.get_mut().set_len(committed)?;
    }
    store.pending_keys.lock().unwrap().clear();
    store
        .aof_rewrite
        .base_size
        .store(reader.get_ref().metadata()?.len(), Ordering::Relaxed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn paths() -> (PathBuf, PathBuf) {
        // Include a sequence number so parallel test threads never share a
        // timestamp, which would make one test read another test's file.
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "reru-rewrite-{}-{unique}-{sequence}",
            std::process::id()
        ));
        let temporary = base.with_extension("tmp");
        (base, temporary)
    }

    #[test]
    fn rewrite_installs_compacted_base_plus_exact_concurrent_tail() {
        let (path, temp) = paths();
        fs::write(&path, b"old-historynew-writes\0\xff").unwrap();
        fs::write(&temp, b"compacted-base").unwrap();
        let expected = b"compacted-basenew-writes\0\xff";
        assert_eq!(
            install_rewrite(&path, &temp, 11).unwrap(),
            expected.len() as u64
        );
        assert_eq!(fs::read(&path).unwrap(), expected);
        assert!(!temp.exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn truncated_source_aborts_rewrite_without_replacing_original() {
        let (path, temp) = paths();
        fs::write(&path, b"keep-original").unwrap();
        fs::write(&temp, b"replacement").unwrap();
        let guard = Temporary(temp.clone());
        assert!(install_rewrite(&path, &temp, 100).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"keep-original");
        drop(guard);
        assert!(!temp.exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn writes_resp_arrays() {
        let mut output = Vec::new();
        command(&mut output, &[b"SET", b"a", b"x\0y"]);
        assert_eq!(output, b"*3\r\n$3\r\nSET\r\n$1\r\na\r\n$3\r\nx\0y\r\n");
    }
}
