import copy
import hashlib
import importlib.util
import json
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("scene_review", Path(__file__).with_name("scene-writing-review.py"))
review = importlib.util.module_from_spec(spec)
spec.loader.exec_module(review)


def fixture():
    cases = [{"id": f"D{i}", "sourceItems": ["预算3500元，先不要发送。"], "baseMode": "auto", "rules": "",
              "scene": "chat", "useNearbyText": False, "nearbyText": None, "expectations": ["数字和否定保留"]} for i in range(8)]
    corpus = {"schemaVersion": 1, "split": "dev", "cases": cases}
    encoded = json.dumps(corpus, ensure_ascii=False).encode()
    plan, results = [], []
    for case in cases:
        for arm in review.ARMS:
            plan.append({"caseId": case["id"], "arm": arm, "source": {"items": [{"text": case["sourceItems"][0]}]},
                         "mode": {"baseMode": "auto", "rules": ""}, "sceneSnapshot": {key: case[key] for key in ("scene", "useNearbyText", "nearbyText")},
                         "maximumCalls": 4 if arm == "baseline" else 2})
            results.append({"caseId": case["id"], "arm": arm, "status": "completed", "clientStartedCalls": 2,
                            "pipelineAccepted": True, "fallback": False, "deliveredText": case["sourceItems"][0], "elapsedMs": 2000})
    receipt = {"tool": "scene-writing-lab-v1", "status": "completed", "split": "dev", "fixtureSha256": hashlib.sha256(encoded).hexdigest(),
               "maximumCalls": 64, "clientStartedCalls": 48, "plan": plan, "results": results}
    return corpus, encoded, receipt


class ReviewIntegrity(unittest.TestCase):
    def test_missing_or_duplicate_arm_cannot_improve_denominator(self):
        corpus, encoded, receipt = fixture()
        review.validate(receipt, corpus, encoded)
        receipt["results"][0] = copy.deepcopy(receipt["results"][1])
        with self.assertRaises(ValueError):
            review.validate(receipt, corpus, encoded)

    def test_source_context_and_request_accounting_must_match_frozen_plan(self):
        for mutate in (
            lambda r: r["plan"][0]["source"]["items"][0].update(text="预算9000元"),
            lambda r: r["plan"][0]["sceneSnapshot"].update(useNearbyText=True),
            lambda r: r.update(clientStartedCalls=47),
            lambda r: r["results"][0].update(clientStartedCalls=5),
        ):
            corpus, encoded, receipt = fixture()
            mutate(receipt)
            with self.assertRaises(ValueError):
                review.validate(receipt, corpus, encoded)

    def test_truncated_fallback_is_not_valid_evidence(self):
        corpus, encoded, receipt = fixture()
        receipt["results"][0].update(fallback=True, pipelineAccepted=False, deliveredText="预算3500元。")
        with self.assertRaises(ValueError):
            review.validate(receipt, corpus, encoded)

    def test_packet_hides_arm_and_transport_verdict(self):
        corpus, _, receipt = fixture()
        packet = review.prepare(receipt, corpus)
        self.assertEqual(len(packet["cases"]), 8)
        for case in packet["cases"]:
            self.assertEqual(len(case["outputs"]), 3)
            for output in case["outputs"]:
                self.assertEqual(set(output), {"id", "text"})

    def test_semantic_scores_cannot_be_inferred_from_acceptance(self):
        _, _, receipt = fixture()
        scores = [{"id": review.blind_id(receipt, row), **{k: True for k in review.DIMENSIONS},
                   "note": "合成核对", "correctedText": row["deliveredText"]} for row in receipt["results"]]
        result = review.summarize(receipt, {"fixtureSha256": receipt["fixtureSha256"], "scores": scores})
        self.assertEqual(result["arms"]["scene"]["textUsable"], 8)
        scores[0]["faithful"] = "pass"
        with self.assertRaises(ValueError):
            review.summarize(receipt, {"fixtureSha256": receipt["fixtureSha256"], "scores": scores})

    def test_unicode_edit_distance_includes_actual_layout_changes(self):
        self.assertEqual(review.distance("你好🚀", "你好🚀"), 0)
        self.assertEqual(review.distance("甲。乙。", "甲。\n乙。"), 1)
        self.assertEqual(review.distance("预算9000", "预算3500"), 2)


if __name__ == "__main__":
    unittest.main()
