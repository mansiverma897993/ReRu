// SPDX-License-Identifier: AGPL-3.0-only
use crate::{mark_modified, protocol, pubsub::glob_matches, remove_expired, Entry, Store, Value};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};

mod algebra;

#[derive(Clone)]
struct ScoredMember {
    score: f64,
    member: Vec<u8>,
}

impl PartialEq for ScoredMember {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for ScoredMember {}
impl PartialOrd for ScoredMember {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScoredMember {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.member.cmp(&other.member))
    }
}

#[derive(Clone, Default)]
pub struct SortedSet {
    by_member: HashMap<Vec<u8>, f64>,
    ordered: BTreeSet<ScoredMember>,
}

impl SortedSet {
    pub fn len(&self) -> usize {
        self.by_member.len()
    }
    pub fn is_empty(&self) -> bool {
        self.by_member.is_empty()
    }
    pub fn get(&self, member: &[u8]) -> Option<f64> {
        self.by_member.get(member).copied()
    }
    pub fn insert(&mut self, member: Vec<u8>, score: f64) -> Option<f64> {
        let score = if score == 0.0 { 0.0 } else { score };
        let previous = self.by_member.insert(member.clone(), score);
        if let Some(old) = previous {
            self.ordered.remove(&ScoredMember {
                score: old,
                member: member.clone(),
            });
        }
        self.ordered.insert(ScoredMember { score, member });
        previous
    }
    pub fn remove(&mut self, member: &[u8]) -> Option<f64> {
        let score = self.by_member.remove(member)?;
        self.ordered.remove(&ScoredMember {
            score,
            member: member.to_vec(),
        });
        Some(score)
    }
    pub fn rank(&self, member: &[u8], reverse: bool) -> Option<usize> {
        self.get(member)?;
        if reverse {
            self.ordered
                .iter()
                .rev()
                .position(|item| item.member == member)
        } else {
            self.ordered.iter().position(|item| item.member == member)
        }
    }
    pub fn entries(&self) -> impl DoubleEndedIterator<Item = (&Vec<u8>, f64)> {
        self.ordered.iter().map(|item| (&item.member, item.score))
    }
    pub fn scores(&self) -> impl Iterator<Item = f64> + '_ {
        self.by_member.values().copied()
    }
}

#[derive(Clone, Copy)]
enum Command {
    Add,
    Increment,
    Card,
    Score,
    Rank,
    RevRank,
    Range,
    RevRange,
    RangeStore,
    RangeByScore,
    RevRangeByScore,
    Count,
    LexCount,
    RangeByLex,
    RevRangeByLex,
    RemoveRangeByLex,
    Remove,
    RemoveRangeByScore,
    RemoveRangeByRank,
    MultiScore,
    Scan,
    PopMin,
    PopMax,
    Union,
    UnionStore,
    Intersect,
    IntersectStore,
    Difference,
    DifferenceStore,
    IntersectCard,
}

fn parse(name: &[u8]) -> Option<Command> {
    use Command::*;
    [
        (b"ZADD".as_slice(), Add),
        (b"ZINCRBY", Increment),
        (b"ZCARD", Card),
        (b"ZSCORE", Score),
        (b"ZRANK", Rank),
        (b"ZREVRANK", RevRank),
        (b"ZRANGE", Range),
        (b"ZREVRANGE", RevRange),
        (b"ZRANGESTORE", RangeStore),
        (b"ZRANGEBYSCORE", RangeByScore),
        (b"ZREVRANGEBYSCORE", RevRangeByScore),
        (b"ZCOUNT", Count),
        (b"ZLEXCOUNT", LexCount),
        (b"ZRANGEBYLEX", RangeByLex),
        (b"ZREVRANGEBYLEX", RevRangeByLex),
        (b"ZREMRANGEBYLEX", RemoveRangeByLex),
        (b"ZREM", Remove),
        (b"ZREMRANGEBYSCORE", RemoveRangeByScore),
        (b"ZREMRANGEBYRANK", RemoveRangeByRank),
        (b"ZMSCORE", MultiScore),
        (b"ZSCAN", Scan),
        (b"ZPOPMIN", PopMin),
        (b"ZPOPMAX", PopMax),
        (b"ZUNION", Union),
        (b"ZUNIONSTORE", UnionStore),
        (b"ZINTER", Intersect),
        (b"ZINTERSTORE", IntersectStore),
        (b"ZDIFF", Difference),
        (b"ZDIFFSTORE", DifferenceStore),
        (b"ZINTERCARD", IntersectCard),
    ]
    .iter()
    .find(|(candidate, _)| name.eq_ignore_ascii_case(candidate))
    .map(|(_, kind)| *kind)
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    parse(&command[0]).map(|kind| match kind {
        Command::Add => command.len() >= 4,
        Command::Increment => command.len() == 4,
        Command::Card => command.len() == 2,
        Command::Score | Command::Rank | Command::RevRank => command.len() == 3,
        Command::Range | Command::RevRange | Command::RangeByScore | Command::RevRangeByScore => {
            command.len() >= 4
        }
        Command::RangeStore => command.len() >= 5,
        Command::Count => command.len() == 4,
        Command::LexCount | Command::RemoveRangeByLex => command.len() == 4,
        Command::RangeByLex | Command::RevRangeByLex => command.len() == 4 || command.len() == 7,
        Command::Remove => command.len() >= 3,
        Command::RemoveRangeByScore | Command::RemoveRangeByRank => command.len() == 4,
        Command::MultiScore => command.len() >= 3,
        Command::Scan => command.len() >= 3,
        Command::PopMin | Command::PopMax => command.len() == 2 || command.len() == 3,
        Command::Union | Command::Intersect | Command::Difference | Command::IntersectCard => {
            command.len() >= 3
        }
        Command::UnionStore | Command::IntersectStore | Command::DifferenceStore => {
            command.len() >= 4
        }
    })
}

