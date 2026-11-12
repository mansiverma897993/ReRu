// SPDX-License-Identifier: AGPL-3.0-only
use std::io::{self, BufRead, Read};

const MAX_BULK: usize = 64 * 1024 * 1024;
const MAX_ARGS: usize = 1024;

fn line<R: BufRead>(reader: &mut R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(65).read_until(b'\n', &mut bytes)?;
    if bytes.len() > 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "header too long",
        ));
    }
    if !bytes.ends_with(b"\r\n") {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected CRLF"));
    }
    bytes.truncate(bytes.len() - 2);
    Ok(bytes)
}

fn number(bytes: &[u8]) -> io::Result<usize> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid length"))
}

pub fn read_command<R: BufRead>(reader: &mut R) -> io::Result<Option<Vec<Vec<u8>>>> {
    let first = reader.fill_buf()?;
    if first.is_empty() {
        return Ok(None);
    }
    let header = line(reader)?;
    if header.first() != Some(&b'*') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected RESP array",
        ));
    }
    let count = number(&header[1..])?;
    if count == 0 || count > MAX_ARGS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid argument count",
        ));
    }
    let mut args = Vec::with_capacity(count);
    let mut total_bytes = 0usize;
    for _ in 0..count {
        let header = line(reader)?;
        if header.first() != Some(&b'$') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "expected bulk string",
            ));
        }
        let len = number(&header[1..])?;
        total_bytes = total_bytes
            .checked_add(len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "command too large"))?;
        if len > MAX_BULK || total_bytes > MAX_BULK {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "command too large",
            ));
        }
        let mut value = vec![0; len];
        reader.read_exact(&mut value)?;
        let mut suffix = [0; 2];
        reader.read_exact(&mut suffix)?;
        if suffix != *b"\r\n" {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "expected CRLF"));
        }
        args.push(value);
    }
    Ok(Some(args))
}

pub fn error(message: &str) -> Vec<u8> {
    let message = message.replace(['\r', '\n'], " ");
    format!("-ERR {message}\r\n").into_bytes()
}

pub fn bulk(value: &[u8]) -> Vec<u8> {
    let mut response = format!("${}\r\n", value.len()).into_bytes();
    response.extend_from_slice(value);
    response.extend_from_slice(b"\r\n");
    response
}

pub fn array(values: &[Vec<u8>]) -> Vec<u8> {
    let mut response = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        response.extend(bulk(value));
    }
    response
}

pub fn stream_entries(entries: &[&crate::streams::StreamEntry]) -> Vec<u8> {
    let mut response = format!("*{}\r\n", entries.len()).into_bytes();
    for entry in entries {
        response.extend_from_slice(b"*2\r\n");
        response.extend(bulk(entry.id.to_string().as_bytes()));
        response.extend(array(&entry.fields));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rejects_oversized_header_without_reading_the_whole_input() {
        let input = vec![b'9'; 1_000_000];
        let mut reader = Cursor::new(input);
        let error = read_command(&mut reader).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(reader.position(), 65);
    }
}
