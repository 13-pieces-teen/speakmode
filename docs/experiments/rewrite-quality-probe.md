# Frozen rewrite prompt comparison

`rewrite_quality_probe` compares the 0.1.9 system prompt from commit
`8c95c36a685ce7bc7e0a4e2ca54b32cf95f16a1c` with the current production request
on four frozen synthetic text cases. It tests prompt and protocol behavior;
it does not measure microphone quality, ASR accuracy, human correction effort,
or superiority over Typeless.

The old source bytes have SHA-256
`ff9bd5c0a5aa79327d43c1f2953aa464d58fdf18c507675833239f5ec7e9fbac`.
The JSON fixture stores that revision, the exact old system message, its hash,
four source cases, and a human review checklist for each. The fixture lives at
`app/src-tauri/tests/fixtures/rewrite_quality_0_2_0.json`.

## What is held constant

- Same configured rewrite model, source items, Mode, format preferences,
  source hints, temperature, response format and token budget in both arms.
- Only the system message differs between the baseline and candidate request.
- Four cases: topic regrouping, targeted correction, literal questions/quoted
  labels, and amounts/units/negation.
- Seed `20261008` shuffles case order and a balanced two-baseline/two-candidate
  first-arm list. The resulting order is stored in the fixture. Each case's two
  arms run together; this is one sample per arm, not a statistical benchmark.
- Eight calls maximum, no automatic retries. Authentication or rate-limit
  rejection stops later calls. Three network, timeout or service failures also
  stop later calls. Failed and skipped rows remain in the receipt.

## Run

Build with the repository's Windows MSVC environment. Use a normal user shell
to run the built executable so Windows Credential Manager uses the intended
user account.

```powershell
# Planning only: reads provider_config metadata; no Key access or network.
& '<built-example>/rewrite_quality_probe.exe' `
  --config-db '<absolute-config-database>' `
  --out '<absolute-new-plan.json>'

# Explicit experiment: eight new fixed-text requests at most.
& '<built-example>/rewrite_quality_probe.exe' `
  --config-db '<absolute-config-database>' `
  --out '<absolute-new-results.json>' `
  --execute-cloud

# Follow-up diagnostic only: four candidate requests, no new baseline calls.
& '<built-example>/rewrite_quality_probe.exe' `
  --config-db '<absolute-config-database>' `
  --out '<absolute-new-diagnostic.json>' `
  --candidate-only --execute-cloud
```

`--candidate-only` retains the same four frozen sources and order, and labels
the receipt `diagnostic-candidate-only`, with a four-call maximum and a scope
that explicitly excludes paired or human-quality measurement. It does not
authorize a request by itself: without `--execute-cloud` it only writes a plan.
The original eight-call comparison remains the default. Follow-up diagnostics
must use new output paths and never replace failed initial comparisons.

The only database query reads `provider_config`; no history or audio is read.
Credentials stay inside the native process and are never printed or included
in the artifact. The output path and its `.jsonl` journal must both be new.
Successful response content is retained because all inputs are this fixed
synthetic fixture. HTTP error bodies are not retained. Each completed call is
flushed to the journal and checkpointed to the receipt.

## Read results correctly

Receipts include exact request, system-message and fixture hashes, order,
elapsed time, token usage when returned, content, finish reason and parser
outcome. Both arms are evaluated by the **current candidate parser**; baseline
rows are not an execution of the old parser. `parsed` means the response was
complete and passed the finite protocol/risk checks. It does not prove that
the text preserves meaning. Review each checklist against the actual output,
record omissions and additions, and retain failed runs without relabeling them.

The first eight-call experiment found a concrete prompt failure: an unrelated
targeted correction copied equipment/travel facts from a worked example and
passed the finite parser. The question case also produced answers or statements
instead of preserving questions. The subsequent production prompt therefore
uses abstract editing rules with no worked-example facts, explicitly treats
`items[].text` as the sole factual source, and requires questions to remain
questions. A candidate-only follow-up can check these known failures; it is an
exploratory recheck, not an independent quality benchmark or a replacement for
the initial failure record.

The 0.2.0 parser additionally checks supported numeric-unit/version additions
against the effective corrected source within the cited range. Bare scalars
and list numbering are excluded from this reverse check. A directly adjacent
parenthetical equivalent (for example `3秒（即3000毫秒）`) may reuse the preceding
fact's source support only when its normalized value/unit and polarity match;
different values and new labels still require independent source support.

Source-to-output qualifier preservation checks remain in place. Reverse
qualifier rejection was excluded after review reproduced false positives on
ordinary paraphrases such as `等确认了再发邀请` becoming `如果确认了，再发邀请`.
This is not a general semantics checker: arbitrary added conditions/prose,
unsupported numeric notation, clause attachment and all paraphrases remain
outside its guarantee. Source comparison and restoration remain necessary.
