# Archive installer acceptance evidence

[Issue #169](https://github.com/ashlrai/locus/issues/169) asks for a
one-command shell installer that downloads and verifies a release archive,
including on machines without npm or Rust. This page is the evidence-recording
companion to [archive installer acceptance](archive-installer-acceptance.md),
which owns the checklist of testable outcomes for the current script. Like it,
this is a review contract for a candidate installer and release, not a claim that
a published command or assets exist. It describes observable outcomes, not the
implementation.

Before testing, record the exact published command, installer revision,
release/tag, supported OS/architecture and runtime baselines, prerequisites,
installed commands, destination, and the verification method and its trust
source. Undecided items are release decisions, not established promises. Run
on disposable hosts and directories; do not use a real Locus home or credentials.
Leave a check unaccepted when its evidence is missing.

## Outcomes that need explicit evidence

- **One command, usable result:** On each declared supported native host, start
  with no installed Locus, npm, Node.js, Cargo, or Rust. Run the exact published
  command once, without a source build, credentials, interactive input, or
  elevated privileges, then run each installed command's version or equivalent
  noninteractive smoke check and confirm the intended release. Record any PATH
  guidance needed to invoke it in a fresh shell. Selection by a simulated
  platform alone does not establish native execution.
- **Verified before installed:** Observe that the archive selected for the host
  and release is downloaded and verified before its contents are installed or
  executed. Record the verification material and its trust source. A checksum
  fetched alongside an archive detects a mismatch but does not authenticate the
  publisher if both can be replaced; do not claim stronger provenance without
  independent evidence.
- **Failures leave an installation alone:** With controlled fixtures, make the
  archive differ from its verification material, and make that material missing
  or invalid; also exercise unsupported hosts, unavailable release assets,
  interrupted downloads, malformed archives, and an unwritable destination. Each
  exits nonzero with a useful diagnostic and does not execute the unverified
  payload. Seed an existing installation and report whether its executable bytes
  remain unchanged; check that paths outside the intended destination are not
  written. For verification mismatches or missing or invalid verification
  material, confirm the seeded executable bytes remain unchanged.
- **Repeat and upgrade:** Repeat the command and install a newer candidate over a
  seeded older release. Verify the resulting commands and versions, and record the
  behavior if replacement fails. Claim atomicity or rollback only when
  demonstrated for the complete installed set; the current script replaces
  `locus` and `locus-mcp` in two separate final renames.
- **Side effects:** Compare the disposable home and destination before and after
  success and failure. Confirm changes are limited to documented install
  artifacts and any disclosed temporary state; existing Locus configuration,
  credentials, and shell startup files are not silently modified.

## Evidence and scope

For every check record the date, platform and runtime baseline, exact command,
release and installer revisions, relevant asset/verification identity, exit
status, diagnostic or smoke result, and filesystem observations. Never record
secret values. Record failures and untested combinations rather than treating
missing evidence as a pass. Historical single-host measurements, when linked
from the issue, are context rather than current performance thresholds.

Offline fixture tests can exercise deterministic failures, but they cannot prove
that a hosted command and assets are available or that release binaries run
natively. Record fixture results separately from real-release, real-host results.
For the repository's current script, use the implementation-specific
[acceptance checklist](installer-acceptance.md),
[platform matrix](installer-platform-matrix.md), and
[failure matrix](installer-failure-matrix.md); their current behavior is not a
requirement that all future implementations use the same mechanism.
