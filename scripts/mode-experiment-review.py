"""Prepare label-hidden Agent review packets and calculate reproducible counts.

Synthetic experiment data only. No cloud calls or application database access.
Reviews are Agent judgments, never described as human or semantic ground truth.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path


def read(path):
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write_new(path, value):
    with Path(path).open("x", encoding="utf-8") as handle:
        json.dump(value, handle, ensure_ascii=False, indent=2)


def packet(receipt, fixture, prefix):
    cases = {case["id"]: case for case in fixture["cases"]}
    unique, mapping = {}, []
    for row in receipt["results"]:
        output = row.get("validatedOutput")
        # Protocol-rejected outputs cannot be delivered. Do not manufacture a
        # final text by bypassing the tested renderer or production validator.
        if not output:
            mapping.append({"index": row["index"], "reviewId": None})
            continue
        text = row.get("renderedText", "\n\n".join(block["text"] for block in output["blocks"]))
        key = json.dumps([row["caseId"], row["mode"], output["resolvedMode"], text], ensure_ascii=False)
        identifier = "r-" + hashlib.sha256(key.encode()).hexdigest()[:16]
        unique[identifier] = {
            "reviewId": identifier, "caseId": row["caseId"], "mode": row["mode"],
            "resolvedMode": output["resolvedMode"], "text": text,
            "sourceItems": cases[row["caseId"]]["sourceItems"],
            "formatRules": cases[row["caseId"]].get("formatRules", ""),
            "mustPreserve": cases[row["caseId"]]["mustPreserve"],
            "expectedOrganization": cases[row["caseId"]]["expectedOrganization"],
            "acceptableAutoModes": cases[row["caseId"]]["acceptableAutoModes"],
        }
        mapping.append({"index": row["index"], "reviewId": identifier})
    items = sorted(unique.values(), key=lambda item: item["reviewId"])
    write_new(str(prefix) + ".packet.json", {"reviewType": "arm-label-hidden-agent-review-not-human", "items": items})
    write_new(str(prefix) + ".mapping.json", mapping)
    print(json.dumps({"uniqueOutputs": len(items), "calls": len(receipt["results"]), "unreviewable": sum(item["reviewId"] is None for item in mapping)}))


def summarize(receipt, mapping, reviews, out):
    expected = {item["index"] for item in receipt["plan"]}
    actual = [item["index"] for item in receipt["results"]]
    if len(actual) != len(set(actual)) or set(actual) != expected:
        raise ValueError("Incomplete or duplicated results; cannot shrink the preregistered denominator")
    if {item["index"] for item in mapping} != expected or len(mapping) != len(expected):
        raise ValueError("Review mapping must cover each planned call exactly once")
    grades = {}
    for filename in reviews:
        for item in read(filename)["reviews"]:
            rid = item["reviewId"]
            if rid in grades:
                raise ValueError("Duplicate review ID; adjudicate explicitly before aggregation: " + rid)
            for flag in ("fidelity", "critical", "modeTask", "route"):
                if not isinstance(item.get(flag), bool):
                    raise ValueError("Review field must be boolean: " + flag)
            if item["critical"] and item["fidelity"]:
                raise ValueError("Critical semantic error cannot pass fidelity")
            grades[rid] = item
    ids = {item["index"]: item["reviewId"] for item in mapping}
    stats, rows = {}, []
    for row in receipt["results"]:
        arm = row["arm"]
        stat = stats.setdefault(arm, {"planned": 0, "attempted": 0, "protocolAccepted": 0,
            "semanticPass": 0, "criticalErrors": 0, "unreviewable": 0, "modeTaskPass": 0,
            "routePass": 0, "deliveredTaskPass": 0, "elapsedMs": [], "inputTokens": 0,
            "outputTokens": 0, "byMode": {}})
        stat["planned"] += 1
        per_mode = stat["byMode"].setdefault(row["mode"], {"planned": 0, "deliveredTaskPass": 0})
        per_mode["planned"] += 1
        if "elapsedMs" in row:
            stat["attempted"] += 1
            stat["elapsedMs"].append(row["elapsedMs"])
        usage = row.get("response", {}).get("usage", {})
        stat["inputTokens"] += usage.get("promptTokens") or 0
        stat["outputTokens"] += usage.get("completionTokens") or 0
        rid = ids[row["index"]]
        grade = grades.get(rid) if rid else None
        if rid and not grade:
            raise ValueError("Missing review: " + rid)
        success = False
        if grade:
            stat["protocolAccepted"] += 1
            for key, field in [("semanticPass", "fidelity"), ("criticalErrors", "critical"),
                               ("modeTaskPass", "modeTask"), ("routePass", "route")]:
                stat[key] += int(grade[field])
            success = grade["fidelity"] and grade["modeTask"] and grade["route"]
            stat["deliveredTaskPass"] += int(success)
            per_mode["deliveredTaskPass"] += int(success)
        else:
            stat["unreviewable"] += 1
        rows.append({"index": row["index"], "caseId": row["caseId"], "mode": row["mode"],
                     "arm": arm, "reviewId": rid, "deliveredTaskPass": success,
                     "grade": grade, "status": row["status"]})
    for stat in stats.values():
        times = sorted(stat.pop("elapsedMs"))
        for quantile, label in [(0.5, "p50Ms"), (0.95, "p95Ms")]:
            stat[label] = times[max(0, math.ceil(len(times) * quantile) - 1)] if times else None
        stat["deliveredTaskRate"] = stat["deliveredTaskPass"] / stat["planned"]
        stat["requestsPerSuccessfulTask"] = stat["attempted"] / stat["deliveredTaskPass"] if stat["deliveredTaskPass"] else None
    ranking = sorted(stats, key=lambda arm: (-stats[arm]["deliveredTaskPass"], stats[arm]["criticalErrors"],
                      -stats[arm]["semanticPass"], stats[arm]["p95Ms"] or math.inf))
    recommendation = None
    if receipt.get("split") == "confirmation":
        candidate = next(arm for arm in stats if arm != "baseline-flash")
        base, new = stats["baseline-flash"], stats[candidate]
        base_key = (-base["deliveredTaskPass"], base["criticalErrors"], -base["semanticPass"])
        new_key = (-new["deliveredTaskPass"], new["criticalErrors"], -new["semanticPass"])
        recommendation = candidate if new_key < base_key and new["criticalErrors"] <= base["criticalErrors"] else "baseline-flash"
    write_new(out, {"reviewType": "agent-reviewed-not-human", "latencyScope": "pre-call-durable-journal-plus-cloud-request-plus-parser-not-end-to-end",
                   "semanticScope": "protocol-accepted-outputs-only; rejected raw outputs require separate diagnostic review",
                   "ranking": ranking, "finalRecommendation": recommendation, "arms": stats, "rows": rows})
    print(json.dumps({"ranking": ranking, "finalRecommendation": recommendation, "arms": stats}, ensure_ascii=False))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    export = sub.add_parser("packet")
    export.add_argument("--receipt", required=True)
    export.add_argument("--fixture", required=True)
    export.add_argument("--prefix", required=True)
    score = sub.add_parser("score")
    score.add_argument("--receipt", required=True)
    score.add_argument("--mapping", required=True)
    score.add_argument("--reviews", nargs="+", required=True)
    score.add_argument("--out", required=True)
    args = parser.parse_args()
    if args.command == "packet":
        packet(read(args.receipt), read(args.fixture), args.prefix)
    else:
        summarize(read(args.receipt), read(args.mapping), args.reviews, args.out)
