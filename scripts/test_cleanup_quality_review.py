import importlib.util
import copy
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location("cleanup_review", Path(__file__).with_name("cleanup-quality-review.py"))
review = importlib.util.module_from_spec(spec)
spec.loader.exec_module(review)


class CleanupReviewTests(unittest.TestCase):
    def fixture(self):
        rows = [{"index": i, "caseId": "fixed", "arm": "baseline" if i % 2 == 0 else "candidate",
                 "mode": ("light", "structure", "auto")[(i // 2) % 3], "status": "parsed", "cloudElapsedMs": 1000}
                for i in range(144)]
        receipt = {"tool": "speakmode-cleanup-quality-lab", "maximumCalls": 144, "status": "completed",
                   "plan": [dict(r) for r in rows], "results": rows}
        mapping = {"receiptSha256": review.digest(receipt), "rows": [{"index": i, "reviewId": "R001"} for i in range(144)]}
        verdict = {"reviewerId": "test-reviewer", "items": [{"reviewId": "R001", **{k: True for k in review.DIMENSIONS},
                   "criticalError": False, "notes": "Synthetic test only"}]}
        return receipt, mapping, verdict

    def test_failure_stays_in_denominator_and_does_not_count_as_accepted(self):
        receipt, mapping, verdict = self.fixture()
        receipt["results"][0]["status"] = "rejected"
        mapping["receiptSha256"] = review.digest(receipt)
        result = review.score(receipt, mapping, verdict)["arms"]["baseline"]
        self.assertEqual(result["planned"], 72)
        self.assertEqual(result["generatedTaskPass"], 72)
        self.assertEqual(result["acceptedTaskPass"], 71)

    def test_retained_correction_process_cannot_pass_by_reviewer_override(self):
        receipt, mapping, verdict = self.fixture()
        receipt["results"][1]["necessaryChecks"] = {"remainingRemovableTerms": ["我说错了"]}
        mapping["receiptSha256"] = review.digest(receipt)
        self.assertEqual(review.score(receipt, mapping, verdict)["arms"]["candidate"]["cleanupPass"], 71)

    def test_missing_review_cannot_be_reported_as_quality_pass(self):
        receipt, mapping, verdict = self.fixture()
        verdict["items"] = []
        with self.assertRaises(ValueError):
            review.score(receipt, mapping, verdict)

    def test_changed_receipt_rejected_after_blind_packet_prepared(self):
        receipt, mapping, verdict = self.fixture()
        receipt["results"][1]["cloudElapsedMs"] = 1
        with self.assertRaises(ValueError):
            review.score(receipt, mapping, verdict)

    def test_replay_changes_only_acceptance_and_budget_counts(self):
        receipt, mapping, verdict = self.fixture()
        receipt["results"][1]["cloudElapsedMs"] = 3500
        mapping["receiptSha256"] = review.digest(receipt)
        replay = {"tool": "speakmode-cleanup-parser-replay", "results": [
            {"index": r["index"], "caseId": r["caseId"], "arm": r["arm"], "mode": r["mode"],
             "replayedParser": {"accepted": r["index"] != 0}} for r in receipt["results"]]}
        result = review.score(receipt, mapping, verdict, replay)
        self.assertEqual(result["arms"]["baseline"]["acceptedTaskPass"], 71)
        self.assertEqual(result["arms"]["baseline"]["generatedTaskPass"], 72)
        self.assertEqual(result["arms"]["candidate"]["acceptedByBudgetMs"], {"2000": 71, "3000": 71, "5000": 72})

    def model_inputs(self):
        flash, _, _ = self.fixture()
        flash.update(configuration={"rewriteModel": "qwen-flash", "region": "beijing"},
                     fixtureSha256="f", candidateSha256="c", candidatePromptVersion="frozen",
                     reviewChecklists=[], scoring={}, parserSha256="p", factsSha256="f", integritySha256="i", semanticsSha256="s")
        for row, plan in zip(flash["results"], flash["plan"]):
            row["caseId"] = plan["caseId"] = f"fixed-{row['index'] // 6}"
            plan.update(source={"text": row["caseId"]}, requestBody={"model": "qwen-flash", "messages": ["frozen"]})
        plus = copy.deepcopy(flash)
        plus.update(maximumCalls=72, candidateModelOverride="qwen-plus", configuration={"rewriteModel": "qwen-plus", "region": "beijing"})
        plus["results"] = [copy.deepcopy(r) for r in flash["results"] if r["arm"] == "candidate"]
        plus["plan"] = [copy.deepcopy(p) for p in flash["plan"] if p["arm"] == "candidate"]
        for i, (row, plan) in enumerate(zip(plus["results"], plus["plan"])):
            row["index"] = plan["index"] = i
            plan["requestBody"]["model"] = "qwen-plus"
        replay = {"tool": "speakmode-cleanup-parser-replay", "results": [
            {"index": r["index"], "caseId": r["caseId"], "arm": r["arm"], "mode": r["mode"], "replayedParser": {"accepted": True}} for r in flash["results"]]}
        replay.update({key: flash[key] for key in ("parserSha256", "factsSha256", "integritySha256", "semanticsSha256")})
        return flash, plus, replay

    def test_model_comparison_preserves_source_receipts_and_records_separate_stages(self):
        flash, plus, replay = self.model_inputs()
        frozen = copy.deepcopy((flash, plus))
        result = review.combine_models(flash, plus, replay)
        self.assertEqual(len(review.validate_results(result)), 144)
        self.assertEqual((flash, plus), frozen)
        self.assertIn("SEPARATE_STAGES", result["scope"])
        self.assertEqual(result["newRequestsInThisDerivedArtifact"], 0)

    def test_model_comparison_rejects_prompt_or_parser_drift(self):
        flash, plus, replay = self.model_inputs()
        plus["plan"][0]["requestBody"]["messages"] = ["tuned-after-results"]
        with self.assertRaises(ValueError):
            review.combine_models(flash, plus, replay)
        flash, plus, replay = self.model_inputs()
        plus["factsSha256"] = "different-parser"
        with self.assertRaises(ValueError):
            review.combine_models(flash, plus, replay)


if __name__ == "__main__":
    unittest.main()
