// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn geo_scores_positions_hashes_and_distances_match_redis_examples() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[
            b"GEOADD",
            b"cities",
            b"13.361389",
            b"38.115556",
            b"Palermo",
            b"15.087269",
            b"37.502669",
            b"Catania"
        ]),
        integer(2)
    );
    assert_eq!(
        c.call(&[b"TYPE", b"cities"]),
        Reply::Simple(b"zset".to_vec())
    );
    assert_eq!(
        c.call(&[b"ZSCORE", b"cities", b"Palermo"]),
        bulk(b"3479099956230698")
    );
    assert_eq!(
        c.call(&[b"GEOPOS", b"cities", b"Palermo", b"Catania", b"missing"]),
        array(vec![
            array(vec![
                bulk(b"13.36138933897018433"),
                bulk(b"38.11555639549629859")
            ]),
            array(vec![
                bulk(b"15.08726745843887329"),
                bulk(b"37.50266842333162032")
            ]),
            Reply::Bulk(None),
        ])
    );
    assert_eq!(
        c.call(&[b"GEOHASH", b"cities", b"Palermo", b"Catania", b"missing"]),
        array(vec![
            bulk(b"sqc8b49rny0"),
            bulk(b"sqdtr74hyu0"),
            Reply::Bulk(None)
        ])
    );
    assert_eq!(
        c.call(&[b"GEODIST", b"cities", b"Palermo", b"Catania"]),
        bulk(b"166274.1516")
    );
    assert_eq!(
        c.call(&[b"GEODIST", b"cities", b"Palermo", b"Catania", b"km"]),
        bulk(b"166.2742")
    );
    assert_eq!(
        c.call(&[b"GEODIST", b"cities", b"Palermo", b"missing"]),
        Reply::Bulk(None)
    );
}

#[test]
fn geo_add_conditions_expiry_and_invalid_coordinates() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"GEOADD", b"cities", b"XX", b"0", b"0", b"none"]),
        integer(0)
    );
    assert_eq!(
        c.call(&[b"TYPE", b"cities"]),
        Reply::Simple(b"none".to_vec())
    );
    assert_eq!(
        c.call(&[b"GEOADD", b"cities", b"0", b"0", b"origin"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"PEXPIRE", b"cities", b"60000"]), integer(1));
    assert_eq!(
        c.call(&[b"GEOADD", b"cities", b"NX", b"1", b"1", b"origin"]),
        integer(0)
    );
    assert_eq!(
        c.call(&[b"GEOADD", b"cities", b"XX", b"CH", b"1", b"1", b"origin"]),
        integer(1)
    );
    assert!(matches!(c.call(&[b"PTTL", b"cities"]), Reply::Integer(ttl) if ttl > 0));
    assert!(matches!(
        c.call(&[b"GEOADD", b"cities", b"0", b"91", b"bad"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"ZCARD", b"cities"]), integer(1));
    assert_eq!(c.call(&[b"SET", b"wrong", b"x"]), ok());
    assert!(matches!(
        c.call(&[b"GEOPOS", b"wrong", b"point"]),
        Reply::Error(_)
    ));
    assert_eq!(
        c.call(&[b"GEOHASH", b"missing", b"point"]),
        array(vec![Reply::Bulk(None)])
    );
}

#[test]
fn geo_search_returns_sorted_nearby_members_and_stores_results() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[
            b"GEOADD",
            b"cities",
            b"13.361389",
            b"38.115556",
            b"Palermo",
            b"15.087269",
            b"37.502669",
            b"Catania"
        ]),
        integer(2)
    );
    assert_eq!(
        c.call(&[
            b"GEOSEARCH",
            b"cities",
            b"FROMMEMBER",
            b"Palermo",
            b"BYRADIUS",
            b"200",
            b"km",
            b"ASC"
        ]),
        array(vec![bulk(b"Palermo"), bulk(b"Catania")])
    );
    assert_eq!(
        c.call(&[
            b"GEOSEARCH",
            b"cities",
            b"FROMLONLAT",
            b"13",
            b"38",
            b"BYBOX",
            b"500",
            b"500",
            b"km",
            b"ASC",
            b"COUNT",
            b"1"
        ]),
        array(vec![bulk(b"Palermo")])
    );
    assert_eq!(
        c.call(&[
            b"GEOSEARCH",
            b"cities",
            b"FROMMEMBER",
            b"Palermo",
            b"BYRADIUS",
            b"200",
            b"km",
            b"ASC",
            b"WITHDIST",
            b"WITHHASH",
            b"WITHCOORD"
        ]),
        array(vec![
            array(vec![
                bulk(b"Palermo"),
                bulk(b"0.0000"),
                integer(3479099956230698),
                array(vec![
                    bulk(b"13.36138933897018433"),
                    bulk(b"38.11555639549629859")
                ])
            ]),
            array(vec![
                bulk(b"Catania"),
                bulk(b"166.2742"),
                integer(3479447370796909),
                array(vec![
                    bulk(b"15.08726745843887329"),
                    bulk(b"37.50266842333162032")
                ])
            ]),
        ])
    );
    assert_eq!(
        c.call(&[
            b"GEOSEARCHSTORE",
            b"near",
            b"cities",
            b"FROMMEMBER",
            b"Palermo",
            b"BYRADIUS",
            b"200",
            b"km",
            b"ASC",
            b"STOREDIST"
        ]),
        integer(2)
    );
    assert_eq!(
        c.call(&[b"ZSCORE", b"near", b"Catania"]),
        bulk(b"166.27415156960032")
    );
    assert_eq!(c.call(&[b"PEXPIRE", b"near", b"60000"]), integer(1));
    assert_eq!(
        c.call(&[
            b"GEOSEARCHSTORE",
            b"near",
            b"cities",
            b"FROMMEMBER",
            b"Palermo",
            b"BYRADIUS",
            b"1",
            b"m"
        ]),
        integer(1)
    );
    assert_eq!(c.call(&[b"TTL", b"near"]), integer(-1));
    assert_eq!(c.call(&[b"ZCARD", b"near"]), integer(1));
}
