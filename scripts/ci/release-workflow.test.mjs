import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import test from "node:test";

const dir = new URL("../../.github/workflows/", import.meta.url);
const release = readFileSync(new URL("release.yml", dir), "utf8");
const job = (name) => release.match(new RegExp(`^  ${name}:\\n([\\s\\S]*?)(?=^  [a-z][\\w-]*:|$(?![\\s\\S]))`, "m"))?.[1];

test("release.yml triggers: tag, main version change, dispatch, and the PR dry run", () => {
  assert.match(release, /tags:\n\s+- "v\*"/);
  assert.match(release, /branches: \[main\]\n\s+paths:\n\s+- "Cargo.toml"/);
  assert.match(release, /^  workflow_dispatch:$/m);
  assert.match(release, /pull_request:\n\s+paths:\n\s+- "\.github\/workflows\/release\.yml"/);
  assert.doesNotMatch(release, /workflow_run|pull_request_target/);
});

test("every build, release and publish job is gated by the plan", () => {
  assert.match(job("build"), /needs: \[plan\]\n\s+if: needs\.plan\.outputs\.run == 'true'/);
  assert.match(job("release"), /needs: \[plan, build\]/);
  assert.match(job("npm-publish"), /needs: \[plan, release\]/);
});

test("publishing never happens on a pull request", () => {
  assert.match(job("npm-publish"), /if: github\.event_name != 'pull_request'/);
  assert.match(job("release"), /name: Create release\n\s+if: github\.event_name != 'pull_request'/);
  assert.match(job("plan"), /if \[ "\$EVENT" = "pull_request" \]/);
});

test("the plan checks version carriers and fails closed without green CI", () => {
  const plan = job("plan");
  assert.match(plan, /node scripts\/release-version\.mjs check\n/);
  assert.match(plan, /check --tag "\$REF_NAME"/);
  assert.match(plan, /automatic releases run from main only/);
  assert.match(plan, /\$\{n\}.*-lt 1|"\$n" -lt 1/);
  // A manual tag keeps the stricter full-suite evidence.
  assert.match(plan, /events="workflow_dispatch"/);
  assert.match(plan, /events="merge_group workflow_dispatch"/);
});

test("id-token is confined to npm-publish; no token or secret is added", () => {
  assert.equal(release.match(/id-token: write/g)?.length, 1);
  assert.match(job("npm-publish"), /id-token: write/);
  // Comments excluded: the file keeps a commented signing-key TODO.
  const live = release.split("\n").filter((l) => !/^\s*#/.test(l)).join("\n");
  assert.doesNotMatch(live, /NPM_TOKEN: |NODE_AUTH_TOKEN|secrets\.(?!GITHUB_TOKEN)/);
  assert.match(release, /token-based npm auth must not be configured/);
});

test("no active workflow uses the App token or pull_request_target until reviewed", () => {
  for (const f of readdirSync(dir)) {
    const text = readFileSync(new URL(f, dir), "utf8");
    assert.doesNotMatch(text, /ASHLR_RELEASE_APP_KEY|pull_request_target/, f);
  }
});
