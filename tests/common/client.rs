// SPDX-License-Identifier: AGPL-3.0-only
use crate::common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

#[derive(Debug, PartialEq)]
pub enum Reply {
    Simple(Vec<u8>),
    Error(Vec<u8>),
    Integer(i64),
    Bulk(Option<Vec<u8>>),
    Array(Option<Vec<Reply>>),
}

pub struct Client(BufReader<TcpStream>);

impl Client {
    pub fn new(server: &common::Server) -> Self {
        Self::on_port(server.port)
    }

    pub fn on_port(port: u16) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        Self(BufReader::new(stream))
    }

    pub fn call(&mut self, args: &[&[u8]]) -> Reply {
        let mut command = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            command.extend(format!("${}\r\n", arg.len()).as_bytes());
            command.extend(*arg);
            command.extend(b"\r\n");
        }
        self.0.get_mut().write_all(&command).unwrap();
        self.read()
    }

    pub fn read(&mut self) -> Reply {
        let mut line = Vec::new();
        self.0.read_until(b'\n', &mut line).unwrap();
        assert!(line.ends_with(b"\r\n"), "invalid response: {line:?}");
        line.truncate(line.len() - 2);
        let value = &line[1..];
        match line[0] {
            b'+' => Reply::Simple(value.to_vec()),
            b'-' => Reply::Error(value.to_vec()),
            b':' => Reply::Integer(std::str::from_utf8(value).unwrap().parse().unwrap()),
            b'$' => {
                let len = std::str::from_utf8(value).unwrap().parse::<i64>().unwrap();
                if len == -1 {
                    return Reply::Bulk(None);
                }
                let mut bytes = vec![0; len as usize + 2];
                self.0.read_exact(&mut bytes).unwrap();
                assert!(bytes.ends_with(b"\r\n"));
                bytes.truncate(len as usize);
                Reply::Bulk(Some(bytes))
            }
            b'*' => {
                let len = std::str::from_utf8(value).unwrap().parse::<i64>().unwrap();
                if len == -1 {
                    return Reply::Array(None);
                }
                Reply::Array(Some((0..len).map(|_| self.read()).collect()))
            }
            _ => panic!("invalid response {line:?}"),
        }
    }
}

pub fn ok() -> Reply {
    Reply::Simple(b"OK".to_vec())
}
pub fn queued() -> Reply {
    Reply::Simple(b"QUEUED".to_vec())
}
pub fn integer(value: i64) -> Reply {
    Reply::Integer(value)
}
pub fn array(values: Vec<Reply>) -> Reply {
    Reply::Array(Some(values))
}
