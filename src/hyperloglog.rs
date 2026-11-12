// SPDX-License-Identifier: AGPL-3.0-only
//! Redis HYLL string interchange and approximate distinct counting.
//! The estimator follows Ertl, arXiv:1702.01284, equations 32, 33 and 37.
//! New values use dense storage; sparse strings are accepted on import.
use crate::{mark_modified, protocol, remove_expired, Entry, Store, Value};

const REGISTERS: usize = 1 << 14;
const HEADER: usize = 16;
const DENSE_BYTES: usize = HEADER + REGISTERS * 6 / 8;
const WRONGTYPE: &[u8] = b"-WRONGTYPE Key is not a valid HyperLogLog string value.\r\n";
const CORRUPT: &[u8] = b"-INVALIDOBJ Corrupted HLL object detected\r\n";
type Result<T> = std::result::Result<T, Vec<u8>>;

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    [b"PFADD".as_slice(), b"PFCOUNT", b"PFMERGE"]
        .iter()
        .any(|name| command[0].eq_ignore_ascii_case(name))
        .then_some(command.len() >= 2)
}
pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    command[0].eq_ignore_ascii_case(b"PFADD") || command[0].eq_ignore_ascii_case(b"PFMERGE")
}
// PFCOUNT remains usable on replicas, but updating its cached cardinality is
// an observable string modification which must participate in WATCH/durability.
pub fn may_update_cache(command: &[Vec<u8>]) -> bool {
    command[0].eq_ignore_ascii_case(b"PFCOUNT") && command.len() == 2
}

fn validate_header(bytes: &[u8]) -> Result<()> {
    if bytes.len() < HEADER
        || &bytes[..4] != b"HYLL"
        || bytes[4] > 1
        || (bytes[4] == 0 && bytes.len() != DENSE_BYTES)
    {
        return Err(WRONGTYPE.to_vec());
    }
    Ok(())
}

fn decode(bytes: &[u8]) -> Result<Vec<u8>> {
    validate_header(bytes)?;
    let mut registers = vec![0; REGISTERS];
    if bytes[4] == 0 {
        for (index, value) in registers.iter_mut().enumerate() {
            let bit = index * 6;
            let byte = HEADER + bit / 8;
            let pair =
                u16::from(bytes[byte]) | (u16::from(*bytes.get(byte + 1).unwrap_or(&0)) << 8);
            *value = ((pair >> (bit % 8)) & 63) as u8;
            // A 64-bit hash has 50 rank bits after the 14-bit register index.
            if *value > 51 {
                return Err(CORRUPT.to_vec());
            }
        }
    } else {
        let mut at = HEADER;
        let mut index = 0;
        while at < bytes.len() {
            let opcode = bytes[at];
            at += 1;
            let (value, length) = match opcode >> 6 {
                0 => (0, usize::from(opcode) + 1),
                1 => {
                    let next = *bytes.get(at).ok_or_else(|| CORRUPT.to_vec())?;
                    at += 1;
                    (0, ((usize::from(opcode & 63) << 8) | usize::from(next)) + 1)
                }
                _ => (((opcode >> 2) & 31) + 1, usize::from(opcode & 3) + 1),
            };
            if index + length > REGISTERS {
                return Err(CORRUPT.to_vec());
            }
            registers[index..index + length].fill(value);
            index += length;
        }
        if index != REGISTERS {
            return Err(CORRUPT.to_vec());
        }
    }
    Ok(registers)
}

fn encode(registers: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; DENSE_BYTES];
    bytes[..4].copy_from_slice(b"HYLL");
    bytes[15] = 0x80; // Cached cardinality is invalid until PFCOUNT.
    for (index, value) in registers.iter().enumerate() {
        let bit = index * 6;
        let byte = HEADER + bit / 8;
        let packed = u16::from(*value) << (bit % 8);
        bytes[byte] |= packed as u8;
        if byte + 1 < bytes.len() {
            bytes[byte + 1] |= (packed >> 8) as u8;
        }
    }
    bytes
}

// MurmurHash64A with the seed and byte order used by Redis's HYLL format.
fn hash(bytes: &[u8]) -> u64 {
    const M: u64 = 0xc6a4_a793_5bd1_e995;
    let mut value = 0xadc8_3b19 ^ (bytes.len() as u64).wrapping_mul(M);
    let (blocks, tail) = bytes.as_chunks::<8>();
    for block in blocks {
        let word = u64::from_le_bytes(*block).wrapping_mul(M);
        value ^= (word ^ (word >> 47)).wrapping_mul(M);
        value = value.wrapping_mul(M);
    }
    if !tail.is_empty() {
        let mut word = [0; 8];
        word[..tail.len()].copy_from_slice(tail);
        value = (value ^ u64::from_le_bytes(word)).wrapping_mul(M);
    }
    value ^= value >> 47;
    value = value.wrapping_mul(M);
    value ^ (value >> 47)
}

