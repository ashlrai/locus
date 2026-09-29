#!/usr/bin/env node
// Locus recipes, executed: two client identities on one machine, a
// per-directory allowlist, and a short-lived CI session. Runs in a
// disposable LOCUS_HOME with fake env: credentials; no provider is called.
//
//   node examples/recipes/run.mjs --locus /absolute/path/to/locus

import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { isAbsolute, join } from "node:path";

const args = process.argv.slice(2);
let locus = "locus";
if (args.length) {
  if (args[0] !== "--locus" || !args[1] || !isAbsolute(args[1])) {
    console.error("Usage: node examples/recipes/run.mjs [--locus <absolute-path>]");
    process.exit(1);
  }
  locus = args[1];
}

const root = mkdtempSync(join(tmpdir(), "locus-recipes-"));
const home = join(root, "home");
const locusHome = join(home, ".locus");
mkdirSync(join(locusHome, "bindings"), { recursive: true });

// Two bindings whose GitHub credential comes from env: refs (the CI/test
// scheme). Real setups use phm:NAME refs resolved through Phantom.
function binding(alias, tenant, envVar) {
  return `[binding]
id = "bnd_${alias}"
alias = "${alias}"
tenant = "${tenant}"

[binding.policy]
default = "allow"
max_ttl = "8h"

[[binding.providers]]
provider = "github"
account = "${tenant}"
credential_ref = "env:${envVar}"
scope = { orgs = ["${tenant}"], repos = ["${tenant}/*"] }
`;
}
writeFileSync(join(locusHome, "bindings", "acme.toml"), binding("acme", "acme-corp", "ACME_GH_TOKEN"));
writeFileSync(join(locusHome, "bindings", "personal.toml"), binding("personal", "personal", "PERSONAL_GH_TOKEN"));

const env = {};
for (const key of ["PATH", "Path", "SystemRoot", "SYSTEMROOT", "WINDIR", "COMSPEC", "PATHEXT"]) {
  if (process.env[key]) env[key] = process.env[key];
}
Object.assign(env, {
  HOME: home, USERPROFILE: home, LOCUS_HOME: locusHome, TMPDIR: root, TMP: root, TEMP: root,
  LOCUS_CONTROL_CAPABILITY: randomBytes(32).toString("hex"),
  ACME_GH_TOKEN: "fake-acme-token",
  PERSONAL_GH_TOKEN: "fake-personal-token",
  // Ambient identity an agent would otherwise inherit.
  GH_TOKEN: "ambient-token-from-last-gh-auth",
  AWS_PROFILE: "ambient-profile",
  NO_COLOR: "1",
});

const PROBE = "const e=process.env;console.log('PROBE '+JSON.stringify({gh:e.GH_TOKEN??null,aws:e.AWS_PROFILE??null,tenant:e.LOCUS_TENANT??null,acme:e.ACME_GH_TOKEN??null,personal:e.PERSONAL_GH_TOKEN??null}))";

class Skip extends Error {}
function locusRun(argv, { cwd = root, expectStatus = 0 } = {}) {
  const r = spawnSync(locus, argv, { cwd, env, encoding: "utf8", timeout: 30_000, windowsHide: true });
  if (r.error?.code === "ENOENT") throw new Skip(`${locus} is not installed; supply --locus <absolute-path>`);
  const out = `${r.stdout}${r.stderr}`;
  if (r.status !== expectStatus) throw new Error(`locus ${argv.slice(0, 3).join(" ")} exited ${r.status}, expected ${expectStatus}`);
  return out;
}
function probe(argv, opts) {
  const out = locusRun([...argv, "--", process.execPath, "-e", PROBE], opts);
  const line = out.split(/\r?\n/).find((l) => l.startsWith("PROBE "));
  if (!line) throw new Error("the child process did not run");
  return JSON.parse(line.slice(6));
}
function expect(seen, want, label) {
  for (const [k, v] of Object.entries(want)) {
    if (seen[k] !== v) throw new Error(`${label}: expected ${k}=${v}, got ${seen[k]}`);
  }
}

const steps = [
  ["locus run -b acme: the child gets only acme's token; ambient GH_TOKEN and AWS_PROFILE are scrubbed", () => {
    const seen = probe(["run", "-b", "acme"]);
    expect(seen, { gh: "fake-acme-token", aws: null, tenant: "acme-corp", acme: null, personal: null }, "acme");
    console.log(`     child saw GH_TOKEN=${seen.gh} LOCUS_TENANT=${seen.tenant} AWS_PROFILE=${seen.aws}`);
  }],
  ["locus run -b personal: same command, the other identity, nothing from acme", () => {
    const seen = probe(["run", "-b", "personal"]);
    expect(seen, { gh: "fake-personal-token", aws: null, tenant: "personal", acme: null }, "personal");
  }],
  ["a client directory's .locus.toml refuses the wrong binding", () => {
    const dir = join(root, "clients", "acme");
    mkdirSync(dir, { recursive: true });
    locusRun(["workspace", "--default", "acme", "--allow", "acme", "--require-pin"], { cwd: dir });
    if (!existsSync(join(dir, ".locus.toml"))) throw new Error(".locus.toml was not written");
    const refused = locusRun(["run", "-b", "personal", "--", process.execPath, "-e", PROBE], { cwd: dir, expectStatus: 1 });
    if (!/not allowed in this workspace/.test(refused)) throw new Error("personal was not refused in the acme workspace");
    if (refused.includes("PROBE ")) throw new Error("the refused command still ran");
    expect(probe(["run", "-b", "acme"], { cwd: dir }), { gh: "fake-acme-token" }, "acme in its workspace");
  }],
  ["locus ci run -b acme: a short-lived CI session with the same scrubbing", () => {
    const seen = probe(["ci", "run", "-b", "acme"]);
    expect(seen, { gh: "fake-acme-token", aws: null, tenant: "acme-corp" }, "ci run");
    if (existsSync(join(locusHome, "active.json"))) throw new Error("ci run changed the global pin (active.json)");
  }],
];

let code = 0;
try {
  for (const [name, fn] of steps) {
    fn();
    console.log(`PASS ${name}`);
  }
  console.log(`COMPLETE: locus recipes (${steps.length} checks)`);
} catch (error) {
  if (error instanceof Skip) {
    console.log(`SKIP ${error.message}\nINCOMPLETE: locus recipes`);
    code = 2;
  } else {
    console.log(`FAIL ${error.message}`);
    code = 1;
  }
} finally {
  rmSync(root, { recursive: true, force: true });
}
process.exit(code);
