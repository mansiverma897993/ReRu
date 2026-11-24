// SPDX-License-Identifier: AGPL-3.0-only
use crate::{command_table, mark_modified, persistence, protocol, remove_expired, Entry, Store};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

const SLOT_COUNT: u16 = 16_384;

#[derive(Clone)]
struct Node {
    id: String,
    address: SocketAddr,
    first: u16,
    last: u16,
}

pub(crate) struct Cluster {
    nodes: [Node; 2],
    local: usize,
    state: Mutex<State>,
    path: Option<PathBuf>,
}

#[derive(Clone)]
struct State {
    owner: Vec<u8>,
    migrating: Vec<u8>,
    importing: Vec<u8>,
}

const STATE_MAGIC: &[u8] = b"RERUCLS1";
const NONE: u8 = 2;

impl Cluster {
    pub(crate) fn configure(
        specs: &[String],
        listener: SocketAddr,
        path: Option<PathBuf>,
    ) -> io::Result<Option<Self>> {
        if specs.is_empty() {
            if path.is_some() {
                return Err(invalid("--cluster-state requires --cluster-node"));
            }
            return Ok(None);
        }
        if specs.len() != 2 {
            return Err(invalid("exactly two --cluster-node entries are required"));
        }
        let mut nodes = [parse_node(&specs[0])?, parse_node(&specs[1])?];
        nodes.sort_by_key(|node| node.first);
        if nodes[0].first != 0
            || nodes[0].last.checked_add(1) != Some(nodes[1].first)
            || nodes[1].last != SLOT_COUNT - 1
            || nodes[0].id == nodes[1].id
            || nodes[0].address == nodes[1].address
        {
            return Err(invalid(
                "cluster slots must be complete and disjoint with unique nodes",
            ));
        }
        let local = nodes
            .iter()
            .position(|node| node.address == listener)
            .ok_or_else(|| invalid("one --cluster-node address must match the listener"))?;
        let mut state = State {
            owner: (0..SLOT_COUNT)
                .map(|slot| u8::from(slot > nodes[0].last))
                .collect(),
            migrating: vec![NONE; SLOT_COUNT as usize],
            importing: vec![NONE; SLOT_COUNT as usize],
        };
        if let Some(path) = &path {
            if path.exists() {
                let bytes = std::fs::read(path)?;
                if bytes.len() != STATE_MAGIC.len() + SLOT_COUNT as usize * 3
                    || !bytes.starts_with(STATE_MAGIC)
                {
                    return Err(invalid("invalid cluster state file"));
                }
                let chunks = bytes[STATE_MAGIC.len()..]
                    .chunks_exact(SLOT_COUNT as usize)
                    .collect::<Vec<_>>();
                state.owner.copy_from_slice(chunks[0]);
                state.migrating.copy_from_slice(chunks[1]);
                state.importing.copy_from_slice(chunks[2]);
                if state.owner.iter().any(|v| *v > 1)
                    || state.migrating.iter().any(|v| *v > NONE)
                    || state.importing.iter().any(|v| *v > NONE)
                {
                    return Err(invalid("invalid cluster state file"));
                }
            }
        }
        Ok(Some(Self {
            nodes,
            local,
            state: Mutex::new(state),
            path,
        }))
    }

    pub(crate) fn route(
        &self,
        command: &[Vec<u8>],
        entries: &HashMap<Vec<u8>, Entry>,
        asking: bool,
    ) -> Option<Vec<u8>> {
        self.route_commands(std::iter::once(command), entries, asking)
    }

    pub(crate) fn validate_keys(&self, entries: &HashMap<Vec<u8>, Entry>) -> io::Result<()> {
        let state = self.state.lock().unwrap();
        if entries.keys().any(|key| {
            let slot = key_slot(key) as usize;
            state.owner[slot] as usize != self.local && state.importing[slot] != state.owner[slot]
        }) {
            return Err(invalid(
                "persisted dataset contains a key outside local cluster slots",
            ));
        }
        Ok(())
    }

