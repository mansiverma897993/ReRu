# Governance

This document says who decides, how a disagreement is resolved, and what a
contributor agrees to. It is short on purpose, and it is a promise rather than a
plan.

## Owners and maintainers

| Person | GitHub | Role |
| --- | --- | --- |
| Mansi Verma | [@mansiverma897993](https://github.com/mansiverma897993) | Owner |
| Mohit Goyal | [@MohitGoyal09](https://github.com/MohitGoyal09) | Owner |

The two owners have equal authority over scope, design, releases, and who joins
the project. GitHub records one account with repository administration,
currently [@mansiverma897993](https://github.com/mansiverma897993); that is a
hosting detail, not extra decision-making weight.

Areas are not split formally. The owner who has worked on a subsystem leads the
review of changes to it, and either owner can merge.

## How decisions are made

- **Lazy consensus.** A proposal in an issue or pull request stands unless an
  owner objects within seven days.
- **Equal owners.** Either owner can block a change. A blocked change stays open
  until both agree or the objection lapses as described below. It does not land
  while they disagree, and the reason is written in the issue. Neither owner can
  override the other.
- **No tie-break.** There is no vote and no casting decision. A disagreement is
  a block, not a vote.
- **Technical disagreements.** The owner who has worked on the area leads the
  discussion. If the owners still disagree, the change waits. Status quo wins
  for as long as the block stands.
- **Blocks must be reasoned.** An objection is written in the issue with a
  reason. If the objecting owner does not restate the reason within 30 days of
  the objection, the change may proceed and the objection stays on record.
- **The one exception.** A minimal fix for a live vulnerability may land with
  one owner's approval when waiting is itself the risk. This is the only
  exception to the block rule. The other owner is told before the merge, the
  merge and the reason are recorded in the issue, and the block resumes for
  anything beyond the minimal fix.
- **Final escape.** Either owner may fork the project under AGPL-3.0-only. The
  fork must publish its source, and it may not use the project name. A block
  never traps work.
- **Behaviour changes.** A change to a command reply, error, or stored format
  needs the test and the compatibility note that [AGENTS.md](AGENTS.md) requires
  before it merges.
- **Large changes** start as an issue, not as a surprise pull request. That is
  how the project avoids work that cannot merge.

## Contributions and licensing

- **No CLA.** There is no contributor licence agreement, no copyright
  assignment, and no developer certificate of origin sign-off. Opening a pull
  request is the confirmation.
- **Inbound equals outbound.** A contribution is accepted under the licence the
  project already uses, [AGPL-3.0-only](LICENSE). Contributors keep their
  copyright.
- **No relicensing promise.** Changing the licence would need the consent of
  every contributor whose code is still included.
- **Provenance.** By opening a pull request you confirm that you wrote the code
  or have the right to submit it, and that it is not copied or translated from
  Redis or a Redis fork. ReRu is a clean-room implementation; see
  [CONTRIBUTING.md](CONTRIBUTING.md).
- **Credit.** Contributions are recorded in [CONTRIBUTORS.md](CONTRIBUTORS.md).

## Becoming a maintainer

- The usual bar is three merged pull requests and two substantive reviews.
  Owners decide case by case.
- An invitation from both owners, recorded in the table above.
- There is no separate reviewer role yet; reviews come from the owners.
- An owner or maintainer may step back at any time. The other owner removes
  access and updates the table.

## Releases

- Changes accumulate in [CHANGELOG.md](CHANGELOG.md) under `Unreleased`.
- While pre-1.0 the owners cut a release at least monthly.
- Either owner can cut a release: move those entries under a dated version, bump
  the crate version, and tag the commit.
- Pre-1.0 releases may include breaking changes. The changelog says so when they
  do.

## Response times

- New issues and pull requests get an answer within 48 hours. If that is not
  possible, the first response arrives within seven days.
- Security reports follow [SECURITY.md](SECURITY.md); do not open a public issue.
- An issue that needs information the reporter has not supplied is closed after
  30 days without a reply. It can be reopened. Stale claims close earlier, as
  [CONTRIBUTING.md](CONTRIBUTING.md#claiming-an-issue) sets out.

## Code of conduct

[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies to every project space. The
owners enforce it. Reports go through the private channel that document names,
and a report about one owner goes to the other.

## Changing this document

Open a pull request that explains the reason. Both owners must approve it, and
the change is recorded in the changelog. Governance changes are exempt from the
30-day lapse in the block rule and always need both approvals.
