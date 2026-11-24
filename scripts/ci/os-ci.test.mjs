import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { firstFailure } from "./fail-report.mjs";
import { parseSha256Sidecar, releaseFromApiBody, releaseShipsArchive, sha256File, verifyBeforeExecute } from "./install-smoke.mjs";
import { gateProblems } from "./os-gate.mjs";
import { selectPackages } from "./os-cargo.mjs";
import { isInstallerPath, isPlatformPath, plan } from "./os-path-gate.mjs";

test("docs-only diffs skip windows and macos", () => {
  const files = ["README.md", "docs/architecture.md"];
  assert.equal(files.some(isPlatformPath), false);
  assert.equal(files.some(isInstallerPath), false);
  const decided = plan({ event: "pull_request", schedule: "", smokeCron: "23 16 * * *", files });
  assert.equal(decided.fast_windows, false);
  assert.equal(decided.smoke_windows, false);
  assert.equal(decided.smoke_linux, false);
});

test("installer paths run every install smoke, crate paths run only the fast legs", () => {
  const installer = plan({
    event: "merge_group",
    schedule: "",
    smokeCron: "23 16 * * *",
    files: ["scripts/install.sh"],
  });
  assert.equal(installer.smoke_linux, true);
  assert.equal(installer.smoke_windows, true);
  assert.equal(installer.smoke_macos, true);
  assert.equal(installer.fast_windows, true);

  const code = plan({
    event: "pull_request",
    schedule: "",
    smokeCron: "23 16 * * *",
    files: ["crates/locus-core/src/lib.rs"],
  });
  assert.equal(code.fast_macos, true);
  assert.equal(code.smoke_macos, false);
  assert.equal(code.smoke_linux, false);
});

test("the smoke schedule runs all three smokes and not the fast legs", () => {
  const decided = plan({
    event: "schedule",
    schedule: "23 16 * * *",
    smokeCron: "23 16 * * *",
    files: [],
  });
  assert.equal(decided.smoke_linux && decided.smoke_windows && decided.smoke_macos, true);
  assert.equal(decided.fast_windows, false);
  assert.equal(decided.fast_macos, false);
});

test("a path-gate skip passes the OS gate and a failure does not", () => {
  assert.deepEqual(gateProblems({ smoke: { result: "skipped" }, fast: { result: "success" } }), []);
  assert.deepEqual(gateProblems({ smoke: { result: "failure" } }), ["smoke=failure"]);
  assert.deepEqual(gateProblems({ smoke: { result: "cancelled" } }), ["smoke=cancelled"]);
});

test("the first panic becomes file:line", () => {
  const found = firstFailure("thread 'tests::boot' panicked at crates/locus-core/src/lib.rs:12:5:\nnope\n");
  assert.equal(found.file, "crates/locus-core/src/lib.rs");
  assert.equal(found.line, "12");
});

test("a manifest warning is not reported ahead of a later error", () => {
  const found = firstFailure([
    "warning: unused field `homepage`",
    "  --> Cargo.toml:16:1",
    "error: linking with link.exe failed: exit code: 1",
    "  --> crates/locus-cli/src/main.rs:1:1",
  ].join("\n"));
  assert.equal(found.file, "crates/locus-cli/src/main.rs");
  assert.equal(found.line, "1");
});

test("checksum verification fails closed before a caller could execute", () => {
  const dir = mkdtempSync(join(tmpdir(), "locus-sum-"));
  const file = join(dir, "locus");
  writeFileSync(file, "#!/bin/sh\necho ok\n");
  assert.equal(verifyBeforeExecute(file), sha256File(file));
  assert.throws(() => parseSha256Sidecar("not a sidecar", "locus"), /invalid checksum sidecar/);
  assert.throws(() => parseSha256Sidecar("abcd  other\nffff  locus\n", "locus"), /exactly one record/);
});

test("release tag parsing rejects a missing tag", () => {
  assert.equal(releaseFromApiBody({ tag_name: "v0.7.2" }).version, "0.7.2");
  assert.throws(() => releaseFromApiBody({}), /unexpected release tag/);
});

test("a release that lists no archive for this host skips only the archive check", () => {
  const win = "locus-x86_64-pc-windows-msvc.zip";
  const v070 = releaseFromApiBody({
    tag_name: "v0.7.0",
    assets: [{ name: "locus-x86_64-unknown-linux-gnu.tar.gz" }, { name: "locus-x86_64-unknown-linux-gnu.tar.gz.sha256" }],
  });
  assert.equal(releaseShipsArchive(v070, win), false);
  assert.equal(releaseShipsArchive(v070, "locus-x86_64-unknown-linux-gnu.tar.gz"), true);
  assert.equal(releaseShipsArchive(releaseFromApiBody({ tag_name: "v0.7.2", assets: [{ name: `${win}.sha256` }] }), win), true);
  assert.equal(releaseShipsArchive(releaseFromApiBody({ tag_name: "v0.7.2" }), win), true);
});

test("affected crates follow the diff and include dependents", () => {
  const metadata = {
    workspace_root: "/src",
    packages: [
      {
        name: "locus-core",
        manifest_path: "/src/crates/locus-core/Cargo.toml",
        dependencies: [],
      },
      {
        name: "locus-cli",
        manifest_path: "/src/crates/locus-cli/Cargo.toml",
        dependencies: [{ name: "locus-core" }],
      },
    ],
  };
  assert.deepEqual(selectPackages(["README.md"], metadata), []);
  assert.deepEqual(selectPackages(["crates/locus-core/src/lib.rs"], metadata), ["locus-cli", "locus-core"]);
  assert.deepEqual(selectPackages(["Cargo.lock"], metadata), ["locus-cli", "locus-core"]);
});

test("workflow schedule strings match the smoke cron env", () => {
  const yml = readFileSync(new URL("../../.github/workflows/ci.yml", import.meta.url), "utf8");
  const crons = [...yml.matchAll(/- cron: "([^"]+)"/g)].map((match) => match[1]);
  assert.ok(crons.includes("0 10 * * *"));
  assert.ok(crons.includes("23 16 * * *"));
  assert.match(yml, /INSTALL_SMOKE_CRON: "23 16 \* \* \*"/);
  assert.match(yml, /FULL_SUITE_CRON: "0 10 \* \* \*"/);
  const smoke = "23 16 * * *";
  const [minute, hour] = smoke.split(" ");
  assert.equal(["0", "57", "59"].includes(minute), false);
  assert.equal(["10", "22"].includes(hour), false);
});
