// SPDX-License-Identifier: AGPL-3.0-only
use super::{integer, optional_bulk, wrong_type};
use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};
use std::collections::{BTreeSet, HashMap};
use std::ops::Deref;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Default)]
pub(crate) struct HashValue {
    fields: HashMap<Vec<u8>, Vec<u8>>,
    deadlines: HashMap<Vec<u8>, FieldDeadline>,
    ordered_deadlines: BTreeSet<(Instant, Vec<u8>)>,
}

#[derive(Clone, Copy)]
pub(crate) struct FieldDeadline {
    pub(crate) at: Instant,
    pub(crate) unix_ms: i64,
}

impl HashValue {
    // Arithmetic updates the value in place and keeps an existing field TTL.
    pub(crate) fn insert(&mut self, field: Vec<u8>, value: Vec<u8>) -> Option<Vec<u8>> {
        self.fields.insert(field, value)
    }

    pub(crate) fn overwrite_field(&mut self, field: Vec<u8>, value: Vec<u8>) -> Option<Vec<u8>> {
        self.clear_deadline(&field);
        self.fields.insert(field, value)
    }

    pub(crate) fn remove_field(&mut self, field: &[u8]) -> Option<Vec<u8>> {
        self.clear_deadline(field);
        self.fields.remove(field)
    }

    pub(crate) fn deadline(&self, field: &[u8]) -> Option<Instant> {
        self.deadlines.get(field).map(|deadline| deadline.at)
    }

    pub(crate) fn deadline_record(&self, field: &[u8]) -> Option<FieldDeadline> {
        self.deadlines.get(field).copied()
    }

    pub(crate) fn set_deadline(&mut self, field: &[u8], deadline: Option<FieldDeadline>) {
        self.clear_deadline(field);
        if let Some(deadline) = deadline {
            self.ordered_deadlines.insert((deadline.at, field.to_vec()));
            self.deadlines.insert(field.to_vec(), deadline);
        }
    }

    pub(crate) fn clear_deadline(&mut self, field: &[u8]) -> bool {
        let Some(previous) = self.deadlines.remove(field) else {
            return false;
        };
        self.ordered_deadlines
            .remove(&(previous.at, field.to_vec()));
        true
    }

    pub(crate) fn deadlines(&self) -> &HashMap<Vec<u8>, FieldDeadline> {
        &self.deadlines
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.ordered_deadlines.first().map(|(at, _)| *at)
    }

    pub(crate) fn prune(&mut self, now: Instant) -> usize {
        let mut removed = 0;
        while self
            .ordered_deadlines
            .first()
            .is_some_and(|(at, _)| *at <= now)
        {
            let (_, field) = self.ordered_deadlines.pop_first().unwrap();
            self.deadlines.remove(&field);
            removed += usize::from(self.fields.remove(&field).is_some());
        }
        removed
    }
}

impl From<HashMap<Vec<u8>, Vec<u8>>> for HashValue {
    fn from(fields: HashMap<Vec<u8>, Vec<u8>>) -> Self {
        Self {
            fields,
            deadlines: HashMap::new(),
            ordered_deadlines: BTreeSet::new(),
        }
    }
}

impl Deref for HashValue {
    type Target = HashMap<Vec<u8>, Vec<u8>>;

    fn deref(&self) -> &Self::Target {
        &self.fields
    }
}

impl<'a> IntoIterator for &'a HashValue {
    type Item = (&'a Vec<u8>, &'a Vec<u8>);
    type IntoIter = std::collections::hash_map::Iter<'a, Vec<u8>, Vec<u8>>;

