// SPDX-License-Identifier: AGPL-3.0-only
use super::{integer, parse_score, protocol, score_text, wrong_type, Command, SortedSet};
use crate::{mark_modified, remove_expired, Entry, Store, Value};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy)]
enum Aggregate {
    Sum,
    Min,
    Max,
    Count,
}

#[derive(Clone, Copy)]
enum Operation {
    Union,
    Intersect,
    Difference,
    IntersectCard,
}

struct Options<'a> {
    operation: Operation,
    destination: Option<&'a Vec<u8>>,
    keys: &'a [Vec<u8>],
    weights: Vec<f64>,
    aggregate: Aggregate,
    withscores: bool,
    limit: usize,
}

fn parse<'a>(kind: Command, command: &'a [Vec<u8>]) -> Result<Options<'a>, Vec<u8>> {
    let destination = match kind {
        Command::UnionStore | Command::IntersectStore | Command::DifferenceStore => {
            Some(&command[1])
        }
        _ => None,
    };
    let operation = match kind {
        Command::Union | Command::UnionStore => Operation::Union,
        Command::Intersect | Command::IntersectStore => Operation::Intersect,
        Command::Difference | Command::DifferenceStore => Operation::Difference,
        Command::IntersectCard => Operation::IntersectCard,
        _ => unreachable!(),
    };
    let count_at = if destination.is_some() { 2 } else { 1 };
    let count = std::str::from_utf8(&command[count_at])
        .ok()
        .and_then(|text| text.parse::<usize>().ok())
        .filter(|count| *count > 0)
        .ok_or_else(|| protocol::error("numkeys should be greater than 0"))?;
    let from = count_at + 1;
    let end = from
        .checked_add(count)
        .filter(|end| *end <= command.len())
        .ok_or_else(|| protocol::error("syntax error"))?;
    let mut options = Options {
        operation,
        destination,
        keys: &command[from..end],
        weights: vec![1.0; count],
        aggregate: Aggregate::Sum,
        withscores: false,
        limit: 0,
    };
    let mut at = end;
    let mut seen_weights = false;
    let mut seen_aggregate = false;
    let mut seen_limit = false;
    while at < command.len() {
        if matches!(operation, Operation::IntersectCard)
            && command[at].eq_ignore_ascii_case(b"LIMIT")
            && !seen_limit
        {
            let value = command
                .get(at + 1)
                .ok_or_else(|| protocol::error("syntax error"))?;
            options.limit = std::str::from_utf8(value)
                .ok()
                .and_then(|text| text.parse::<usize>().ok())
                .ok_or_else(|| protocol::error("LIMIT can't be negative"))?;
            seen_limit = true;
            at += 2;
        } else if matches!(operation, Operation::Union | Operation::Intersect)
            && command[at].eq_ignore_ascii_case(b"WEIGHTS")
            && !seen_weights
        {
            let weight_end = at
                .checked_add(1 + count)
                .filter(|end| *end <= command.len())
                .ok_or_else(|| protocol::error("syntax error"))?;
            options.weights = command[at + 1..weight_end]
                .iter()
                .map(|weight| parse_score(weight))
                .collect::<Result<_, _>>()?;
            seen_weights = true;
            at = weight_end;
        } else if matches!(operation, Operation::Union | Operation::Intersect)
            && command[at].eq_ignore_ascii_case(b"AGGREGATE")
            && !seen_aggregate
        {
            let value = command
                .get(at + 1)
                .ok_or_else(|| protocol::error("syntax error"))?;
            options.aggregate = if value.eq_ignore_ascii_case(b"SUM") {
                Aggregate::Sum
            } else if value.eq_ignore_ascii_case(b"MIN") {
                Aggregate::Min
            } else if value.eq_ignore_ascii_case(b"MAX") {
                Aggregate::Max
            } else if value.eq_ignore_ascii_case(b"COUNT") {
                Aggregate::Count
            } else {
                return Err(protocol::error("syntax error"));
            };
            seen_aggregate = true;
            at += 2;
        } else if destination.is_none()
            && !matches!(operation, Operation::IntersectCard)
            && command[at].eq_ignore_ascii_case(b"WITHSCORES")
            && !options.withscores
        {
            options.withscores = true;
            at += 1;
        } else {
            return Err(protocol::error("syntax error"));
        }
    }
    Ok(options)
}