pub fn is_mutating(command: &[Vec<u8>]) -> bool {
    parse(&command[0]).is_some_and(|kind| {
        matches!(
            kind,
            Command::Add
                | Command::Increment
                | Command::Remove
                | Command::RemoveRangeByScore
                | Command::RemoveRangeByRank
                | Command::RemoveRangeByLex
                | Command::PopMin
                | Command::PopMax
                | Command::UnionStore
                | Command::IntersectStore
                | Command::DifferenceStore
                | Command::RangeStore
        )
    })
}

fn integer(value: usize) -> Vec<u8> {
    format!(":{value}\r\n").into_bytes()
}
fn wrong_type() -> Vec<u8> {
    b"-WRONGTYPE Operation against a key holding the wrong kind of value\r\n".to_vec()
}
pub(crate) fn score_text(score: f64) -> Vec<u8> {
    if score == f64::INFINITY {
        b"inf".to_vec()
    } else if score == f64::NEG_INFINITY {
        b"-inf".to_vec()
    } else if score == 0.0 {
        b"0".to_vec()
    } else {
        score.to_string().into_bytes()
    }
}

fn parse_score(bytes: &[u8]) -> Result<f64, Vec<u8>> {
    let score = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<f64>().ok());
    match score {
        Some(score) if !score.is_nan() => Ok(if score == 0.0 { 0.0 } else { score }),
        _ => Err(protocol::error("value is not a valid float")),
    }
}

fn parse_index(bytes: &[u8]) -> Result<i64, Vec<u8>> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse::<i64>().ok())
        .ok_or_else(|| protocol::error("value is not an integer or out of range"))
}

#[derive(Default)]
struct AddOptions {
    nx: bool,
    xx: bool,
    gt: bool,
    lt: bool,
    ch: bool,
    incr: bool,
}

type ScoredMembers = Vec<(f64, Vec<u8>)>;
type AddArguments = Result<(AddOptions, ScoredMembers), Vec<u8>>;

fn add_arguments(command: &[Vec<u8>], increment_only: bool) -> AddArguments {
    let mut options = AddOptions {
        incr: increment_only,
        ..Default::default()
    };
    let mut start = 2;
    if !increment_only {
        while start < command.len() {
            let option = &command[start];
            let slot = if option.eq_ignore_ascii_case(b"NX") {
                &mut options.nx
            } else if option.eq_ignore_ascii_case(b"XX") {
                &mut options.xx
            } else if option.eq_ignore_ascii_case(b"GT") {
                &mut options.gt
            } else if option.eq_ignore_ascii_case(b"LT") {
                &mut options.lt
            } else if option.eq_ignore_ascii_case(b"CH") {
                &mut options.ch
            } else if option.eq_ignore_ascii_case(b"INCR") {
                &mut options.incr
            } else {
                break;
            };
            if *slot {
                return Err(protocol::error("syntax error"));
            }
            *slot = true;
            start += 1;
        }
    }
    if (options.nx && (options.xx || options.gt || options.lt)) || (options.gt && options.lt) {
        return Err(protocol::error("syntax error"));
    }
    if start + 2 > command.len()
        || !(command.len() - start).is_multiple_of(2)
        || (options.incr && command.len() - start != 2)
    {
        return Err(protocol::error("syntax error"));
    }
    let mut pairs = Vec::with_capacity((command.len() - start) / 2);
    for pair in command[start..].as_chunks::<2>().0 {
        pairs.push((parse_score(&pair[0])?, pair[1].clone()));
    }
    Ok((options, pairs))
}

