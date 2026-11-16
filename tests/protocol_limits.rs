// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
mod common;

use client::{integer, ok, Client, Reply};

fn snapshot_path() -> std::path::PathBuf {
    common::temp_path("protocol-large", ".snap")
}

struct RemoveOnDrop(std::path::PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

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
    let value = vec![b'x'; 65 * 1024 * 1024];
    assert_eq!(c.call(&[b"SET", b"large", &value]), ok());
    assert_eq!(c.call(&[b"STRLEN", b"large"]), integer(value.len() as i64));
    assert_eq!(c.call(&[b"GET", b"large"]), Reply::Bulk(Some(value)));
}

#[test]
fn large_values_survive_a_snapshot_restart() {
    let path = snapshot_path();
    let _cleanup = RemoveOnDrop(path.clone());
    let filename = path.to_str().unwrap().to_owned();
    let value = vec![b'y'; 65 * 1024 * 1024];
    {
        let server = common::Server::start_with(&["--snapshot", &filename]);
        let mut c = Client::new(&server);
        // Snapshot mode persists synchronously before the reply.
        assert_eq!(c.call(&[b"SET", b"snap-large", &value]), ok());
    }
    {
        let server = common::Server::start_with(&["--snapshot", &filename]);
        let mut c = Client::new(&server);
        assert_eq!(
            c.call(&[b"STRLEN", b"snap-large"]),
            integer(value.len() as i64)
        );
    }
}
