#!/usr/bin/env python3
"""Offline, blinded text review for fixed automatic/natural-quality-lab receipts.

No cloud, credential, database, audio, native input or automatic quality judging.
Bindings are private: give a reviewer only packet.json and review-template.json.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import secrets
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from typing import Any

TOOL = "speakmode-automatic-quality-lab"
NATURAL_TOOL = "speakmode-natural-quality-lab"
DESIGNS = {
    TOOL: {"dev": (12, 1), "holdout": (4, 2)},
    NATURAL_TOOL: {"dev": (12, 1), "holdout": (8, 1), "user": (1, 1)},
}
LABEL = "Independent Agent review; not human listening"
SCOPE = "OFFLINE_SYNTHETIC_TEXT_REVIEW_NOT_REAL_ASR_TYPELESS_AUDIO_OR_NATIVE_PASTE"
USER_SCOPE = "OFFLINE_USER_PROVIDED_TRANSCRIPT_REVIEW_NOT_REAL_ASR_TYPELESS_AUDIO_OR_NATIVE_PASTE"
REPLAY_SCOPE = "OFFLINE_REPARSE_OF_EXISTING_RESPONSES_NOT_NEW_CLOUD_RUN"
FIELDS = ("fidelity", "cleanup", "readability", "tone", "properNames")
CHECKLISTS = ("mustPreserve", "allowedCleanup", "mustNotDo")
MAX_FILE_BYTES = 64 * 1024 * 1024
LEGACY_REPLAY_GUARD_KEYS = frozenset({"parser", "integrity", "facts", "semantics", "disfluency"})
REPLAY_GUARD_KEY_SETS = (
    LEGACY_REPLAY_GUARD_KEYS,
    LEGACY_REPLAY_GUARD_KEYS | {"evaluationDisfluency"},
    LEGACY_REPLAY_GUARD_KEYS | {"evaluationDisfluency", "quantityRoles"},
)


class ReviewError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ReviewError(message)


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value: Any) -> bytes:
    return (json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n").encode("utf-8")


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        require(key not in result, "Duplicate JSON object key")
        result[key] = value
    return result


def load(path: Path) -> tuple[Any, bytes]:
    require(path.is_absolute(), "Use an absolute input path")
    require(path.is_file() and path.stat().st_size <= MAX_FILE_BYTES, "Input missing or above 64 MiB limit")
    raw = path.read_bytes()
    require(len(raw) <= MAX_FILE_BYTES, "Input above 64 MiB limit")
    try:
        value = json.loads(raw.decode("utf-8-sig"), object_pairs_hook=unique_object,
                           parse_constant=lambda _: (_ for _ in ()).throw(ReviewError("Nonfinite JSON number")))
    except (UnicodeDecodeError, json.JSONDecodeError) as failure:
        raise ReviewError("Input is not valid UTF-8 JSON") from failure
    return value, raw


def write_new(path: Path, value: Any) -> bytes:
    raw = json_bytes(value)
    with path.open("xb") as file:
        file.write(raw)
        file.flush()
    return raw


def integer(value: Any) -> bool:
    return type(value) is int


def source_text(plan: dict[str, Any]) -> str:
    source = plan.get("source")
    require(isinstance(source, dict) and source.get("complete") is True, "Only complete synthetic sources are supported")
    items = source.get("items")
    require(isinstance(items, list) and 1 <= len(items) <= 16, "Invalid source fragments")
    ids: set[str] = set()
    for index, item in enumerate(items):
        require(isinstance(item, dict) and isinstance(item.get("text"), str) and bool(item["text"]), "Invalid source text")
        require(isinstance(item.get("id"), str) and item["id"] not in ids, "Invalid or duplicate source ID")
        # SourceItem explicitly renames is_final to "final" in core/types.rs.
        require(item.get("final") is True and item.get("captureOrder") == index, "Source order/finality mismatch")
        ids.add(item["id"])
    return "\n".join(item["text"] for item in items)


def decode_candidate(row: dict[str, Any]) -> tuple[str | None, str]:
    response = row.get("response")
    content = response.get("modelContent") if isinstance(response, dict) else None
    if not isinstance(content, str):
        return None, "missing_model_content"
    if len(content.encode("utf-8")) > 256 * 1024:
        raise ReviewError("Generated content above retention limit")
    try:
        value = json.loads(content, object_pairs_hook=unique_object)
    except (json.JSONDecodeError, ReviewError):
        return None, "invalid_json"
    if not isinstance(value, dict):
        return None, "unsupported_json_shape"
    if isinstance(value.get("text"), str):
        # Decode for review, not schema acceptance: parser status remains separate.
        return value["text"], "single_text"
    blocks = value.get("blocks")
    if isinstance(blocks, list) and blocks and all(isinstance(block, dict) and isinstance(block.get("text"), str) for block in blocks):
        return "\n\n".join(block["text"] for block in blocks), "blocks_text"
    return None, "unsupported_json_shape"


def receipt_design(receipt: dict[str, Any]) -> tuple[int, int]:
    tool, split = receipt.get("tool"), receipt.get("split")
    require(isinstance(tool, str) and tool in DESIGNS, "Unsupported receipt tool")
    require(isinstance(split, str) and split in DESIGNS[tool], "Unknown fixed split for this tool")
    return DESIGNS[tool][split]


def review_scope(receipt: dict[str, Any]) -> str:
    return USER_SCOPE if receipt.get("tool") == NATURAL_TOOL and receipt.get("split") == "user" else SCOPE


def offline_replay_metadata(receipt: dict[str, Any]) -> dict[str, Any] | None:
    if "offlineReplay" not in receipt:
        return None
    metadata = receipt["offlineReplay"]
    require(receipt.get("tool") == NATURAL_TOOL and isinstance(metadata, dict), "Unsupported offline replay evidence")
    require(metadata.get("scope") == REPLAY_SCOPE, "Unknown offline replay scope")
    require(integer(metadata.get("cloudRequests")) and metadata["cloudRequests"] == 0,
            "Offline replay must explicitly report zero new cloud requests")
    valid_hash = lambda value: isinstance(value, str) and len(value) == 64 and all(char in "0123456789abcdef" for char in value)
    require(valid_hash(metadata.get("originalReceiptSha256")), "Offline replay original receipt hash missing")
    hashes = metadata.get("currentGuardHashes")
    require(isinstance(hashes, dict) and set(hashes) in REPLAY_GUARD_KEY_SETS
            and all(valid_hash(value) for value in hashes.values()), "Offline replay compiled guard hashes missing")
    return metadata


def validate_receipt(receipt: Any) -> list[tuple[dict[str, Any], dict[str, Any], dict[str, Any]]]:
    require(isinstance(receipt, dict), "Unsupported receipt tool")
    case_count, trials = receipt_design(receipt)
    offline_replay_metadata(receipt)
    require(receipt.get("status") == "completed", "Receipt must be completed, not partial/halted/planned")
    split = receipt.get("split")
    if receipt["tool"] == NATURAL_TOOL:
        source_scope = "USER_PROVIDED_TRANSCRIPT_NO_AUDIO" if split == "user" else "SYNTHETIC_TRANSCRIPT_NO_AUDIO"
        require(receipt.get("sourceScope") == source_scope, "Natural experiment source scope mismatch")
    maximum = case_count * trials * 2
    require(receipt.get("caseCount") == case_count and receipt.get("trials") == trials and receipt.get("maximumCalls") == maximum,
            "Receipt does not match finite tool/split design")
    plans, rows, checklists = receipt.get("plan"), receipt.get("results"), receipt.get("reviewChecklists")
    require(isinstance(plans, list) and len(plans) == maximum and isinstance(rows, list) and len(rows) == maximum, "Incomplete plan/results")
    require(isinstance(checklists, list) and len(checklists) == case_count, "Incomplete case checklists")
    cases: dict[str, dict[str, Any]] = {}
    for case in checklists:
        require(isinstance(case, dict) and isinstance(case.get("id"), str) and case["id"] not in cases, "Duplicate/invalid case ID")
        # The preregistered corpus deliberately keeps its original reservation
        # label; the runtime phase is named holdout without rewriting the data.
        fixture_split = {"holdout": "holdout_reserved", "user": "user_provided", "dev": "dev"}[split]
        require(case.get("split") == fixture_split, "Checklist split mismatch")
        for name in CHECKLISTS:
            require(isinstance(case.get(name), list) and bool(case[name]) and all(isinstance(text, str) and text for text in case[name]), "Invalid semantic checklist")
        cases[case["id"]] = case
    rows_by_index: dict[int, dict[str, Any]] = {}
    for row in rows:
        require(isinstance(row, dict) and integer(row.get("index")) and 0 <= row["index"] < maximum, "Invalid result index")
        require(row["index"] not in rows_by_index, "Duplicate result index")
        rows_by_index[row["index"]] = row
    identities: set[tuple[str, str, int]] = set()
    source_by_case: dict[str, str] = {}
    resolved = []
    for index, plan in enumerate(plans):
        require(isinstance(plan, dict) and plan.get("index") == index, "Plan index mismatch")
        row = rows_by_index[index]
        for field in ("caseId", "arm", "trial"):
            require(row.get(field) == plan.get(field), "Result identity does not match its frozen plan")
        identity = (plan.get("caseId"), plan.get("arm"), plan.get("trial"))
        require(identity[0] in cases and identity[1] in ("baseline", "candidate") and integer(identity[2]) and 1 <= identity[2] <= trials, "Unknown case, arm or trial")
        require(identity not in identities, "Duplicate case/arm/trial identity")
        identities.add(identity)
        mode = plan.get("mode")
        require(isinstance(mode, dict) and mode.get("baseMode") == "auto" and mode.get("rules") == "" and row.get("mode") == "auto", "Mode must remain automatic with empty rules")
        require(row.get("status") in ("parsed", "rejected", "failed", "truncated"), "Unexpected incomplete result state")
        require("response" not in row or isinstance(row["response"], dict), "Invalid response envelope")
        if row["status"] == "parsed":
            require(row.get("parser") == "passed" and isinstance(row.get("validatedText"), str), "Accepted row missing accepted text")
        if row["status"] == "rejected":
            require(row.get("parser") == "failed" and isinstance(row.get("validationError"), dict), "Rejected row missing parser error")
        text = source_text(plan)
        if identity[0] in source_by_case:
            require(source_by_case[identity[0]] == text, "Paired arms/trials have different source text")
        source_by_case[identity[0]] = text
        require(isinstance(row.get("projectedDelivery"), dict), "Missing projected-delivery evidence")
        projection = row["projectedDelivery"]
        projection_scope = "SIMULATED_CURRENT_INITIAL_BUDGET" if receipt["tool"] == NATURAL_TOOL else "SYNTHETIC_NORMAL_INITIAL_PROJECTION_ONLY"
        require(projection.get("scope") == projection_scope and projection.get("actualNativeDelivery") == "NOT_RUN", "Unexpected native-delivery claim")
        require(projection.get("status") in ("not_projected", "source_fallback", "rewrite_candidate"), "Unknown projected-delivery status")
        if projection["status"] == "not_projected":
            require(projection.get("text") is None, "Unprojected result must not supply final text")
        else:
            require(isinstance(projection.get("text"), str), "Projected text is missing")
        if projection["status"] == "source_fallback":
            require(projection["text"] == text, "Projected source fallback differs from frozen source")
        if projection["status"] == "rewrite_candidate":
            require(row["status"] == "parsed" and projection["text"] == row["validatedText"], "Projected candidate differs from accepted text")
        resolved.append((plan, row, cases[identity[0]]))
    expected = {(case, arm, trial) for case in cases for arm in ("baseline", "candidate") for trial in range(1, trials + 1)}
    require(identities == expected, "Incomplete arm pairing")
    return resolved


def review_payload(source: str, output: str, case: dict[str, Any]) -> dict[str, Any]:
    return {"source": source, **{name: case[name] for name in CHECKLISTS}, "output": output}


def expected_outputs(plan: dict[str, Any], row: dict[str, Any]) -> tuple[dict[str, str | None], str]:
    candidate, schema = decode_candidate(row)
    return {"candidateReviewId": candidate, "acceptedReviewId": row.get("validatedText") if row["status"] == "parsed" else None,
            "finalReviewId": row["projectedDelivery"].get("text")}, schema


def prepare(receipt_path: Path, out: Path) -> dict[str, Any]:
    receipt, raw = load(receipt_path)
    resolved = validate_receipt(receipt)
    require(out.is_absolute() and not out.exists(), "Output packet directory must be a new absolute path")
    packets: dict[tuple[str, str], dict[str, Any]] = {}
    bindings = []
    for plan, row, case in resolved:
        source = source_text(plan)
        outputs, schema = expected_outputs(plan, row)
        binding = {"index": plan["index"], "caseId": plan["caseId"], "arm": plan["arm"], "trial": plan["trial"],
                   "rowStatus": row["status"], "candidateSchemaOrigin": schema}
        for field, output in outputs.items():
            if output is None:
                binding[field] = None
                continue
            key = (source, output)
            payload = review_payload(source, output, case)
            if key not in packets:
                packets[key] = {"id": "review-" + uuid.uuid4().hex, **payload}
            else:
                require({name: value for name, value in packets[key].items() if name != "id"} == payload,
                        "Same source/output has conflicting semantic checklists")
            binding[field] = packets[key]["id"]
        bindings.append(binding)
    shuffled = list(packets.values())
    secrets.SystemRandom().shuffle(shuffled)
    packet = {"schemaVersion": 1, "reviewType": LABEL, "scope": review_scope(receipt),
              "instructions": "Judge each source/output against all semantic constraints. Do not infer product identity. No unique target wording is required. Mark critical semantic errors explicitly. This packet does not establish real audio recognition or paste quality.",
              "items": shuffled}
    packet_raw = json_bytes(packet)
    binding_document = {"schemaVersion": 1, "privateDoNotSendToReviewer": True, "receiptSha256": digest(raw),
                        "packetSha256": digest(packet_raw), "rows": bindings}
    bindings_raw = json_bytes(binding_document)
    template = {"schemaVersion": 1, "reviewType": LABEL, "receiptSha256": digest(raw), "packetSha256": digest(packet_raw),
                "bindingsSha256": digest(bindings_raw), "judgments": [
                    {"id": item["id"], **{field: None for field in FIELDS}, "criticalErrors": None, "notes": None}
                    for item in shuffled]}
    out.mkdir(parents=False, exist_ok=False)
    write_new(out / "packet.json", packet)
    write_new(out / "bindings.json", binding_document)
    write_new(out / "review-template.json", template)
    return {"status": "prepared_unreviewed", "reviewItems": len(shuffled), "plannedCalls": len(resolved),
            "reviewType": LABEL, "privateFile": "bindings.json", "scope": review_scope(receipt)}


def validate_packet(receipt: dict[str, Any], receipt_raw: bytes, packet: Any, packet_raw: bytes, bindings: Any) -> dict[str, dict[str, Any]]:
    require(isinstance(packet, dict) and packet.get("schemaVersion") == 1 and packet.get("reviewType") == LABEL, "Unknown packet schema")
    require(isinstance(bindings, dict) and bindings.get("receiptSha256") == digest(receipt_raw) and bindings.get("packetSha256") == digest(packet_raw), "Receipt/packet hash mismatch")
    items = packet.get("items")
    require(isinstance(items, list), "Missing packet items")
    by_id: dict[str, dict[str, Any]] = {}
    seen_texts: set[tuple[str, str]] = set()
    for item in items:
        require(isinstance(item, dict) and set(item) == {"id", "source", "output", *CHECKLISTS}, "Packet item contains unexpected metadata")
        require(isinstance(item["id"], str) and item["id"] not in by_id, "Duplicate packet review ID")
        require(isinstance(item["source"], str) and isinstance(item["output"], str), "Invalid packet text")
        key = (item["source"], item["output"])
        require(key not in seen_texts, "Packet must deduplicate source/output")
        seen_texts.add(key)
        by_id[item["id"]] = item
    resolved = validate_receipt(receipt)
    rows = bindings.get("rows")
    require(isinstance(rows, list) and len(rows) == len(resolved), "Incomplete private bindings")
    used: set[str] = set()
    for (plan, row, case), binding in zip(resolved, rows):
        require(isinstance(binding, dict), "Invalid binding row")
        for field in ("index", "caseId", "arm", "trial"):
            require(binding.get(field) == plan[field], "Binding identity mismatch")
        require(binding.get("rowStatus") == row["status"], "Binding status mismatch")
        outputs, schema = expected_outputs(plan, row)
        require(binding.get("candidateSchemaOrigin") == schema, "Candidate schema provenance mismatch")
        for field, output in outputs.items():
            review_id = binding.get(field)
            if output is None:
                require(review_id is None, "Missing output cannot bind a review")
            else:
                require(review_id in by_id, "Output has no review packet")
                expected = {"id": review_id, **review_payload(source_text(plan), output, case)}
                require(by_id[review_id] == expected, "Bound packet content differs from original receipt")
                used.add(review_id)
    require(used == set(by_id), "Packet has unbound or missing review items")
    return by_id


def validate_judgments(review: Any, by_id: dict[str, Any], receipt_raw: bytes, packet_raw: bytes, bindings_raw: bytes) -> dict[str, Any]:
    require(isinstance(review, dict) and review.get("schemaVersion") == 1 and review.get("reviewType") == LABEL, "Unknown review schema/type")
    require(review.get("receiptSha256") == digest(receipt_raw) and review.get("packetSha256") == digest(packet_raw)
            and review.get("bindingsSha256") == digest(bindings_raw), "Review evidence hashes mismatch")
    judgments = review.get("judgments")
    require(isinstance(judgments, list) and len(judgments) == len(by_id), "Review must cover every packet ID exactly once")
    result = {}
    for judgment in judgments:
        require(isinstance(judgment, dict) and set(judgment) == {"id", *FIELDS, "criticalErrors", "notes"}, "Unexpected judgment fields")
        review_id = judgment["id"]
        require(isinstance(review_id, str) and review_id in by_id and review_id not in result, "Unknown or duplicate judged ID")
        require(all(type(judgment[field]) is bool for field in FIELDS), "All five quality judgments must be explicit booleans; null is unreviewed")
        require(isinstance(judgment["criticalErrors"], list) and all(isinstance(text, str) and text.strip() for text in judgment["criticalErrors"]), "criticalErrors must be a list of nonempty strings")
        require(isinstance(judgment["notes"], str), "notes must be an explicit string")
        require(not judgment["criticalErrors"] or judgment["fidelity"] is False, "Critical semantic errors require fidelity=false")
        result[review_id] = judgment
    require(set(result) == set(by_id), "Some review IDs are missing")
    return result


def usable(judgment: dict[str, Any] | None) -> bool:
    return judgment is not None and all(judgment[field] is True for field in FIELDS) and not judgment["criticalErrors"]


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    position = (len(ordered) - 1) * fraction
    lower, upper = math.floor(position), math.ceil(position)
    return round(ordered[lower] + (ordered[upper] - ordered[lower]) * (position - lower), 3)


def score(receipt_path: Path, packet_dir: Path, review_path: Path, out: Path) -> dict[str, Any]:
    require(packet_dir.is_absolute() and out.is_absolute() and not out.exists(), "Packet/output paths must be absolute; output must be new")
    receipt, receipt_raw = load(receipt_path)
    packet, packet_raw = load(packet_dir / "packet.json")
    bindings, bindings_raw = load(packet_dir / "bindings.json")
    review, review_raw = load(review_path)
    by_id = validate_packet(receipt, receipt_raw, packet, packet_raw, bindings)
    judgments = validate_judgments(review, by_id, receipt_raw, packet_raw, bindings_raw)
    resolved = validate_receipt(receipt)
    arms = {}
    details = []
    for arm in ("baseline", "candidate"):
        stats: dict[str, Any] = {"denominator": 0, "generatedExtractable": 0, "generatedCompleteUsable": 0,
            "accepted": 0, "acceptedAndUsable": 0, "projectedFinalUsable": 0, "projectedSourceFallback": 0,
            "projectedSourceFallbackUsable": 0, "guardFalseRejects": 0, "usableButRejected": 0,
            "acceptedCritical": 0, "generatedCritical": 0, "acceptedCriticalItems": [], "guardFalseRejectItems": [],
            "candidateDimensionPass": {field: 0 for field in FIELDS}, "projectedFinalDimensionPass": {field: 0 for field in FIELDS}}
        latencies = []
        for (plan, row, _case), binding in zip(resolved, bindings["rows"]):
            if plan["arm"] != arm:
                continue
            stats["denominator"] += 1
            candidate = judgments.get(binding["candidateReviewId"])
            accepted = judgments.get(binding["acceptedReviewId"])
            final = judgments.get(binding["finalReviewId"])
            generated_complete = row.get("response", {}).get("finishReason") == "stop" and candidate is not None
            generated_usable = generated_complete and usable(candidate)
            accepted_usable = row["status"] == "parsed" and usable(accepted)
            final_usable = usable(final)
            stats["generatedExtractable"] += candidate is not None
            stats["generatedCompleteUsable"] += generated_usable
            stats["accepted"] += row["status"] == "parsed"
            stats["acceptedAndUsable"] += accepted_usable
            stats["projectedFinalUsable"] += final_usable
            is_fallback = row["projectedDelivery"]["status"] == "source_fallback"
            stats["projectedSourceFallback"] += is_fallback
            stats["projectedSourceFallbackUsable"] += is_fallback and final_usable
            rejected_usable = row["status"] == "rejected" and generated_usable
            false_guard = rejected_usable and row["validationError"].get("code") == "rewrite_meaning_risk"
            stats["usableButRejected"] += rejected_usable
            stats["guardFalseRejects"] += false_guard
            identity = {"index": plan["index"], "caseId": plan["caseId"], "trial": plan["trial"]}
            if false_guard:
                stats["guardFalseRejectItems"].append(identity)
            if candidate and candidate["criticalErrors"]:
                stats["generatedCritical"] += 1
            if row["status"] == "parsed" and accepted and accepted["criticalErrors"]:
                stats["acceptedCritical"] += 1
                stats["acceptedCriticalItems"].append({**identity, "criticalErrors": accepted["criticalErrors"]})
            for field in FIELDS:
                stats["candidateDimensionPass"][field] += candidate is not None and candidate[field] is True
                stats["projectedFinalDimensionPass"][field] += final is not None and final[field] is True
            timing = row.get("cloudElapsedMs")
            if type(timing) in (int, float) and math.isfinite(timing) and timing >= 0:
                latencies.append(float(timing))
            details.append({**identity, "arm": arm, "rowStatus": row["status"], "generatedCompleteUsable": generated_usable,
                "acceptedAndUsable": accepted_usable, "projectedFinalUsable": final_usable,
                "projectedKind": row["projectedDelivery"]["status"], "guardFalseReject": false_guard,
                "candidateReviewId": binding["candidateReviewId"], "acceptedReviewId": binding["acceptedReviewId"], "finalReviewId": binding["finalReviewId"]})
        stats["latencyMs"] = {"scope": "attempted cloud calls with recorded timing, including failed requests; excludes native paste and ASR",
            "count": len(latencies), "p50": percentile(latencies, .5), "p95": percentile(latencies, .95),
            "min": min(latencies) if latencies else None, "max": max(latencies) if latencies else None}
        arms[arm] = stats
    result = {"schemaVersion": 1, "tool": "speakmode-automatic-quality-review", "reviewType": LABEL, "scope": review_scope(receipt),
        "receiptSha256": digest(receipt_raw), "packetSha256": digest(packet_raw), "bindingsSha256": digest(bindings_raw),
        "reviewSha256": digest(review_raw), "split": receipt["split"], "caseCount": receipt["caseCount"], "trials": receipt["trials"],
        "judgedUniqueSourceOutputPairs": len(judgments), "arms": arms, "results": sorted(details, key=lambda row: row["index"]),
        "definitions": {"usable": "All five independent-agent boolean judgments true and no critical errors",
            "generatedCompleteUsable": "Extractable model output with finishReason=stop and full review pass",
            "acceptedAndUsable": "Parser accepted actual validatedText and that text passed review; never source fallback",
            "projectedFinalUsable": "Reviewed projected final text passed, including separately counted source fallback; not measured delivery",
            "guardFalseRejects": "Complete generated text passed all review checks but was rejected with rewrite_meaning_risk; schema rejects counted only in usableButRejected",
            "dimensionDenominator": "All planned calls for that arm; missing generations never count as dimension passes",
            "percentiles": "Linear interpolation using (n-1)*p; small fixed sample, no population confidence claim"},
        "limitations": ["Independent Agent review; not human listening", "Finite synthetic text does not measure ASR, microphone capture, real Typeless output, editing time or native paste",
            "Candidate may combine prompt, dictionary and parser changes; no component-level causal attribution",
            "Source fallback is not model success. Projected final usability is not confirmed paste.",
            "Trials reuse the same fixed cases; denominator is calls, not independent speakers or recordings"]}
    if receipt["tool"] == NATURAL_TOOL:
        result["sourceTool"] = NATURAL_TOOL
        result["sourceScope"] = receipt["sourceScope"]
        if receipt["split"] == "user":
            result["limitations"][1] = "One user-provided transcript does not measure ASR, microphone capture, real Typeless output, editing time or native paste"
    replay_metadata = offline_replay_metadata(receipt)
    if replay_metadata is not None:
        result["evaluationKind"] = "offline_reparse"
        result["offlineReplay"] = replay_metadata
        result["newCloudRequests"] = 0
        result["definitions"]["acceptedAndUsable"] = "Current offline parser accepted retained original model text and that text passed review; not a new provider result"
        result["limitations"].append("Offline replay only: current compiled guards reparsed original responses. No new cloud requests; original request/response and latency measurements are retained.")
        for arm in result["arms"].values():
            arm["latencyMs"]["scope"] = "retained original cloud timings, including failed requests; offline reparse adds zero cloud calls and is not a new latency measurement"
    write_new(out, result)
    return result


def synthetic_receipt(split: str = "dev", tool: str = TOOL) -> dict[str, Any]:
    """Unit-test-only fixtures, never the project's actual holdout or provider outputs."""
    count, trials = receipt_design({"tool": tool, "split": split})
    fixture_split = {"holdout": "holdout_reserved", "user": "user_provided", "dev": "dev"}[split]
    cases = [{"id": f"TEST{index}", "split": fixture_split, "mustPreserve": ["Keep the meaning"],
              "allowedCleanup": ["Remove synthetic filler"], "mustNotDo": ["Invent facts"]} for index in range(count)]
    plans, rows = [], []
    for trial in range(1, trials + 1):
        for case in cases:
            for arm in ("baseline", "candidate"):
                index = len(plans)
                text, output = "Um synthetic " + case["id"], "Synthetic " + case["id"]
                plans.append({"index": index, "caseId": case["id"], "arm": arm, "trial": trial,
                    "mode": {"baseMode": "auto", "rules": ""}, "source": {"complete": True,
                    "items": [{"id": case["id"] + ":0", "text": text, "final": True, "captureOrder": 0}]}})
                content = {"text": output} if arm == "candidate" else {"blocks": [{"text": output}]}
                rows.append({"index": index, "caseId": case["id"], "arm": arm, "trial": trial, "mode": "auto",
                    "status": "parsed", "parser": "passed", "validatedText": output,
                    "response": {"modelContent": json.dumps(content), "finishReason": "stop"}, "cloudElapsedMs": 100 + index,
                    "projectedDelivery": {"scope": "SIMULATED_CURRENT_INITIAL_BUDGET" if tool == NATURAL_TOOL else "SYNTHETIC_NORMAL_INITIAL_PROJECTION_ONLY", "actualNativeDelivery": "NOT_RUN",
                                          "status": "rewrite_candidate", "text": output}})
    receipt = {"tool": tool, "status": "completed", "split": split, "caseCount": count, "trials": trials,
            "maximumCalls": count * trials * 2, "reviewChecklists": cases, "plan": plans, "results": rows}
    if tool == NATURAL_TOOL:
        receipt["sourceScope"] = "USER_PROVIDED_TRANSCRIPT_NO_AUDIO" if split == "user" else "SYNTHETIC_TRANSCRIPT_NO_AUDIO"
    return receipt