fn add(command: &[Vec<u8>], store: &Store, increment_only: bool) -> Vec<u8> {
    let (options, pairs) = match add_arguments(command, increment_only) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    match entries.get(key).map(|entry| &entry.value) {
        Some(Value::SortedSet(_)) | None => {}
        Some(_) => return wrong_type(),
    }
    let mut added = 0;
    let mut changed = 0;
    let mut last = None;
    for (score, member) in pairs {
        let old = match entries.get(key).map(|entry| &entry.value) {
            Some(Value::SortedSet(values)) => values.get(&member),
            _ => None,
        };
        if (old.is_some() && options.nx)
            || (old.is_none() && options.xx)
            || old.is_some_and(|old| options.gt && score <= old || options.lt && score >= old)
        {
            continue;
        }
        let new_score = if options.incr {
            let value = old.unwrap_or(0.0) + score;
            if value.is_nan() {
                return protocol::error("resulting score is not a number (NaN)");
            }
            if value == 0.0 {
                0.0
            } else {
                value
            }
        } else {
            score
        };
        let modified = old != Some(new_score);
        if old.is_none() {
            added += 1;
        }
        if modified {
            changed += 1;
        }
        let entry = entries.entry(key.clone()).or_insert_with(|| Entry {
            value: Value::SortedSet(SortedSet::default()),
            expires_at: None,
        });
        let Value::SortedSet(values) = &mut entry.value else {
            unreachable!()
        };
        values.insert(member, new_score);
        last = Some(new_score);
    }
    if changed != 0 {
        mark_modified(store, key);
    }
    if options.incr {
        last.map_or_else(
            || b"$-1\r\n".to_vec(),
            |score| protocol::bulk(&score_text(score)),
        )
    } else if options.ch {
        integer(changed)
    } else {
        integer(added)
    }
}

fn score_bound(bytes: &[u8]) -> Result<(f64, bool), Vec<u8>> {
    let (inclusive, number) = if bytes.first() == Some(&b'(') {
        (false, &bytes[1..])
    } else {
        (true, bytes)
    };
    // Redis reads an empty bound as zero, but rejects an empty ZADD score.
    if number.is_empty() {
        return Ok((0.0, inclusive));
    }
    parse_score(number)
        .map(|score| (score, inclusive))
        .map_err(|_| protocol::error("min or max is not a float"))
}

/// A `LIMIT` window as `(offset, count)`. A negative offset selects nothing,
/// and a negative count means "all", both matching Redis.
fn limit_window(limit: Option<(i64, i64)>) -> (usize, usize) {
    match limit {
        Some((offset, _)) if offset < 0 => (0, 0),
        Some((offset, count)) => (
            offset as usize,
            if count < 0 {
                usize::MAX
            } else {
                count as usize
            },
        ),
        None => (0, usize::MAX),
    }
}

#[derive(Clone, Copy)]
enum LexBound<'a> {
    NegativeInfinity,
    PositiveInfinity,
    Member(&'a [u8], bool),
}

fn lex_bound(bytes: &[u8]) -> Result<LexBound<'_>, Vec<u8>> {
    match bytes {
        b"-" => Ok(LexBound::NegativeInfinity),
        b"+" => Ok(LexBound::PositiveInfinity),
        [b'[', rest @ ..] => Ok(LexBound::Member(rest, true)),
        [b'(', rest @ ..] => Ok(LexBound::Member(rest, false)),
        _ => Err(protocol::error("min or max not valid string range item")),
    }
}

fn lex_in_range(member: &[u8], min: LexBound<'_>, max: LexBound<'_>) -> bool {
    let above_min = match min {
        LexBound::NegativeInfinity => true,
        LexBound::PositiveInfinity => false,
        LexBound::Member(bound, inclusive) => member > bound || (inclusive && member == bound),
    };
    let below_max = match max {
        LexBound::PositiveInfinity => true,
        LexBound::NegativeInfinity => false,
        LexBound::Member(bound, inclusive) => member < bound || (inclusive && member == bound),
    };
    above_min && below_max
}

