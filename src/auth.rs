// SPDX-License-Identifier: AGPL-3.0-only
//! Startup-configured default-user authentication. ACL policies and runtime
//! credential changes are intentionally outside this initial implementation.
use crate::protocol;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use subtle::ConstantTimeEq;

pub struct Settings {
    password: Option<Vec<u8>>,
}

impl Settings {
    pub fn new(password: Option<Vec<u8>>) -> Self {
        Self {
            password: password.filter(|bytes| !bytes.is_empty()),
        }
    }

    pub fn required(&self) -> bool {
        self.password.is_some()
    }

    pub fn authenticate(&self, command: &[Vec<u8>], authenticated: &mut bool) -> Vec<u8> {
        if !(2..=3).contains(&command.len()) {
            return protocol::error("wrong number of arguments for 'auth' command");
        }
        if command.len() == 2 && self.password.is_none() {
            return protocol::error("AUTH <password> called without any password configured for the default user. Are you sure your configuration is correct?");
        }
        let username_ok = command.len() == 2 || command[1] == b"default";
        let supplied = command.last().unwrap();
        let password_ok = self
            .password
            .as_ref()
            .is_none_or(|expected| bool::from(expected.as_slice().ct_eq(supplied.as_slice())));
        if username_ok && password_ok {
            *authenticated = true;
            b"+OK\r\n".to_vec()
        } else {
            // Redis preserves an already-authenticated session on failed AUTH.
            b"-WRONGPASS invalid username-password pair or user is disabled.\r\n".to_vec()
        }
    }
}

pub fn read_password_file(path: &Path) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "password file exceeds 4096 bytes",
        ));
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    Ok(bytes)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    if command[0].eq_ignore_ascii_case(b"AUTH") {
        Some((2..=3).contains(&command.len()))
    } else if command[0].eq_ignore_ascii_case(b"ACL") {
        Some(command.len() == 2 && command[1].eq_ignore_ascii_case(b"WHOAMI"))
    } else {
        None
    }
}

pub fn acl(command: &[Vec<u8>]) -> Vec<u8> {
    if valid_arity(command) == Some(true) {
        protocol::bulk(b"default")
    } else {
        protocol::error(
            "unsupported ACL subcommand or wrong number of arguments; supported: WHOAMI",
        )
    }
}