    fn into_iter(self) -> Self::IntoIter {
        self.fields.iter()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Expire,
    Pexpire,
    Expireat,
    Pexpireat,
    Ttl,
    Pttl,
    Expiretime,
    Pexpiretime,
    Persist,
    Getex,
    Setex,
}

fn kind(name: &[u8]) -> Option<Kind> {
    let names: &[(&[u8], Kind)] = &[
        (b"HEXPIRE", Kind::Expire),
        (b"HPEXPIRE", Kind::Pexpire),
        (b"HEXPIREAT", Kind::Expireat),
        (b"HPEXPIREAT", Kind::Pexpireat),
        (b"HTTL", Kind::Ttl),
        (b"HPTTL", Kind::Pttl),
        (b"HEXPIRETIME", Kind::Expiretime),
        (b"HPEXPIRETIME", Kind::Pexpiretime),
        (b"HPERSIST", Kind::Persist),
        (b"HGETEX", Kind::Getex),
        (b"HSETEX", Kind::Setex),
    ];
    names
        .iter()
        .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
        .map(|(_, kind)| *kind)
}

pub(super) fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    kind(&command[0]).map(|kind| match kind {
        Kind::Expire | Kind::Pexpire | Kind::Expireat | Kind::Pexpireat | Kind::Setex => {
            command.len() >= 6
        }
        _ => command.len() >= 5,
    })
}

pub(super) fn is_mutating(command: &[Vec<u8>]) -> bool {
    kind(&command[0]).is_some_and(|kind| match kind {
        Kind::Ttl | Kind::Pttl | Kind::Expiretime | Kind::Pexpiretime => false,
        Kind::Getex => true,
        _ => true,
    })
}

fn unknown_argument(token: &[u8]) -> Vec<u8> {
    let token = token.split(|byte| *byte == 0).next().unwrap_or(b"");
    let mut reply = b"-ERR unknown argument: ".to_vec();
    reply.extend_from_slice(token);
    reply.extend_from_slice(b"\r\n");
    reply
}

fn parse_number(bytes: &[u8]) -> Result<i64, Vec<u8>> {
    protocol::parse_int(bytes)
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))
}

fn deadline(raw: &[u8], millis: bool, absolute: bool) -> Result<FieldDeadline, Vec<u8>> {
    let amount = parse_number(raw)?;
    if amount < 0 {
        return Err(protocol::error("invalid expire time, must be >= 0"));
    }
    let now = Instant::now();
    let current = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| protocol::error("invalid expire time"))?
        .as_millis() as i128;
    let delta_or_absolute = i128::from(amount) * if millis { 1 } else { 1000 };
    let unix_ms = if absolute {
        delta_or_absolute
    } else {
        current + delta_or_absolute
    };
    let unix_ms = i64::try_from(unix_ms).map_err(|_| protocol::error("invalid expire time"))?;
    let remaining = i128::from(unix_ms) - current;
    let at = if remaining <= 0 {
        now
    } else {
        let remaining =
            u64::try_from(remaining).map_err(|_| protocol::error("invalid expire time"))?;
        now.checked_add(Duration::from_millis(remaining))
            .ok_or_else(|| protocol::error("invalid expire time"))?
    };
    Ok(FieldDeadline { at, unix_ms })
}

#[derive(Clone, Copy)]
enum Expiration {
    Deadline(FieldDeadline),
    Persist,
    KeepTtl,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Condition {
    Nx,
    Xx,
    Gt,
    Lt,
    Fnx,
    Fxx,
}

struct HashArguments<'a> {
    condition: Option<Condition>,
    expiration: Option<Expiration>,
    fields: &'a [Vec<u8>],
}