    pub(crate) fn route_commands<'a>(
        &self,
        commands: impl IntoIterator<Item = &'a [Vec<u8>]>,
        entries: &HashMap<Vec<u8>, Entry>,
        asking: bool,
    ) -> Option<Vec<u8>> {
        let mut slot = None;
        let mut keys = Vec::new();
        for command in commands {
            if command[0].eq_ignore_ascii_case(b"SELECT")
                && command.len() == 2
                && std::str::from_utf8(&command[1])
                    .ok()
                    .and_then(|index| index.parse::<i64>().ok())
                    .is_some_and(|index| index != 0)
            {
                return Some(protocol::error("SELECT is not allowed in cluster mode"));
            }
            let Some(indices) = command_table::key_indices(command) else {
                continue;
            };
            for index in indices {
                let current = key_slot(&command[index]);
                if slot.is_some_and(|previous| previous != current) {
                    return Some(
                        b"-CROSSSLOT Keys in request don't hash to the same slot\r\n".to_vec(),
                    );
                }
                slot = Some(current);
                keys.push(&command[index]);
            }
        }
        let slot = slot?;
        let state = self.state.lock().unwrap();
        let owner = state.owner[slot as usize] as usize;
        let present = keys
            .iter()
            .filter(|key| {
                entries.get(key.as_slice()).is_some_and(|entry| {
                    entry
                        .expires_at
                        .is_none_or(|at| at.at > std::time::Instant::now())
                })
            })
            .count();
        if owner == self.local {
            if state.migrating[slot as usize] != NONE {
                if present > 0 && present < keys.len() {
                    return Some(
                        b"-TRYAGAIN Multiple keys request during rehashing of slot\r\n".to_vec(),
                    );
                }
                if present == 0 {
                    let target = state.migrating[slot as usize] as usize;
                    return Some(
                        format!("-ASK {slot} {}\r\n", self.nodes[target].address).into_bytes(),
                    );
                }
            }
            None
        } else if asking && state.importing[slot as usize] == owner as u8 {
            None
        } else {
            Some(format!("-MOVED {slot} {}\r\n", self.nodes[owner].address).into_bytes())
        }
    }

    pub(crate) fn execute(&self, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
        let Some(subcommand) = command.get(1) else {
            return protocol::error("wrong number of arguments for 'cluster' command");
        };
        if subcommand.eq_ignore_ascii_case(b"KEYSLOT") {
            if command.len() != 3 {
                return protocol::error("wrong number of arguments for 'cluster|keyslot' command");
            }
            return format!(":{}\r\n", key_slot(&command[2])).into_bytes();
        }
        if subcommand.eq_ignore_ascii_case(b"SHARDS") {
            if command.len() != 2 {
                return protocol::error("wrong number of arguments for 'cluster|shards' command");
            }
            return self.shards();
        }
        if subcommand.eq_ignore_ascii_case(b"SLOTS") {
            if command.len() != 2 {
                return protocol::error("wrong number of arguments for 'cluster|slots' command");
            }
            return self.slots();
        }
        if subcommand.eq_ignore_ascii_case(b"SETSLOT") {
            return self.setslot(command, store);
        }
        protocol::unknown_subcommand("CLUSTER", subcommand)
    }

    fn save_state(&self, state: &State) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Err(invalid("--cluster-state is required for slot changes"));
        };
        let mut bytes = STATE_MAGIC.to_vec();
        bytes.extend(&state.owner);
        bytes.extend(&state.migrating);
        bytes.extend(&state.importing);
        let temp = path.with_extension("tmp");
        {
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    fn setslot(&self, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
        if command.len() < 4 {
            return protocol::error("wrong number of arguments for 'cluster|setslot' command");
        }
        let slot = command[2].as_slice();
        let Some(slot) = protocol::parse_int(slot)
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v < SLOT_COUNT as usize)
        else {
            return protocol::error("Invalid or out of range slot");
        };
        let mode = &command[3];
        let stable = mode.eq_ignore_ascii_case(b"STABLE");
        if (stable && command.len() != 4) || (!stable && command.len() != 5) {
            return protocol::error(
                "Invalid CLUSTER SETSLOT action or number of arguments. Try CLUSTER HELP",
            );
        }
        if !stable
            && ![b"MIGRATING".as_slice(), b"IMPORTING", b"NODE"]
                .iter()
                .any(|name| mode.eq_ignore_ascii_case(name))
        {
            return protocol::error(
                "Invalid CLUSTER SETSLOT action or number of arguments. Try CLUSTER HELP",
            );
        }
        if mode.eq_ignore_ascii_case(b"IMPORTING")
            && self.state.lock().unwrap().owner[slot] as usize == self.local
        {
            return protocol::error(&format!("I'm already the owner of hash slot {slot}"));
        }
        let node = if stable {
            None
        } else {
            match self
                .nodes
                .iter()
                .position(|n| n.id.as_bytes().eq_ignore_ascii_case(&command[4]))
            {
                Some(n) => Some(n),
                None => {
                    return protocol::error(&format!(
                        "{} {}",
                        if mode.eq_ignore_ascii_case(b"MIGRATING") {
                            "I don't know about node"
                        } else {
                            "Unknown node"
                        },
                        String::from_utf8_lossy(&command[4])
                    ))
                }
            }
        };
        let mut guard = self.state.lock().unwrap();
        let mut next = guard.clone();
        if stable {
            next.migrating[slot] = NONE;
            next.importing[slot] = NONE;
        } else if mode.eq_ignore_ascii_case(b"MIGRATING") {
            if next.owner[slot] as usize != self.local {
                return protocol::error("I'm not the owner of hash slot");
            }
            next.migrating[slot] = node.unwrap() as u8;
        } else if mode.eq_ignore_ascii_case(b"IMPORTING") {
            if next.owner[slot] as usize == self.local {
                return protocol::error(&format!("I'm already the owner of hash slot {slot}"));
            }
            next.importing[slot] = node.unwrap() as u8;
        } else if mode.eq_ignore_ascii_case(b"NODE") {
            if next.owner[slot] as usize == self.local
                && node.unwrap() != self.local
                && store
                    .entries
                    .lock()
                    .unwrap()
                    .keys()
                    .any(|key| key_slot(key) as usize == slot)
            {
                return protocol::error("Can't assign hash slot to a different node while I still hold keys for this hash slot");
            }
            next.owner[slot] = node.unwrap() as u8;
            next.migrating[slot] = NONE;
            next.importing[slot] = NONE;
        } else {
            return protocol::error("Invalid CLUSTER SETSLOT action");
        }
        if let Err(error) = self.save_state(&next) {
            return protocol::error(&error.to_string());
        }
        *guard = next;
        b"+OK\r\n".to_vec()
    }

    pub(crate) fn migrate(&self, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
        if command.len() != 6 {
            return protocol::error(
                "wrong number of arguments for 'migrate' command or unsupported option",
            );
        }
        let Ok(host) = std::str::from_utf8(&command[1]) else {
            return protocol::error("invalid target host");
        };
        let Some(port) = protocol::parse_int(&command[2])
            .and_then(|n| u16::try_from(n).ok())
            .filter(|n| *n > 0)
        else {
            return protocol::error("invalid target port");
        };
        if command[4] != b"0" {
            return protocol::error("MIGRATE only supports database 0");
        }
        let Some(timeout) = protocol::parse_int(&command[5])
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0)
        else {
            return protocol::error("invalid timeout");
        };
        let Some(target) = self
            .nodes
            .iter()
            .enumerate()
            .find(|(i, n)| {
                *i != self.local && n.address.ip().to_string() == host && n.address.port() == port
            })
            .map(|(i, _)| i)
        else {
            return protocol::error("MIGRATE target must be the configured peer");
        };
        let slot = key_slot(&command[3]) as usize;
        let state = self.state.lock().unwrap();
        if state.owner[slot] as usize != self.local || state.migrating[slot] != target as u8 {
            return protocol::error("slot is not migrating to target");
        }
        drop(state);
        let mut entries = store.entries.lock().unwrap();
        remove_expired(&mut entries, &command[3]);
        let Some(entry) = entries.get(&command[3]).cloned() else {
            return b"+NOKEY\r\n".to_vec();
        };
        drop(entries);
        let payload = match persistence::encode(&HashMap::from([(command[3].clone(), entry)])) {
            Ok(bytes) => bytes,
            Err(error) => return protocol::error(&error.to_string()),
        };
        let address = self.nodes[target].address;
        let duration = Duration::from_millis(timeout.min(30_000));
        let response = (|| -> io::Result<Vec<u8>> {
            let mut stream = TcpStream::connect_timeout(&address, duration)?;
            stream.set_read_timeout(Some(duration))?;
            stream.set_write_timeout(Some(duration))?;
            let args: [&[u8]; 4] = [
                b"__RERU_IMPORT",
                &command[3],
                &payload,
                self.nodes[self.local].id.as_bytes(),
            ];
            let mut frame = format!("*{}\r\n", args.len()).into_bytes();
            for arg in args {
                frame.extend(format!("${}\r\n", arg.len()).as_bytes());
                frame.extend(arg);
                frame.extend(b"\r\n");
            }
            stream.write_all(&frame)?;
            let mut reply = Vec::new();
            BufReader::new(stream).read_until(b'\n', &mut reply)?;
            Ok(reply)
        })();
        match response {
            Ok(reply) if reply == b"+OK\r\n" => {
                store.entries.lock().unwrap().remove(&command[3]);
                mark_modified(store, &command[3]);
                b"+OK\r\n".to_vec()
            }
            Ok(reply) if reply.starts_with(b"-") => reply,
            _ => b"-IOERR error or timeout writing to target instance\r\n".to_vec(),
        }
    }

    pub(crate) fn import(&self, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
        if command.len() != 4 {
            return protocol::error("wrong number of arguments for import");
        }
        let slot = key_slot(&command[1]) as usize;
        let state = self.state.lock().unwrap();
        let owner = state.owner[slot] as usize;
        if owner == self.local
            || state.importing[slot] != owner as u8
            || self.nodes[owner].id.as_bytes() != command[3]
        {
            return protocol::error("slot is not importing from source");
        }
        drop(state);
        let mut decoded = match persistence::decode(&command[2]) {
            Ok(decoded) => decoded,
            Err(_) => return protocol::error("invalid migration payload"),
        };
        if decoded.len() != 1 {
            return protocol::error("invalid migration payload");
        }
        let Some(entry) = decoded.remove(&command[1]) else {
            return protocol::error("invalid migration payload");
        };
        let mut entries = store.entries.lock().unwrap();
        remove_expired(&mut entries, &command[1]);
        if entries.contains_key(&command[1]) {
            return b"-BUSYKEY Target key name already exists.\r\n".to_vec();
        }
        entries.insert(command[1].clone(), entry);
        mark_modified(store, &command[1]);
        b"+OK\r\n".to_vec()
    }

    fn slots(&self) -> Vec<u8> {
        let state = self.state.lock().unwrap();
        nested_array(
            ranges(&state.owner)
                .into_iter()
                .map(|(first, last, owner)| {
                    let node = &self.nodes[owner];
                    nested_array([
                        integer(first),
                        integer(last),
                        nested_array([
                            protocol::bulk(node.address.ip().to_string().as_bytes()),
                            integer(node.address.port()),
                            protocol::bulk(node.id.as_bytes()),
                            b"*0\r\n".to_vec(),
                        ]),
                    ])
                }),
        )
    }

    fn shards(&self) -> Vec<u8> {
        let state = self.state.lock().unwrap();
        nested_array(self.nodes.iter().enumerate().map(|(index, node)| {
            let ip = node.address.ip().to_string();
            let node_info = nested_array([
                protocol::bulk(b"id"),
                protocol::bulk(node.id.as_bytes()),
                protocol::bulk(b"port"),
                integer(node.address.port()),
                protocol::bulk(b"ip"),
                protocol::bulk(ip.as_bytes()),
                protocol::bulk(b"endpoint"),
                protocol::bulk(ip.as_bytes()),
                protocol::bulk(b"role"),
                protocol::bulk(b"master"),
                protocol::bulk(b"replication-offset"),
                integer(0),
                protocol::bulk(b"health"),
                protocol::bulk(b"online"),
            ]);
            nested_array([
                protocol::bulk(b"slots"),
                nested_array(
                    ranges(&state.owner)
                        .into_iter()
                        .filter(|(_, _, owner)| *owner == index)
                        .flat_map(|(first, last, _)| [integer(first), integer(last)]),
                ),
                protocol::bulk(b"nodes"),
                nested_array([node_info]),
            ])
        }))
    }
}

