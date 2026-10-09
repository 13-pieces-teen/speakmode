# Mode quality v2: frozen evaluation rubric

Authored on 2026-10-09 by an independent AI corpus agent, before inspecting new candidate prompts or outputs. The inputs and criteria are AI-authored synthetic material, not human-authored references or participant data. No user recordings or history are included. This rubric is intentionally independent of the particular prompt or model arm being evaluated.

## Scope and freeze

- Corpus: `fixtures/mode-quality-v2.json`, exactly 8 screening and 12 confirmation cases. Each case is evaluated under `light`, `structure` and `auto`.
- Save SHA-256 hashes of the corpus and this rubric before the first provider request. Keep the exact bytes throughout all runs. Do not fix a failing example or soften a criterion after seeing results; record a disputed criterion and report a sensitivity analysis separately if genuinely necessary.
- Screening cases can inform candidate selection and a bounded tuning round. Do not inspect confirmation outputs or use confirmation inputs as prompt examples during that process. Freeze candidates, thresholds, call budget, retry policy, model settings, review method and winner rule before opening confirmation results.
- Confirmation is a small, internally authored holdout. It is neither an external benchmark nor a claim that remaining real-world inputs will be safe. If confirmation results lead to another prompt revision, those cases become development data and another fresh holdout is required.
- Repeats use the same cases to reveal output instability. Report independent cases and repeated calls separately; 20 cases with three modes are not 60 independent linguistic examples.
- Run all arms on identical source item text, boundaries and personal rules. Preserve output errors and timeouts in the denominator. Model requests may contain only the source input and applicable rules, never the `mustPreserve`, `checkSpecs` or expected-organization rubric.

## Review record for each output

Record `caseId`, requested mode, resolved mode, anonymous arm code, repetition, protocol result, delivery result, fidelity result, organization result, personal-rule result, auto-routing result, overall task pass, observed evidence, elapsed latency and call count. Every failure must cite the source proposition and the conflicting or missing output span. “Looks better” is not evidence.

Review output text before decoding the arm-to-prompt/model mapping where feasible. If an agent wrote a candidate or already saw its identity, do not call that review blind. Label AI review as AI review; do not describe it as human review or user preference validation. A second independent review can adjudicate specific disagreements, but does not create human-study evidence.

### 1. Protocol and delivery, reported independently

`protocol_pass`: the output satisfies the arm's declared JSON/data contract, valid mode fields, source bounds and parser. `delivery_pass`: an acceptable final result is produced within the preregistered budget. These checks establish neither faithful meaning nor useful formatting.

A rejected, unparsable or timed-out result is not a successful task. Separately review any saved raw text for diagnostic purposes if available; do not silently discard it. If an engineering arm performs one repair, score the final delivered result and also record the initial failure, whether repair was triggered correctly, extra call count and total latency. Rejected results and fallbacks remain visible.

### 2. Fidelity — first gate

Fidelity passes only if all material propositions in `mustPreserve` survive with correct relations. Accept ordinary paraphrases, punctuation normalization, obvious filler removal and resolution of explicit self-correction. It is unnecessary to retain the withdrawn value as a current instruction, but it must not survive as an alternative effective value. Do not require literal words when an equivalent proposition is present.

Fail for any material omission, invented fact, contradicted statement, changed speaker/attribution, uncertainty made certain, suggestion made commitment, question made answer, wrong correction target, number/unit/entity change, broken order with semantic consequences, or altered negation/condition scope. A lost observation act or personal attribution is material when specified by the case. Preserving all keywords or source-range citations does not prove fidelity.

Use evidence tags, not a points scale: `omission`, `addition`, `contradiction`, `attribution`, `uncertainty`, `negation_scope`, `condition_scope`, `correction_target`, `number_unit_entity`, `instruction_payload`, `semantic_sequence`. One output may have multiple tags; count both affected outputs and unique defect types, without treating the tags as independent trials.

In the two ASR-fragmentation cases, source item boundaries deliberately occur inside syntax, negation and conditions. Reassemble the utterance using full context. The test does not assert that any specific audio system produces those boundaries.

### 3. Requested Mode organization