/// Ordered `(member, score)` pairs selected by a `ZRANGE`-style range, plus the
/// `WITHSCORES` flag the caller asked for.
struct Selection {
    members: Vec<(Vec<u8>, f64)>,
    withscores: bool,
}

/// Select the members of a `ZRANGE`-style query.
///
/// `first` is the index of the `min` argument: 2 for `ZRANGE`, 3 for
/// `ZRANGESTORE`, whose destination comes first. Options start at `first + 2`.
fn select(
    command: &[Vec<u8>],
    first: usize,
    scores: Option<&SortedSet>,
    kind: Command,
    allow_withscores: bool,
) -> Result<Selection, Vec<u8>> {
    let reverse_default = matches!(
        kind,
        Command::RevRange | Command::RevRangeByScore | Command::RevRangeByLex
    );
    let mut reverse = reverse_default;
    let mut byscore = matches!(kind, Command::RangeByScore | Command::RevRangeByScore);
    let mut bylex = matches!(kind, Command::RangeByLex | Command::RevRangeByLex);
    // Redis accepts options in any order, so a LIMIT before BYSCORE or BYLEX is
    // still valid. Look ahead before rejecting it.
    let mode_later = command[first + 2..].iter().any(|option| {
        option.eq_ignore_ascii_case(b"BYSCORE") || option.eq_ignore_ascii_case(b"BYLEX")
    });
    let mut withscores = false;
    let mut limit = None;
    let mut at = first + 2;
    while at < command.len() {
        let option = &command[at];
        if option.eq_ignore_ascii_case(b"WITHSCORES") && allow_withscores && !withscores {
            withscores = true;
            at += 1;
        } else if option.eq_ignore_ascii_case(b"REV") && !reverse_default && !reverse {
            reverse = true;
            at += 1;
        } else if option.eq_ignore_ascii_case(b"BYSCORE")
            && matches!(kind, Command::Range | Command::RangeStore)
            && !byscore
            && !bylex
        {
            byscore = true;
            at += 1;
        } else if option.eq_ignore_ascii_case(b"BYLEX")
            && matches!(kind, Command::Range | Command::RangeStore)
            && !byscore
            && !bylex
        {
            bylex = true;
            at += 1;
        } else if option.eq_ignore_ascii_case(b"LIMIT") {
            if !(byscore || bylex || mode_later) {
                return Err(protocol::error(
                    "syntax error, LIMIT is only supported in combination with either BYSCORE or BYLEX",
                ));
            }
            if limit.is_some() || at + 2 >= command.len() {
                return Err(protocol::error("syntax error"));
            }
            let offset = parse_index(&command[at + 1])?;
            let count = parse_index(&command[at + 2])?;
            limit = Some((offset, count));
            at += 3;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    let (min, max) = if reverse {
        (first + 1, first)
    } else {
        (first, first + 1)
    };
    if bylex {
        if withscores {
            return Err(protocol::error(
                "syntax error, WITHSCORES not supported in combination with BYLEX",
            ));
        }
        let min = lex_bound(&command[min])?;
        let max = lex_bound(&command[max])?;
        let (offset, count) = limit_window(limit);
        let mut members: Vec<(Vec<u8>, f64)> = scores
            .into_iter()
            .flat_map(SortedSet::entries)
            .filter(|(member, _)| lex_in_range(member, min, max))
            .map(|(member, score)| (member.clone(), score))
            .collect();
        if reverse {
            members.reverse();
        }
        return Ok(Selection {
            members: members.into_iter().skip(offset).take(count).collect(),
            withscores,
        });
    }
    if byscore {
        let (min, max) = match (score_bound(&command[min]), score_bound(&command[max])) {
            (Ok(min), Ok(max)) => (min, max),
            (Err(error), _) | (_, Err(error)) => return Err(error),
        };
        let mut members = Vec::new();
        if let Some(scores) = scores {
            let in_range = |score: f64| {
                (if min.1 { score >= min.0 } else { score > min.0 })
                    && (if max.1 { score <= max.0 } else { score < max.0 })
            };
            let (offset, count) = limit_window(limit);
            let selected: Vec<_> = if reverse {
                scores
                    .entries()
                    .rev()
                    .filter(|(_, score)| in_range(*score))
                    .skip(offset)
                    .take(count)
                    .collect()
            } else {
                scores
                    .entries()
                    .filter(|(_, score)| in_range(*score))
                    .skip(offset)
                    .take(count)
                    .collect()
            };
            members.extend(
                selected
                    .into_iter()
                    .map(|(member, score)| (member.clone(), score)),
            );
        }
        return Ok(Selection {
            members,
            withscores,
        });
    }
    let start = parse_index(&command[first])?;
    let stop = parse_index(&command[first + 1])?;
    let Some(scores) = scores else {
        return Ok(Selection {
            members: Vec::new(),
            withscores,
        });
    };
    let len = scores.len() as i128;
    let index = |value: i64| {
        if value < 0 {
            len + i128::from(value)
        } else {
            i128::from(value)
        }
    };
    let from = index(start).max(0).min(len) as usize;
    let to = index(stop).max(-1).min(len - 1);
    let members = if to >= from as i128 {
        let count = to as usize - from + 1;
        let selected: Vec<_> = if reverse {
            scores.entries().rev().skip(from).take(count).collect()
        } else {
            scores.entries().skip(from).take(count).collect()
        };
        selected
            .into_iter()
            .map(|(member, score)| (member.clone(), score))
            .collect()
    } else {
        Vec::new()
    };
    Ok(Selection {
        members,
        withscores,
    })
}

fn encode(members: Vec<(Vec<u8>, f64)>, withscores: bool) -> Vec<u8> {
    let mut values =
        Vec::with_capacity(members.len().saturating_mul(if withscores { 2 } else { 1 }));
    for (member, score) in members {
        values.push(member);
        if withscores {
            values.push(score_text(score));
        }
    }
    protocol::array(&values)
}

fn range(command: &[Vec<u8>], scores: Option<&SortedSet>, kind: Command) -> Vec<u8> {
    match select(command, 2, scores, kind, true) {
        Ok(selection) => encode(selection.members, selection.withscores),
        Err(error) => error,
    }
}

fn scan(command: &[Vec<u8>], scores: Option<&SortedSet>) -> Vec<u8> {
    let cursor = match std::str::from_utf8(&command[2])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        Some(value) => value,
        None => return protocol::error("invalid cursor"),
    };
    let mut count = 10usize;
    let mut pattern = None;
    if !(command.len() - 3).is_multiple_of(2) {
        return protocol::error("syntax error");
    }
    for pair in command[3..].as_chunks::<2>().0 {
        if pair[0].eq_ignore_ascii_case(b"MATCH") {
            pattern = Some(pair[1].as_slice());
        } else if pair[0].eq_ignore_ascii_case(b"COUNT") {
            count = match std::str::from_utf8(&pair[1])
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
            {
                Some(value) => value,
                None => return protocol::error("value is not an integer or out of range"),
            };
        } else {
            return protocol::error("syntax error");
        }
    }
    let mut members: Vec<_> = scores.into_iter().flat_map(SortedSet::entries).collect();
    members.sort_unstable_by_key(|(member, _)| *member);
    let start = cursor.min(members.len());
    let end = start.saturating_add(count).min(members.len());
    let mut values = Vec::new();
    for (member, score) in &members[start..end] {
        if pattern.is_none_or(|pattern| glob_matches(pattern, member)) {
            values.push((*member).clone());
            values.push(score_text(*score));
        }
    }
    let next = if end == members.len() { 0 } else { end };
    let mut reply = b"*2\r\n".to_vec();
    reply.extend(protocol::bulk(next.to_string().as_bytes()));
    reply.extend(protocol::array(&values));
    reply
}

fn members_to_remove(
    kind: Command,
    command: &[Vec<u8>],
    scores: Option<&SortedSet>,
) -> Result<Vec<Vec<u8>>, Vec<u8>> {
    if matches!(kind, Command::RemoveRangeByLex) {
        let min = lex_bound(&command[2])?;
        let max = lex_bound(&command[3])?;
        return Ok(scores
            .into_iter()
            .flat_map(SortedSet::entries)
            .filter(|(member, _)| lex_in_range(member, min, max))
            .map(|(member, _)| member.clone())
            .collect());
    }
    if matches!(kind, Command::RemoveRangeByScore) {
        let min = score_bound(&command[2])?;
        let max = score_bound(&command[3])?;
        return Ok(scores
            .into_iter()
            .flat_map(SortedSet::entries)
            .filter(|(_, score)| {
                (if min.1 {
                    *score >= min.0
                } else {
                    *score > min.0
                }) && (if max.1 {
                    *score <= max.0
                } else {
                    *score < max.0
                })
            })
            .map(|(member, _)| member.clone())
            .collect());
    }
    let first = parse_index(&command[2])?;
    let last = parse_index(&command[3])?;
    let Some(scores) = scores else {
        return Ok(Vec::new());
    };
    let len = scores.len() as i128;
    let index = |value: i64| {
        if value < 0 {
            len + i128::from(value)
        } else {
            i128::from(value)
        }
    };
    let from = index(first).max(0).min(len) as usize;
    let to = index(last).max(-1).min(len - 1);
    if to < from as i128 {
        return Ok(Vec::new());
    }
    Ok(scores
        .entries()
        .skip(from)
        .take(to as usize - from + 1)
        .map(|(member, _)| member.clone())
        .collect())
}

/// `ZRANGESTORE destination source min max [...]`. Selects from the source first,
/// so a syntax error cannot leave a half-written destination.
fn range_store(command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let destination = &command[1];
    let source = &command[2];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, source);
    let members = {
        let scores = match entries.get(source).map(|entry| &entry.value) {
            Some(Value::SortedSet(scores)) => Some(scores),
            Some(_) => return wrong_type(),
            None => None,
        };
        match select(command, 3, scores, Command::RangeStore, false) {
            Ok(selection) => selection.members,
            Err(error) => return error,
        }
    };
    remove_expired(&mut entries, destination);
    let count = members.len();
    if count == 0 {
        if entries.remove(destination).is_some() {
            mark_modified(store, destination);
        }
    } else {
        let mut result = SortedSet::default();
        for (member, score) in members {
            result.insert(member, score);
        }
        entries.insert(
            destination.clone(),
            Entry {
                value: Value::SortedSet(result),
                expires_at: None,
            },
        );
        mark_modified(store, destination);
    }
    integer(count)
}

