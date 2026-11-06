# Installer archive requirements

This is a requirements record for [issue #169](https://github.com/ashlrai/locus/issues/169),
not a specification of published assets or proof that an installer is hosted.

## Goal and constraints from the issue

Provide a **one-command shell installation** path that downloads and verifies a
release archive, in the shape of the direnv installer. It must work for the
intended machines without npm or a Rust toolchain; neither npm installation nor
a local Rust build can be a prerequisite for this path.

The issue motivates the request with measurements from one Linux x86-64 VM:
npm needed two commands, while Cargo took longer and failed on an older default
toolchain. These observations are context, not timing thresholds or a platform
support guarantee. See [first-run results](FIRST-RUN.md) and
[benchmarks](BENCHMARKS.md).

## Archive questions to resolve

The issue does not specify the following; each remains an open question for a
published installer and release process:

- **Format and layout:** What archive format, filename convention and internal
  layout are required? Which files and executables must it contain, and must
  both `locus` and `locus-mcp` be installed?
- **Targets and compatibility:** Which operating systems, architectures and
  libc baselines will have archives, and which native hosts will be tested?
- **Release selection and delivery:** Where will the command and archives be
  hosted? How are versions selected, and which releases must contain assets for
  each supported target?
- **Verification and trust:** What verification material and algorithm are
  required, how is that material authenticated, and what should happen if it
  is missing or verification fails? A checksum obtained alongside an archive
  alone does not establish publisher authenticity.
- **Installation behavior:** What destination, permissions, PATH changes,
  upgrade and rollback behavior, and non-npm/non-Rust prerequisites are
  acceptable? How is a successful installation and first run confirmed?

## Existing implementation and evidence

The current [installer](../scripts/install.sh) makes concrete choices, including
`.tar.gz` archives and SHA-256 sidecars; those choices do not resolve the issue's
open questions for a published release. The
[archive selection guide](shell-installer-archive-selection.md) and
[platform matrix](installer-platform-matrix.md) describe current behavior and
its limits. The [acceptance checklist](installer-acceptance.md) distinguishes
offline fixture tests from evidence of hosted assets and native execution.
