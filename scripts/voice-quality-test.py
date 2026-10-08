"""Offline denominator, revision, scoring and human-rating boundary tests.

Test WAVs are generated silence, explicitly synthetic; they are never evidence
of microphone or model accuracy. No network, credentials or app DB are read.
"""
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
import wave

spec = importlib.util.spec_from_file_location("voice_quality", Path(__file__).with_name("voice-quality-lab.py"))
lab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lab)


def write(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")


def reference(fid):
    text = "如果时间没确认，先不要发邀请，可能周五试用 SpeakMode。"
    return {"schemaVersion": 1, "fixtureId": fid, "language": "mixed", "transcript": text,
            "deletableFillers": [],
            "facts": [{"id": "conditional-action", "description": "未确认时不得发邀请；试用日期只是可能",
                       "sourceStart": 0, "sourceEnd": len(text), "disposition": "retained", "critical": True, "kind": "negation"}],
            "positiveTerms": ["SpeakMode"], "negativeTerms": ["WASAPI"],
            "modeCriteria": {mode: [{"id": "format", "description": "条件和行动保持关联"}]
                             for mode in ("light", "structure")}}


class QualityBoundaries(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.audio = self.root / "silent-test.wav"
        with wave.open(str(self.audio), "wb") as writer:
            writer.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
            writer.writeframes(b"\0\0" * 1600)
        self.ref = self.root / "ref.json"
        write(self.ref, reference("sample"))
        config = {"asrModel": "test-asr", "rewriteModel": "test-rewrite", "adapter": "test-adapter", "region": "local",
                  "language": "auto", "dictionarySha256": lab.sha(b""), "promptSha256": lab.sha(b"prompt"), "parameters": {}}
        self.manifest = {"schemaVersion": 1, "experimentId": "test-local", "sourceCommit": "a" * 40,
                         "arms": [{"id": "baseline", "task": "pipeline", "mode": "light", "config": config}],
                         "fixtures": [{"id": "sample", "speakerId": "synthetic-01", "split": "test", "sourceKind": "synthetic",
                                       "category": "negation", "tags": [], "collectionStatus": "ready",
                                       "audioPath": str(self.audio), "referencePath": str(self.ref)}]}
        self.manifest_path = self.root / "manifest.json"
        self.lock_path = self.root / "lock.json"
        self.receipts = self.root / "receipts"
        self.reviews = self.root / "reviews"

    def freeze(self):
        write(self.manifest_path, self.manifest)
        self.lock = lab.freeze(self.manifest_path, self.lock_path)
        self.rid = next(iter(lab.registrations(self.lock)))

    def receipt(self, run="first", status="succeeded", purpose="primary", parent=None):
        return {"schemaVersion": 1, "runId": run, "registrationId": self.rid, "purpose": purpose,
                "diagnosticOf": parent, "status": status, "asrText": reference("sample")["transcript"],
                "outputText": reference("sample")["transcript"], "protocolComplete": True,
                "latencyMs": {key: None for key in lab.LATENCIES}, "cost": {"currency": None, "amount": None},
                "errorCode": None, "executedConfigSha256": lab.digest(self.manifest["arms"][0]["config"]),
                "actualAsrVersion": None, "actualRewriteVersion": None}

    def record(self, row):
        path = self.root / "draft.json"
        write(path, row)
        return lab.record(self.lock_path, path, self.receipts)

    def review(self, reviewer="r1", verdict="correct", role="reviewer", run="first"):
        rating = {"schemaVersion": 1, "runId": run, "reviewerId": reviewer, "role": role, "blind": True,
                  "factVerdicts": {"conditional-action": verdict}, "modeVerdicts": {"format": "correct"}, "addedFacts": False}
        path = self.root / "rating.json"
        write(path, rating)
        lab.review_record(self.lock_path, self.receipts, path, self.reviews)
        return rating

    def evaluate(self):
        return lab.evaluate(self.lock_path, self.receipts, self.reviews)

    def test_absent_and_failed_entries_stay_in_preregistered_denominator(self):
        missing = copy.deepcopy(self.manifest["fixtures"][0])
        missing.update(id="unrecorded", speakerId="synthetic-02", collectionStatus="missing", audioPath=None, referencePath=None)
        self.manifest["fixtures"].append(missing)
        self.freeze()
        initial = self.evaluate()["groups"][0]
        self.assertEqual(initial["registered"], 2)
        self.assertIsNone(initial["usableDraft"]["rate"])
        row = self.receipt(status="failed")
        row.update(asrText=None, outputText=None, protocolComplete=False, errorCode="transport-failure")
        self.record(row)
        score = self.evaluate()["groups"][0]
        self.assertEqual(score["statuses"], {"missing": 1, "failed": 1, "succeeded": 0})
        self.assertEqual(score["usableDraft"], {"numerator": 0, "denominator": 2, "rate": 0.0})
        self.assertEqual(score["alignment"]["cer"]["rate"], 1.0)
        self.assertEqual(score["positiveTerms"]["rate"], 0)
        self.assertEqual(score["negativeTermIntrusions"]["unobservedTermChecks"], 1)

    def test_duplicate_primary_and_run_ids_cannot_replace_first_failure(self):
        self.freeze()
        self.record(self.receipt(status="failed"))
        with self.assertRaises(lab.ValidationError):
            self.record(self.receipt())
        with self.assertRaises(lab.ValidationError):
            self.record(self.receipt(run="second"))
        original, _ = lab.read_json(self.receipts / "first.json")
        self.assertEqual(original["status"], "failed")

    def test_diagnostic_success_never_changes_primary_quality_but_cost_is_retained(self):
        self.freeze()
        first = self.receipt(status="failed")
        first["cost"] = {"currency": "CNY", "amount": .2}
        self.record(first)
        retry = self.receipt(run="diagnostic", purpose="diagnostic", parent="first")
        retry["cost"] = {"currency": "CNY", "amount": .3}
        self.record(retry)
        report = self.evaluate()
        self.assertEqual(report["groups"][0]["usableDraft"]["numerator"], 0)
        self.assertEqual(report["diagnostics"]["runs"], 1)
        self.assertEqual(report["allCallsCost"]["total"], .5)
        self.assertEqual(report["groups"][0]["cost"]["total"], .2)

    def test_orphan_diagnostic_rejected(self):
        self.freeze()
        with self.assertRaises(lab.ValidationError):
            self.record(self.receipt(run="retry", purpose="diagnostic", parent="unseen"))

    def test_audio_reference_and_manifest_changes_are_detected(self):
        for kind in ("audio", "reference", "manifest"):
            with self.subTest(kind=kind):
                if self.lock_path.exists():
                    self.lock_path.unlink()  # Temporary synthetic test path only.
                write(self.ref, reference("sample"))
                self.freeze()
                if kind == "audio":
                    with self.audio.open("ab") as out:
                        out.write(b"changed")
                elif kind == "reference":
                    ref = reference("sample")
                    ref["facts"][0]["description"] = "changed rubric"
                    write(self.ref, ref)
                else:
                    self.manifest_path.write_text(self.manifest_path.read_text(encoding="utf-8") + "\n", encoding="utf-8")
                with self.assertRaises(lab.ValidationError):
                    lab.verify_lock(self.lock_path)

    def test_pcm_format_and_truncation_rejected(self):
        with wave.open(str(self.audio), "wb") as writer:
            writer.setparams((2, 2, 16000, 0, "NONE", "not compressed"))
            writer.writeframes(b"\0\0\0\0" * 1600)
        with self.assertRaises(lab.ValidationError):
            self.freeze()
        with wave.open(str(self.audio), "wb") as writer:
            writer.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
            writer.writeframes(b"\0\0" * 1600)
        self.audio.write_bytes(self.audio.read_bytes()[:-2])
        with self.assertRaises(lab.ValidationError):
            self.freeze()

    def test_strict_schema_rejects_duplicate_json_unknown_fields_and_boolean_numbers(self):
        path = self.root / "invalid.json"
        path.write_text('{"id":1,"id":2}', encoding="utf-8")
        with self.assertRaises(lab.ValidationError):
            lab.read_json(path)
        self.manifest["accidentalField"] = "ignored data"
        with self.assertRaises(lab.ValidationError):
            self.freeze()
        del self.manifest["accidentalField"]
        self.freeze()
        row = self.receipt()
        row["latencyMs"]["stopToReady"] = True
        with self.assertRaises(lab.ValidationError):
            self.record(row)

    def test_dev_and_blind_test_cannot_share_a_speaker(self):
        duplicate = copy.deepcopy(self.manifest["fixtures"][0])
        duplicate.update(id="other", split="dev")
        self.manifest["fixtures"].append(duplicate)
        with self.assertRaises(lab.ValidationError):
            self.freeze()

    def test_human_and_synthetic_never_pool(self):
        human_ref = self.root / "human-ref.json"
        write(human_ref, reference("human"))
        human = copy.deepcopy(self.manifest["fixtures"][0])
        human.update(id="human", sourceKind="human", speakerId="human-01", referencePath=str(human_ref))
        self.manifest["fixtures"].append(human)
        self.freeze()
        report = self.evaluate()
        self.assertEqual({g["sourceKind"] for g in report["groups"]}, {"human", "synthetic"})
        self.assertTrue(all(g["registered"] == 1 and g["state"] == "NOT_RUN" for g in report["groups"]))

    def test_alignment_and_terms_have_explicit_case_width_and_word_boundaries(self):
        self.assertEqual(lab.cer_tokens("ＡＢＣ，３.５！"), list("abc35"))
        self.assertEqual(lab.wer_tokens("Don't submit 3.5%"), ["don", "t", "submit", "3", "5"])
        self.assertEqual(lab.wer_tokens("检查 WASAPI 和 version 1.7", mixed=True), ["wasapi", "version", "1", "7"])
        self.assertFalse(lab.term_present("WASAPI", "WASAPIs"))
        self.assertTrue(lab.term_present("SpeakMode", "ＳｐｅａｋＭｏｄｅ"))
        self.assertEqual(lab.distance(["not", "submit"], ["submit"]), 1)

    def test_complete_independent_reviews_are_required_and_disputes_need_adjudication(self):
        self.freeze()
        self.record(self.receipt())
        self.review()
        self.assertEqual(self.evaluate()["groups"][0]["usableDraft"]["numerator"], 0)
        self.review(reviewer="r2", verdict="incorrect")
        self.assertEqual(self.evaluate()["groups"][0]["humanConsensusRuns"], 0)
        self.review(reviewer="judge", role="adjudicator")
        self.assertEqual(self.evaluate()["groups"][0]["usableDraft"]["numerator"], 1)

    def test_missing_mode_rating_and_duplicate_rater_rejected(self):
        self.freeze()
        self.record(self.receipt())
        self.review()
        with self.assertRaises(lab.ValidationError):
            self.review()
        raw = {"schemaVersion": 1, "runId": "first", "reviewerId": "r2", "role": "reviewer", "blind": True,
               "factVerdicts": {"conditional-action": "correct"}, "modeVerdicts": {}, "addedFacts": False}
        path = self.root / "rating.json"
        write(path, raw)
        with self.assertRaises(lab.ValidationError):
            lab.review_record(self.lock_path, self.receipts, path, self.reviews)

    def test_protocol_complete_cannot_substitute_for_semantic_quality(self):
        self.freeze()
        self.record(self.receipt())
        self.review(verdict="incorrect")
        self.review(reviewer="r2", verdict="incorrect")
        score = self.evaluate()["groups"][0]
        self.assertEqual(score["protocolComplete"]["rate"], 1)
        self.assertEqual(score["usableDraft"]["rate"], 0)

    def test_cost_unknown_is_not_zero_and_small_sample_latency_is_labeled(self):
        self.freeze()
        row = self.receipt()
        row["latencyMs"]["stopToReady"] = 2500
        self.record(row)
        score = self.evaluate()["groups"][0]
        self.assertEqual(score["cost"]["unknownCostRuns"], 1)
        self.assertIsNone(score["cost"]["total"])
        self.assertEqual(score["latencyMs"]["stopToReady"]["p95"], 2500)
        self.assertTrue(score["latencyMs"]["stopToReady"]["smallSample"])

    def test_result_edits_and_execution_config_drift_are_detected(self):
        self.freeze()
        row = self.receipt()
        row["executedConfigSha256"] = "f" * 64
        with self.assertRaises(lab.ValidationError):
            self.record(row)
        self.record(self.receipt())
        result_path = self.receipts / "first.json"
        result, _ = lab.read_json(result_path)
        result["outputText"] = "edited later"
        write(result_path, result)
        with self.assertRaises(lab.ValidationError):
            self.evaluate()

    def test_statistics_do_not_export_text_or_local_paths(self):
        self.freeze()
        self.record(self.receipt())
        report_text = json.dumps(self.evaluate(), ensure_ascii=False)
        self.assertNotIn(reference("sample")["transcript"], report_text)
        self.assertNotIn(str(self.root), report_text)
        self.assertNotIn("不要发邀请", report_text)

    def test_24_collection_tasks_claim_no_recordings_or_rate(self):
        directory = self.root / "collection"
        result = lab.init_collection(directory, "a" * 40)
        self.assertEqual(result["collected"], 0)
        self.assertEqual(result["tasks"], 24)
        manifest, _ = lab.read_json(directory / "manifest.json")
        self.assertTrue(all(f["collectionStatus"] == "missing" and f["sourceKind"] == "human" for f in manifest["fixtures"]))
        frozen = directory / "lock.json"
        lab.freeze(directory / "manifest.json", frozen)
        report = lab.evaluate(frozen, directory / "receipts", directory / "reviews")
        self.assertTrue(all(g["usableDraft"]["rate"] is None for g in report["groups"]))
        with self.assertRaises(FileExistsError):
            lab.init_collection(directory, "a" * 40)


if __name__ == "__main__":
    unittest.main(verbosity=2)