pub fn execute(command: &[Vec<u8>], store: &Store) -> Option<Vec<u8>> {
    let kind = parse(&command[0])?;
    if valid_arity(command) == Some(false) {
        return Some(protocol::error(&format!(
            "wrong number of arguments for '{}' command",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase()
        )));
    }
    if matches!(kind, Command::Add | Command::Increment) {
        return Some(add(command, store, matches!(kind, Command::Increment)));
    }
    if matches!(kind, Command::RangeStore) {
        return Some(range_store(command, store));
    }
    if matches!(
        kind,
        Command::Union
            | Command::UnionStore
            | Command::Intersect
            | Command::IntersectStore
            | Command::Difference
            | Command::DifferenceStore
            | Command::IntersectCard
    ) {
        return Some(algebra::execute(kind, command, store));
    }
    let key = &command[1];
    let mut entries = store.entries.lock().unwrap();
    remove_expired(&mut entries, key);
    let scores = match entries.get(key).map(|entry| &entry.value) {
        Some(Value::SortedSet(scores)) => Some(scores),
        Some(_) => return Some(wrong_type()),
        None => None,
    };
    Some(match kind {
        Command::Card => integer(scores.map_or(0, SortedSet::len)),
        Command::LexCount => {
            let min = match lex_bound(&command[2]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let max = match lex_bound(&command[3]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            integer(
                scores
                    .into_iter()
                    .flat_map(SortedSet::entries)
                    .filter(|(member, _)| lex_in_range(member, min, max))
                    .count(),
            )
        }
        Command::MultiScore => {
            let mut response = format!("*{}\r\n", command.len() - 2).into_bytes();
            for member in &command[2..] {
                response.extend(scores.and_then(|scores| scores.get(member)).map_or_else(
                    || b"$-1\r\n".to_vec(),
                    |score| protocol::bulk(&score_text(score)),
                ));
            }
            response
        }
        Command::Scan => scan(command, scores),
        Command::Score => scores
            .and_then(|values| values.get(&command[2]))
            .map_or_else(
                || b"$-1\r\n".to_vec(),
                |score| protocol::bulk(&score_text(score)),
            ),
        Command::Rank | Command::RevRank => {
            let found = scores
                .and_then(|scores| scores.rank(&command[2], matches!(kind, Command::RevRank)));
            found.map_or_else(|| b"$-1\r\n".to_vec(), integer)
        }
        Command::Range
        | Command::RevRange
        | Command::RangeByScore
        | Command::RevRangeByScore
        | Command::RangeByLex
        | Command::RevRangeByLex => range(command, scores, kind),
        Command::Count => {
            let min = match score_bound(&command[2]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            let max = match score_bound(&command[3]) {
                Ok(value) => value,
                Err(error) => return Some(error),
            };
            integer(scores.map_or(0, |scores| {
                scores
                    .scores()
                    .filter(|score| {
                        (if min.1 {
                            *score >= min.0
                        } else {
                            *score > min.0
                        }) && (if max.1 {
                            *score <= max.0
                        } else {
                            *score < max.0
                        })
                    })
                    .count()
            }))
        }
        Command::Remove => {
            let Some(Entry {
                value: Value::SortedSet(scores),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(integer(0));
            };
            let mut removed = 0;
            for member in &command[2..] {
                removed += usize::from(scores.remove(member).is_some());
            }
            if scores.is_empty() {
                entries.remove(key);
            }
            if removed != 0 {
                mark_modified(store, key);
            }
            integer(removed)
        }
        Command::RemoveRangeByScore | Command::RemoveRangeByRank | Command::RemoveRangeByLex => {
            let targets = match members_to_remove(kind, command, scores) {
                Ok(targets) => targets,
                Err(error) => return Some(error),
            };
            if targets.is_empty() {
                return Some(integer(0));
            }
            let Some(Entry {
                value: Value::SortedSet(scores),
                ..
            }) = entries.get_mut(key)
            else {
                unreachable!()
            };
            for member in &targets {
                scores.remove(member);
            }
            if scores.is_empty() {
                entries.remove(key);
            }
            mark_modified(store, key);
            integer(targets.len())
        }
        Command::PopMin | Command::PopMax => {
            let count = if command.len() == 3 {
                match std::str::from_utf8(&command[2])
                    .ok()
                    .and_then(|value| value.parse::<usize>().ok())
                {
                    Some(value) => value,
                    None => {
                        return Some(protocol::error("value is not an integer or out of range"))
                    }
                }
            } else {
                1
            };
            let Some(Entry {
                value: Value::SortedSet(scores),
                ..
            }) = entries.get_mut(key)
            else {
                return Some(protocol::array(&[]));
            };
            let ordered: Vec<_> = if matches!(kind, Command::PopMax) {
                scores
                    .entries()
                    .rev()
                    .take(count)
                    .map(|(member, score)| (member.clone(), score))
                    .collect()
            } else {
                scores
                    .entries()
                    .take(count)
                    .map(|(member, score)| (member.clone(), score))
                    .collect()
            };
            let mut values = Vec::with_capacity(ordered.len().saturating_mul(2));
            for (member, score) in &ordered {
                scores.remove(member);
                values.push(member.clone());
                values.push(score_text(*score));
            }
            if scores.is_empty() {
                entries.remove(key);
            }
            if !ordered.is_empty() {
                mark_modified(store, key);
            }
            protocol::array(&values)
        }
        Command::Add | Command::Increment | Command::RangeStore => unreachable!(),
        Command::Union
        | Command::UnionStore
        | Command::Intersect
        | Command::IntersectStore
        | Command::Difference
        | Command::DifferenceStore
        | Command::IntersectCard => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_index_stays_consistent_after_score_changes_and_removals() {
        let mut set = SortedSet::default();
        for i in 0..300 {
            set.insert(format!("member-{i:03}").into_bytes(), (i % 9) as f64);
        }
        for i in (0..300).step_by(3) {
            set.insert(format!("member-{i:03}").into_bytes(), -(i as f64));
        }
        for i in (0..300).step_by(5) {
            set.remove(format!("member-{i:03}").as_bytes());
        }
        assert_eq!(set.len(), set.ordered.len());
        let ordered: Vec<_> = set.entries().collect();
        assert!(ordered.windows(2).all(|pair| {
            pair[0]
                .1
                .total_cmp(&pair[1].1)
                .then_with(|| pair[0].0.cmp(pair[1].0))
                != Ordering::Greater
        }));
        for (rank, (member, score)) in ordered.iter().enumerate() {
            assert_eq!(set.rank(member, false), Some(rank));
            assert_eq!(set.rank(member, true), Some(set.len() - rank - 1));
            assert_eq!(set.get(member), Some(*score));
        }
    }
}
