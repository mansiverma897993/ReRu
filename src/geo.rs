// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, remove_expired, sorted_sets::SortedSet, Entry, Store, Value};
use std::f64::consts::PI;

const LATITUDE_LIMIT: f64 = 85.05112878;
const GRID: f64 = (1u64 << 26) as f64;
const EARTH_RADIUS_METERS: f64 = 6_372_797.560856;
const BASE32: &[u8] = b"0123456789bcdefghjkmnpqrstuvwxyz";

#[derive(Clone, Copy)]
enum Command {
    Add,
    Position,
    Distance,
    Hash,
    Search,
    SearchStore,
}

fn parse(name: &[u8]) -> Option<Command> {
    if name.eq_ignore_ascii_case(b"GEOADD") {
        Some(Command::Add)
    } else if name.eq_ignore_ascii_case(b"GEOPOS") {
        Some(Command::Position)
    } else if name.eq_ignore_ascii_case(b"GEODIST") {
        Some(Command::Distance)
    } else if name.eq_ignore_ascii_case(b"GEOHASH") {
        Some(Command::Hash)
    } else if name.eq_ignore_ascii_case(b"GEOSEARCH") {
        Some(Command::Search)
    } else if name.eq_ignore_ascii_case(b"GEOSEARCHSTORE") {
        Some(Command::SearchStore)
    } else {
        None
    }
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| match kind {
        Command::Add => command.len() >= 5,
        Command::Position | Command::Hash => command.len() >= 3,
        Command::Distance => command.len() == 4 || command.len() == 5,
        Command::Search => command.len() >= 7,
        Command::SearchStore => command.len() >= 8,
    })
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    matches!(
        parse(&command[0]),
        Some(Command::Add | Command::SearchStore)
    )
}

fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}

fn coordinate(bytes: &[u8]) -> Option<f64> {
    std::str::from_utf8(bytes)
        .ok()?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn encode(longitude: f64, latitude: f64) -> u64 {
    let x = (((longitude + 180.0) / 360.0) * GRID)
        .floor()
        .min(GRID - 1.0) as u64;
    let y = (((latitude + LATITUDE_LIMIT) / (2.0 * LATITUDE_LIMIT)) * GRID)
        .floor()
        .min(GRID - 1.0) as u64;
    let mut score = 0;
    for bit in (0..26).rev() {
        score = (score << 1) | ((x >> bit) & 1);
        score = (score << 1) | ((y >> bit) & 1);
    }
    score
}

fn decode(score: f64) -> Option<(f64, f64)> {
    if !score.is_finite() || score < 0.0 || score >= (1u64 << 52) as f64 || score.fract() != 0.0 {
        return None;
    }
    let score = score as u64;
    let mut x = 0u64;
    let mut y = 0u64;
    for bit in (0..26).rev() {
        x = (x << 1) | ((score >> (bit * 2 + 1)) & 1);
        y = (y << 1) | ((score >> (bit * 2)) & 1);
    }
    let longitude_step = 360.0 / GRID;
    let latitude_step = 2.0 * LATITUDE_LIMIT / GRID;
    let longitude_min = -180.0 + x as f64 * longitude_step;
    let longitude_max = -180.0 + (x + 1) as f64 * longitude_step;
    let latitude_min = -LATITUDE_LIMIT + y as f64 * latitude_step;
    let latitude_max = -LATITUDE_LIMIT + (y + 1) as f64 * latitude_step;
    Some((
        (longitude_min + longitude_max) / 2.0,
        (latitude_min + latitude_max) / 2.0,
    ))
}

fn geohash(longitude: f64, latitude: f64) -> Vec<u8> {
    let (mut min_lon, mut max_lon) = (-180.0, 180.0);
    let (mut min_lat, mut max_lat) = (-90.0, 90.0);
    let mut bits = 0u64;
    for index in 0..50 {
        bits <<= 1;
        if index % 2 == 0 {
            let middle = (min_lon + max_lon) / 2.0;
            if longitude >= middle {
                bits |= 1;
                min_lon = middle;
            } else {
                max_lon = middle;
            }
        } else {
            let middle = (min_lat + max_lat) / 2.0;
            if latitude >= middle {
                bits |= 1;
                min_lat = middle;
            } else {
                max_lat = middle;
            }
        }
    }
    let mut text = Vec::with_capacity(11);
    for shift in (0..10).rev() {
        text.push(BASE32[((bits >> (shift * 5)) & 31) as usize]);
    }
    text.push(b'0');
    text
}

fn coordinate_text(value: f64) -> Vec<u8> {
    let text = format!("{value:.17}");
    text.trim_end_matches('0')
        .trim_end_matches('.')
        .as_bytes()
        .to_vec()
}

fn add(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let mut nx = false;
    let mut xx = false;
    let mut ch = false;
    let mut start = 2;
    while start < command.len() {
        if command[start].eq_ignore_ascii_case(b"NX") && !nx {
            nx = true;
        } else if command[start].eq_ignore_ascii_case(b"XX") && !xx {
            xx = true;
        } else if command[start].eq_ignore_ascii_case(b"CH") && !ch {
            ch = true;
        } else {
            break;
        }
        start += 1;
    }
    if nx && xx || start == command.len() || !(command.len() - start).is_multiple_of(3) {
        return protocol::error("syntax error");
    }
    let mut points = Vec::new();
    for triple in command[start..].as_chunks::<3>().0 {
        let (Some(longitude), Some(latitude)) = (coordinate(&triple[0]), coordinate(&triple[1]))
        else {
            return protocol::error("value is not a valid float");
        };
        if !(-180.0..=180.0).contains(&longitude)
            || !(-LATITUDE_LIMIT..=LATITUDE_LIMIT).contains(&latitude)
        {
            return protocol::error("invalid longitude,latitude pair");
        }
        points.push((triple[2].clone(), encode(longitude, latitude) as f64));
    }
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    match entries.get(key).map(|entry| &entry.value) {
        Some(Value::SortedSet(_)) | None => {}
        Some(_) => return wrong_type(),
    }
    let mut added = 0;
    let mut changed = 0;
    for (member, score) in points {
        let old = entries.get(key).and_then(|entry| match &entry.value {
            Value::SortedSet(scores) => scores.get(&member),
            _ => None,
        });
        if (old.is_some() && nx) || (old.is_none() && xx) {
            continue;
        }
        if old.is_none() {
            added += 1;
        }
        if old != Some(score) {
            changed += 1;
        }
        let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
            value: Value::SortedSet(SortedSet::default()),
            expires_at: None,
        });
        let Value::SortedSet(scores) = &mut entry.value else {
            unreachable!();
        };
        scores.insert(member, score);
    }
    if changed > 0 {
        mark_modified(store, key);
    }
    format!(":{}\r\n", if ch { changed } else { added }).into_bytes()
}

