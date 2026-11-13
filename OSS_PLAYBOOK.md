# ReRu — Open Source Playbook

One file with everything needed to take ReRu from a private repository to a
project that outside people contribute to.

Last updated: 13 November 2026.

---

## 1. Snapshot

| Item | State |
| --- | --- |
| Code | 21k lines of Rust, 204 tests, 1 runtime dependency |
| License | AGPL-3.0-only, clean-room rule in `CONTRIBUTING.md` |
| Governance | `CONTRIBUTING`, `GOVERNANCE`, `CODE_OF_CONDUCT`, `SECURITY`, `CHANGELOG`, `AGENTS` |
| CI | fmt, clippy, and tests on Linux/macOS/Windows, MSRV, rustdoc; weekly dependency audit |
| Docs | README, architecture, compatibility, development guide, interop testing |
| Issues | 20 scoped tasks created (#5–#24) |
| **Blocker** | **Repository is private. Only the account with repository administration can make it public and set the description and topics.** |

Repository: https://github.com/mansiverma897993/ReRu

Ownership: `mansiverma897993` and `MohitGoyal09` are both owners with equal
authority; see [GOVERNANCE.md](GOVERNANCE.md). GitHub records one account with
repository administration, currently `mansiverma897993`, and that is a hosting
detail rather than extra decision-making weight.

---

## 2. Step 0 — unblock (repository admin)

Nothing below works while the repository is private. The account with
repository administration must run:

```sh
# Make it public
gh repo edit mansiverma897993/ReRu \
  --visibility public --accept-visibility-change-consequences

# Description and topics
gh repo edit mansiverma897993/ReRu \
  --description "A Redis-compatible cache and data structure server written in Rust. RESP2, 203 commands, TTLs, persistence, replication, streams, and pub/sub. Experimental preview." \
  --add-topic redis --add-topic rust --add-topic database --add-topic cache \
  --add-topic resp2 --add-topic key-value --add-topic in-memory \
  --add-topic data-structures --add-topic redis-server --add-topic storage \
  --add-topic redis-alternative --add-topic ttl
```

Then, in repository settings:

- Enable **Discussions**.
- Enable **Private vulnerability reporting**.
- Upload `docs/assets/social-preview.png` as the social preview.
- Re-add the CI badge to `README.md` (it fails on private repositories).

---

## 3. The backlog (already created)

20 issues, grouped by difficulty. Every issue states the files, acceptance
criteria, test location, and the clean-room rule.

### Good first issue / easy

| Issue | Task |
| --- | --- |
| [#5](https://github.com/mansiverma897993/ReRu/issues/5) | `OBJECT ENCODING` and `OBJECT REFCOUNT` |
| [#7](https://github.com/mansiverma897993/ReRu/issues/7) | `MEMORY USAGE` |
| [#11](https://github.com/mansiverma897993/ReRu/issues/11) | `COMMAND`, `COMMAND COUNT`, `COMMAND INFO` |
| [#13](https://github.com/mansiverma897993/ReRu/issues/13) | `ZRANGESTORE` |
| [#16](https://github.com/mansiverma897993/ReRu/issues/16) | Docker Compose quickstart |
| [#17](https://github.com/mansiverma897993/ReRu/issues/17) | SPDX license headers |
| [#18](https://github.com/mansiverma897993/ReRu/issues/18) | `CONTRIBUTORS.md` and all-contributors |
| [#19](https://github.com/mansiverma897993/ReRu/issues/19) | `docs/CLI.md` for every flag |

### Medium

| Issue | Task |
| --- | --- |
| [#6](https://github.com/mansiverma897993/ReRu/issues/6) | `OBJECT IDLETIME` and `OBJECT FREQ` |
| [#8](https://github.com/mansiverma897993/ReRu/issues/8) | `CLIENT LIST` |
| [#9](https://github.com/mansiverma897993/ReRu/issues/9) | `CLIENT KILL` |
| [#10](https://github.com/mansiverma897993/ReRu/issues/10) | `SLOWLOG` |
| [#15](https://github.com/mansiverma897993/ReRu/issues/15) | `INFO commandstats` |
| [#20](https://github.com/mansiverma897993/ReRu/issues/20) | Compatibility dashboard |
| [#21](https://github.com/mansiverma897993/ReRu/issues/21) | Criterion benchmarks |

### Hard

| Issue | Task |
| --- | --- |
| [#12](https://github.com/mansiverma897993/ReRu/issues/12) | `SORT` and `SORT_RO` |
| [#14](https://github.com/mansiverma897993/ReRu/issues/14) | `WAIT` |
| [#22](https://github.com/mansiverma897993/ReRu/issues/22) | cargo-fuzz target for the RESP parser |
| [#23](https://github.com/mansiverma897993/ReRu/issues/23) | FIFO ordering for blocking waiters |
| [#24](https://github.com/mansiverma897993/ReRu/issues/24) | Cancel blocking waiters on disconnect |

### How to use the backlog

1. Label 3–5 issues `first-timers-only` for the first wave.
2. Comment "assigned to @user, please open a PR within 7 days" when claimed.
3. Do not assign more than one issue per new contributor at a time.
4. Close stale claims after 7 days of no activity and re-label the issue.
5. Every merged PR gets a thank-you comment and a line in the next release notes.

---

## 4. Program calendar

| Program | Window | Fit | Effort |
| --- | --- | --- | --- |
| **SWOC Season 7** | Project list published **15 Dec 2026**; registrations close **31 Dec 2026**; coding **1 Jan – 30 Mar 2027** | **Best immediate fit** | Low |
| **Winter of Code Social** | Coding **1 Nov 2026 – 31 Jan 2027** | Good if project slots remain | Low |
| **24 Pull Requests** | **1–24 Dec 2026** | Easy, friendly, low effort | Low |
| **GSSoC 2027** | Usually Feb–May; verify | Good reach in India | Medium |
| **JWoC** | Dec–Jan; verify | Small but real | Low |
| **Hacktoberfest 2027** | October 2027 | Huge reach; add `hacktoberfest` topic and label | Low |
| **European Summer of Code** | Project batches around Feb and Apr; verify | Possible | Medium |
| **GSoC 2027** | Org applications **~19 Jan – 3 Feb**; accepted orgs ~19 Feb; contributor apps **16–31 Mar** | **Low odds now**: new org, no community | High |
| **Outreachy May 2027** | Community signup ~Jan–Feb; applications ~Feb | Low–medium; needs a mentor | High |
| **LFX Mentorship Term 1 2027** | Proposals **6–27 Jan**; applications **3–16 Feb** | **Not a fit**: Linux Foundation / CNCF projects only | — |
| **C4GT** | ~April 2027 | **Not a fit**: govtech / digital public goods | — |

### Recommendation

1. Register for **SWOC** as a project before 15 December 2026.
2. Run a **24 Pull Requests** push in December.
3. Use **Hacktoberfest 2027** as the big annual growth event.
4. Apply to **GSoC in 2028**, after 10+ merged external PRs and an additional
   maintainer beyond the two owners.

GSoC selects mentoring organizations with a community and a mentor bench. A
project with zero outside contributors is unlikely to be accepted.

---

## 5. Directories and aggregators

| Site | Action |
| --- | --- |
| **Up For Grabs** (`up-for-grabs.net`) | Open a PR adding ReRu with the `good first issue` label |
| **CodeTriage** (`codetriage.com`) | Add the repository; subscribers get daily issue emails |
| **First Timers Only** | Submit the repository |
| **Good First Issue** (`goodfirstissue.dev`) | Automatic; it crawls the label |
| **This Week in Rust** | Submit the project and a call for contributors |
| **Awesome Rust** | PR to the list |
| **Awesome Redis** | PR to the alternatives section |
| **Awesome Database** / **Awesome Self-Hosted** | PR to the relevant section |
| **lib.rs** | Automatic after a crates.io publish |
| **GitHub topics** | `hacktoberfest`, `good-first-issue` in October |

Note: the crate name `reru` is already taken on crates.io (a `0.0.0`
placeholder). Publish as **`reru-server`** so `cargo install reru-server` works.

---

## 6. Launch plan (4 weeks after the repo is public)

| Week | Actions |
| --- | --- |
| 1 | Public repo, description, topics, Discussions, 20 issues, labels, social preview |
| 2 | Publish `reru-server` to crates.io, push a GHCR image, create `v0.1.0-alpha.1` (draft first) |
| 3 | Publish one technical post; then Show HN, r/rust, r/redis, Lobsters, TWiR |
| 4 | First good-first-issue drive, SWOC registration, answer every issue within 48 hours |

Cadence after launch:

- **Weekly:** triage new issues, publish or refresh one good-first issue.
- **Monthly:** alpha release with changelog.
- **Quarterly:** update the roadmap and the compatibility inventory.

---

## 7. Content plan

Publish in this order. Each post should end with one concrete call to action.

1. "Writing a Redis-compatible server in Rust" — architecture and the command gate.
2. "RESP2 from scratch" — the parser, limits, and fuzzing.
3. "Streams and consumer groups" — pending entries and recovery.
4. "AOF rewrite without stopping writes" — the compaction design.
5. "Redis RDB interoperability" — the subset and the checks.
6. "What 204 tests taught us about compatibility" — the dashboard and gaps.

Supporting assets:

- A 3-minute demo: start, `redis-cli`, restart with AOF, replica.
- A public compatibility dashboard (issue #20).
- Honest benchmarks against Redis 7.0.15, with the "not production" warning.

---

## 8. Ready-to-paste copy

### Repository description

> A Redis-compatible cache and data structure server written in Rust. RESP2,
> 203 commands, TTLs, persistence, replication, streams, and pub/sub.
> Experimental preview.

### Topics

`redis` `rust` `database` `cache` `resp2` `key-value` `in-memory`
`data-structures` `redis-server` `storage` `redis-alternative` `ttl`

### Show HN

**Title:** Show HN: ReRu – a Redis-compatible server in Rust, from scratch

**Body:**

> ReRu is a Redis-compatible server written in Rust. It speaks RESP2, so
> `redis-cli` and existing clients connect without changes. It implements
> strings, lists, hashes, sets, sorted sets, bitmaps, streams with consumer
> groups, geospatial commands, TTLs, eviction, transactions, pub/sub,
> persistence (snapshot, AOF, part of RDB), and read-only replication.
>
> It is an experimental preview, not a production replacement. The interesting
> part is the constraint: about 21k lines, one runtime dependency, no async
> runtime, one thread per connection, and 204 tests against a pinned Redis
> reference.
>
> I would value feedback on two things: which compatibility gaps matter most,
> and whether the module layout is easy to follow.
>
> https://github.com/mansiverma897993/ReRu

### Reddit r/rust

**Title:** ReRu: a Redis-compatible server in Rust with one dependency and no async runtime

**Body:** Same three paragraphs as Show HN, plus: "Good first issues are
labelled and scoped. Feedback and PRs welcome."

### X / Twitter

> ReRu is a Redis-compatible server in Rust.
>
> RESP2. 203 commands. TTLs, eviction, streams, persistence, replication.
> 21k lines. One dependency. No async runtime.
>
> 204 tests. Experimental, not production.
>
> Good first issues are open:
> github.com/mansiverma897993/ReRu

### LinkedIn

> I have been building ReRu, a Redis-compatible data server in Rust.
>
> It speaks RESP2, so `redis-cli` works unchanged. It covers strings, lists,
> hashes, sets, sorted sets, bitmaps, streams with consumer groups, geospatial
> queries, TTLs, eviction, transactions, pub/sub, persistence, and read-only
> replication.
>
> The goal is not to replace Redis. The goal is a readable, tested
> implementation that people can learn from and extend.
>
> It is an experimental preview with 204 tests and a clear list of gaps. Good
> first issues are labelled. Contributions and compatibility reports are
> welcome.
>
> github.com/mansiverma897993/ReRu

### This Week in Rust

Submit the repository link with:

> ReRu is an experimental Redis-compatible server in Rust: RESP2, 203 command
> names, persistence, replication, and 204 tests. Good first issues are
> available for contributors who want to work on a database server.

---

## 9. Legal and governance

- **License:** AGPL-3.0-only. Modified versions and hosted forks must publish
  their source.
- **Relicensing consent:** the history contains commits by Mansi Verma and
  Mohit Goyal. Keep their written agreement to the AGPL change on file.
- **Clean-room rule:** never copy or translate Redis source. Redis is AGPLv3;
  ReRu must stay an independent implementation.
- **Sign-off:** there is no DCO or CLA. Opening a pull request is the
  confirmation; see [GOVERNANCE.md](GOVERNANCE.md).
- **No CLA:** contributions are accepted under AGPL-3.0 directly.
- **Trademark:** the README states ReRu is independent and not affiliated with
  Redis Ltd. Keep that sentence.

---

## 10. Targets

| Horizon | Goal |
| --- | --- |
| 30 days | Public repo, 20 issues, 5 external PRs, 3 contributors |
| 90 days | 10 merged external PRs, 5 repeat contributors, 100 stars, 1 program accepted |
| 6 months | 2 maintainers beyond the owners, monthly releases, 300 stars, 25 outside contributors |

Track weekly: stars, forks, open issues, merged PRs, unique contributors, and
median time to first response.

---

## 11. Checklist

**Owner**

- [ ] Make the repository public
- [ ] Set description and topics
- [ ] Enable Discussions and private vulnerability reporting
- [ ] Upload the social preview
- [ ] Keep repository administration on one owner account (hosting detail only)

**Maintainers**

- [ ] Re-add the CI badge
- [ ] Publish `reru-server` to crates.io
- [ ] Push a GHCR image
- [ ] Create `v0.1.0-alpha.1` and publish the release
- [ ] Register for SWOC before 15 December 2026
- [ ] Submit to Up For Grabs, CodeTriage, First Timers Only, TWiR
- [ ] Publish the first blog post
- [ ] Answer every issue within 48 hours

---

## 12. Links

- Repository: https://github.com/mansiverma897993/ReRu
- Issues: https://github.com/mansiverma897993/ReRu/issues
- Actions: https://github.com/mansiverma897993/ReRu/actions
- Compatibility: `docs/COMPATIBILITY.md`
- Architecture: `docs/ARCHITECTURE.md`
- Contributing: `CONTRIBUTING.md`
- Security: `SECURITY.md`
- License: `LICENSE` (AGPL-3.0-only)
