// SPDX-License-Identifier: AGPL-3.0-only
//! Redis vector-set milestone: bounded FP32/Q8 storage and exact linear search.
use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};
use std::collections::HashMap;

pub const MAX_DIMENSIONS: usize = 65_536;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Quantization {
    Q8,
    F32,
}

#[derive(Clone)]
pub struct Vector {
    pub input: Vec<f32>,
    normalized: Vec<f32>,
    q8: Vec<i8>,
    norm: f32,
    range: f32,
}

impl Vector {
    fn new(input: Vec<f32>, quantization: Quantization) -> Self {
        let norm = input
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt() as f32;
        let norm = if norm.is_finite() { norm } else { 0.0 };
        let normalized: Vec<f32> = if norm == 0.0 {
            vec![0.0; input.len()]
        } else {
            input.iter().map(|v| *v / norm).collect()
        };
        let range = normalized.iter().fold(0.0f32, |max, v| max.max(v.abs()));
        let q8 = if quantization == Quantization::Q8 && range > 0.0 {
            normalized
                .iter()
                .map(|v| ((*v / range) * 127.0).round().clamp(-127.0, 127.0) as i8)
                .collect()
        } else {
            vec![0; input.len()]
        };
        Self {
            input,
            normalized,
            q8,
            norm,
            range,
        }
    }

    fn component(&self, index: usize, quantization: Quantization) -> f32 {
        match quantization {
            Quantization::Q8 => f32::from(self.q8[index]) / 127.0 * self.range,
            Quantization::F32 => self.normalized[index],
        }
    }

    fn display(&self, quantization: Quantization) -> Vec<Vec<u8>> {
        (0..self.input.len())
            .map(|i| float(self.component(i, quantization) * self.norm))
            .collect()
    }
}

#[derive(Clone)]
pub struct VectorSet {
    pub dimensions: usize,
    pub quantization: Quantization,
    pub members: HashMap<Vec<u8>, Vector>,
}

impl VectorSet {
    pub fn new(dimensions: usize, quantization: Quantization) -> Self {
        Self {
            dimensions,
            quantization,
            members: HashMap::new(),
        }
    }

    pub fn insert(&mut self, member: Vec<u8>, input: Vec<f32>) -> bool {
        let vector = Vector::new(input, self.quantization);
        self.members.insert(member, vector).is_none()
    }

    pub fn estimated_bytes(&self) -> usize {
        self.members
            .iter()
            .map(|(member, vector)| {
                member
                    .len()
                    .saturating_add(vector.input.len().saturating_mul(9))
                    .saturating_add(80)
            })
            .sum()
    }
}

fn float(value: f32) -> Vec<u8> {
    if value == 0.0 {
        b"0".to_vec()
    } else {
        f64::from(value).to_string().into_bytes()
    }
}

fn integer(value: usize) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}

fn invalid_vector() -> Vec<u8> {
    protocol::error("invalid vector specification")
}

