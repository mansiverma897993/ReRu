// SPDX-License-Identifier: AGPL-3.0-only
use crate::{command_table, protocol, Entry};
use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;

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
}

impl Cluster {
    pub(crate) fn configure(specs: &[String], listener: SocketAddr) -> io::Result<Option<Self>> {
        if specs.is_empty() {
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
        Ok(Some(Self { nodes, local }))
    }

    pub(crate) fn route(&self, command: &[Vec<u8>]) -> Option<Vec<u8>> {
        self.route_commands(std::iter::once(command))
    }

    pub(crate) fn validate_keys(&self, entries: &HashMap<Vec<u8>, Entry>) -> io::Result<()> {
        if entries.keys().any(|key| {
            let slot = key_slot(key);
            let local = &self.nodes[self.local];
            slot < local.first || slot > local.last
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
    ) -> Option<Vec<u8>> {
        let mut slot = None;
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
            }
        }
        let slot = slot?;
        let owner = self
            .nodes
            .iter()
            .position(|node| node.first <= slot && slot <= node.last)?;
        (owner != self.local)
            .then(|| format!("-MOVED {slot} {}\r\n", self.nodes[owner].address).into_bytes())
    }

    pub(crate) fn execute(&self, command: &[Vec<u8>]) -> Vec<u8> {
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
        protocol::unknown_subcommand("CLUSTER", subcommand)
    }

    fn slots(&self) -> Vec<u8> {
        nested_array(self.nodes.iter().map(|node| {
            nested_array([
                integer(node.first),
                integer(node.last),
                nested_array([
                    protocol::bulk(node.address.ip().to_string().as_bytes()),
                    integer(node.address.port()),
                    protocol::bulk(node.id.as_bytes()),
                    b"*0\r\n".to_vec(),
                ]),
            ])
        }))
    }

    fn shards(&self) -> Vec<u8> {
        nested_array(self.nodes.iter().map(|node| {
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
                nested_array([integer(node.first), integer(node.last)]),
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
        _ => command.len() >= 2,
    })
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
