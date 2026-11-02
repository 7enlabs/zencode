# Multi-Model Review - Design (DRAFT)

Status: DRAFT for Leader review. Task 6 (assigned 2026-10-20), branch target
`dev/mmr-design`, acceptance target `docs/design/multi-model-review.md`.
Revision note (Leader decision, 2026-11-02): section 3 now uses the verdict
buckets and roster decided by the Leader, taken from the P1 reference
(`zencode-pstack-research-2026-10-19.md`, pstack `interrogate` playbook).
The P1 file gives no per-provider rate-limit figures, so none are quoted;
that one gap is recorded as deferred in section 7 instead of invented.

## 1. Purpose

ZenCode work is verified by independent review: a task is DONE only with a
named verifier, a verification command, and a receipt - never by the author
checking their own work. Today that review is one reviewer at a time. This
design adds a multi-model review pass: several free models review the same
artifact in parallel, their findings are normalized into verdict buckets,
and disagreements are surfaced to a human decider instead of averaged away.

Scope of a review unit: a pull request, a research study, or a design doc.
Out of scope for v1: automatic merging, automatic code edits by reviewers,
and any paid model call.

## 2. Architecture

```
artifact (PR / study / design doc)
        |
        v
[Dispatcher] -- selects reviewer roster (section 4), freezes the artifact
        |       version under review (commit hash or file checksum)
        +-----> [Reviewer A: free model 1] --\
        +-----> [Reviewer B: free model 2] ---+--> [Normalizer]
        +-----> [Reviewer C: free model 3] --/         |
                                                        v
                                              [Bucket aggregator]
                                                        |
                                                        v
                                        [Human gate: Leader / task owner]
```

Components:

- **Dispatcher.** Takes one artifact plus its acceptance criteria, freezes
  the exact version (commit hash for code, checksum for files), and fans out
  the same review brief to every reviewer. Reviewers never see each other's
  output before submitting (independence, same rule as the team's Scout v4
  critic step).
- **Reviewer.** A free model behind an existing seat tool (for example the
  pi coding agent on free OpenRouter models). Read-only: a reviewer reads
  and reports, it never edits the artifact and never merges.
- **Normalizer.** Maps each free-text verdict onto the bucket vocabulary of
  section 3 and extracts the evidence lines (file, line, command output)
  each claim rests on. Claims without evidence are kept but flagged.
- **Bucket aggregator.** Counts reviewers per bucket and lists every
  cross-reviewer contradiction explicitly. It never averages scores and
  never hides a minority REJECT.
- **Human gate.** The Leader (or the task owner the Leader names) makes the
  final call. The aggregator output is advice with evidence attached, not a
  decision.

## 3. Verdict buckets and flow (decided per Leader decision, from P1)

Reviewers report findings; a named lead judges them into four buckets
(taken from the pstack `interrogate` playbook in the P1 reference). The
reviewers never apply changes themselves - the verdict is a report, not an
edit.

1. **Act on** - findings the lead judges must be applied; each carries
   its evidence.
2. **Consider** - findings worth applying after the lead weighs them
   against cost and risk; applied only on the lead's call.
3. **Noted** - findings recorded for the record, no action now.
4. **Dismissed** - findings rejected, with the reason stated.

Flow:

- Findings from two or more independent reviewers pointing the same way
  are the strongest signal (P1's rule) and go to Act on unless the lead
  finds the shared evidence weak.
- Act on items are applied by the author, then one reviewer (not the
  author) verifies only those items.
- A finding backed by evidence that an acceptance criterion fails, or a
  claim in the artifact is fabricated or unverifiable, goes to Act on
  regardless of how many reviewers missed it; after rework the artifact
  re-enters review as a new frozen version.
- Reviewer disagreement on a matter of fact the artifact cannot settle,
  or a reviewer blocked (missing file, missing access), goes to the lead
  with the contradiction stated, not smoothed over; the lead decides,
  asks for the missing evidence, or assigns one more independent
  reviewer, and records the decision with the artifact.

## 4. Model selection

- **Free-only.** Reviewers run on free models only (for example OpenRouter
  `:free` models through pi). A paid model is used only with a separate
  per-case approval, and the design must keep working when that approval
  does not exist.
- **Diversity over strength.** The roster is one reviewer per model
  family, never two seats of the same family, so failure modes do not
  correlate. P1's example roster is nemotron + qwen + gemma families,
  run free through the local proxy (OpenRouter `:free` + IteraCompute);
  the exact families are chosen per review from whatever free families
  are available that day, and the choice is recorded with the result.
- **Cheapest sufficient.** Review depth scales with risk: a docs-only
  change gets 2 reviewers, a contract or schema change gets 3, and anything
  touching release or security gets 3 plus the human gate reading the raw
  evidence.
- **BYOK constraint.** Any provider key is the user's own key, held in the
  secure vault and injected only into the reviewer's environment. Keys are
  never written into the artifact, the brief, logs, or chat messages.
  Subscription credentials are never routed through third-party gateways.
- **Quota exhaustion.** When a free roster member is rate-limited or out of
  quota, the dispatcher substitutes another free model and records the
  substitution; the review never silently drops below 2 independent
  reviewers.

## 5. Cost accounting - target $0

- Model calls: $0 by construction (free tiers only; see section 4).
- Infrastructure: reviewers run inside existing seat tooling and boxes; no
  new service, no new subscription.
- Human cost: the human gate reads the aggregated one-page result, not
  three raw transcripts. Raw reviewer outputs are stored with the artifact
  for audit.
- The one real budget is reviewer tokens on free quotas; the dispatcher
  caps tokens per review and reports usage with the result, so a review
  that would exhaust a seat's daily free quota is split or deferred rather
  than silently switched to a paid path.

## 6. Failure modes and safeguards

- **Reviewer collusion by sameness:** same model on every seat produces
  correlated mistakes. Mitigated by the diversity rule in section 4.
- **False consensus:** aggregator must list contradictions; a verdict with
  hidden disagreement is a design bug, not a rounding choice.
- **Stale artifact:** the frozen hash is printed on the result; if the
  artifact changes, the verdict is void and review restarts.
- **Secret leakage:** briefs carry only the artifact and its acceptance
  criteria - never credentials, never unrelated user data.
- **Self-review:** the author of an artifact is never on its roster, and
  the human gate for a Leader-authored artifact is Zen.
- **Reviewer steered through the artifact:** a reviewer that is handed
  only the author's summary, framing, or chosen excerpts will reproduce
  the author's conclusions instead of checking them. Reviewers therefore
  receive the raw artifact plus its acceptance criteria, never the
  author's verdict on it; the normalizer drops any finding that rests
  only on the author's summary without artifact evidence, and the
  aggregator flags summaries that omit a material part of the artifact.

## 7. Open points for Leader

1. Confirm roster defaults (2 reviewers for docs, 3 for contract/schema,
   3 plus raw-evidence read for release/security) - the family rule in
   section 4 is decided; only the counts by risk level need confirming.
2. Deferred, not invented: the P1 reference gives no per-provider
   rate-limit figures, so section 4 describes quotas qualitatively and
   the dispatcher records the actual quota observed per review. If the
   Leader wants figures quoted, they need a source outside P1.

(No secrets in this document.)