fn sigma(mut x: f64) -> f64 {
    if x == 1.0 {
        return f64::INFINITY;
    }
    let mut sum = x;
    let mut weight = 1.0;
    for _ in 0..64 {
        x *= x;
        let next = sum + x * weight;
        if next == sum {
            break;
        }
        sum = next;
        weight *= 2.0;
    }
    sum
}
fn tau(mut x: f64) -> f64 {
    if x == 0.0 || x == 1.0 {
        return 0.0;
    }
    let mut sum = 1.0 - x;
    let mut weight = 0.5;
    for _ in 0..64 {
        x = x.sqrt();
        let next = sum - (1.0 - x).powi(2) * weight;
        if next == sum {
            break;
        }
        sum = next;
        weight *= 0.5;
    }
    sum / 3.0
}
fn estimate(registers: &[u8]) -> u64 {
    let mut histogram = [0u32; 52];
    for value in registers {
        histogram[*value as usize] += 1;
    }
    let m = REGISTERS as f64;
    let mut denominator = m * tau((m - f64::from(histogram[51])) / m);
    for count in histogram[1..=50].iter().rev() {
        denominator = (denominator + f64::from(*count)) * 0.5;
    }
    denominator += m * sigma(f64::from(histogram[0]) / m);
    let estimate = (0.5 / std::f64::consts::LN_2) * m * m / denominator;
    (estimate.round() as u64).min(i64::MAX as u64)
}
fn merge(destination: &mut [u8], source: &[u8]) {
    for (target, value) in destination.iter_mut().zip(source) {
        *target = (*target).max(*value);
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
    Some(run(command, store).unwrap_or_else(|error| error))
}

fn run(c: &[Vec<u8>], store: &Store) -> Result<Vec<u8>> {
    let mut entries = store.entries.lock().unwrap();
    let key = &c[1];
    if may_update_cache(c) {
        remove_expired(&mut entries, key);
        let bytes = match entries.get_mut(key).map(|entry| &mut entry.value) {
            None => return Ok(b":0\r\n".to_vec()),
            Some(Value::String(bytes)) => bytes,
            Some(_) => return Err(WRONGTYPE.to_vec()),
        };
        validate_header(bytes)?;
        let count = if bytes[15] & 0x80 == 0 {
            u64::from_le_bytes(bytes[8..16].try_into().unwrap())
        } else {
            let count = estimate(&decode(bytes)?);
            bytes[8..16].copy_from_slice(&count.to_le_bytes());
            mark_modified(store, key);
            count
        };
        return Ok(format!(":{count}\r\n").into_bytes());
    }
    if c[0].eq_ignore_ascii_case(b"PFADD") {
        remove_expired(&mut entries, key);
        let existing = entries.get(key);
        let mut registers = match existing.map(|entry| &entry.value) {
            None => vec![0; REGISTERS],
            Some(Value::String(bytes)) => decode(bytes)?,
            Some(_) => return Err(WRONGTYPE.to_vec()),
        };
        let mut changed = existing.is_none();
        let expires_at = existing.and_then(|entry| entry.expires_at);
        for element in &c[2..] {
            let hash = hash(element);
            let index = hash as usize & (REGISTERS - 1);
            let rank = (((hash >> 14) | (1 << 50)).trailing_zeros() + 1) as u8;
            if rank > registers[index] {
                registers[index] = rank;
                changed = true;
            }
        }
        if changed {
            entries.insert(
                key.clone(),
                Entry {
                    value: Value::String(encode(&registers)),
                    expires_at,
                },
            );
            mark_modified(store, key);
        }
        return Ok(if changed { b":1\r\n" } else { b":0\r\n" }.to_vec());
    }

    // PFMERGE includes the existing destination and preserves its TTL. Decode
    // every source before replacing anything, including for aliased keys.
    let mut registers = vec![0; REGISTERS];
    for key in &c[1..] {
        remove_expired(&mut entries, key);
        match entries.get(key).map(|entry| &entry.value) {
            Some(Value::String(bytes)) => merge(&mut registers, &decode(bytes)?),
            Some(_) => return Err(WRONGTYPE.to_vec()),
            None => {}
        }
    }
    if c[0].eq_ignore_ascii_case(b"PFMERGE") {
        let expires_at = entries.get(key).and_then(|entry| entry.expires_at);
        entries.insert(
            key.clone(),
            Entry {
                value: Value::String(encode(&registers)),
                expires_at,
            },
        );
        mark_modified(store, key);
        return Ok(b"+OK\r\n".to_vec());
    }

    let count = estimate(&registers);
    Ok(format!(":{count}\r\n").into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_bit_boundaries_round_trip_and_reject_impossible_registers() {
        let values: Vec<_> = (0..REGISTERS).map(|i| (i % 52) as u8).collect();
        let bytes = encode(&values);
        assert_eq!(decode(&bytes).unwrap(), values);
        let mut corrupt = bytes;
        corrupt[HEADER] |= 63;
        assert_eq!(decode(&corrupt), Err(CORRUPT.to_vec()));
    }

    #[test]
    fn sparse_validation_bounds_every_run_and_requires_exact_register_count() {
        let mut bytes = vec![0; HEADER];
        bytes[..4].copy_from_slice(b"HYLL");
        bytes[4] = 1;
        bytes.extend([0x7f, 0xff]);
        assert_eq!(decode(&bytes).unwrap(), vec![0; REGISTERS]);
        bytes.push(0);
        assert!(decode(&bytes).is_err());
        bytes.truncate(HEADER + 1);
        assert!(decode(&bytes).is_err());
        bytes.truncate(HEADER);
        bytes.extend([0x87, 0x7f, 0xfb]); // value 2 x4, zero x16380
        let registers = decode(&bytes).unwrap();
        assert_eq!(&registers[..4], &[2; 4]);
        assert!(registers[4..].iter().all(|r| *r == 0));
        assert_eq!(estimate(&vec![0; REGISTERS]), 0);
    }
}