fn position_or_hash(command: &[Vec<u8>], store: &Store, hash: bool) -> Vec<u8> {
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &command[1]);
    let scores = match entries.get(&command[1]).map(|entry| &entry.value) {
        Some(Value::SortedSet(scores)) => Some(scores),
        Some(_) => return wrong_type(),
        None => None,
    };
    let mut reply = format!("*{}\r\n", command.len() - 2).into_bytes();
    for member in &command[2..] {
        let position = scores
            .and_then(|scores| scores.get(member))
            .and_then(decode);
        match position {
            Some((longitude, latitude)) if hash => {
                reply.extend(protocol::bulk(&geohash(longitude, latitude)));
            }
            Some((longitude, latitude)) => {
                reply.extend(b"*2\r\n");
                reply.extend(protocol::bulk(&coordinate_text(longitude)));
                reply.extend(protocol::bulk(&coordinate_text(latitude)));
            }
            None => reply.extend(b"$-1\r\n"),
        }
    }
    reply
}

fn distance(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let factor = if command.len() == 4 || command[4].eq_ignore_ascii_case(b"M") {
        1.0
    } else if command[4].eq_ignore_ascii_case(b"KM") {
        1_000.0
    } else if command[4].eq_ignore_ascii_case(b"FT") {
        0.3048
    } else if command[4].eq_ignore_ascii_case(b"MI") {
        1609.34
    } else {
        return protocol::error("unsupported unit provided. please use M, KM, FT, MI");
    };
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, &command[1]);
    let scores = match entries.get(&command[1]).map(|entry| &entry.value) {
        Some(Value::SortedSet(scores)) => scores,
        Some(_) => return wrong_type(),
        None => return b"$-1\r\n".to_vec(),
    };
    let positions: Option<Vec<_>> = command[2..4]
        .iter()
        .map(|member| scores.get(member).and_then(decode))
        .collect();
    let Some(positions) = positions else {
        return b"$-1\r\n".to_vec();
    };
    let meters = distance_meters(positions[0], positions[1]);
    protocol::bulk(format!("{:.4}", meters / factor).as_bytes())
}

enum Origin {
    Member(Vec<u8>),
    Coordinates(f64, f64),
}

enum Shape {
    Radius(f64),
    Box(f64, f64),
}

struct SearchOptions {
    origin: Origin,
    shape: Shape,
    factor: f64,
    descending: Option<bool>,
    count: Option<usize>,
    with_distance: bool,
    with_hash: bool,
    with_coordinates: bool,
    store_distance: bool,
}

