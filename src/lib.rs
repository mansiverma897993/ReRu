// SPDX-License-Identifier: AGPL-3.0-only
#![forbid(unsafe_code)]

//! Library surface for ReRu tooling.
//!
//! The server lives in `src/main.rs`. This crate exists so out-of-tree tools
//! can reach the wire protocol without running the server. The `fuzz/`
//! workspace uses it to drive [`protocol::read_command`] over arbitrary bytes.

pub mod protocol;
