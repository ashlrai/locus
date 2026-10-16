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

Additive fields (tolerate unknown keys): `locus whoami --json` and the doctor
`pin` slice now carry `expires_in_secs` (seconds until pin expiry, `0` when
expired). Doctor also emits a transient `pin_expiring` **WARN** finding during
the last 5 minutes of any pin — fleet dashboards should treat it as a nudge to
re-pin (`locus enter <alias>`), not an incident.

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
| alias, tenant, binding_id | Credential locator names or values |
| Credential presence/source metadata | Worker env secret maps |
| finding codes / next_steps | Approval digests as secrets |

---

## `checkLocusFirm` (hub-only soft-warn — hub #277)

**Observational only.** Surfaces a non-blocking “consider firm for production” nudge when a fleet has enrolled repos, Locus is installed, and `locus.firm` is not yet enabled. **Never hard-blocks mutate.** Not exported from monorepo [`locus.ts`](./locus.ts) — lives in hub doctor/readiness only.

Contract sketch (hub production; illustrative):

```ts
// hub: doctor / readiness — do NOT port into monorepo locus.ts

export type FirmDoctorCheck = {
  id: "locus-firm";
  ok: boolean;          // true = pass (no nudge) or firm already on
  severity: "info" | "warn";
  detail: string;
  fix?: string;         // e.g. `ashlr config set locus.firm true`
};

/**
 * Soft-warn when ALL of:
 *   1. enrolled repos > 0
 *   2. locus CLI available
 *   3. config.locus.firm is not true
 * → id "locus-firm": consider locus.firm for production
 *
 * Never a blocker. Doctor exit stays driven by fails, not this warn.
 * Monorepo / 0-enrolled / locus-absent / firm=true → quiet pass.
 */
export function checkLocusFirm(opts: {
  enrolledCount: number;
  locusAvailable: boolean;
  firm: boolean; // config.locus.firm === true
}): FirmDoctorCheck {
  if (opts.enrolledCount > 0 && opts.locusAvailable && !opts.firm) {
    return {
      id: "locus-firm",
      ok: true, // non-blocking — still "ok" so doctor exit is not fail
      severity: "warn",
      detail: "consider locus.firm for production",
      fix: "ashlr config set locus.firm true  # or { locus: { firm: true } }",
    };
  }
  return {
    id: "locus-firm",
    ok: true,
    severity: "info",
    detail: opts.firm
      ? "locus.firm enabled"
      : "firm soft-warn quiet (0 enrolled, locus absent, or firm on)",
  };
}
```

| Case | Result |
|------|--------|
| Fresh install (0 enrolled) | pass / no warn |
| Locus absent | pass on firm check (`checkLocus` handles install) |
| `locus.firm=true` | pass / info |
| enrolled>0 + locus + firm false | **warn only** (`locus-firm`) — never blocks mutate |
| Degraded enrollment probe | quiet `info` — pass `enrolledCount: 0`; the sketch has no separate skip branch |

Hub production fleet checklist: `docs/LOCUS-FIRM-FLEET.md` on [ashlr-hub](https://github.com/ashlrai/ashlr-hub) (PR [#277](https://github.com/ashlrai/ashlr-hub/pull/277)).

---

## Checklist for the hub PR

- [ ] `checkLocus()` calls `locusDoctorLine()` (or equivalent shell-out)
- [ ] Aggregated doctor includes `id: "locus"`
- [ ] Fail detail includes `fix` command when `ok === false`
- [ ] Mutating jobs call `ensureLocusReady()` (not doctor-only soft warn)
- [ ] `checkLocusFirm` (if present) is **warn-only** — never contributes to mutate hard-block
- [ ] `LOCUS_HOME` overridable for tests
- [ ] No secret values in doctor JSON/logs