fn parse_vector(command: &[Vec<u8>], start: usize) -> Result<(Vec<f32>, usize), Vec<u8>> {
    let Some(mode) = command.get(start) else {
        return Err(invalid_vector());
    };
    if mode.eq_ignore_ascii_case(b"FP32") {
        let Some(bytes) = command.get(start + 1) else {
            return Err(invalid_vector());
        };
        if bytes.is_empty() || bytes.len() % 4 != 0 || bytes.len() / 4 > MAX_DIMENSIONS {
            return Err(invalid_vector());
        }
        let values = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .copied()
            .map(f32::from_le_bytes)
            .collect::<Vec<_>>();
        if values.iter().any(|v| !v.is_finite()) {
            return Err(invalid_vector());
        }
        Ok((values, start + 2))
    } else if mode.eq_ignore_ascii_case(b"VALUES") {
        let count = command
            .get(start + 1)
            .and_then(|bytes| protocol::parse_int(bytes))
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=MAX_DIMENSIONS).contains(n))
            .ok_or_else(invalid_vector)?;
        let end = (start + 2).checked_add(count).ok_or_else(invalid_vector)?;
        let values = command
            .get(start + 2..end)
            .ok_or_else(invalid_vector)?
            .iter()
            .map(|bytes| {
                std::str::from_utf8(bytes)
                    .ok()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|v| v.is_finite())
                    .ok_or_else(invalid_vector)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((values, end))
    } else {
        Err(invalid_vector())
    }
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    let name = &command[0];
    if name.eq_ignore_ascii_case(b"VADD") {
        Some(command.len() >= 5)
    } else if name.eq_ignore_ascii_case(b"VCARD") || name.eq_ignore_ascii_case(b"VDIM") {
        Some(command.len() == 2)
    } else if name.eq_ignore_ascii_case(b"VEMB") {
        Some(command.len() == 3 || command.len() == 4)
    } else if name.eq_ignore_ascii_case(b"VREM") {
        Some(command.len() == 3)
    } else if name.eq_ignore_ascii_case(b"VSIM") {
        Some(command.len() >= 4)
    } else {
        None
    }
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    command[0].eq_ignore_ascii_case(b"VADD") || command[0].eq_ignore_ascii_case(b"VREM")
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let valid = valid_arity(command)?;
    if !valid {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0])
        )));
    }
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let set = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::VectorSet(set)) => Some(set),
        Some(_) => return Some(wrong_type()),
        None => None,
    };
    let response = if command[0].eq_ignore_ascii_case(b"VADD") {
        let (values, member_index) = match parse_vector(command, 2) {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        let Some(member) = command.get(member_index) else {
            return Some(invalid_vector());
        };
        let mut quantization = Quantization::Q8;
        for option in &command[member_index + 1..] {
            if option.eq_ignore_ascii_case(b"NOQUANT") {
                quantization = Quantization::F32;
            } else if option.eq_ignore_ascii_case(b"Q8") {
                quantization = Quantization::Q8;
            } else {
                return Some(protocol::error("unsupported VADD option"));
            }
        }
        if let Some(existing) = set {
            if existing.dimensions != values.len() {
                return Some(protocol::error(&format!(
                    "Vector dimension mismatch - got {} but set has {}",
                    values.len(),
                    existing.dimensions
                )));
            }
            if existing.quantization != quantization {
                return Some(protocol::error(
                    "asked quantization mismatch with existing vector set",
                ));
            }
        }
        let new = set.is_none();
        if new {
            entries.insert(
                key.clone(),
                Entry {
                    value: Value::VectorSet(VectorSet::new(values.len(), quantization)),
                    expires_at: None,
                },
            );
        }
        let Some(Entry {
            value: Value::VectorSet(set),
            ..
        }) = entries.get_mut(key)
        else {
            unreachable!()
        };
        let changed = set
            .members
            .get(member)
            .is_none_or(|old| old.input != values);
        let added = set.insert(member.clone(), values);
        if changed {
            mark_modified(store, key);
        }
        integer(usize::from(added))
    } else if command[0].eq_ignore_ascii_case(b"VCARD") {
        integer(set.map_or(0, |set| set.members.len()))
    } else if command[0].eq_ignore_ascii_case(b"VDIM") {
        match set {
            Some(set) => integer(set.dimensions),
            None => protocol::error("key does not exist"),
        }
    } else if command[0].eq_ignore_ascii_case(b"VEMB") {
        if command.len() == 4 && !command[3].eq_ignore_ascii_case(b"RAW") {
            return Some(protocol::error("syntax error"));
        }
        match set.and_then(|set| set.members.get(&command[2]).map(|vector| (set, vector))) {
            None => b"*-1\r\n".to_vec(),
            Some((set, vector)) if command.len() == 4 => {
                let mut out = match set.quantization {
                    Quantization::F32 => {
                        let mut out = b"*3\r\n+f32\r\n".to_vec();
                        let bytes = vector
                            .normalized
                            .iter()
                            .flat_map(|v| v.to_le_bytes())
                            .collect::<Vec<_>>();
                        out.extend(protocol::bulk(&bytes));
                        out
                    }
                    Quantization::Q8 => {
                        let mut out = b"*4\r\n+int8\r\n".to_vec();
                        let bytes = vector.q8.iter().map(|v| *v as u8).collect::<Vec<_>>();
                        out.extend(protocol::bulk(&bytes));
                        out
                    }
                };
                out.extend(protocol::bulk(&float(vector.norm)));
                if set.quantization == Quantization::Q8 {
                    out.extend(protocol::bulk(&float(vector.range)));
                }
                out
            }
            Some((set, vector)) => protocol::array(&vector.display(set.quantization)),
        }
    } else if command[0].eq_ignore_ascii_case(b"VREM") {
        let Some(Entry {
            value: Value::VectorSet(set),
            ..
        }) = entries.get_mut(key)
        else {
            return Some(integer(0));
        };
        let removed = set.members.remove(&command[2]).is_some();
        if removed {
            if set.members.is_empty() {
                entries.remove(key);
            }
            mark_modified(store, key);
        }
        integer(usize::from(removed))
    } else {
        match similarity(command, set) {
            Ok(value) => value,
            Err(error) => error,
        }
    };
    Some(response)
}

