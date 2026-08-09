# Agency certainty — identity vs epistemic layers

Short design note for the Ashlr stack narrative. Mechanism-first.

## The problem split

Delegating work to coding agents fails in two different ways:

| Failure | Question | Wrong answer looks like |
|---------|----------|-------------------------|
| **Identity** | *As whom, against which tenant, right now?* | Mutate Acme while pinned personal; ambient `gh` / Supabase |
| **Epistemic** | *Is this claim true enough to act?* | Hallucinated schema, stale docs, confident wrong plan |
| **Secret surface** | *Can this secret enter the model?* | PAT in context, tool result leaking keys |

These are orthogonal. Mixing them into one “be careful” prompt loses mechanical guarantees.

## Layers (current + next)

```
┌──────────────────────────────────────────────────────────┐
│  Verification plane (next)                                 │
│  Epistemic certainty: evidence, tool-grounded checks,      │
│  approval of *claims* before high-blast actions            │
└────────────────────────────┬─────────────────────────────┘
                             │ only acts when identity allows
┌────────────────────────────▼─────────────────────────────┐
│  Locus — identity plane                                    │
│  Binding × sealed session × policy × scope freeze          │
│  Wrong account impossible; agent cannot re-pin             │
└────────────────────────────┬─────────────────────────────┘
                             │ credentials never in model
┌────────────────────────────▼─────────────────────────────┐
│  Phantom — secret plane                                    │
│  phm: refs; reveal only into worker/proxy env              │
└──────────────────────────────────────────────────────────┘
```

| Layer | Product | Certainty kind | Gate mechanism |
|-------|---------|----------------|----------------|
| Secret | **Phantom** | Confidentiality | Token/proxy; values stay out of context |
| Identity | **Locus** | Agency / tenancy | HMAC pin, exclusive catalog, scrub ambient |
| Verification | **Next** | Epistemic | Evidence requirements, dual-control on claims, audit of *why* |

Locus answers *who may act*. It does **not** assert the model’s plan is correct. Phantom answers *what may be known as a secret*. Neither replaces test runners, schema checks, or human review of intent.

## Mechanism boundaries (do not blur)

1. **Pin is not proof.** `locus whoami` proves session → binding; not that the task is right.
2. **Policy is not epistemology.** `require_approval` / dual-control gate *capability*, not truth of args.
3. **Audit is chronology, not validation.** `locus events` records who acted as whom; it does not certify outcomes.
4. **Next layer composes upward.** Verification should call Locus-scoped tools only; never reintroduce ambient identity.

## Firm-mode implication

Agencies need identity certainty first (wrong tenant = contractual incident), then secret hygiene, then epistemic gates on prod-shaped mutations. Install order: Phantom refs in bindings → Locus pin/workspace → verification checks on the critical path.

See also: [architecture.md](./architecture.md), [firm-mode.md](./firm-mode.md), [DESIGN.md](../DESIGN.md).
