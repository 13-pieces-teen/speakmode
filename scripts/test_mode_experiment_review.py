import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("review", Path(__file__).with_name("mode-experiment-review.py"))
review = importlib.util.module_from_spec(spec)
spec.loader.exec_module(review)


class ReviewAccounting(unittest.TestCase):
    def test_duplicate_text_counts_each_call_and_rejections_stay_in_denominator(self):
        fixture = {"cases": [{"id": "x", "sourceItems": ["不要发。"], "mustPreserve": ["禁止发送"],
                    "expectedOrganization": {"light": "保序", "structure": "短句"}, "acceptableAutoModes": ["light"]}]}
        accepted = {"caseId": "x", "mode": "light", "arm": "candidate", "status": "parsed",
                    "validatedOutput": {"resolvedMode": "light", "blocks": [{"text": "不要发。"}]}}
        receipt = {"results": [{**accepted, "index": 0, "elapsedMs": 100},
                                {**accepted, "index": 1, "elapsedMs": 200},
                    {"caseId": "x", "mode": "light", "arm": "candidate", "status": "rejected", "index": 2, "elapsedMs": 300}]}
        receipt["plan"] = [{"index": i} for i in range(3)]
        with tempfile.TemporaryDirectory() as directory:
            prefix = Path(directory) / "p"
            review.packet(receipt, fixture, prefix)
            packet = review.read(str(prefix) + ".packet.json")
            self.assertEqual(len(packet["items"]), 1)
            grade_path = Path(directory) / "g.json"
            review.write_new(grade_path, {"reviews": [{"reviewId": packet["items"][0]["reviewId"],
                "fidelity": True, "critical": False, "modeTask": True, "route": True, "reason": "完整"}]})
            out = Path(directory) / "s.json"
            review.summarize(receipt, review.read(str(prefix) + ".mapping.json"), [grade_path], out)
            stats = review.read(out)["arms"]["candidate"]
            self.assertEqual((stats["planned"], stats["deliveredTaskPass"], stats["unreviewable"]), (3, 2, 1))
            self.assertEqual((stats["p50Ms"], stats["p95Ms"]), (200, 300))

    def test_no_silent_double_review_or_missing_grade(self):
        receipt = {"results": [{"index": 0, "arm": "a", "mode": "light", "caseId": "x", "status": "parsed"}]}
        receipt["plan"] = [{"index": 0}]
        with tempfile.TemporaryDirectory() as directory:
            grades = Path(directory) / "g.json"
            review.write_new(grades, {"reviews": []})
            with self.assertRaisesRegex(ValueError, "Missing review"):
                review.summarize(receipt, [{"index": 0, "reviewId": "r"}], [grades], Path(directory) / "out.json")
            grades.write_text(json.dumps({"reviews": [{"reviewId": "r", "fidelity": True, "critical": False,
                "modeTask": True, "route": True}]}), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "Duplicate review"):
                review.summarize(receipt, [{"index": 0, "reviewId": "r"}], [grades, grades], Path(directory) / "out.json")

    def test_cannot_overwrite_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "evidence.json"
            review.write_new(path, {"original": True})
            with self.assertRaises(FileExistsError):
                review.write_new(path, {})

    def test_interrupted_results_cannot_shrink_denominator(self):
        with self.assertRaisesRegex(ValueError, "Incomplete"):
            review.summarize({"plan": [{"index": 0}], "results": []}, [], [], "never-written.json")


if __name__ == "__main__":
    unittest.main()
