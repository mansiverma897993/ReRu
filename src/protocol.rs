// SPDX-License-Identifier: AGPL-3.0-only
use std::io::{self, BufRead, Read};

/// Largest bulk string and total argument bytes the parser accepts in one
/// command. This is below Redis's 512 MiB `proto-max-bulk-len` so a single
/// command always fits ReRu's 512 MiB snapshot, RDB, and replication frames.
pub const MAX_BULK: usize = 256 * 1024 * 1024;
/// Largest number of arguments the parser accepts in one command. This is
/// ReRu's own cap, below Redis's INT_MAX argument count and 1 GiB query buffer.
pub const MAX_ARGS: usize = 1024 * 1024;

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
    read_command_with_limits(reader, MAX_ARGS, MAX_BULK)
}

fn read_command_with_limits<R: BufRead>(
    reader: &mut R,
    max_args: usize,
    max_bulk: usize,
) -> io::Result<Option<Vec<Vec<u8>>>> {
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
    if count == 0 || count > max_args {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid argument count",
        ));
    }
    // Reserve at most 1024 slots; a hostile header must not allocate 25 MiB.
    let mut args = Vec::with_capacity(count.min(1024));
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
        if len > max_bulk || total_bytes > max_bulk {
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

pub fn unknown_subcommand(parent: &str, name: &[u8]) -> Vec<u8> {
    let name = name
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() && *byte != b'\'' {
                *byte as char
            } else {
                '?'
            }
        })
        .collect::<String>();
    error(&format!("unknown subcommand '{name}'. Try {parent} HELP."))
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

    #[test]
    fn parser_limits_are_redis_sized() {
        assert_eq!(MAX_ARGS, 1_048_576);
        assert_eq!(MAX_BULK, 256 * 1024 * 1024);
    }

    #[test]
    fn accepts_more_than_1024_arguments() {
        let input = format!("*1025\r\n{}", "$1\r\na\r\n".repeat(1025)).into_bytes();
        let mut reader = Cursor::new(input);
        let command = read_command(&mut reader).unwrap().unwrap();
        assert_eq!(command.len(), 1025);
    }

    #[test]
    fn rejects_counts_and_bulk_lengths_against_the_configured_limits() {
        let input = b"*3\r\n".to_vec();
        let mut reader = Cursor::new(input);
        let error = read_command_with_limits(&mut reader, 2, 100).unwrap_err();
        assert_eq!(error.to_string(), "invalid argument count");

        let input = b"*1\r\n$101\r\n".to_vec();
        let mut reader = Cursor::new(input);
        let error = read_command_with_limits(&mut reader, 16, 100).unwrap_err();
        assert_eq!(error.to_string(), "command too large");

        // Each argument fits, but the sum exceeds the configured total.
        let input = b"*2\r\n$60\r\n".to_vec();
        let mut payload = input;
        payload.extend(std::iter::repeat_n(b'x', 60));
        payload.extend_from_slice(b"\r\n$60\r\n");
        let mut reader = Cursor::new(payload);
        let error = read_command_with_limits(&mut reader, 16, 100).unwrap_err();
        assert_eq!(error.to_string(), "command too large");
    }

    #[test]
    fn committed_fuzz_corpus_keeps_its_line_endings() {
        // The seed corpus is raw RESP bytes. If `.gitattributes` ever normalizes
        // CRLF to LF, these frames stop parsing. The corpus is not part of the
        // published package, so skip the check when it is absent.
        let corpus =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz/corpus/resp_parser");
        if !corpus.is_dir() {
            return;
        }
        let read = |name: &str| std::fs::read(corpus.join(name)).unwrap();

        for (name, commands) in [
            ("array_ping", 1),
            ("array_set", 1),
            ("array_empty_bulk", 1),
            ("two_frames", 2),
        ] {
            let frame = read(name);
            let mut reader = Cursor::new(frame.as_slice());
            for _ in 0..commands {
                assert!(read_command(&mut reader).unwrap().is_some(), "{name}");
            }
            assert_eq!(reader.position() as usize, frame.len(), "{name}");
            assert!(read_command(&mut reader).unwrap().is_none(), "{name}");
        }

        for name in [
            "inline_ping",
            "inline_arguments",
            "truncated_bulk",
            "too_many_arguments",
        ] {
            let frame = read(name);
            let mut reader = Cursor::new(frame.as_slice());
            assert!(read_command(&mut reader).is_err(), "{name}");
        }
    }
}