fn similarity(command: &[Vec<u8>], set: Option<&VectorSet>) -> Result<Vec<u8>, Vec<u8>> {
    let mut index = 2;
    let query = if command[index].eq_ignore_ascii_case(b"ELE") {
        index += 1;
        let member = command.get(index).ok_or_else(invalid_vector)?;
        index += 1;
        match set {
            Some(set) => set
                .members
                .get(member)
                .map(|vector| vector.normalized.clone())
                .ok_or_else(|| protocol::error("element not found in set"))?,
            None => Vec::new(),
        }
    } else {
        let (values, end) = parse_vector(command, index)?;
        index = end;
        Vector::new(values, Quantization::F32).normalized
    };
    let mut with_scores = false;
    let mut truth = false;
    let mut count = 10usize;
    while index < command.len() {
        if command[index].eq_ignore_ascii_case(b"WITHSCORES") {
            with_scores = true;
            index += 1;
        } else if command[index].eq_ignore_ascii_case(b"TRUTH") {
            truth = true;
            index += 1;
        } else if command[index].eq_ignore_ascii_case(b"COUNT") {
            let requested = command
                .get(index + 1)
                .and_then(|n| protocol::parse_int(n))
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| protocol::error("invalid COUNT"))?;
            count = requested;
            index += 2;
        } else {
            return Err(protocol::error("unsupported VSIM option"));
        }
    }
    if !truth {
        return Err(protocol::error(
            "approximate vector search is not supported; use TRUTH",
        ));
    }
    let Some(set) = set else {
        return Ok(b"*0\r\n".to_vec());
    };
    if query.len() != set.dimensions {
        return Err(protocol::error(&format!(
            "Vector dimension mismatch - got {} but set has {}",
            query.len(),
            set.dimensions
        )));
    }
    let mut results = set
        .members
        .iter()
        .map(|(member, vector)| {
            let dot = query
                .iter()
                .enumerate()
                .map(|(i, value)| value * vector.component(i, set.quantization))
                .sum::<f32>();
            let score = ((dot.clamp(-1.0, 1.0) + 1.0) * 0.5).clamp(0.0, 1.0);
            (member, score)
        })
        .collect::<Vec<_>>();
    results.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let mut reply = Vec::new();
    for (member, score) in results.into_iter().take(count) {
        reply.push(member.clone());
        if with_scores {
            reply.push(float(score));
        }
    }
    Ok(protocol::array(&reply))
}
