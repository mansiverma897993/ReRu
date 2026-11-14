// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;
use client::{array, integer, ok, queued, Client, Reply};
use common::Server;

fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}
fn command_entry(name: &[u8], arity: i64, keys: (i64, i64, i64), subcommands: Vec<Reply>) -> Reply {
    array(vec![
        bulk(name),
        integer(arity),
        array(vec![]),
        integer(keys.0),
        integer(keys.1),
        integer(keys.2),
        array(vec![]),
        array(vec![]),
        array(vec![]),
        array(subcommands),
    ])
}
fn reset() -> Reply {
    Reply::Simple(b"RESET".to_vec())
}
fn pong() -> Reply {
    Reply::Simple(b"PONG".to_vec())
}
fn noauth() -> Reply {
    Reply::Error(b"NOAUTH Authentication required.".to_vec())
}
fn field<'a>(reply: &'a Reply, name: &[u8]) -> &'a Reply {
    let Reply::Array(Some(fields)) = reply else {
        panic!("expected map: {reply:?}")
    };
    &fields
        .as_chunks::<2>()
        .0
        .iter()
        .find(|pair| pair[0] == bulk(name))
        .unwrap()[1]
}
fn info(c: &mut Client) -> String {
    let Reply::Bulk(Some(bytes)) = c.call(&[b"CLIENT", b"INFO"]) else {
        panic!()
    };
    String::from_utf8(bytes).unwrap()
}

#[test]
fn command_introspection_reports_dispatch_metadata_and_unknown_names_in_place() {
    let server = Server::start();
    let mut c = Client::new(&server);

    let all_info = c.call(&[b"COMMAND", b"INFO"]);
    assert_eq!(all_info, c.call(&[b"COMMAND"]));
    let Reply::Array(Some(commands)) = c.call(&[b"COMMAND"]) else {
        panic!("COMMAND must return an array")
    };
    assert_eq!(
        c.call(&[b"COMMAND", b"COUNT"]),
        integer(commands.len() as i64)
    );
    assert_eq!(
        c.call(&[b"COMMAND", b"INFO", b"GET"]),
        array(vec![command_entry(b"get", 2, (1, 1, 1), vec![])])
    );
    assert_eq!(
        c.call(&[b"COMMAND", b"INFO", b"CLIENT"]),
        array(vec![command_entry(
            b"client",
            -2,
            (0, 0, 0),
            vec![
                command_entry(b"client|getname", 2, (0, 0, 0), vec![]),
                command_entry(b"client|id", 2, (0, 0, 0), vec![]),
                command_entry(b"client|info", 2, (0, 0, 0), vec![]),
                command_entry(b"client|setinfo", 4, (0, 0, 0), vec![]),
                command_entry(b"client|setname", 3, (0, 0, 0), vec![]),
            ],
        )])
    );
    assert_eq!(
        c.call(&[b"COMMAND", b"INFO", b"GET", b"not-a-command", b"PING"]),
        array(vec![
            command_entry(b"get", 2, (1, 1, 1), vec![]),
            Reply::Bulk(None),
            command_entry(b"ping", -1, (0, 0, 0), vec![]),
        ])
    );
    assert!(commands
        .iter()
        .all(|command| { matches!(command, Reply::Array(Some(fields)) if fields.len() == 10) }));
    assert_eq!(
        c.call(&[b"COMMAND", b"FOO"]),
        Reply::Error(b"ERR unknown subcommand 'FOO'. Try COMMAND HELP.".to_vec())
    );
    assert_eq!(c.call(&[b"CONFIG", b"RESETSTAT"]), ok());
}