pub(crate) fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    if !command.first()?.eq_ignore_ascii_case(b"CLUSTER") {
        return None;
    }
    Some(match command.get(1).map(Vec::as_slice) {
        Some(subcommand) if subcommand.eq_ignore_ascii_case(b"KEYSLOT") => command.len() == 3,
        Some(subcommand)
            if subcommand.eq_ignore_ascii_case(b"SHARDS")
                || subcommand.eq_ignore_ascii_case(b"SLOTS") =>
        {
            command.len() == 2
        }
        Some(subcommand) if subcommand.eq_ignore_ascii_case(b"SETSLOT") => command.len() >= 4,
        _ => command.len() >= 2,
    })
}

fn ranges(owner: &[u8]) -> Vec<(u16, u16, usize)> {
    let mut result = Vec::new();
    let mut first = 0;
    for slot in 1..=owner.len() {
        if slot == owner.len() || owner[slot] != owner[first] {
            result.push((first as u16, (slot - 1) as u16, owner[first] as usize));
            first = slot;
        }
    }
    result
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn parse_node(spec: &str) -> io::Result<Node> {
    let parts = spec.split('@').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0].len() != 40
        || !parts[0].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid(
            "--cluster-node must be ID@IP:PORT@START-END with a 40-hex ID",
        ));
    }
    let address = parts[1]
        .parse::<SocketAddr>()
        .map_err(|_| invalid("--cluster-node endpoint must be IP:PORT"))?;
    if address.port() == 0 || address.ip().is_unspecified() {
        return Err(invalid(
            "--cluster-node endpoint must be a nonzero, specific IP:PORT",
        ));
    }
    let (first, last) = parts[2]
        .split_once('-')
        .ok_or_else(|| invalid("--cluster-node slot range must be START-END"))?;
    let first = first
        .parse::<u16>()
        .map_err(|_| invalid("invalid cluster slot"))?;
    let last = last
        .parse::<u16>()
        .map_err(|_| invalid("invalid cluster slot"))?;
    if first > last || last >= SLOT_COUNT {
        return Err(invalid("cluster slot range must be within 0-16383"));
    }
    Ok(Node {
        id: parts[0].to_ascii_lowercase(),
        address,
        first,
        last,
    })
}

pub(crate) fn key_slot(key: &[u8]) -> u16 {
    let hash_input = key
        .iter()
        .position(|&byte| byte == b'{')
        .and_then(|start| {
            key[start + 1..]
                .iter()
                .position(|&byte| byte == b'}')
                .filter(|&end| end > 0)
                .map(|end| &key[start + 1..start + 1 + end])
        })
        .unwrap_or(key);
    let mut crc = 0u16;
    for &byte in hash_input {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc % SLOT_COUNT
}

fn integer(value: u16) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}

fn nested_array(parts: impl IntoIterator<Item = Vec<u8>>) -> Vec<u8> {
    let parts = parts.into_iter().collect::<Vec<_>>();
    let mut reply = format!("*{}\r\n", parts.len()).into_bytes();
    for part in parts {
        reply.extend(part);
    }
    reply
}
