// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
mod common;

use client::{integer, ok, Client, Reply};

#[test]
fn commands_with_more_than_1024_arguments_are_accepted() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let mut set = vec![b"MSET".to_vec()];
    for index in 0..512 {
        set.push(format!("bulk-key-{index}").into_bytes());
        set.push(b"value".to_vec());
    }
    assert_eq!(set.len(), 1025);
    assert_eq!(c.call_many(&set), ok());
    assert_eq!(
        c.call(&[b"GET", b"bulk-key-511"]),
        Reply::Bulk(Some(b"value".to_vec()))
    );

    let mut delete = vec![b"DEL".to_vec()];
    for index in 0..1100 {
        delete.push(format!("missing-{index}").into_bytes());
    }
    assert_eq!(c.call_many(&delete), integer(0));
}

#[test]
fn values_above_the_old_64_mib_limit_are_accepted() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let value = vec![b'x'; 70 * 1024 * 1024];
    assert_eq!(c.call(&[b"SET", b"large", &value]), ok());
    assert_eq!(c.call(&[b"STRLEN", b"large"]), integer(value.len() as i64));
    assert_eq!(c.call(&[b"GET", b"large"]), Reply::Bulk(Some(value)));
}
