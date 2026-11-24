// SPDX-License-Identifier: AGPL-3.0-only
//! Per-socket RESP2 negotiation and client metadata. No dataset mutations.
use crate::{protocol, Store};
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct Session {
    id: u64,
    name: Vec<u8>,
    library: Vec<u8>,
    library_version: Vec<u8>,
    peer: SocketAddr,
    local: SocketAddr,
    created: Instant,
    reply_mode: ReplyMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ReplyMode {
    On,
    Off,
    Skip,
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    Some(match command[0].to_ascii_uppercase().as_slice() {
        b"HELLO" => true,
        b"SELECT" => command.len() == 2,
        b"RESET" => command.len() == 1,
        b"CLIENT" => match command.get(1).map(|s| s.to_ascii_uppercase()).as_deref() {
            Some(b"ID" | b"GETNAME" | b"INFO") => command.len() == 2,
            Some(b"SETNAME") => command.len() == 3,
            Some(b"SETINFO") => command.len() == 4,
            Some(b"REPLY") => command.len() == 3,
            _ => false,
        },
        _ => return None,
    })
}

fn valid_text(value: &[u8]) -> bool {
    value.iter().all(|byte| (33..=126).contains(byte))
}
fn invalid_name() -> Vec<u8> {
    protocol::error("Client names cannot contain spaces, newlines or special characters.")
}

impl Session {
    pub fn new(stream: &TcpStream) -> io::Result<Self> {
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            name: Vec::new(),
            library: Vec::new(),
            library_version: Vec::new(),
            peer: stream.peer_addr()?,
            local: stream.local_addr()?,
            created: Instant::now(),
            reply_mode: ReplyMode::On,
        })
    }

    pub fn reset(&mut self) {
        // The socket ID and library identity survive RESET, as in Redis.
        self.name.clear();
        self.reply_mode = ReplyMode::On;
    }

    /// Consume a pending SKIP before dispatch. A valid CLIENT REPLY action can
    /// override this decision for its own reply (notably SKIP followed by ON).
    pub fn begin_command(&mut self) -> bool {
        match self.reply_mode {
            ReplyMode::On => true,
            ReplyMode::Off => false,
            ReplyMode::Skip => {
                self.reply_mode = ReplyMode::On;
                false
            }
        }
    }

    pub fn replies_off(&self) -> bool {
        self.reply_mode == ReplyMode::Off
    }

    pub fn is_reply_action(command: &[Vec<u8>]) -> bool {
        command.len() == 3
            && command[0].eq_ignore_ascii_case(b"CLIENT")
            && command[1].eq_ignore_ascii_case(b"REPLY")
            && matches!(
                command[2].to_ascii_uppercase().as_slice(),
                b"ON" | b"OFF" | b"SKIP"
            )
    }

    pub fn reply_for(&self, is_reply_action: bool, initially_enabled: bool) -> bool {
        if is_reply_action {
            self.reply_mode == ReplyMode::On
        } else {
            initially_enabled
        }
    }

    pub fn execute(
        &mut self,
        c: &[Vec<u8>],
        store: &Store,
        authenticated: &mut bool,
    ) -> Option<Vec<u8>> {
        let valid = valid_arity(c)?;
        if !valid {
            if c[0].eq_ignore_ascii_case(b"CLIENT")
                && c.get(1)
                    .is_some_and(|sub| sub.eq_ignore_ascii_case(b"REPLY"))
            {
                return Some(protocol::error(
                    "wrong number of arguments for 'client|reply' command",
                ));
            }
            return Some(protocol::error(
                "wrong number of arguments or unsupported connection subcommand",
            ));
        }
        let reply = if c[0].eq_ignore_ascii_case(b"HELLO") {
            self.hello(c, store, authenticated)
        } else if c[0].eq_ignore_ascii_case(b"SELECT") {
            match protocol::parse_int(&c[1]) {
                Some(0) => b"+OK\r\n".to_vec(),
                Some(_) => {
                    protocol::error("DB index is out of range (ReRu supports only database 0)")
                }
                None => protocol::error("value is not an integer or out of range"),
            }
        } else if c[0].eq_ignore_ascii_case(b"CLIENT") {
            match c[1].to_ascii_uppercase().as_slice() {
                b"ID" => format!(":{}\r\n", self.id).into_bytes(),
                b"GETNAME" => if self.name.is_empty() { b"$-1\r\n".to_vec() } else { protocol::bulk(&self.name) },
                b"SETNAME" => {
                    if !valid_text(&c[2]) { invalid_name() }
                    else { self.name.clone_from(&c[2]); b"+OK\r\n".to_vec() }
                }
                b"SETINFO" => {
                    let field = if c[2].eq_ignore_ascii_case(b"LIB-NAME") { Some(&mut self.library) }
                        else if c[2].eq_ignore_ascii_case(b"LIB-VER") { Some(&mut self.library_version) }
                        else { None };
                    match field {
                        None => protocol::error("Unrecognized CLIENT SETINFO option"),
                        Some(_) if !valid_text(&c[3]) => protocol::error("Client library information cannot contain spaces, newlines or special characters."),
                        Some(field) => { field.clone_from(&c[3]); b"+OK\r\n".to_vec() }
                    }
                }
                b"REPLY" => match c[2].to_ascii_uppercase().as_slice() {
                    b"ON" => { self.reply_mode = ReplyMode::On; b"+OK\r\n".to_vec() }
                    b"OFF" => { self.reply_mode = ReplyMode::Off; b"+OK\r\n".to_vec() }
                    b"SKIP" => {
                        // Redis ignores SKIP while replies are OFF.
                        if self.reply_mode != ReplyMode::Off { self.reply_mode = ReplyMode::Skip; }
                        b"+OK\r\n".to_vec()
                    }
                    _ => protocol::error("syntax error"),
                },
                b"INFO" => protocol::bulk(format!(
                    "id={} addr={} laddr={} name={} age={} db=0 user=default resp=2 lib-name={} lib-ver={}\n",
                    self.id, self.peer, self.local, String::from_utf8_lossy(&self.name),
                    self.created.elapsed().as_secs(), String::from_utf8_lossy(&self.library),
                    String::from_utf8_lossy(&self.library_version)).as_bytes()),
                _ => unreachable!(),
            }
        } else {
            // RESET is dispatched before transaction/subscription handling.
            protocol::error("RESET must execute in the connection handler")
        };
        Some(reply)
    }

    fn hello(&mut self, c: &[Vec<u8>], store: &Store, authenticated: &mut bool) -> Vec<u8> {
        if let Some(version) = c.get(1) {
            let version = protocol::parse_int(version);
            match version {
                Some(2) => {}
                Some(_) => return b"-NOPROTO unsupported protocol version\r\n".to_vec(),
                None => {
                    return protocol::error("Protocol version is not an integer or out of range")
                }
            }
        }
        let mut auth = None;
        let mut name = None;
        let mut at = 2;
        while at < c.len() {
            if c[at].eq_ignore_ascii_case(b"AUTH") && at + 2 < c.len() {
                auth = Some(vec![b"AUTH".to_vec(), c[at + 1].clone(), c[at + 2].clone()]);
                at += 3;
            } else if c[at].eq_ignore_ascii_case(b"SETNAME") && at + 1 < c.len() {
                if !valid_text(&c[at + 1]) {
                    return invalid_name();
                }
                name = Some(&c[at + 1]);
                at += 2;
            } else {
                return protocol::error("syntax error in HELLO option");
            }
        }
        // Validate all options before changing authentication or the name.
        if let Some(auth) = auth {
            let response = store.auth.authenticate(&auth, authenticated);
            if response.starts_with(b"-") {
                return response;
            }
        }
        if !*authenticated {
            return b"-NOAUTH HELLO must be called with the client already authenticated, otherwise use HELLO 2 AUTH default <password>.\r\n".to_vec();
        }
        if let Some(name) = name {
            self.name.clone_from(name);
        }
        let mut response = b"*14\r\n".to_vec();
        for (key, value) in [
            (b"server".as_slice(), protocol::bulk(b"reru")),
            (
                b"version",
                protocol::bulk(env!("CARGO_PKG_VERSION").as_bytes()),
            ),
            (b"proto", b":2\r\n".to_vec()),
            (b"id", format!(":{}\r\n", self.id).into_bytes()),
            (
                b"mode",
                protocol::bulk(if store.cluster.is_some() {
                    b"cluster"
                } else {
                    b"standalone"
                }),
            ),
            (
                b"role",
                protocol::bulk(if store.read_only {
                    b"replica"
                } else {
                    b"master"
                }),
            ),
            (b"modules", b"*0\r\n".to_vec()),
        ] {
            response.extend(protocol::bulk(key));
            response.extend(value);
        }
        response
    }
}
