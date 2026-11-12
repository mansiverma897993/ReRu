// SPDX-License-Identifier: AGPL-3.0-only
use std::io::{self, Cursor, Read};

const MAGIC: &[u8; 8] = b"RERUREP1";
pub const MAX_FRAME: usize = 256 * 1024 * 1024;

pub struct Update<'a> {
    pub full_sync: bool,
    pub changed_keys: Vec<Vec<u8>>,
    pub snapshot: &'a [u8],
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid ReRu replication frame")
}

pub fn encode(snapshot: &[u8], changed_keys: Option<&[Vec<u8>]>) -> io::Result<Vec<u8>> {
    let mut bytes = MAGIC.to_vec();
    bytes.push(u8::from(changed_keys.is_some()));
    let keys = changed_keys.unwrap_or(&[]);
    bytes.extend((keys.len() as u64).to_be_bytes());
    for key in keys {
        bytes.extend((key.len() as u64).to_be_bytes());
        bytes.extend(key);
        if bytes.len() > MAX_FRAME {
            return Err(invalid());
        }
    }
    if bytes
        .len()
        .checked_add(snapshot.len())
        .is_none_or(|len| len > MAX_FRAME)
    {
        return Err(invalid());
    }
    bytes.extend(snapshot);
    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> io::Result<Update<'_>> {
    if bytes.len() > MAX_FRAME || !bytes.starts_with(MAGIC) || bytes.len() < 17 {
        return Err(invalid());
    }
    let full_sync = match bytes[8] {
        0 => true,
        1 => false,
        _ => return Err(invalid()),
    };
    let mut cursor = Cursor::new(bytes);
    cursor.set_position(9);
    let mut number = [0; 8];
    cursor.read_exact(&mut number)?;
    let count = u64::from_be_bytes(number);
    if count > 1_000_000 || (full_sync && count != 0) {
        return Err(invalid());
    }
    let mut changed_keys = Vec::new();
    for _ in 0..count {
        cursor.read_exact(&mut number)?;
        let length = usize::try_from(u64::from_be_bytes(number)).map_err(|_| invalid())?;
        if length > bytes.len().saturating_sub(cursor.position() as usize) {
            return Err(invalid());
        }
        let mut key = vec![0; length];
        cursor.read_exact(&mut key)?;
        changed_keys.push(key);
    }
    Ok(Update {
        full_sync,
        changed_keys,
        snapshot: &bytes[cursor.position() as usize..],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_frame_metadata_and_preserves_binary_keys() {
        let bytes = encode(b"snapshot", Some(&[b"key\0data".to_vec()])).unwrap();
        let update = decode(&bytes).unwrap();
        assert!(!update.full_sync);
        assert_eq!(update.changed_keys, [b"key\0data".to_vec()]);
        assert_eq!(update.snapshot, b"snapshot");
        let mut invalid = bytes.clone();
        invalid[8] = 3;
        assert!(decode(&invalid).is_err());
        invalid[8] = 0;
        assert!(decode(&invalid).is_err());
        assert!(decode(&bytes[..20]).is_err());
    }
}