fn parse_arguments<'a>(kind: Kind, command: &'a [Vec<u8>]) -> Result<HashArguments<'a>, Vec<u8>> {
    let expire = matches!(
        kind,
        Kind::Expire | Kind::Pexpire | Kind::Expireat | Kind::Pexpireat
    );
    let ttl = matches!(
        kind,
        Kind::Ttl | Kind::Pttl | Kind::Expiretime | Kind::Pexpiretime | Kind::Persist
    );
    let setex = matches!(kind, Kind::Setex);
    let mut condition = None;
    let mut expiration = None;
    let mut fields = None;
    let mut at = 2;
    while at < command.len() {
        let token = command[at].as_slice();
        if expire && at == 2 {
            expiration = Some(Expiration::Deadline(deadline(
                &command[2],
                matches!(kind, Kind::Pexpire | Kind::Pexpireat),
                matches!(kind, Kind::Expireat | Kind::Pexpireat),
            )?));
            at += 1;
            continue;
        }
        if token.eq_ignore_ascii_case(b"FIELDS") {
            if fields.is_some() {
                return Err(if ttl {
                    field_list_error(kind)
                } else {
                    protocol::error("FIELDS keyword specified multiple times")
                });
            }
            let (list, next) = parse_field_list(kind, command, at)?;
            fields = Some(list);
            at = next;
            continue;
        }
        if ttl {
            return Err(if fields.is_some() {
                field_list_error(kind)
            } else {
                protocol::error("Mandatory argument FIELDS is missing or not at the right position")
            });
        }
        if let Some(flag) = condition_flag(kind, token) {
            if condition.is_some() {
                return Err(protocol::error(if setex {
                    "Only one of FXX or FNX arguments can be specified"
                } else {
                    "Multiple condition flags specified"
                }));
            }
            condition = Some(flag);
            at += 1;
            continue;
        }
        if is_expiry_token(kind, token) {
            if expiration.is_some() {
                return Err(expiry_conflict(kind));
            }
            expiration = Some(parse_expiry_token(command, &mut at)?);
            continue;
        }
        return Err(unknown_argument(token));
    }
    let Some(fields) = fields else {
        return Err(protocol::error("wrong number of arguments"));
    };
    Ok(HashArguments {
        condition,
        expiration,
        fields,
    })
}