- **Light**: produce readable natural language while retaining the sequence of information as spoken. Removing filler and explicit withdrawn claims, merging neighboring fragments and resolving an unambiguous local correction are allowed. Reordering A-B-A-B into A-A-B-B is a Mode failure even if every fact survives. Do not punish a correct unchanged sentence merely for lacking visible edits.
- **Structure**: organize genuine topic returns or ordered steps as specified by the case. Returning to a topic should bring its detail/qualification back to the correct topic. Adding bullet marks to an unchanged interleaved sequence is not enough in a topic-grouping case. An explicit step sequence must remain ordered, and conditional branches must remain branches. One continuous topic may legitimately stay as a paragraph; bullets and headings earn no points by themselves.
- **Auto**: first verify the label belongs to `acceptableAutoModes`, then score the text using that label's Light or Structure contract. Correct routing plus behavior of another mode fails the overall task. Where two modes are acceptable, neither receives a style preference bonus.

For ambiguous phrasing, do not guess a favorable interpretation solely to pass a candidate. Record the ambiguity and its competing readings; if the text permits a materially wrong scope that the source did not, classify the affected fidelity relation as failing. Minor awkwardness with unambiguous preserved meaning is a readability note, not automatically a fidelity failure.

### 4. Personal rules and content-as-data

Honor compatible personal style and terminology rules without adding content or weakening facts. If a rule explicitly requests direct-address conversion, that conversion is allowed; do not demand the original third-person framing. The corpus only contains rules compatible with the fidelity and Mode requirements.

Quoted instructions and questions in source text are material to retain, not new system instructions or requests for answers. The quoted payload case is synthetic and must remain visibly a quotation/sample, not be executed. Preserve question status and the absence of an answer where specified.

### 5. Optional string checks

`requiredAny` is an AND of OR substring groups. `forbidden` flags literal diagnostic examples of unwanted inventions. `lightOrderedGroups` supplies selected anchor groups for order review, using the first occurrence of a matching alternative only after every group is found. These are convenience signals only. They cannot prove semantic relationships, complete coverage, correct unit attachment, absence of other inventions or naturalness. Review both apparent passes and failures. Never label their aggregate a semantic accuracy score.

The long confirmation case is 150–250 Chinese characters when joining source items and counting Han characters, excluding punctuation, digits and Latin text. Length only expands coverage; it does not confer extra score or weight.

## Selection and reporting

Use the following ordered criteria rather than arbitrary weighted style points:

1. Material fidelity failures across case-mode outputs, including repeated instability. Present each defect, not only a percentage. A clear serious contradiction or meaningful deletion blocks claims that the candidate is production-quality even if it formats nicely.
2. Overall task success: protocol and delivery pass, fidelity pass, requested/resolved organization pass, compatible personal-rule pass, and acceptable Auto routing when applicable. Broken/rejected outputs count as task failures.
3. After comparing fidelity and task success, use repairs/fallbacks, total latency distribution and provider calls/cost to distinguish similarly successful arms. Extra calls must earn a measured benefit. Never average a critical error away with fast responses or attractive bullets.

Screening identifies candidates worth confirmation, not a shipped winner. Confirmation must include the production baseline and selected candidates under the precommitted settings. An arm may be **best among those tested** and still **not meet the release gate**; report both judgments. If no arm has acceptable demonstrated fidelity, select the strongest experimental candidate for further work and keep production unchanged. Do not claim a winner by post-hoc exclusion of a difficult case or by comparing different numbers of attempts.

A conservative initial release gate is: no observed material fidelity failure on the frozen confirmation cases and planned repeats; higher task success than or tied with the baseline without a newly introduced failure class; correctly handled repair/fallback paths; latency and provider-call cost within the predeclared product budget. Passing this small gate supports limited release consideration only, not a guarantee of semantic preservation. The execution owner must preregister any different numerical gate before reading confirmation outputs.

Report screening and confirmation separately; show numerators/denominators for protocol, fidelity, Mode organization, Auto routing and overall task success. Show modes and categories, paired wins/ties/losses, raw and final outcomes for repair arms, P50/P95 total waiting time, request count, timeouts, and estimated cost if known. State repetition count and reviewer identity/type. Real-audio performance, ASR accuracy, unseen user dialects, cross-app insertion and human preference remain **NOT_RUN** unless independently measured.