#[test]
fn connection_ids_names_library_metadata_and_resp2_handshake_are_isolated() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut other = Client::new(&server);
    let id = c.call(&[b"CLIENT", b"ID"]);
    assert!(matches!(id, Reply::Integer(n) if n > 0));
    assert_ne!(id, other.call(&[b"CLIENT", b"ID"]));
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), Reply::Bulk(None));
    assert_eq!(c.call(&[b"CLIENT", b"SETNAME", b"worker-1"]), ok());
    assert_eq!(
        c.call(&[b"CLIENT", b"SETINFO", b"lib-name", b"rust-client"]),
        ok()
    );
    assert_eq!(c.call(&[b"CLIENT", b"SETINFO", b"LIB-VER", b"1.2.3"]), ok());
    assert_eq!(other.call(&[b"CLIENT", b"GETNAME"]), Reply::Bulk(None));
    let metadata = info(&mut c);
    for expected in [
        "name=worker-1",
        "lib-name=rust-client",
        "lib-ver=1.2.3",
        "db=0",
        "resp=2",
        "user=default",
        "addr=127.0.0.1:",
    ] {
        assert!(metadata.contains(expected), "{metadata}");
    }
    let hello = c.call(&[b"HELLO", b"2", b"SETNAME", b"pool"]);
    assert_eq!(field(&hello, b"id"), &id);
    assert_eq!(field(&hello, b"server"), &bulk(b"reru"));
    assert_eq!(field(&hello, b"proto"), &integer(2));
    assert_eq!(field(&hello, b"modules"), &array(vec![]));
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), bulk(b"pool"));
    for bad in [
        b"space name".as_slice(),
        b"line\r\n",
        b"\0",
        b"\xff",
        b"\x7f",
    ] {
        assert!(matches!(
            c.call(&[b"CLIENT", b"SETNAME", bad]),
            Reply::Error(_)
        ));
        assert!(matches!(
            c.call(&[b"CLIENT", b"SETINFO", b"LIB-NAME", bad]),
            Reply::Error(_)
        ));
    }
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), bulk(b"pool"));
    assert!(info(&mut c).contains("lib-name=rust-client"));
    assert_eq!(c.call(&[b"RESET"]), reset());
    assert_eq!(c.call(&[b"CLIENT", b"ID"]), id);
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), Reply::Bulk(None));
    assert!(info(&mut c).contains("lib-ver=1.2.3"));
    c.call(&[b"CLIENT", b"SETINFO", b"LIB-NAME", b""]);
    assert!(info(&mut c).contains("lib-name= lib-ver="));
    assert_eq!(c.call(&[b"CLIENT", b"SETNAME", b""]), ok());
    assert_eq!(c.call(&[b"SELECT", b"0"]), ok());
    assert!(matches!(c.call(&[b"SELECT", b"1"]), Reply::Error(_)));
    assert!(matches!(c.call(&[b"SELECT", b"invalid"]), Reply::Error(_)));
    assert!(matches!(c.call(&[b"HELLO", b"3"]), Reply::Error(e) if e.starts_with(b"NOPROTO")));
    assert_eq!(field(&c.call(&[b"HELLO"]), b"proto"), &integer(2));
}

#[test]
fn hello_auth_validation_is_atomic_and_reset_requires_reauthentication() {
    let server = Server::start_with(&["--requirepass", "secret"]);
    let mut c = Client::new(&server);
    assert!(matches!(c.call(&[b"HELLO"]), Reply::Error(e) if e.starts_with(b"NOAUTH")));
    assert_eq!(c.call(&[b"CLIENT", b"ID"]), noauth());
    assert_eq!(c.call(&[b"RESET"]), reset());
    for command in [
        vec![
            b"HELLO".as_slice(),
            b"2",
            b"AUTH",
            b"default",
            b"secret",
            b"SETNAME",
            b"bad name",
        ],
        vec![
            b"HELLO".as_slice(),
            b"2",
            b"AUTH",
            b"default",
            b"secret",
            b"UNKNOWN",
        ],
        vec![b"HELLO".as_slice(), b"3", b"AUTH", b"default", b"secret"],
        vec![b"HELLO".as_slice(), b"2", b"AUTH", b"default"],
    ] {
        assert!(matches!(c.call(&command), Reply::Error(_)));
        assert_eq!(c.call(&[b"PING"]), noauth());
    }
    let hello = c.call(&[
        b"HELLO", b"2", b"AUTH", b"default", b"secret", b"SETNAME", b"authed",
    ]);
    let id = c.call(&[b"CLIENT", b"ID"]);
    assert_eq!(field(&hello, b"id"), &id);
    assert_eq!(c.call(&[b"PING"]), pong());
    assert!(
        matches!(c.call(&[b"HELLO", b"2", b"AUTH", b"default", b"wrong", b"SETNAME", b"changed"]), Reply::Error(e) if e.starts_with(b"WRONGPASS"))
    );
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), bulk(b"authed"));
    assert_eq!(c.call(&[b"PING"]), pong());
    assert_eq!(c.call(&[b"RESET"]), reset());
    assert_eq!(c.call(&[b"PING"]), noauth());
    assert_eq!(c.call(&[b"AUTH", b"secret"]), ok());
    assert_eq!(c.call(&[b"CLIENT", b"ID"]), id);
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), Reply::Bulk(None));
}

#[test]
fn reset_discards_transactions_errors_and_watch_registrations() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut other = Client::new(&server);
    c.call(&[b"WATCH", b"k"]);
    c.call(&[b"MULTI"]);
    c.call(&[b"SET", b"k", b"queued"]);
    assert!(matches!(c.call(&[b"UNKNOWN"]), Reply::Error(_)));
    assert_eq!(c.call(&[b"RESET"]), reset());
    assert_eq!(c.call(&[b"GET", b"k"]), Reply::Bulk(None));
    other.call(&[b"SET", b"k", b"v"]);
    c.call(&[b"MULTI"]);
    c.call(&[b"GET", b"k"]);
    assert_eq!(c.call(&[b"EXEC"]), array(vec![bulk(b"v")]));
    c.call(&[b"MULTI"]);
    assert!(matches!(c.call(&[b"RESET", b"invalid"]), Reply::Error(_)));
    assert!(matches!(c.call(&[b"EXEC"]), Reply::Error(e) if e.starts_with(b"EXECABORT")));
    assert_eq!(c.call(&[b"PING"]), pong());
}

