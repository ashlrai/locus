# ashlr doctor — `checkLocus`

How to add a Locus identity-plane check to **ashlr doctor** (or any hub health pane).

Drop-in helper: [`locus.ts`](./locus.ts) → `locusDoctorLine()` / `locusAgentReport()` / `ensureLocusReady()`.

---

## Goal

Surface a single line in `ashlr doctor` (or hub UI):

```
locus   ok|fail   status=ready pin=acme:acme-corp ready=true
```

Never print secrets. Prefer status + pin oneline + ready.

---

## Minimal integration

### 1. Copy or import the drop-in

```ts
// hub: src/core/integrations/locus.ts  (copy from this repo)
import {
  locusDoctorLine,
  locusAgentReport,
  ensureLocusReady,
  type LocusProbeResult,
} from "../integrations/locus";
```

### 2. Register `checkLocus` in the doctor runner

```ts
// hub: src/commands/doctor.ts  (illustrative)

export type DoctorCheck = {
  id: string;
  ok: boolean;
  detail: string;
  fix?: string;
  severity?: "info" | "warn" | "error";
};

export function checkLocus(): DoctorCheck {
  const line = locusDoctorLine();
  return {
    id: line.id, // "locus"
    ok: line.ok,
    detail: line.detail,
    fix: line.fix,
    severity: line.ok ? "info" : "error",
  };
}

// In the aggregated doctor list:
export function runDoctorChecks(): DoctorCheck[] {
  return [
    // … existing checks (git, node, phantom, …)
    checkLocus(),
  ];
}
```

### 3. Exit / verdict mapping

| Locus agent status | Doctor check `ok` | Suggested hub verdict impact |
|--------------------|-------------------|------------------------------|
| `ready` + healthy oneline | `true` | no change |
| `protected` | `false` | WARN (setup / pin) |
| `unsafe` / missing CLI | `false` | UNSAFE / hard fail for mutate jobs |

Agent report exit codes (for scripts):

| Exit | Meaning |
|------|---------|
| 0 | ready |
| 1 | protected |
| 2 | unsafe |

```ts
const probe = locusAgentReport();
// probe.exitCode === 0 | 1 | 2
// probe.gateOk === true only when ready + healthy pin
```

### 4. Human output example

```
ashlr doctor

  phantom   ok    vault unlocked, 12 names
  locus     ok    status=ready pin=acme:acme-corp ready=true
  node      ok    v22.x

  verdict: SAFE
```

Unpinned / incomplete:

```
  locus     FAIL  status=protected pin=unpinned ready=false
          fix: locus enter <alias> && locus agent setup --apply
```

---

## Full report path (optional)

When hub wants the full mission-control pane nested under doctor:

```ts
export function checkLocusDetailed(): DoctorCheck & {
  report?: ReturnType<typeof locusAgentReport>["report"];
} {
  const probe = locusAgentReport();
  const line = locusDoctorLine();
  return {
    ...line,
    report: probe.report ?? undefined,
    severity: probe.report?.status === "unsafe" ? "error" : line.ok ? "info" : "warn",
  };
}
```

Stable keys on `locus agent report --json`: see [`schema/agent-report.schema.json`](../../schema/agent-report.schema.json).

---

## Pre-flight for mutating hub jobs

Doctor is observational. For jobs that mutate infra, **also** hard-gate:

```ts
import { ensureLocusReady, LocusNotReadyError } from "../integrations/locus";

export async function beforeMutate() {
  try {
    ensureLocusReady();
  } catch (e) {
    if (e instanceof LocusNotReadyError) {
      // surface e.message + e.probe.report?.next_steps
      throw e;
    }
    throw e;
  }
}
```

Ephemeral job pin (no touch of human `active.json`):

```ts
import { withLocusSession, ensureLocusReady } from "../integrations/locus";

await withLocusSession("acme", async ({ env }) => {
  ensureLocusReady(env);
  // run job children with env
});
```

---

## Env for tests

```bash
export LOCUS_HOME=/tmp/ashlr-doctor-locus
export PATH="$HOME/.cargo/bin:$PATH"
locus init --with-samples
locus pin personal
# then run hub doctor — checkLocus should see pin=personal:personal
```

Never point CI doctor at the developer's real `~/.locus` when tests mint/teardown state.

---

## Secrets policy (doctor output)

| Safe in doctor detail | Never |
|-----------------------|--------|
| status, status_oneline, ready | API keys, PATs, tokens |
| alias, tenant, binding_id | Resolved `phm:` values |
| `credential_ref` **names** | Worker env secret maps |
| finding codes / next_steps | Approval digests as secrets |

---

## Checklist for the hub PR

- [ ] `checkLocus()` calls `locusDoctorLine()` (or equivalent shell-out)
- [ ] Aggregated doctor includes `id: "locus"`
- [ ] Fail detail includes `fix` command when `ok === false`
- [ ] Mutating jobs call `ensureLocusReady()` (not doctor-only soft warn)
- [ ] `LOCUS_HOME` overridable for tests
- [ ] No secret values in doctor JSON/logs