enum Input<'a> {
    Sorted(&'a SortedSet),
    Set(&'a HashSet<Vec<u8>>),
    Missing,
}

impl Input<'_> {
    fn score(&self, member: &[u8]) -> Option<f64> {
        match self {
            Self::Sorted(set) => set.get(member),
            Self::Set(set) => set.contains(member).then_some(1.0),
            Self::Missing => None,
        }
    }

    fn for_each(&self, mut visit: impl FnMut(&[u8], f64)) {
        match self {
            Self::Sorted(set) => {
                for (member, score) in set.entries() {
                    visit(member, score);
                }
            }
            Self::Set(set) => {
                for member in *set {
                    visit(member, 1.0);
                }
            }
            Self::Missing => {}
        }
    }
}

fn normalize(score: f64) -> f64 {
    if score.is_nan() || score == 0.0 {
        0.0
    } else {
        score
    }
}

fn contribution(score: f64, weight: f64, aggregate: Aggregate) -> f64 {
    normalize(if matches!(aggregate, Aggregate::Count) {
        weight
    } else {
        score * weight
    })
}

fn merge(previous: f64, next: f64, aggregate: Aggregate) -> f64 {
    normalize(match aggregate {
        Aggregate::Sum | Aggregate::Count => previous + next,
        Aggregate::Min => previous.min(next),
        Aggregate::Max => previous.max(next),
    })
}

fn combine(inputs: &[Input<'_>], options: &Options<'_>) -> SortedSet {
    let mut members = HashMap::<Vec<u8>, f64>::new();
    match options.operation {
        Operation::Union => {
            for (index, input) in inputs.iter().enumerate() {
                input.for_each(|member, score| {
                    let value = contribution(score, options.weights[index], options.aggregate);
                    members
                        .entry(member.to_vec())
                        .and_modify(|old| *old = merge(*old, value, options.aggregate))
                        .or_insert(value);
                });
            }
        }
        Operation::Intersect | Operation::IntersectCard => {
            inputs[0].for_each(|member, score| {
                members.insert(
                    member.to_vec(),
                    contribution(score, options.weights[0], options.aggregate),
                );
            });
            for (index, input) in inputs.iter().enumerate().skip(1) {
                members.retain(|member, old| {
                    if let Some(score) = input.score(member) {
                        let value = contribution(score, options.weights[index], options.aggregate);
                        *old = merge(*old, value, options.aggregate);
                        true
                    } else {
                        false
                    }
                });
            }
        }
        Operation::Difference => {
            inputs[0].for_each(|member, score| {
                members.insert(member.to_vec(), score);
            });
            for input in &inputs[1..] {
                input.for_each(|member, _| {
                    members.remove(member);
                });
            }
        }
    }
    let mut result = SortedSet::default();
    for (member, score) in members {
        result.insert(member, score);
    }
    result
}

pub(super) fn execute(kind: Command, command: &[Vec<u8>], store: &Store) -> Vec<u8> {
    let options = match parse(kind, command) {
        Ok(options) => options,
        Err(error) => return error,
    };
    let mut entries = store.entries.lock().unwrap();
    for key in options.keys {
        remove_expired(&mut entries, key);
    }
    let result = {
        let mut inputs = Vec::with_capacity(options.keys.len());
        for key in options.keys {
            match entries.get(key).map(|entry| &entry.value) {
                Some(Value::SortedSet(set)) => inputs.push(Input::Sorted(set)),
                Some(Value::Set(set)) => inputs.push(Input::Set(set)),
                Some(_) => return wrong_type(),
                None => inputs.push(Input::Missing),
            }
        }
        combine(&inputs, &options)
    };
    if matches!(options.operation, Operation::IntersectCard) {
        return integer(if options.limit == 0 {
            result.len()
        } else {
            result.len().min(options.limit)
        });
    }
    if let Some(destination) = options.destination {
        remove_expired(&mut entries, destination);
        let count = result.len();
        if count == 0 {
            if entries.remove(destination).is_some() {
                mark_modified(store, destination);
            }
        } else {
            entries.insert(
                destination.clone(),
                Entry {
                    value: Value::SortedSet(result),
                    expires_at: None,
                },
            );
            mark_modified(store, destination);
        }
        return integer(count);
    }
    let mut values =
        Vec::with_capacity(
            result
                .len()
                .saturating_mul(if options.withscores { 2 } else { 1 }),
        );
    for (member, score) in result.entries() {
        values.push(member.clone());
        if options.withscores {
            values.push(score_text(score));
        }
    }
    protocol::array(&values)
}