fn condition_flag(kind: Kind, token: &[u8]) -> Option<Condition> {
    match kind {
        Kind::Expire | Kind::Pexpire | Kind::Expireat | Kind::Pexpireat => {
            if token.eq_ignore_ascii_case(b"NX") {
                Some(Condition::Nx)
            } else if token.eq_ignore_ascii_case(b"XX") {
                Some(Condition::Xx)
            } else if token.eq_ignore_ascii_case(b"GT") {
                Some(Condition::Gt)
            } else if token.eq_ignore_ascii_case(b"LT") {
                Some(Condition::Lt)
            } else {
                None
            }
        }
        Kind::Setex => {
            if token.eq_ignore_ascii_case(b"FNX") {
                Some(Condition::Fnx)
            } else if token.eq_ignore_ascii_case(b"FXX") {
                Some(Condition::Fxx)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_expiry_token(kind: Kind, token: &[u8]) -> bool {
    let unit = [b"EX".as_slice(), b"PX", b"EXAT", b"PXAT"]
        .iter()
        .any(|unit| token.eq_ignore_ascii_case(unit));
    match kind {
        Kind::Getex => unit || token.eq_ignore_ascii_case(b"PERSIST"),
        Kind::Setex => unit || token.eq_ignore_ascii_case(b"KEEPTTL"),
        _ => false,
    }
}

fn parse_expiry_token(command: &[Vec<u8>], at: &mut usize) -> Result<Expiration, Vec<u8>> {
    let token = command[*at].as_slice();
    if token.eq_ignore_ascii_case(b"PERSIST") {
        *at += 1;
        return Ok(Expiration::Persist);
    }
    if token.eq_ignore_ascii_case(b"KEEPTTL") {
        *at += 1;
        return Ok(Expiration::KeepTtl);
    }
    let millis = token.eq_ignore_ascii_case(b"PX") || token.eq_ignore_ascii_case(b"PXAT");
    let absolute = token.eq_ignore_ascii_case(b"EXAT") || token.eq_ignore_ascii_case(b"PXAT");
    let raw = command
        .get(*at + 1)
        .ok_or_else(|| protocol::error("missing expire time"))?;
    let expiry = deadline(raw, millis, absolute)?;
    *at += 2;
    Ok(Expiration::Deadline(expiry))
}

fn expiry_conflict(kind: Kind) -> Vec<u8> {
    protocol::error(if matches!(kind, Kind::Getex) {
        "Only one of EX, PX, EXAT, PXAT or PERSIST arguments can be specified"
    } else {
        "Only one of EX, PX, EXAT, PXAT or KEEPTTL arguments can be specified"
    })
}

fn field_count_error(kind: Kind) -> Vec<u8> {
    protocol::error(match kind {
        Kind::Setex | Kind::Getex => "invalid number of fields",
        Kind::Ttl | Kind::Pttl | Kind::Expiretime | Kind::Pexpiretime | Kind::Persist => {
            "Number of fields must be a positive integer"
        }
        _ => "Parameter `numFields` should be greater than 0",
    })
}

fn field_list_error(kind: Kind) -> Vec<u8> {
    protocol::error(match kind {
        Kind::Ttl | Kind::Pttl | Kind::Expiretime | Kind::Pexpiretime | Kind::Persist => {
            "The `numfields` parameter must match the number of arguments"
        }
        _ => "wrong number of arguments",
    })
}

fn parse_field_list(
    kind: Kind,
    command: &[Vec<u8>],
    at: usize,
) -> Result<(&[Vec<u8>], usize), Vec<u8>> {
    let raw = command.get(at + 1).ok_or_else(|| field_count_error(kind))?;
    let parsed = parse_number(raw).map_err(|_| field_count_error(kind))?;
    if matches!(
        kind,
        Kind::Ttl | Kind::Pttl | Kind::Expiretime | Kind::Pexpiretime | Kind::Persist
    ) && parsed > i64::from(i32::MAX)
    {
        return Err(field_list_error(kind));
    }
    let count = usize::try_from(parsed)
        .ok()
        .filter(|count| *count > 0 && parsed <= i64::from(i32::MAX))
        .ok_or_else(|| field_count_error(kind))?;
    let width = if matches!(kind, Kind::Setex) { 2 } else { 1 };
    let Some(end) = count.checked_mul(width).and_then(|fields| {
        at.checked_add(2)
            .and_then(|start| start.checked_add(fields))
    }) else {
        return Err(field_count_error(kind));
    };
    if end > command.len() {
        return Err(field_list_error(kind));
    }
    Ok((&command[at + 2..end], end))
}

fn array_integers(values: Vec<i64>) -> Vec<u8> {
    let mut reply = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        reply.extend(integer(value));
    }
    reply
}

pub(super) fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = kind(&command[0])?;
    if !valid_arity(command)? {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    Some(execute_inner(kind, command, store).unwrap_or_else(|error| error))
}

fn execute_inner(kind: Kind, command: &[Vec<u8>], store: &Store) -> Result<Vec<u8>, Vec<u8>> {
    let setex = matches!(kind, Kind::Setex);
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let wrong_kind = entries
        .get(key)
        .is_some_and(|entry| !matches!(entry.value, Value::Hash(_)));
    // HSETEX validates its whole argument list before the type check; the
    // other commands reject a wrong type first.
    if wrong_kind && !setex {
        return Err(wrong_type());
    }
    let HashArguments {
        condition,
        expiration,
        fields,
    } = parse_arguments(kind, command)?;
    if wrong_kind {
        return Err(wrong_type());
    }
    if matches!(kind, Kind::Setex) {
        let existing = entries.get(key).and_then(|entry| match &entry.value {
            Value::Hash(hash) => Some(hash),
            _ => None,
        });
        if (condition == Some(Condition::Fnx)
            && existing.is_some_and(|hash| {
                fields
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .any(|pair| hash.contains_key(&pair[0]))
            }))
            || (condition == Some(Condition::Fxx)
                && existing.is_none_or(|hash| {
                    fields
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .any(|pair| !hash.contains_key(&pair[0]))
                }))
        {
            return Ok(integer(0));
        }
        let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
            value: Value::Hash(HashValue::default()),
            expires_at: None,
        });
        let Value::Hash(hash) = &mut entry.value else {
            unreachable!()
        };
        for pair in fields.as_chunks::<2>().0 {
            let previous = hash.deadline_record(&pair[0]);
            hash.overwrite_field(pair[0].clone(), pair[1].clone());
            match expiration {
                Some(Expiration::Deadline(expiry)) => hash.set_deadline(&pair[0], Some(expiry)),
                Some(Expiration::KeepTtl) => hash.set_deadline(&pair[0], previous),
                _ => {}
            }
        }
        if hash.prune(Instant::now()) > 0 && hash.is_empty() {
            entries.remove(key);
        }
        mark_modified(store, key);
        return Ok(integer(1));
    }
    let mut hash = entries
        .get_mut(key)
        .and_then(|entry| match &mut entry.value {
            Value::Hash(hash) => Some(hash),
            _ => None,
        });
    let mut changed = false;
    if matches!(kind, Kind::Getex) {
        let mut values = Vec::with_capacity(fields.len());
        for field in fields {
            let value = hash.as_ref().and_then(|hash| hash.get(field).cloned());
            if value.is_some() {
                let hash = &mut **hash.as_mut().unwrap();
                match expiration {
                    Some(Expiration::Deadline(expiry)) => {
                        if expiry.at <= Instant::now() {
                            hash.remove_field(field);
                        } else {
                            hash.set_deadline(field, Some(expiry));
                        }
                        changed = true;
                    }
                    Some(Expiration::Persist) => changed |= hash.clear_deadline(field),
                    _ => {}
                }
            }
            values.push(value);
        }
        if changed {
            if hash.as_ref().is_some_and(|hash| hash.is_empty()) {
                entries.remove(key);
            }
            mark_modified(store, key);
        }
        let mut reply = format!("*{}\r\n", values.len()).into_bytes();
        for value in values {
            reply.extend(optional_bulk(value.as_ref()));
        }
        return Ok(reply);
    }
    let mut replies = Vec::with_capacity(fields.len());
    for field in fields {
        let Some(hash) = hash.as_mut() else {
            replies.push(-2);
            continue;
        };
        let hash = &mut **hash;
        if !hash.contains_key(field) {
            replies.push(-2);
        } else if matches!(kind, Kind::Persist) {
            let removed = hash.clear_deadline(field);
            changed |= removed;
            replies.push(if removed { 1 } else { -1 });
        } else if let Some(Expiration::Deadline(expiry)) = expiration {
            let old = hash.deadline_record(field);
            let allowed = match condition {
                Some(Condition::Nx) => old.is_none(),
                Some(Condition::Xx) => old.is_some(),
                Some(Condition::Gt) => old.is_some_and(|old| expiry.unix_ms > old.unix_ms),
                Some(Condition::Lt) => old.is_none_or(|old| expiry.unix_ms < old.unix_ms),
                _ => true,
            };
            if !allowed {
                replies.push(0);
            } else if expiry.at <= Instant::now() {
                hash.remove_field(field);
                replies.push(2);
                changed = true;
            } else {
                hash.set_deadline(field, Some(expiry));
                replies.push(1);
                changed = true;
            }
        } else {
            let old = hash.deadline_record(field);
            let ttl = match kind {
                Kind::Ttl | Kind::Pttl => old.map(|at| {
                    let remaining = at.at.saturating_duration_since(Instant::now());
                    let millis = i64::try_from(remaining.as_millis()).unwrap_or(i64::MAX);
                    if matches!(kind, Kind::Ttl) {
                        millis / 1000
                    } else {
                        millis
                    }
                }),
                Kind::Expiretime | Kind::Pexpiretime => old.map(|at| {
                    let millis = at.unix_ms;
                    if matches!(kind, Kind::Expiretime) {
                        millis / 1000
                    } else {
                        millis
                    }
                }),
                _ => unreachable!(),
            };
            replies.push(ttl.unwrap_or(-1));
        }
    }
    if changed {
        if entries
            .get(key)
            .is_some_and(|entry| matches!(&entry.value, Value::Hash(hash) if hash.is_empty()))
        {
            entries.remove(key);
        }
        mark_modified(store, key);
    }
    Ok(array_integers(replies))
}
