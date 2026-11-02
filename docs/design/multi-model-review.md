# Multi-Model Review - Design (DRAFT)

Status: DRAFT for Leader review. Task 6 (assigned 2026-10-20), branch target
`dev/mmr-design`, acceptance target `docs/design/multi-model-review.md`.
Open inputs, stated honestly instead of filled in: the P1 reference file
`zencode-pstack-research-2026-10-19.md` is not present on this seat's VM, so
P1-specific details are marked TBD where they belong. The 4 verdict buckets
in section 3 are proposed from the team's actual verdict vocabulary and are
pending Leader reconciliation - that reconciliation was the recorded blocker
on this task, and this draft is the artifact to reconcile against.

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

## 3. Verdict buckets and flow (PROPOSED - pending Leader reconciliation)

Four buckets, matching verdict words the team already uses in Telegram
review rounds:

1. **ACCEPT** - the artifact meets its acceptance criteria; evidence is
   attached and reproducible.
2. **ACCEPT WITH CONDITIONS** - acceptable once listed mechanical or minor
   fixes are applied; conditions are enumerated, each with an owner.
   Re-review of the whole artifact is not required, only verification that
   the conditions were applied.
3. **REJECT** - at least one acceptance criterion fails on evidence, or a
   claim in the artifact is fabricated or unverifiable. A single evidenced
   REJECT blocks acceptance regardless of how many ACCEPTs exist.
4. **ESCALATE** - reviewers disagree on a matter of fact that the evidence
   in the artifact cannot settle, or a reviewer is blocked (missing file,
   missing access). Escalation goes to the Leader with the contradiction
   stated, not smoothed over.

Flow:

- All reviewers ACCEPT -> artifact proceeds to the human gate as accepted.
- Any ACCEPT WITH CONDITIONS -> conditions are applied by the author, then
  one reviewer (not the author) verifies only the conditions.
- Any evidenced REJECT -> artifact returns to the author with the evidence;
  after rework it re-enters review as a new frozen version.
- ESCALATE -> the Leader decides, asks for the missing evidence, or assigns
  one more independent reviewer. The decision and its reason are recorded
  with the artifact.

## 4. Model selection

- **Free-only.** Reviewers run on free models only (for example OpenRouter
  `:free` models through pi). A paid model is used only with a separate
  per-case approval, and the design must keep working when that approval
  does not exist.
- **Diversity over strength.** The roster prefers models from different
  providers or families over three seats of the same model, so failure
  modes do not correlate.
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

## 7. Open points for Leader

1. Reconcile the 4 buckets in section 3 against the verdict vocabulary the
   Leader wants as the team standard (this is the recorded blocker).
2. Supply or point to the P1 file `zencode-pstack-research-2026-10-19.md`;
   sections marked TBD get their P1 specifics from it.
3. Confirm roster defaults (2 reviewers for docs, 3 for contract/schema,
   3 plus raw-evidence read for release/security).

TBD (waiting on P1): provider-specific notes from the P1 stack research,
including any per-provider rate-limit figures the design should quote
instead of describing qualitatively.

(No secrets in this document.)