#[test]
fn reset_leaves_pubsub_without_extra_replies_or_remaining_listeners() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut publisher = Client::new(&server);
    c.call(&[b"SUBSCRIBE", b"channel"]);
    c.call(&[b"PSUBSCRIBE", b"chan*"]);
    assert!(matches!(c.call(&[b"CLIENT", b"ID"]), Reply::Error(_)));
    assert_eq!(c.call(&[b"RESET"]), reset());
    assert_eq!(
        publisher.call(&[b"PUBLISH", b"channel", b"payload"]),
        integer(0)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(0));
    assert_eq!(c.call(&[b"PING"]), pong());
    assert_eq!(
        c.call(&[b"SUBSCRIBE", b"again"]),
        array(vec![bulk(b"subscribe"), bulk(b"again"), integer(1)])
    );
    assert_eq!(c.call(&[b"RESET"]), reset());
    assert_eq!(c.call(&[b"SET", b"after-reset", b"v"]), ok());
}

#[test]
fn connection_commands_queue_and_do_not_modify_watched_dataset_keys() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut other = Client::new(&server);
    c.call(&[b"WATCH", b"k"]);
    other.call(&[b"CLIENT", b"SETNAME", b"k"]);
    c.call(&[b"MULTI"]);
    assert_eq!(c.call(&[b"CLIENT", b"SETNAME", b"queued"]), queued());
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), queued());
    assert_eq!(c.call(&[b"SELECT", b"0"]), queued());
    assert_eq!(c.call(&[b"EXEC"]), array(vec![ok(), bulk(b"queued"), ok()]));
    c.call(&[b"MULTI"]);
    c.call(&[b"CLIENT", b"SETNAME", b"bad name"]);
    c.call(&[b"HELLO", b"2", b"SETNAME", b"after-error"]);
    let Reply::Array(Some(results)) = c.call(&[b"EXEC"]) else {
        panic!()
    };
    assert!(matches!(results[0], Reply::Error(_)));
    assert_eq!(field(&results[1], b"proto"), &integer(2));
    assert_eq!(c.call(&[b"CLIENT", b"GETNAME"]), bulk(b"after-error"));
    c.call(&[b"MULTI"]);
    assert!(matches!(c.call(&[b"CLIENT", b"SETNAME"]), Reply::Error(_)));
    assert!(matches!(c.call(&[b"EXEC"]), Reply::Error(e) if e.starts_with(b"EXECABORT")));
}

#[test]
fn replica_reports_role_and_accepts_connection_metadata() {
    let primary = Server::start();
    let replica = Server::start_with(&["--replicaof", "127.0.0.1", &primary.port.to_string()]);
    let mut c = Client::new(&replica);
    assert_eq!(field(&c.call(&[b"HELLO"]), b"role"), &bulk(b"slave"));
    assert_eq!(c.call(&[b"CLIENT", b"SETNAME", b"reader"]), ok());
    assert_eq!(c.call(&[b"SELECT", b"0"]), ok());
    assert_eq!(c.call(&[b"RESET"]), reset());
    assert!(matches!(c.call(&[b"SET", b"k", b"v"]), Reply::Error(e) if e.starts_with(b"READONLY")));
}

#[test]
fn common_connection_sequences_match_redis_reference() {
    let Ok(port) = std::env::var("RERU_REDIS_REFERENCE_PORT") else {
        return;
    };
    let server = Server::start();
    let mut ours = Client::new(&server);
    let mut redis = Client::on_port(port.parse().unwrap());
    for command in [
        vec![b"CLIENT".as_slice(), b"GETNAME"],
        vec![b"CLIENT".as_slice(), b"SETNAME", b"reference-client"],
        vec![b"CLIENT".as_slice(), b"GETNAME"],
        vec![b"SELECT".as_slice(), b"0"],
        vec![b"MULTI".as_slice()],
        vec![b"CLIENT".as_slice(), b"SETNAME", b"queued"],
        vec![b"CLIENT".as_slice(), b"GETNAME"],
        vec![b"EXEC".as_slice()],
        vec![b"RESET".as_slice()],
        vec![b"CLIENT".as_slice(), b"GETNAME"],
        vec![b"SUBSCRIBE".as_slice(), b"reru-diff:reset"],
        vec![b"RESET".as_slice()],
        vec![b"PING".as_slice()],
    ] {
        assert_eq!(ours.call(&command), redis.call(&command), "{command:?}");
    }
    let ours_hello = ours.call(&[b"HELLO", b"2", b"SETNAME", b"hello"]);
    let redis_hello = redis.call(&[b"HELLO", b"2", b"SETNAME", b"hello"]);
    for key in [b"proto".as_slice(), b"mode", b"role", b"modules"] {
        assert_eq!(field(&ours_hello, key), field(&redis_hello, key));
    }
    assert_eq!(field(&ours_hello, b"id"), &ours.call(&[b"CLIENT", b"ID"]));
    assert_eq!(field(&redis_hello, b"id"), &redis.call(&[b"CLIENT", b"ID"]));
}