class SelfTests(unittest.TestCase):
    def setup_packet(self, directory: str, receipt: dict[str, Any] | None = None):
        root = Path(directory).resolve()
        receipt_path, packet_dir = root / "receipt.json", root / "packet"
        write_new(receipt_path, synthetic_receipt() if receipt is None else receipt)
        prepare(receipt_path, packet_dir)
        template, _ = load(packet_dir / "review-template.json")
        return root, receipt_path, packet_dir, template

    def fill_test_review(self, template: dict[str, Any]) -> None:
        # Test data only. Real prepare never calls this or supplies a default grade.
        for judgment in template["judgments"]:
            judgment.update({field: True for field in FIELDS})
            judgment["criticalErrors"], judgment["notes"] = [], "Unit-test fixture only"

    def test_prepare_blinds_deduplicates_and_leaves_all_judgments_null(self):
        with tempfile.TemporaryDirectory() as directory:
            _, _, packet_dir, template = self.setup_packet(directory)
            packet, _ = load(packet_dir / "packet.json")
            self.assertEqual(len(packet["items"]), 12)
            for item in packet["items"]:
                self.assertEqual(set(item), {"id", "source", "output", *CHECKLISTS})
            self.assertTrue(all(all(row[field] is None for field in (*FIELDS, "criticalErrors", "notes")) for row in template["judgments"]))

    def test_finite_design_and_pair_identity_validation(self):
        self.assertEqual(len(validate_receipt(synthetic_receipt("holdout"))), 16)
        bad = synthetic_receipt(); bad["results"].pop()
        with self.assertRaises(ReviewError): validate_receipt(bad)

    def test_natural_splits_prepare_score_with_fixed_counts_and_blind_null_reviews(self):
        for split, count in (("dev", 12), ("holdout", 8), ("user", 1)):
            with self.subTest(split=split), tempfile.TemporaryDirectory() as directory:
                receipt = synthetic_receipt(split, NATURAL_TOOL)
                self.assertEqual(len(validate_receipt(receipt)), count * 2)
                root, receipt_path, packet_dir, template = self.setup_packet(directory, receipt)
                packet, _ = load(packet_dir / "packet.json")
                self.assertEqual(len(packet["items"]), count)
                self.assertEqual(packet["scope"], USER_SCOPE if split == "user" else SCOPE)
                self.assertTrue(all(all(row[field] is None for field in (*FIELDS, "criticalErrors", "notes")) for row in template["judgments"]))
                self.assertTrue(all(set(item) == {"id", "source", "output", *CHECKLISTS} for item in packet["items"]))
                self.fill_test_review(template)
                review_path = root / "review.json"; write_new(review_path, template)
                result = score(receipt_path, packet_dir, review_path, root / "score.json")
                self.assertEqual(result["caseCount"], count); self.assertEqual(result["trials"], 1)
                self.assertEqual(result["sourceTool"], NATURAL_TOOL)
                for arm in ("baseline", "candidate"):
                    self.assertEqual(result["arms"][arm]["denominator"], count)
                    self.assertEqual(result["arms"][arm]["generatedCompleteUsable"], count)
                    self.assertEqual(result["arms"][arm]["projectedFinalUsable"], count)

    def test_natural_finite_counts_and_source_projection_labels_cannot_be_mixed(self):
        for split in ("dev", "holdout", "user"):
            for field in ("caseCount", "trials", "maximumCalls"):
                with self.subTest(split=split, field=field):
                    bad = synthetic_receipt(split, NATURAL_TOOL); bad[field] += 1
                    with self.assertRaises(ReviewError): validate_receipt(bad)
            bad = synthetic_receipt(split, NATURAL_TOOL); bad["results"].pop()
            with self.assertRaises(ReviewError): validate_receipt(bad)
            bad = synthetic_receipt(split, NATURAL_TOOL); bad["reviewChecklists"][0]["split"] = "wrong"
            with self.assertRaises(ReviewError): validate_receipt(bad)
            bad = synthetic_receipt(split, NATURAL_TOOL); bad["results"][0]["projectedDelivery"]["scope"] = "SYNTHETIC_NORMAL_INITIAL_PROJECTION_ONLY"
            with self.assertRaises(ReviewError): validate_receipt(bad)
        old = synthetic_receipt("holdout"); old["tool"] = NATURAL_TOOL
        old["sourceScope"] = "SYNTHETIC_TRANSCRIPT_NO_AUDIO"
        with self.assertRaises(ReviewError): validate_receipt(old)
        bad = synthetic_receipt("user", NATURAL_TOOL); bad["sourceScope"] = "SYNTHETIC_TRANSCRIPT_NO_AUDIO"
        with self.assertRaises(ReviewError): validate_receipt(bad)
        bad = synthetic_receipt("user", NATURAL_TOOL); bad["tool"] = TOOL
        with self.assertRaises(ReviewError): validate_receipt(bad)

    def test_natural_private_review_still_requires_bound_explicit_quality_judgments(self):
        with tempfile.TemporaryDirectory() as directory:
            root, receipt_path, packet_dir, template = self.setup_packet(directory, synthetic_receipt("user", NATURAL_TOOL))
            blank_path = root / "blank.json"; write_new(blank_path, template)
            with self.assertRaises(ReviewError): score(receipt_path, packet_dir, blank_path, root / "score.json")
            self.fill_test_review(template)
            template["judgments"][0].update(fidelity=False, criticalErrors=["Synthetic critical error"])
            review_path = root / "review.json"; write_new(review_path, template)
            result = score(receipt_path, packet_dir, review_path, root / "score.json")
            self.assertEqual(result["arms"]["candidate"]["acceptedCritical"], 1)
            self.assertEqual(result["arms"]["candidate"]["generatedCompleteUsable"], 0)
            with (packet_dir / "packet.json").open("ab") as file: file.write(b" ")
            with self.assertRaises(ReviewError): score(receipt_path, packet_dir, review_path, root / "score2.json")

    def test_offline_reparse_score_keeps_original_measurements_and_compiled_guard_identity(self):
        for keys in REPLAY_GUARD_KEY_SETS:
            with self.subTest(guard_count=len(keys)), tempfile.TemporaryDirectory() as directory:
                receipt = synthetic_receipt("user", NATURAL_TOOL)
                receipt["offlineReplay"] = {"scope": REPLAY_SCOPE, "originalReceiptSha256": "a" * 64, "cloudRequests": 0,
                    "currentGuardHashes": {key: "b" * 64 for key in sorted(keys)},
                    "retainedLatency": "Original timings only", "meaningChecks": "Synthetic test guards"}
                original = json_bytes(receipt)
                root, receipt_path, packet_dir, template = self.setup_packet(directory, receipt)
                self.fill_test_review(template)
                review_path = root / "review.json"; write_new(review_path, template)
                result = score(receipt_path, packet_dir, review_path, root / "score.json")
                self.assertEqual(result["evaluationKind"], "offline_reparse")
                self.assertEqual(result["newCloudRequests"], 0)
                self.assertEqual(result["offlineReplay"], receipt["offlineReplay"])
                self.assertEqual(result["arms"]["baseline"]["latencyMs"]["p50"], 100.0)
                self.assertIn("retained original", result["arms"]["baseline"]["latencyMs"]["scope"])
                self.assertEqual(receipt_path.read_bytes(), original)
                packet, _ = load(packet_dir / "packet.json")
                self.assertTrue(all(set(item) == {"id", "source", "output", *CHECKLISTS} for item in packet["items"]))

    def test_offline_reparse_guard_key_sets_reject_missing_and_unknown_keys(self):
        metadata = {"scope": REPLAY_SCOPE, "originalReceiptSha256": "a" * 64, "cloudRequests": 0}
        for keys in REPLAY_GUARD_KEY_SETS:
            # Removing only the new key yields the explicitly supported legacy set.
            invalid_sets = [keys - {missing} for missing in LEGACY_REPLAY_GUARD_KEYS]
            invalid_sets += [keys | {"unknownGuard"}, (keys - {"facts"}) | {"unknownGuard"},
                             LEGACY_REPLAY_GUARD_KEYS | {"quantityRoles"}]
            for invalid_keys in invalid_sets:
                bad = synthetic_receipt("user", NATURAL_TOOL)
                bad["offlineReplay"] = {**metadata, "currentGuardHashes": {key: "b" * 64 for key in invalid_keys}}
                with self.subTest(keys=sorted(invalid_keys)), self.assertRaises(ReviewError):
                    validate_receipt(bad)

    def test_offline_reparse_guard_hashes_require_valid_values_in_both_schemas(self):
        for keys in REPLAY_GUARD_KEY_SETS:
            for invalid_key in keys:
                for invalid_hash in (None, "", "b" * 63, "B" * 64, "g" * 64):
                    hashes = {key: "b" * 64 for key in keys}
                    hashes[invalid_key] = invalid_hash
                    bad = synthetic_receipt("user", NATURAL_TOOL)
                    bad["offlineReplay"] = {"scope": REPLAY_SCOPE, "originalReceiptSha256": "a" * 64,
                        "cloudRequests": 0, "currentGuardHashes": hashes}
                    with self.subTest(guard_count=len(keys), key=invalid_key, value=invalid_hash), self.assertRaises(ReviewError):
                        validate_receipt(bad)

    def test_offline_reparse_cannot_claim_calls_or_omit_current_guard_hashes(self):
        metadata = {"scope": REPLAY_SCOPE, "originalReceiptSha256": "a" * 64, "cloudRequests": 0,
            "currentGuardHashes": {key: "b" * 64 for key in ("parser", "integrity", "facts", "semantics", "disfluency")}}
        for field, invalid in (("cloudRequests", 1), ("cloudRequests", False), ("scope", "cloud_run"),
                               ("originalReceiptSha256", None), ("currentGuardHashes", {"parser": "b" * 64})):
            bad = synthetic_receipt("user", NATURAL_TOOL)
            bad["offlineReplay"] = {**metadata, field: invalid}
            with self.subTest(field=field, invalid=invalid), self.assertRaises(ReviewError): validate_receipt(bad)
        for tool, split in ((TOOL, "dev"), (TOOL, "holdout"), (NATURAL_TOOL, "dev"), (NATURAL_TOOL, "holdout"), (NATURAL_TOOL, "user")):
            self.assertIsNone(offline_replay_metadata(synthetic_receipt(split, tool)))

    def test_holdout_reserved_metadata_prepares_and_scores_without_corpus_relabeling(self):
        with tempfile.TemporaryDirectory() as directory:
            receipt = synthetic_receipt("holdout")
            self.assertTrue(all(case["split"] == "holdout_reserved" for case in receipt["reviewChecklists"]))
            root, receipt_path, packet_dir, template = self.setup_packet(directory, receipt)
            self.fill_test_review(template)
            review_path = root / "review.json"; write_new(review_path, template)
            result = score(receipt_path, packet_dir, review_path, root / "score.json")
            self.assertEqual(result["caseCount"], 4); self.assertEqual(result["trials"], 2)
            for arm in ("baseline", "candidate"):
                self.assertEqual(result["arms"][arm]["denominator"], 8)
                self.assertEqual(result["arms"][arm]["generatedCompleteUsable"], 8)
            receipt["reviewChecklists"][0]["split"] = "holdout"
            with self.assertRaises(ReviewError): validate_receipt(receipt)
        bad = synthetic_receipt(); bad["results"][0]["arm"] = "candidate"
        with self.assertRaises(ReviewError): validate_receipt(bad)
        bad = synthetic_receipt()
        item = bad["plan"][0]["source"]["items"][0]
        item["isFinal"] = item.pop("final")
        with self.assertRaises(ReviewError): validate_receipt(bad)

    def test_score_uses_all_call_denominators(self):
        with tempfile.TemporaryDirectory() as directory:
            root, receipt_path, packet_dir, template = self.setup_packet(directory)
            self.fill_test_review(template)
            review_path = root / "review.json"; write_new(review_path, template)
            result = score(receipt_path, packet_dir, review_path, root / "score.json")
            for arm in ("baseline", "candidate"):
                self.assertEqual(result["arms"][arm]["denominator"], 12)
                self.assertEqual(result["arms"][arm]["generatedCompleteUsable"], 12)
                self.assertEqual(result["arms"][arm]["acceptedAndUsable"], 12)
                self.assertEqual(result["arms"][arm]["projectedFinalUsable"], 12)

    def test_source_fallback_is_not_model_acceptance_and_guard_needs_review_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            receipt = synthetic_receipt(); row = receipt["results"][1]
            row.update(status="rejected", parser="failed", validationError={"code": "rewrite_meaning_risk"})
            row.pop("validatedText")
            row["projectedDelivery"].update(status="source_fallback", text=source_text(receipt["plan"][1]))
            root, receipt_path, packet_dir, template = self.setup_packet(directory, receipt)
            self.fill_test_review(template)
            bindings, _ = load(packet_dir / "bindings.json")
            final_id = bindings["rows"][1]["finalReviewId"]
            next(judgment for judgment in template["judgments"] if judgment["id"] == final_id)["cleanup"] = False
            review_path = root / "review.json"; write_new(review_path, template)
            result = score(receipt_path, packet_dir, review_path, root / "score.json")["arms"]["candidate"]
            self.assertEqual(result["generatedCompleteUsable"], 12)
            self.assertEqual(result["acceptedAndUsable"], 11)
            self.assertEqual(result["guardFalseRejects"], 1)
            self.assertEqual(result["projectedFinalUsable"], 11)
            self.assertEqual(result["projectedSourceFallback"], 1)

    def test_missing_or_nonboolean_judgments_cannot_score(self):
        with tempfile.TemporaryDirectory() as directory:
            root, receipt_path, packet_dir, template = self.setup_packet(directory)
            review_path = root / "review.json"; write_new(review_path, template)
            with self.assertRaises(ReviewError): score(receipt_path, packet_dir, review_path, root / "score.json")
            self.fill_test_review(template); template["judgments"][0]["fidelity"] = "true"
            second = root / "review2.json"; write_new(second, template)
            with self.assertRaises(ReviewError): score(receipt_path, packet_dir, second, root / "score.json")

    def test_hash_tampering_and_accepted_critical_errors(self):
        with tempfile.TemporaryDirectory() as directory:
            root, receipt_path, packet_dir, template = self.setup_packet(directory)
            self.fill_test_review(template)
            template["judgments"][0].update(fidelity=False, criticalErrors=["Synthetic critical error"])
            review_path = root / "review.json"; write_new(review_path, template)
            result = score(receipt_path, packet_dir, review_path, root / "score.json")
            self.assertEqual(result["arms"]["candidate"]["acceptedCritical"], 1)
            self.assertEqual(result["arms"]["baseline"]["acceptedCritical"], 1)
            with (packet_dir / "packet.json").open("ab") as file: file.write(b" ")
            with self.assertRaises(ReviewError): score(receipt_path, packet_dir, review_path, root / "score2.json")

    def test_parsing_keeps_actual_single_text_and_blocks_outputs(self):
        self.assertEqual(decode_candidate({"response": {"modelContent": '{"text":"single"}'}}), ("single", "single_text"))
        self.assertEqual(decode_candidate({"response": {"modelContent": '{"blocks":[{"text":"a"},{"text":"b"}]}'}}), ("a\n\nb", "blocks_text"))
        self.assertEqual(decode_candidate({"response": {"modelContent": '{bad'}}), (None, "invalid_json"))

    def test_changed_accepted_text_gets_its_own_blind_judgment(self):
        with tempfile.TemporaryDirectory() as directory:
            receipt = synthetic_receipt()
            receipt["results"][1]["validatedText"] = "Postprocessed output"
            receipt["results"][1]["projectedDelivery"]["text"] = "Postprocessed output"
            _, _, packet_dir, _ = self.setup_packet(directory, receipt)
            bindings, _ = load(packet_dir / "bindings.json")
            binding = bindings["rows"][1]
            self.assertNotEqual(binding["candidateReviewId"], binding["acceptedReviewId"])
            self.assertEqual(binding["acceptedReviewId"], binding["finalReviewId"])


def main() -> int:
    if sys.argv[1:] == ["--self-test"]:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SelfTests)
        return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    preparation = sub.add_parser("prepare")
    preparation.add_argument("--receipt", type=Path, required=True)
    preparation.add_argument("--out", type=Path, required=True)
    scoring = sub.add_parser("score")
    scoring.add_argument("--receipt", type=Path, required=True)
    scoring.add_argument("--packet-dir", type=Path, required=True)
    scoring.add_argument("--review", type=Path, required=True)
    scoring.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == "prepare":
            result = prepare(args.receipt, args.out)
        else:
            full = score(args.receipt, args.packet_dir, args.review, args.out)
            result = {"status": "scored", "reviewType": LABEL, "scope": full["scope"], "split": full["split"], "arms": full["arms"]}
            if "offlineReplay" in full:
                result.update(evaluationKind=full["evaluationKind"], offlineReplay=full["offlineReplay"], newCloudRequests=0)
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0
    except (ReviewError, OSError) as failure:
        print(json.dumps({"error": str(failure)}, ensure_ascii=False), file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
