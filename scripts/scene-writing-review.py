"""Offline review packets and reproducible metrics for the frozen scene experiment.

Semantic judgments must be supplied by a reviewer. Arm labels are hidden in the
packet, but the implementing Agent is not an independent or human evaluator.
Edit distance measures suggested correction, not observed human editing time.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import statistics
from pathlib import Path

ARMS = ("baseline", "natural", "scene")
DIMENSIONS = ("faithful", "readable", "sceneFit", "modeFit")


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write_new(path, value):
    with Path(path).open("x", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)
        handle.write("\n")


def validate(receipt, fixture, fixture_bytes):
    if receipt.get("tool") not in ("scene-writing-lab-v1", "scene-layout-lab-v1") or receipt.get("status") != "completed":
        raise ValueError("Complete scene receipt required; halted runs remain evidence, not a complete comparison")
    if receipt["fixtureSha256"] != hashlib.sha256(fixture_bytes).hexdigest():
        raise ValueError("Fixture identity mismatch")
    layout = receipt["tool"] == "scene-layout-lab-v1"
    arms = ("baseline", "layout") if layout else ARMS
    count, maximum = (6, 24) if layout else (8, 64)
    if len(fixture["cases"]) != count or receipt["split"] != fixture["split"] or receipt["maximumCalls"] != maximum:
        raise ValueError("Unexpected frozen split")
    expected = {(case["id"], arm) for case in fixture["cases"] for arm in arms}
    plan = receipt["plan"]
    rows = receipt["results"]
    if len(plan) != len(expected) or len(rows) != len(expected) or {(r["caseId"], r["arm"]) for r in rows} != expected:
        raise ValueError("Missing or duplicate arms")
    if {(r["caseId"], r["arm"]) for r in plan} != expected:
        raise ValueError("Plan arms differ from fixture")
    planned = {(r["caseId"], r["arm"]): r for r in plan}
    cases = {c["id"]: c for c in fixture["cases"]}
    for row in rows:
        task = planned[(row["caseId"], row["arm"])]
        case = cases[row["caseId"]]
        if task["maximumCalls"] != {"baseline": 4, "natural": 2, "scene": 2, "layout": 0}[row["arm"]]:
            raise ValueError("Arm request cap changed")
        if [i["text"] for i in task["source"]["items"]] != case["sourceItems"]:
            raise ValueError("Source changed")
        if task["mode"]["baseMode"] != case["baseMode"] or task["mode"]["rules"] != case["rules"]:
            raise ValueError("Mode changed")
        for key in ("scene", "useNearbyText", "nearbyText"):
            if task["sceneSnapshot"][key] != case[key]:
                raise ValueError("Context changed")
        if row["status"] != "completed" or not 0 <= row["clientStartedCalls"] <= task["maximumCalls"]:
            raise ValueError("Incomplete arm or request cap exceeded")
        if type(row["fallback"]) is not bool or type(row["pipelineAccepted"]) is not bool or row["fallback"] == row["pipelineAccepted"]:
            raise ValueError("Contradictory fallback")
        if row["fallback"] and row["deliveredText"] != "\n".join(case["sourceItems"]):
            raise ValueError("Fallback lost source")
        if row["arm"] == "layout":
            baseline = next(r for r in rows if r["caseId"] == row["caseId"] and r["arm"] == "baseline")
            if row.get("derivedFromSameBaseline") is not True or row["pipelineAccepted"] != baseline["pipelineAccepted"]:
                raise ValueError("Local layout lost baseline provenance")
    if sum(r["clientStartedCalls"] for r in rows) != receipt["clientStartedCalls"] or receipt["clientStartedCalls"] > maximum:
        raise ValueError("Request accounting mismatch")
    return rows


def blind_id(receipt, row):
    raw = f'{receipt["fixtureSha256"]}:{row["caseId"]}:{row["arm"]}'
    return hashlib.sha256(raw.encode()).hexdigest()[:12]


def prepare(receipt, fixture):
    cases = []
    for case in fixture["cases"]:
        outputs = [{"id": blind_id(receipt, row), "text": row["deliveredText"]}
                   for row in receipt["results"] if row["caseId"] == case["id"]]
        cases.append({"caseId": case["id"], "sourceText": "\n".join(case["sourceItems"]),
                      "scene": case["scene"], "baseMode": case["baseMode"], "rules": case["rules"],
                      "nearbyText": case["nearbyText"] if case["useNearbyText"] else None,
                      "expectations": case["expectations"], "outputs": sorted(outputs, key=lambda v: v["id"])})
    return {"fixtureSha256": receipt["fixtureSha256"], "split": fixture["split"],
            "reviewerScope": "AGENT_REVIEW_NOT_INDEPENDENT", "dimensions": DIMENSIONS,
            "instructions": "Judge each output separately. Supply booleans, a concrete note, and minimal correctedText; identical text when no correction is needed.", "cases": cases}


def distance(left, right):
    previous = list(range(len(right) + 1))
    for i, a in enumerate(left, 1):
        current = [i]
        for j, b in enumerate(right, 1):
            current.append(min(current[-1] + 1, previous[j] + 1, previous[j - 1] + (a != b)))
        previous = current
    return previous[-1]


def summarize(receipt, review):
    if review.get("fixtureSha256") != receipt["fixtureSha256"]:
        raise ValueError("Review identity mismatch")
    mapping = {blind_id(receipt, row): row for row in receipt["results"]}
    scores = review["scores"]
    if len(scores) != len(mapping) or {s["id"] for s in scores} != set(mapping):
        raise ValueError("Every planned arm requires exactly one review")
    detailed = []
    for score in scores:
        if any(type(score.get(key)) is not bool for key in DIMENSIONS) or not score.get("note") or not isinstance(score.get("correctedText"), str):
            raise ValueError("Explicit semantic judgments and correction required")
        row = mapping[score["id"]]
        edits = distance(row["deliveredText"], score["correctedText"])
        if all(score[key] for key in DIMENSIONS) and edits:
            raise ValueError("Usable output should not require correction")
        if not all(score[key] for key in DIMENSIONS) and not edits:
            raise ValueError("Failed output needs a concrete correction")
        detailed.append({**row, "reviewId": score["id"], "review": score,
                         "textUsable": all(score[key] for key in DIMENSIONS), "suggestedEditCharacters": edits})
    arms = {}
    for arm in (("baseline", "layout") if receipt["tool"] == "scene-layout-lab-v1" else ARMS):
        rows = [r for r in detailed if r["arm"] == arm]
        arms[arm] = {"cases": len(rows), "textUsable": sum(r["textUsable"] for r in rows),
                     "faithful": sum(r["review"]["faithful"] for r in rows),
                     "sceneFit": sum(r["review"]["sceneFit"] for r in rows),
                     "pipelineAccepted": sum(r["pipelineAccepted"] for r in rows),
                     "acceptedButUnfaithful": sum(r["pipelineAccepted"] and not r["review"]["faithful"] for r in rows),
                     "fallbacks": sum(r["fallback"] for r in rows), "clientStartedCalls": sum(r["clientStartedCalls"] for r in rows),
                     "medianElapsedMs": statistics.median(r["elapsedMs"] for r in rows),
                     "maxElapsedMs": max(r["elapsedMs"] for r in rows),
                     "suggestedEditCharacters": sum(r["suggestedEditCharacters"] for r in rows)}
    return {"tool": "scene-writing-reviewed-v1", "split": receipt["split"], "fixtureSha256": receipt["fixtureSha256"],
            "reviewerScope": "AGENT_REVIEW_NOT_INDEPENDENT", "actualHumanEditingTime": "NOT_RUN",
            "actualNativeDelivery": "NOT_RUN", "arms": arms, "details": detailed}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("prepare", "summarize"))
    parser.add_argument("--receipt", required=True, type=Path)
    parser.add_argument("--fixture", required=True, type=Path)
    parser.add_argument("--review", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    receipt, fixture = read(args.receipt), read(args.fixture)
    validate(receipt, fixture, args.fixture.read_bytes())
    result = prepare(receipt, fixture) if args.action == "prepare" else summarize(receipt, read(args.review))
    write_new(args.out, result)
    print(json.dumps(result.get("arms", {"cases": len(result["cases"])} if "cases" in result else {}), ensure_ascii=False))


if __name__ == "__main__":
    main()
