import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(new URL("../../.github/workflows/ci.yml", import.meta.url), "utf8");
const fullJob = workflow.match(/^  check:\n([\s\S]*?)(?=^  [a-z][\w-]*:|$(?![\s\S]))/m)?.[1];
const e2eStep = fullJob?.match(/      - name: Release end-to-end gate\n([\s\S]*?)(?=      - name:|$(?![\s\S]))/m)?.[1];

test("release e2e runs only in the existing scheduled/manual Linux full job", () => {
  assert.ok(fullJob);
  assert.match(fullJob, /if: github.event_name == 'schedule' \|\| github.event_name == 'workflow_dispatch'/);
  assert.match(fullJob, /runs-on: ubuntu-latest/);
  assert.equal(workflow.match(/bash scripts\/e2e\.sh/g)?.length, 1);
  assert.ok(e2eStep);
});

test("e2e follows release build and cannot silently continue on failure", () => {
  assert.ok(fullJob.indexOf("name: Release build") < fullJob.indexOf("name: Release end-to-end gate"));
  assert.match(e2eStep, /set -euo pipefail/);
  assert.match(e2eStep, /bash scripts\/e2e\.sh/);
  assert.doesNotMatch(e2eStep, /continue-on-error:|\|\|\s*true|\n\s+if:/);
});

test("gate requires fixture tools and executes this wiring check", () => {
  assert.match(e2eStep, /for tool in bash cargo python3 curl git jq; do/);
  assert.match(e2eStep, /command -v "\$tool"/);
  assert.match(e2eStep, /node --test scripts\/ci\/e2e-release-gate.test.mjs/);
});

test("wiring adds no credentials, publication command or permission override", () => {
  assert.doesNotMatch(e2eStep, /secrets\.|permissions:|gh release|npm publish|git push|workflow_dispatch/);
  assert.match(workflow, /^permissions:\n  contents: read$/m);
  assert.match(workflow, /needs: \[fast, wrappers, sandbox, secret-scan, check, msrv, secret-shape\]/);
});