fn unit_factor(unit: &[u8]) -> Option<f64> {
    if unit.eq_ignore_ascii_case(b"M") {
        Some(1.0)
    } else if unit.eq_ignore_ascii_case(b"KM") {
        Some(1_000.0)
    } else if unit.eq_ignore_ascii_case(b"FT") {
        Some(0.3048)
    } else if unit.eq_ignore_ascii_case(b"MI") {
        Some(1609.34)
    } else {
        None
    }
}

fn positive_number(bytes: &[u8]) -> Result<f64, Vec<u8>> {
    coordinate(bytes)
        .filter(|value| *value > 0.0)
        .ok_or_else(|| protocol::error("value is not a valid float"))
}

fn parse_search(command: &[Vec<u8>], stored: bool) -> Result<SearchOptions, Vec<u8>> {
    let mut at = if stored { 3 } else { 2 };
    let origin = if command
        .get(at)
        .is_some_and(|arg| arg.eq_ignore_ascii_case(b"FROMMEMBER"))
    {
        let member = command
            .get(at + 1)
            .ok_or_else(|| protocol::error("syntax error"))?;
        at += 2;
        Origin::Member(member.clone())
    } else if command
        .get(at)
        .is_some_and(|arg| arg.eq_ignore_ascii_case(b"FROMLONLAT"))
    {
        let longitude = command.get(at + 1).and_then(|value| coordinate(value));
        let latitude = command.get(at + 2).and_then(|value| coordinate(value));
        let (Some(longitude), Some(latitude)) = (longitude, latitude) else {
            return Err(protocol::error("value is not a valid float"));
        };
        if !(-180.0..=180.0).contains(&longitude)
            || !(-LATITUDE_LIMIT..=LATITUDE_LIMIT).contains(&latitude)
        {
            return Err(protocol::error("invalid longitude,latitude pair"));
        }
        at += 3;
        Origin::Coordinates(longitude, latitude)
    } else {
        return Err(protocol::error("syntax error"));
    };
    let shape_name = command
        .get(at)
        .ok_or_else(|| protocol::error("syntax error"))?;
    let (shape, factor) = if shape_name.eq_ignore_ascii_case(b"BYRADIUS") {
        let radius = positive_number(
            command
                .get(at + 1)
                .ok_or_else(|| protocol::error("syntax error"))?,
        )?;
        let factor = unit_factor(
            command
                .get(at + 2)
                .ok_or_else(|| protocol::error("syntax error"))?,
        )
        .ok_or_else(|| protocol::error("unsupported unit provided. please use M, KM, FT, MI"))?;
        at += 3;
        (Shape::Radius(radius * factor), factor)
    } else if shape_name.eq_ignore_ascii_case(b"BYBOX") {
        let width = positive_number(
            command
                .get(at + 1)
                .ok_or_else(|| protocol::error("syntax error"))?,
        )?;
        let height = positive_number(
            command
                .get(at + 2)
                .ok_or_else(|| protocol::error("syntax error"))?,
        )?;
        let factor = unit_factor(
            command
                .get(at + 3)
                .ok_or_else(|| protocol::error("syntax error"))?,
        )
        .ok_or_else(|| protocol::error("unsupported unit provided. please use M, KM, FT, MI"))?;
        at += 4;
        (Shape::Box(width * factor, height * factor), factor)
    } else {
        return Err(protocol::error("syntax error"));
    };
    let mut options = SearchOptions {
        origin,
        shape,
        factor,
        descending: None,
        count: None,
        with_distance: false,
        with_hash: false,
        with_coordinates: false,
        store_distance: false,
    };
    while at < command.len() {
        let arg = &command[at];
        if (arg.eq_ignore_ascii_case(b"ASC") || arg.eq_ignore_ascii_case(b"DESC"))
            && options.descending.is_none()
        {
            options.descending = Some(arg.eq_ignore_ascii_case(b"DESC"));
            at += 1;
        } else if arg.eq_ignore_ascii_case(b"COUNT") && options.count.is_none() {
            let count = command
                .get(at + 1)
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| protocol::error("COUNT must be greater than 0"))?;
            options.count = Some(count);
            at += 2;
            if command
                .get(at)
                .is_some_and(|arg| arg.eq_ignore_ascii_case(b"ANY"))
            {
                at += 1;
            }
        } else if !stored && arg.eq_ignore_ascii_case(b"WITHDIST") && !options.with_distance {
            options.with_distance = true;
            at += 1;
        } else if !stored && arg.eq_ignore_ascii_case(b"WITHHASH") && !options.with_hash {
            options.with_hash = true;
            at += 1;
        } else if !stored && arg.eq_ignore_ascii_case(b"WITHCOORD") && !options.with_coordinates {
            options.with_coordinates = true;
            at += 1;
        } else if stored && arg.eq_ignore_ascii_case(b"STOREDIST") && !options.store_distance {
            options.store_distance = true;
            at += 1;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    Ok(options)
}

fn distance_meters(first: (f64, f64), second: (f64, f64)) -> f64 {
    let (lon1, lat1) = first;
    let (lon2, lat2) = second;
    let factor = PI / 180.0;
    let lon1 = lon1 * factor;
    let lon2 = lon2 * factor;
    let horizontal = ((lon2 - lon1) / 2.0).sin();
    let lat1 = lat1 * factor;
    let lat2 = lat2 * factor;
    if horizontal == 0.0 {
        return EARTH_RADIUS_METERS * (lat2 - lat1).abs();
    }
    let vertical = ((lat2 - lat1) / 2.0).sin();
    let haversine = vertical * vertical + lat1.cos() * lat2.cos() * horizontal * horizontal;
    2.0 * EARTH_RADIUS_METERS * haversine.sqrt().asin()
}

struct SearchResult {
    member: Vec<u8>,
    hash: f64,
    location: (f64, f64),
    meters: f64,
}

fn search(command: &[Vec<u8>], store: &Store, stored: bool) -> Vec<u8> {
    let options = match parse_search(command, stored) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let source = if stored { &command[2] } else { &command[1] };
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, source);
    let scores = match entries.get(source).map(|entry| &entry.value) {
        Some(Value::SortedSet(scores)) => Some(scores),
        Some(_) => return wrong_type(),
        None => None,
    };
    let center = match &options.origin {
        Origin::Coordinates(lon, lat) => Some((*lon, *lat)),
        Origin::Member(member) => scores
            .and_then(|scores| scores.get(member))
            .and_then(decode),
    };
    let mut results = Vec::new();
    if let (Some(scores), Some(center)) = (scores, center) {
        for (member, hash) in scores.entries() {
            let Some(location) = decode(hash) else {
                continue;
            };
            let meters = distance_meters(center, location);
            let in_shape = match options.shape {
                Shape::Radius(radius) => meters <= radius,
                Shape::Box(width, height) => {
                    let east_west = distance_meters(location, (center.0, location.1));
                    let north_south =
                        EARTH_RADIUS_METERS * ((location.1 - center.1) * PI / 180.0).abs();
                    east_west <= width / 2.0 && north_south <= height / 2.0
                }
            };
            if in_shape {
                results.push(SearchResult {
                    member: member.clone(),
                    hash,
                    location,
                    meters,
                });
            }
        }
    }
    if let Some(descending) = options.descending {
        results.sort_by(|a, b| {
            let order = a
                .meters
                .total_cmp(&b.meters)
                .then_with(|| a.member.cmp(&b.member));
            if descending {
                order.reverse()
            } else {
                order
            }
        });
    }
    if let Some(count) = options.count {
        results.truncate(count);
    }
    if stored {
        let destination = &command[1];
        let had_destination = entries.remove(destination).is_some();
        if !results.is_empty() {
            let mut output = SortedSet::default();
            for result in &results {
                let score = if options.store_distance {
                    result.meters / options.factor
                } else {
                    result.hash
                };
                output.insert(result.member.clone(), score);
            }
            entries.insert(
                destination.clone(),
                Entry {
                    value: Value::SortedSet(output),
                    expires_at: None,
                },
            );
        }
        if had_destination || !results.is_empty() {
            mark_modified(store, destination);
        }
        return format!(":{}\r\n", results.len()).into_bytes();
    }
    let mut reply = format!("*{}\r\n", results.len()).into_bytes();
    let decorated = options.with_distance || options.with_hash || options.with_coordinates;
    for result in results {
        if decorated {
            reply.extend(
                format!(
                    "*{}\r\n",
                    1 + usize::from(options.with_distance)
                        + usize::from(options.with_hash)
                        + usize::from(options.with_coordinates)
                )
                .as_bytes(),
            );
        }
        reply.extend(protocol::bulk(&result.member));
        if options.with_distance {
            reply.extend(protocol::bulk(
                format!("{:.4}", result.meters / options.factor).as_bytes(),
            ));
        }
        if options.with_hash {
            reply.extend(format!(":{}\r\n", result.hash as u64).as_bytes());
        }
        if options.with_coordinates {
            reply.extend(b"*2\r\n");
            reply.extend(protocol::bulk(&coordinate_text(result.location.0)));
            reply.extend(protocol::bulk(&coordinate_text(result.location.1)));
        }
    }
    reply
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = parse(&command[0])?;
    if valid_arity(command) == Some(false) {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    Some(match kind {
        Command::Add => add(command, store),
        Command::Position => position_or_hash(command, store, false),
        Command::Hash => position_or_hash(command, store, true),
        Command::Distance => distance(command, store),
        Command::Search => search(command, store, false),
        Command::SearchStore => search(command, store, true),
    })
}
