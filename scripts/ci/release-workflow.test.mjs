import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

// Reconciled from #731: automatic tags use #611's publisher, while every
// publishing path keeps manual full-suite evidence on the exact commit.
const workflows = new URL("../../.github/workflows/", import.meta.url);
const release = readFileSync(new URL("release.yml", workflows), "utf8");
const auto = readFileSync(new URL("auto-tag.yml", workflows), "utf8");
const job = (text, name) => text.match(new RegExp(`^  ${name}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:|$(?![\\s\\S]))`, "m"))?.[1];
const step = (text, name) => {
  const block = text.split("      - name: ").find((s) => s.startsWith(name + "\n"));
  assert.ok(block, `missing workflow step: ${name}`);
  const script = block.split("        run: |\n")[1];
  assert.ok(script, `missing shell body: ${name}`);
  const lines = [];
  for (const line of script.split("\n")) {
    if (line.trim() && !line.startsWith("          ")) break;
    lines.push(line.slice(10));
  }
  return lines.join("\n");
};
const sha = "a".repeat(40);
const other = "b".repeat(40);
const evidence = "Require a successful workflow_dispatch full suite on this exact commit";
const required = "Require every required check green on this exact commit";

// Execute the real workflow shell against local API responses. gh's jq filter
// is evaluated by jq itself, so wrong-SHA/event/status fixtures exercise the
// actual evidence selection. No network or publisher credentials are used.
function fixture(t, data = {}, runtimeWithSpaces = false) {
  const root = mkdtempSync(join(tmpdir(), "locus-release-gate-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const config = {
    main: sha, parent: other, version: "0.7.0", oldVersion: "0.6.2",
    rules: [{ type: "required_status_checks", parameters: { required_status_checks: [{ context: "CI Gate" }] } }],
    classic: {}, checks: [{ name: "CI Gate", status: "completed", conclusion: "success", app: { id: 15368 } }],
    runs: [{ event: "workflow_dispatch", status: "completed", conclusion: "success", head_sha: sha }],
    ...data,
  };
  writeFileSync(join(root, "config.json"), JSON.stringify(config));
  // A shebang cannot quote an executable path. Preserve the commissioned
  // runtime as one argv value through a fixed shell and a separate JS file.
  const runtime = runtimeWithSpaces ? join(root, "node runtime's fixture") : process.execPath;
  if (runtimeWithSpaces) {
    writeFileSync(runtime, '#!/bin/sh\nexec "$FIXTURE_PARENT_NODE" "$@"\n');
    chmodSync(runtime, 0o755);
  }
  writeFileSync(join(root, "gh"), '#!/bin/sh\nexec "$FIXTURE_NODE" "$FIXTURE_SCRIPT" "$@"\n');
  writeFileSync(join(root, "gh.cjs"), `const fs = require('node:fs');
const { spawnSync } = require('node:child_process');
const c = JSON.parse(fs.readFileSync(process.env.FIXTURE_CONFIG));
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FIXTURE_LOG, JSON.stringify(args) + '\\n');
const url = args.find(a => a.startsWith('repos/')) ?? '';
if (c.fail && url.includes(c.fail)) process.exit(1);
let value;
if (url.includes('/rules/')) value = c.rules;
else if (url.includes('/branches/')) value = c.classic;
else if (url.includes('/check-runs?')) value = { check_runs: c.checks };
else if (url.includes('/actions/workflows/ci.yml/runs?')) value = { total_count: 100, workflow_runs: c.runs };
else if (url.endsWith('/commits/main')) value = { sha: c.main };
else if (url.includes('/commits/')) value = { parents: [{ sha: c.parent }] };
else if (url.includes('/contents/')) {
  console.log('[workspace.package]\\nversion = "' + (url.endsWith(c.parent) ? c.oldVersion : c.version) + '"');
  process.exit(0);
} else if (url.endsWith('/git/refs')) process.exit(0);
else process.exit(2);
const i = args.findIndex(a => a === '-q' || a === '--jq');
const out = spawnSync('jq', ['-r', args[i + 1]], { input: JSON.stringify(value), encoding: 'utf8' });
process.stdout.write(out.stdout); process.stderr.write(out.stderr); process.exit(out.status ?? 1);
`);
  writeFileSync(join(root, "git"), `#!/bin/sh\n${config.tag ? `printf '%s\\trefs/tags/v0.7.0\\n' '${config.tag}'` : `exit ${config.gitError ?? 2}`}\n`);
  writeFileSync(join(root, "sleep"), "#!/bin/sh\nexit 0\n");
  for (const name of ["gh", "git", "sleep"]) chmodSync(join(root, name), 0o755);
  const output = join(root, "output");
  const log = join(root, "calls");
  writeFileSync(output, ""); writeFileSync(log, "");
  return {
    run(text, name, env = {}) {
      // Bind the three fixture commands explicitly; never put a writable
      // temporary directory on PATH while executing the real gate body.
      const commands = 'gh() { "$FIXTURE_GH" "$@"; }\ngit() { "$FIXTURE_GIT" "$@"; }\nsleep() { "$FIXTURE_SLEEP" "$@"; }\n';
      return spawnSync("/bin/bash", ["-c", commands + step(text, name)], {
        encoding: "utf8", timeout: 15000,
        env: { ...process.env, PATH: "/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin", HOME: root,
          FIXTURE_GH: join(root, "gh"), FIXTURE_GIT: join(root, "git"), FIXTURE_SLEEP: join(root, "sleep"),
          FIXTURE_CONFIG: join(root, "config.json"), FIXTURE_LOG: log,
          FIXTURE_NODE: runtime, FIXTURE_PARENT_NODE: process.execPath, FIXTURE_SCRIPT: join(root, "gh.cjs"),
          REPO: "ashlrai/locus", BRANCH: "main", SHA: sha, VERSION: "0.7.0",
          VERSION_FILE: "Cargo.toml", ACTIONS_APP_ID: "15368", TRIGGER: "workflow_dispatch",
          EVENT: "push", CI_WORKFLOW: "ci.yml", GITHUB_OUTPUT: output,
          GITHUB_STEP_SUMMARY: join(root, "summary"), ...env },
      });
    },
    output: () => readFileSync(output, "utf8"),
    calls: () => readFileSync(log, "utf8"),
  };
}

test("release triggers preserve tag, manual retry and PR dry run; no direct-main bypass", () => {
  assert.match(release, /tags:\n\s+- "v\*"/);
  assert.match(release, /^  workflow_dispatch:$/m);
  assert.match(release, /pull_request:\n\s+paths:/);
  assert.doesNotMatch(release, /branches: \[main\]|^  (workflow_run|pull_request_target):/m);
  assert.match(job(release, "build"), /needs: \[full-suite-evidence\]/);
  for (const name of ["release", "npm-publish"]) assert.match(job(release, name), /needs: \[full-suite-evidence,/);
  assert.equal(release.match(/ref: \$\{\{ needs.full-suite-evidence.outputs.sha \}\}/g)?.length, 3);
  assert.match(release, /node scripts\/release-version.mjs check --tag "\$RELEASE_TAG"/);
  assert.match(release, /cargo run --release --locked -p locus-cli --bin locus -- adapter registry export/);
});

test("PR never publishes; existing OIDC stays confined to npm", () => {
  assert.match(job(release, "release"), /name: Create release\n\s+if: github.event_name == 'push' \|\| github.event_name == 'workflow_dispatch'/);
  assert.match(job(release, "npm-publish"), /if: \(github.event_name == 'push' && startsWith\(github.ref, 'refs\/tags\/v'\)\) \|\| github.event_name == 'workflow_dispatch'/);
  assert.equal(release.match(/id-token: write/g)?.length, 1);
  assert.match(job(release, "npm-publish"), /id-token: write/);
  assert.match(release, /token-based npm auth must not be configured/);
  const live = release.split("\n").filter((l) => !/^\s*#/.test(l)).join("\n");
  assert.doesNotMatch(live, /NPM_TOKEN: |NODE_AUTH_TOKEN|secrets\.(?!GITHUB_TOKEN)/);
});

test("privileged auto-tag trusts only main manual CI and mints only after full evidence", () => {
  assert.match(auto, /workflow_run.event == 'workflow_dispatch'/);
  assert.match(auto, /workflow_run.head_branch == 'main'/);
  assert.match(auto, /workflow_run.head_repository.full_name == github.repository/);
  assert.doesNotMatch(auto, /uses: actions\/checkout|download-artifact|pull_request_target|git push|git tag|secrets\.(?!ASHLR_RELEASE_APP_KEY)/);
  assert.match(auto, /repositories: \$\{\{ github.event.repository.name \}\}\n\s+permission-contents: write/);
  assert.match(auto, /uses: actions\/create-github-app-token@[0-9a-f]{40}/);
  const privileged = auto.slice(auto.indexOf("      - name: Mint ashlr-release"));
  assert.equal(privileged.match(/if: steps.evidence.outputs.qualified == 'true'/g)?.length, 3);
  assert.match(privileged, /gh api -X POST .*\/git\/refs/);
  assert.doesNotMatch(privileged, /-X (PATCH|DELETE)|force/);
});

for (const [name, overrides] of [
  ["missing", { runs: [] }], ["wrong SHA", { runs: [{ event: "workflow_dispatch", status: "completed", conclusion: "success", head_sha: other }] }],
  ...["merge_group", "schedule", "pull_request"].map((event) => [event, { runs: [{ event, status: "completed", conclusion: "success", head_sha: sha }] }]),
  ...["failure", "skipped", "neutral"].map((conclusion) => [conclusion, { runs: [{ event: "workflow_dispatch", status: "completed", conclusion, head_sha: sha }] }]),
  ["pending", { runs: [{ event: "workflow_dispatch", status: "in_progress", conclusion: "success", head_sha: sha }] }],
  ["API failure", { fail: "/actions/" }],
]) {
  test(`both release gates reject ${name} evidence`, (t) => {
    const f = fixture(t, overrides);
    assert.notEqual(f.run(auto, evidence).status, 0);
    assert.notEqual(f.run(release, "Require a successful workflow_dispatch CI run on this sha").status, 0);
    assert.doesNotMatch(f.output(), /qualified=true/);
  });
}

test("push defers missing manual evidence without qualifying or hiding API errors", (t) => {
  const missing = fixture(t, { runs: [] });
  const result = missing.run(auto, evidence, { TRIGGER: "push" });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /workflow_run trigger re-evaluates/);
  assert.doesNotMatch(missing.output(), /qualified=true/);
  const failed = fixture(t, { fail: "/actions/" });
  assert.notEqual(failed.run(auto, evidence, { TRIGGER: "push" }).status, 0);
  assert.doesNotMatch(failed.output(), /qualified=true/);
});

test("exact successful manual evidence qualifies both gates; PR dry run needs none", (t) => {
  const f = fixture(t);
  assert.equal(f.run(auto, evidence).status, 0);
  assert.match(f.output(), /qualified=true/);
  assert.equal(f.run(release, "Require a successful workflow_dispatch CI run on this sha").status, 0);
  const pr = fixture(t, { fail: "/actions/" });
  assert.equal(pr.run(release, "Require a successful workflow_dispatch CI run on this sha", { EVENT: "pull_request" }).status, 0);
  assert.equal(pr.calls(), "");
});

for (const [name, data] of [
  ["rules API failure", { fail: "/rules/" }], ["classic API failure", { fail: "/branches/" }],
  ["missing required checks", { rules: [] }], ["missing check run", { checks: [] }],
  ["foreign app", { checks: [{ name: "CI Gate", status: "completed", conclusion: "success", app: { id: 1 } }] }],
  ...["skipped", "neutral", "failure"].map((conclusion) => [conclusion, { checks: [{ name: "CI Gate", status: "completed", conclusion, app: { id: 15368 } }] }]),
]) {
  test(`required-check gate rejects ${name}`, (t) => {
    const f = fixture(t, data);
    assert.notEqual(f.run(auto, required).status, 0);
    assert.doesNotMatch(f.output(), /green=true/);
  });
}

test("required GitHub Actions checks succeed; any duplicate failure rejects", (t) => {
  const f = fixture(t);
  assert.equal(f.run(auto, required).status, 0);
  assert.match(f.output(), /green=true/);
  const duplicate = fixture(t, { checks: [
    { name: "CI Gate", status: "completed", conclusion: "success", app: { id: 15368 } },
    { name: "CI Gate", status: "completed", conclusion: "failure", app: { id: 15368 } },
  ] });
  assert.notEqual(duplicate.run(auto, required).status, 0);
});

test("main drift rejects detection and creation before tag POST", (t) => {
  const f = fixture(t, { main: other });
  assert.notEqual(f.run(auto, "Detect version change").status, 0);
  assert.notEqual(f.run(auto, "Create tag v${{ steps.version.outputs.version }}").status, 0);
  assert.doesNotMatch(f.calls(), /POST/);
});

test("version detection fails closed on parent read failure and detects a real bump", (t) => {
  const broken = fixture(t, { fail: `/contents/Cargo.toml?ref=${other}` });
  assert.notEqual(broken.run(auto, "Detect version change").status, 0);
  assert.doesNotMatch(broken.output(), /changed=true/);
  const bump = fixture(t);
  const result = bump.run(auto, "Detect version change");
  assert.equal(result.status, 0, result.stderr);
  assert.match(bump.output(), /version=0.7.0\nchanged=true/);
  const unchanged = fixture(t, { oldVersion: "0.7.0" });
  assert.equal(unchanged.run(auto, "Detect version change", { TRIGGER: "workflow_run" }).status, 0);
  assert.match(unchanged.output(), /changed=false/);
});

test("qualified current-main tag creation uses only immutable POST; existing tag is untouched", (t) => {
  const f = fixture(t);
  assert.equal(f.run(auto, "Create tag v${{ steps.version.outputs.version }}").status, 0);
  assert.match(f.calls(), /POST.*git\/refs.*refs\/tags\/v0.7.0/);
  const existing = fixture(t, { tag: sha });
  assert.equal(existing.run(auto, "Create tag v${{ steps.version.outputs.version }}").status, 0);
  assert.doesNotMatch(existing.calls(), /POST/);
});

test("tag lookup errors and a tag raced onto another commit block publication", (t) => {
  const broken = fixture(t, { gitError: 128 });
  assert.notEqual(broken.run(auto, "Detect version change").status, 0);
  assert.notEqual(broken.run(auto, "Create tag v${{ steps.version.outputs.version }}").status, 0);
  assert.doesNotMatch(broken.calls(), /POST/);
  const conflict = fixture(t, { tag: other });
  assert.notEqual(conflict.run(auto, "Create tag v${{ steps.version.outputs.version }}").status, 0);
  assert.doesNotMatch(conflict.calls(), /POST/);
});

test("manual release resolves and freezes tag commit; invalid/missing tag fails", (t) => {
  const f = fixture(t, { tag: sha });
  assert.equal(f.run(release, "Resolve the release tag on manual dispatch").status, 0);
  assert.match(f.output(), new RegExp(`sha=${sha}`));
  assert.notEqual(f.run(release, "Resolve the release tag on manual dispatch", { VERSION: "0.7.0;exit 0" }).status, 0);
  assert.notEqual(fixture(t).run(release, "Resolve the release tag on manual dispatch").status, 0);
});

test("workflow evidence fixture executes the qualified runtime even with spaces and apostrophes", (t) => {
  const f = fixture(t, {}, true);
  const result = f.run(auto, evidence);
  assert.equal(result.status, 0, result.stderr);
  assert.match(f.output(), /qualified=true/);
  assert.match(f.calls(), /actions\/workflows\/ci\.yml/);
});
