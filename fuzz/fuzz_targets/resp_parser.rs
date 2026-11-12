// SPDX-License-Identifier: AGPL-3.0-only
#![no_main]

//! Drives the RESP2 command parser with arbitrary bytes.
//!
//! The parser is the first code that reads untrusted input from a socket, so
//! every accepted command is checked against the same limits the server
//! enforces: at most `MAX_ARGS` arguments and at most `MAX_BULK` bytes per
//! command.

use std::io::Cursor;

use libfuzzer_sys::fuzz_target;
use reru::protocol::{self, MAX_ARGS, MAX_BULK};

/// Parse at most this many commands per input. A valid frame may be followed by
/// more frames; parsing a bounded number keeps each iteration short.
const MAX_COMMANDS_PER_INPUT: usize = 8;

fuzz_target!(|data: &[u8]| {
    let mut reader = Cursor::new(data);

    for _ in 0..MAX_COMMANDS_PER_INPUT {
        match protocol::read_command(&mut reader) {
            Ok(Some(args)) => {
                assert!(!args.is_empty(), "parser returned an empty command");
                assert!(
                    args.len() <= MAX_ARGS,
                    "parser accepted {} arguments, over MAX_ARGS",
                    args.len()
                );
                for arg in &args {
                    assert!(
                        arg.len() <= MAX_BULK,
                        "parser accepted a {} byte argument, over MAX_BULK",
                        arg.len()
                    );
                }
                let command_bytes: usize = args.iter().map(Vec::len).sum();
                assert!(
                    command_bytes <= MAX_BULK,
                    "parser accepted a {command_bytes} byte command, over MAX_BULK"
                );
            }
            Ok(None) | Err(_) => break,
        }
    }

    // Reply helpers must also survive arbitrary input.
    let _ = protocol::bulk(data);
    let _ = protocol::array(&[data.to_vec()]);
    let _ = protocol::error(&String::from_utf8_lossy(data));
});
