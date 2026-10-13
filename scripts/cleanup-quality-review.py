"""Prepare arm-hidden review of all generated text, then join it to parser results.

Offline only. The corpus and raw results contain fixed synthetic text, never audio.
One Agent review is not human or independent population evaluation. Automated
checks never fill semantic verdicts. Missing/failed/rejected outputs stay in the
144-call denominator. No third-party packages or network operations.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import math
import statistics
from pathlib import Path

DIMENSIONS = ("contentFidelity", "cleanupComplete", "modeLayout", "autoRouting", "personalRules")


def load(path: Path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def digest(value):
    return hashlib.sha256(json.dumps(value, ensure_ascii=False, sort_keys=True).encode()).hexdigest()


def write_new(path: Path, value):
    with path.open("x", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def validate_results(receipt):
    if receipt.get("tool") not in ("speakmode-cleanup-quality-lab", "speakmode-cleanup-model-comparison") or receipt.get("maximumCalls") != 144:
        raise ValueError("Not the frozen 144-call cleanup experiment")
    plan = receipt["plan"]
    rows = receipt["results"]
    if len(plan) != 144 or len(rows) != 144 or receipt.get("status") not in ("completed", "halted"):
        raise ValueError("Receipt must account for all calls, including skipped ones")
    if {r["index"] for r in rows} != set(range(144)):
        raise ValueError("Missing or duplicate result index")
    for row in rows:
        expected = plan[row["index"]]
        if any(row[k] != expected[k] for k in ("caseId", "arm", "mode")):
            raise ValueError("Result identity differs from frozen plan")
    return rows


def combine_models(flash, plus, flash_replay):
    """Derived comparison only: preserve separately collected source receipts."""
    validate_results(flash)
    if (plus.get("tool") != "speakmode-cleanup-quality-lab" or plus.get("maximumCalls") != 72
            or plus.get("candidateModelOverride") != "qwen-plus" or plus.get("status") != "completed"
            or len(plus["plan"]) != 72 or len(plus["results"]) != 72):
        raise ValueError("Expected the complete bounded 72-call Plus diagnostic")
    for key in ("fixtureSha256", "candidateSha256", "candidatePromptVersion", "reviewChecklists", "scoring"):
        if flash[key] != plus[key]:
            raise ValueError(f"Model comparison changed frozen {key}")
    if flash["configuration"]["rewriteModel"] != "qwen-flash" or plus["configuration"]["rewriteModel"] != "qwen-plus":
        raise ValueError("Comparison requires Flash and Plus model identities")
    if flash["configuration"]["region"] != plus["configuration"]["region"]:
        raise ValueError("Comparison changed region")
    if flash_replay.get("tool") != "speakmode-cleanup-parser-replay" or len(flash_replay["results"]) != 144:
        raise ValueError("Complete final-parser replay required for older Flash generation")
    for key in ("parserSha256", "factsSha256", "integritySha256", "semanticsSha256"):
        if flash_replay[key] != plus[key]:
            raise ValueError("Both models must be scored with the same current parser")
    replayed = {r["index"]: r for r in flash_replay["results"]}
    plus_by_key = {(r["caseId"], r["mode"]): r for r in plus["results"]}
    if len(plus_by_key) != 72 or {r["index"] for r in plus["results"]} != set(range(72)):
        raise ValueError("Plus diagnostic contains duplicate or missing identities")
    plan, results = [], []
    for old in flash["results"]:
        if old["arm"] != "candidate":
            continue
        new = plus_by_key[(old["caseId"], old["mode"])]
        old_plan, new_plan = flash["plan"][old["index"]], plus["plan"][new["index"]]
        if new["arm"] != "candidate" or any(new[k] != new_plan[k] for k in ("caseId", "arm", "mode")):
            raise ValueError("Plus row differs from its preregistered plan")
        a, b = copy.deepcopy(old_plan["requestBody"]), copy.deepcopy(new_plan["requestBody"])
        a.pop("model"); b.pop("model")
        if a != b or old_plan["source"] != new_plan["source"]:
            raise ValueError("Model diagnostic changed something besides the requested model")
        for arm, row, original_plan, original_receipt in (("baseline", old, old_plan, flash), ("candidate", new, new_plan, plus)):
            index = len(results)
            derived_row, derived_plan = copy.deepcopy(row), copy.deepcopy(original_plan)
            derived_row.update(index=index, arm=arm, sourceIndex=row["index"], sourceReceiptSha256=digest(original_receipt))
            derived_plan.update(index=index, arm=arm)
            if arm == "baseline":
                parsed = replayed[row["index"]]
                if any(parsed[k] != row[k] for k in ("caseId", "arm", "mode")):
                    raise ValueError("Flash parser replay changed row identity")
                derived_row["originalCloudStatus"] = derived_row["status"]
                derived_row["status"] = "parsed" if parsed["replayedParser"]["accepted"] else "rejected"
                derived_row["replayedParser"] = parsed["replayedParser"]
            plan.append(derived_plan); results.append(derived_row)
    return {"tool": "speakmode-cleanup-model-comparison", "schemaVersion": 1, "maximumCalls": 144,
            "scope": "POSTHOC_SEPARATE_STAGES_SAME_CORPUS_NOT_NEW_HOLDOUT_NOT_RANDOMIZED_MODEL_TIMING",
            "newRequestsInThisDerivedArtifact": 0, "status": "completed", "plan": plan, "results": results,
            "armLabels": {"baseline": "qwen-flash / cleanup-contract-v1 / earlier stage", "candidate": "qwen-plus / cleanup-contract-v1 / later diagnostic stage"},
            "sourceReceipts": {"flash": digest(flash), "plus": digest(plus)},
            "reviewChecklists": flash["reviewChecklists"], "scoring": flash["scoring"],
            "fixtureSha256": flash["fixtureSha256"], "candidateSha256": flash["candidateSha256"],
            "parserSha256": plus["parserSha256"], "factsSha256": plus["factsSha256"],
            "integritySha256": plus["integritySha256"], "semanticsSha256": plus["semanticsSha256"]}


def prepare(receipt, out_dir, reuse_packet=None, reuse_review=None):
    rows = validate_results(receipt)
    cases = {c["id"]: c for c in receipt["reviewChecklists"]}
    groups = {}
    row_keys = {}
    for row in rows:
        text = row.get("unvalidatedText")
        if not isinstance(text, str) or row.get("response", {}).get("finishReason") != "stop":
            continue
        try:
            raw = json.loads(row["response"]["modelContent"])
            resolved = raw.get("resolvedMode")
        except (ValueError, TypeError):
            resolved = None
        key = digest([row["caseId"], row["mode"], resolved, text])
        row_keys[row["index"]] = key
        groups[key] = {"caseId": row["caseId"], "selectedMode": row["mode"],
                       "resolvedMode": resolved, "text": text, "rubric": cases[row["caseId"]]}
    # Hash order, rather than call order, hides both arm alternation and model name.
    packet = []
    keys = {}
    for index, (key, content) in enumerate(sorted(groups.items())):
        review_id = f"R{index + 1:03d}"
        keys[key] = review_id
        packet.append({"reviewId": review_id, **content})
    out_dir.mkdir(parents=True, exist_ok=False)
    write_new(out_dir / "review-packet.json", {"scope": "arm-hidden synthetic Agent review, not human evaluation; same corpus previously reviewed, not new holdout", "items": packet})
    write_new(out_dir / "mapping-private.json", {"receiptSha256": digest(receipt),
              "rows": [{"index": r["index"], "reviewId": keys.get(row_keys.get(r["index"]))} for r in rows]})
    reusable = {}
    if (reuse_packet is None) != (reuse_review is None):
        raise ValueError("Review reuse requires both original hidden packet and completed review")
    if reuse_packet is not None:
        old_verdicts = {v["reviewId"]: v for v in reuse_review["items"]}
        for p in reuse_packet["items"]:
            verdict = old_verdicts[p["reviewId"]]
            if any(type(verdict.get(k)) is not bool for k in (*DIMENSIONS, "criticalError")):
                raise ValueError("Cannot reuse incomplete review")
            reusable[digest([p["caseId"], p["selectedMode"], p["resolvedMode"], p["text"], p["rubric"]])] = verdict
    templates = []
    for p in packet:
        key = digest([p["caseId"], p["selectedMode"], p["resolvedMode"], p["text"], p["rubric"]])
        old = reusable.get(key)
        row = {"reviewId": p["reviewId"], **{k: None for k in DIMENSIONS}, "criticalError": None, "notes": ""}
        if old:
            row.update({k: old[k] for k in (*DIMENSIONS, "criticalError", "notes")})
            row["reusedFromPriorReviewId"] = old["reviewId"]
        templates.append(row)
    reused = sum("reusedFromPriorReviewId" in v for v in templates)
    write_new(out_dir / "review-template.json", {"reviewerId": "REPLACE", "reviewerKind": "agent", "reusedExactOutputCount": reused,
              "scope": "same previously reviewed corpus; exact output and rubric reuse is explicitly tracked", "items": templates})
    return {"rows": len(rows), "uniqueReviewItems": len(packet), "reusedExactOutputCount": reused, "newReviewItems": len(packet) - reused, "withoutReviewableOutput": 144 - len(row_keys)}


def percentile(values, p):
    return sorted(values)[max(0, math.ceil(len(values) * p) - 1)] if values else None


def score(receipt, mapping, review, replay=None):
    rows = validate_results(receipt)
    if mapping["receiptSha256"] != digest(receipt):
        raise ValueError("Receipt changed after preparation")
    if review.get("reviewerId") in (None, "", "REPLACE"):
        raise ValueError("Explicit reviewer identity required")
    expected = {m["reviewId"] for m in mapping["rows"] if m["reviewId"]}
    items = review["items"]
    verdicts = {i["reviewId"]: i for i in items}
    if len(verdicts) != len(items) or set(verdicts) != expected:
        raise ValueError("Every reviewable output must receive exactly one verdict")
    for v in items:
        if any(type(v.get(k)) is not bool for k in (*DIMENSIONS, "criticalError")):
            raise ValueError("No missing/inferred semantic verdicts allowed")
        if not isinstance(v.get("notes"), str):
            raise ValueError("Review notes must be a string")
    index_to_review = {m["index"]: m["reviewId"] for m in mapping["rows"]}
    if len(index_to_review) != 144 or set(index_to_review) != set(range(144)):
        raise ValueError("Review mapping lost result rows")
    replay_rows = {}
    if replay is not None:
        if replay.get("tool") != "speakmode-cleanup-parser-replay":
            raise ValueError("Unexpected replay tool")
        replay_rows = {r["index"]: r for r in replay["results"]}
        if len(replay["results"]) != 144 or set(replay_rows) != set(range(144)):
            raise ValueError("Replay must account for every original result")
    joined = []
    for row in rows:
        accepted = row["status"] == "parsed"
        if replay is not None:
            current = replay_rows[row["index"]]
            if any(current[k] != row[k] for k in ("caseId", "arm", "mode")):
                raise ValueError("Replay changed result identity")
            if type(current.get("replayedParser", {}).get("accepted")) is not bool:
                raise ValueError("Replay missing parser verdict")
            accepted = current["replayedParser"]["accepted"]
        v = verdicts.get(index_to_review[row["index"]])
        # Exact removable markers are a preregistered cleanup failure even if a
        # reviewer accidentally marks the retained correction process as clean.
        residue = row.get("necessaryChecks", {}).get("remainingRemovableTerms", [])
        clean = bool(v and v["cleanupComplete"] and not residue)
        content = bool(v and v["contentFidelity"] and not v["criticalError"])
        generated_complete = bool(v and content and clean and all(v[k] for k in ("modeLayout", "autoRouting", "personalRules")))
        joined.append({"index": row["index"], "caseId": row["caseId"], "arm": row["arm"], "mode": row["mode"],
                       "status": row["status"], "parserAccepted": accepted, "reviewId": index_to_review[row["index"]],
                       "contentFidelityPass": content, "cleanupPass": clean,
                       "criticalError": bool(v and v["criticalError"]), "generatedTaskPass": generated_complete,
                       "acceptedTaskPass": generated_complete and accepted,
                       "usage": row.get("response", {}).get("usage", {}),
                       "responseComplete": row.get("response", {}).get("finishReason") == "stop",
                       "cloudElapsedMs": row.get("cloudElapsedMs")})
    summaries = {}
    for arm in ("baseline", "candidate"):
        subset = [r for r in joined if r["arm"] == arm]
        times = [r["cloudElapsedMs"] for r in subset if r["cloudElapsedMs"] is not None]
        summaries[arm] = {"planned": len(subset), "generatedTaskPass": sum(r["generatedTaskPass"] for r in subset),
                          "acceptedTaskPass": sum(r["acceptedTaskPass"] for r in subset),
                          "parserAccepted": sum(r["parserAccepted"] for r in subset),
                          "contentFidelityPass": sum(r["contentFidelityPass"] for r in subset),
                          "cleanupPass": sum(r["cleanupPass"] for r in subset),
                          "criticalErrorsInGenerated": sum(r["criticalError"] for r in subset),
                          "criticalErrorsInAccepted": sum(r["criticalError"] and r["parserAccepted"] for r in subset),
                          "tokenUsage": {key: sum(r["usage"][key] for r in subset if isinstance(r["usage"].get(key), int)) for key in ("promptTokens", "completionTokens", "totalTokens")},
                          "rowsWithReportedTokenUsage": sum(isinstance(r["usage"].get("totalTokens"), int) for r in subset),
                          "cloudP50Ms": statistics.median(times) if times else None, "cloudP95Ms": percentile(times, .95),
                          "cloudResponsesByBudgetMs": {str(budget): sum(r["responseComplete"] and r["cloudElapsedMs"] is not None and r["cloudElapsedMs"] <= budget for r in subset) for budget in (2000, 3000, 5000)},
                          "modeAcceptedTaskPass": {mode: sum(r["acceptedTaskPass"] for r in subset if r["mode"] == mode) for mode in ("light", "structure", "auto")},
                          "generatedByBudgetMs": {str(budget): sum(r["generatedTaskPass"] and r["cloudElapsedMs"] is not None and r["cloudElapsedMs"] <= budget for r in subset) for budget in (2000, 3000, 5000)},
                          "acceptedByBudgetMs": {str(budget): sum(r["acceptedTaskPass"] and r["cloudElapsedMs"] is not None and r["cloudElapsedMs"] <= budget for r in subset) for budget in (2000, 3000, 5000)}}
    return {"scope": "synthetic fixed text with Agent review; not native/human or statistical superiority",
            "receiptSha256": digest(receipt), "reviewerId": review["reviewerId"], "arms": summaries,
            "armLabels": receipt.get("armLabels", {"baseline": "baseline prompt", "candidate": "cleanup prompt"}),
            "parserSource": "offline_replay" if replay is not None else "original_cloud_run",
            "budgetBoundary": "Retrospective cloud-response completion counts; not timed fallback, ASR, storage, native input or stop-to-paste measurements.",
            "promotionDecision": "requires_explicit_review_not_automatic", "rows": joined}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("--receipt", type=Path, required=True)
    prep.add_argument("--out-dir", type=Path, required=True)
    prep.add_argument("--reuse-packet", type=Path)
    prep.add_argument("--reuse-review", type=Path)
    combine = sub.add_parser("combine-models")
    combine.add_argument("--flash", type=Path, required=True)
    combine.add_argument("--plus", type=Path, required=True)
    combine.add_argument("--flash-replay", type=Path, required=True)
    combine.add_argument("--out", type=Path, required=True)
    scoring = sub.add_parser("score")
    scoring.add_argument("--receipt", type=Path, required=True)
    scoring.add_argument("--mapping", type=Path, required=True)
    scoring.add_argument("--review", type=Path, required=True)
    scoring.add_argument("--out", type=Path, required=True)
    scoring.add_argument("--replay", type=Path)
    args = parser.parse_args()
    if args.action == "combine-models":
        replay = load(args.flash_replay)
        if replay.get("originalReceiptSha256") != hashlib.sha256(args.flash.read_bytes()).hexdigest():
            raise ValueError("Flash replay differs from original bytes")
        result = combine_models(load(args.flash), load(args.plus), replay)
        write_new(args.out, result)
        print(json.dumps({"scope": result["scope"], "rows": len(result["results"]), "newRequests": 0}))
        return
    receipt = load(args.receipt)
    if args.action == "prepare":
        print(json.dumps(prepare(receipt, args.out_dir, load(args.reuse_packet) if args.reuse_packet else None, load(args.reuse_review) if args.reuse_review else None), ensure_ascii=False))
    else:
        replay = load(args.replay) if args.replay else None
        if replay is not None and replay.get("originalReceiptSha256") != hashlib.sha256(args.receipt.read_bytes()).hexdigest():
            raise ValueError("Replay receipt hash does not match original bytes")
        result = score(receipt, load(args.mapping), load(args.review), replay)
        write_new(args.out, result)
        print(json.dumps(result["arms"], ensure_ascii=False))


if __name__ == "__main__":
    main()
