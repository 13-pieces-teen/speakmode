"""Mock-only exporter boundaries. No app, microphone, provider or profile access."""
import copy
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch, mock_open

spec = importlib.util.spec_from_file_location("voice_export", Path(__file__).with_name("export-voice-diagnostics.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

RUN = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
SESSION = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"
ATTEMPT = "cccccccc-cccc-4ccc-8ccc-cccccccccccc"


def fixture():
    return {"schemaVersion": 1, "appVersion": "0.1.2", "inMemoryOnly": True,
            "timingScope": module.TIMING_SCOPE, "droppedEvents": 2, "evictedTraces": 1,
            "traces": [{"runId": RUN, "sessionId": SESSION, "generation": 7,
                        "origin": "uiCommand", "startedAt": "2026-10-07T10:00:00+08:00",
                        "provider": "aliyun", "region": "beijing", "asrModel": "qwen3-asr-flash-realtime",
                        "rewriteModel": "qwen-flash", "modeId": "auto", "modeRevision": 1,
                        "asrAttemptId": ATTEMPT, "captureDurationMs": 4700, "sourceCharCount": 36,
                        "bridgeQueuePeakFrames": 5, "outcome": "failed", "errorCode": "incomplete_source_coverage",
                        "marks": [{"stage": "firstCaptionReceived", "offsetMs": 150.2},
                                  {"stage": "processingWorkerReleased", "offsetMs": 4900}],
                        "durations": {"intentToFirstCaptionMs": 150.2, "captionToRenderReceiptProxyMs": 4.1}}]}


class ExportBoundaries(unittest.TestCase):
    def test_provider_metadata_keeps_known_ids_and_redacts_custom_model_or_endpoint(self):
        raw = fixture()
        raw["traces"][0].update(provider="google", asrModel="gemini-3.5-transcribe", rewriteProvider="custom", rewriteModel="PRIVATE_SENTINEL", rewriteEndpoint="PRIVATE_SENTINEL")
        clean = module.filter_snapshot(raw)
        row = clean["traces"][0]
        self.assertEqual(row["provider"], "google")
        self.assertEqual(row["rewriteProvider"], "custom")
        self.assertEqual(row["asrModel"], "gemini-3.5-transcribe")
        self.assertIsNone(row["rewriteModel"])
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))

    def test_request_timings_export_only_bounded_metadata(self):
        raw = dict(stage="generation", startMs=100, endMs=300, durationMs=200,
                   remainingBudgetMs=4999, outcome="responseReceived", prompt="PRIVATE_SENTINEL",
                   response="PRIVATE_SENTINEL", apiKey="PRIVATE_SENTINEL")
        clean = module.filter_rewrite_requests([raw])
        self.assertEqual(clean[0]["durationMs"], 200)
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))
        self.assertEqual(module.filter_rewrite_requests([]), [])
        inflight = {**raw, "endMs": None, "durationMs": None, "outcome": None}
        self.assertIsNone(module.filter_rewrite_requests([inflight])[0]["outcome"])
        for invalid in ([raw] * 5, [raw, raw], [{**raw, "endMs": 90}],
                        [{**raw, "durationMs": 199}], [{**raw, "remainingBudgetMs": True}],
                        [{**raw, "remainingBudgetMs": 120001}], [{**raw, "startMs": "PRIVATE_SENTINEL"}],
                        [{**raw, "endMs": None}], [{**raw, "durationMs": float("nan")}], {}):
            with self.subTest(invalid=invalid), self.assertRaises(module.ExportError):
                module.filter_rewrite_requests(invalid)

    def test_unknown_request_labels_never_export_arbitrary_text(self):
        raw = dict(stage="PRIVATE_SENTINEL", startMs=0, endMs=1, durationMs=1,
                   remainingBudgetMs=4999, outcome="PRIVATE_SENTINEL")
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(module.filter_rewrite_requests([raw])))

    def test_minimum_edit_protocol_is_known_and_private_review_is_filtered(self):
        for version in (3, 4):
            protocol = f"minimum-edit-v{version}+minimum-review-v{version}+residue-hint-v2+local-repair-v2"
            clean = module.filter_rewrite(dict(protocolVersion=protocol, configuredBudgetMs=5000,
                    outcome="validated", fallbackReason=None, errorCode=None, durationMs=2400,
                    review={"source": "PRIVATE_SENTINEL", "draft": "PRIVATE_SENTINEL"}))
            self.assertEqual(clean["protocolVersion"], protocol)
            self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))
    def test_compact_review_protocol_is_identifiable_without_exporting_review_text(self):
        protocol = "fluent-dictation-v1+semantic-review-compact-v2"
        clean = module.filter_rewrite(dict(protocolVersion=protocol, configuredBudgetMs=5000,
                    outcome="validated", fallbackReason=None, errorCode=None, durationMs=2192,
                    review={"source": "PRIVATE_SENTINEL", "draft": "PRIVATE_SENTINEL"}))
        self.assertEqual(clean["protocolVersion"], protocol)
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))

    def test_rewrite_metadata_survives_ready_outcome_without_claiming_validation(self):
        raw = self.wire_fixture()
        row = raw["traces"][0]
        row.update(outcome="ready", errorCode=None, rewrite={
            "protocolVersion": "mode-contract-v1", "configuredBudgetMs": 5000,
            "outcome": "fallback", "fallbackReason": "meaningRisk",
            "errorCode": "rewrite_meaning_risk", "durationMs": 3100.25,
            "prompt": "PRIVATE_SENTINEL", "response": "PRIVATE_SENTINEL", "apiKey": "PRIVATE_SENTINEL"})
        clean = module.filter_snapshot(raw)["traces"][0]
        self.assertEqual(clean["outcome"], "ready")
        self.assertEqual(clean["rewrite"]["outcome"], "fallback")
        self.assertEqual(clean["rewrite"]["fallbackReason"], "meaningRisk")
        self.assertEqual(clean["rewrite"]["errorCode"], "rewrite_meaning_risk")
        self.assertEqual(clean["rewrite"]["durationMs"], 3100.25)
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))

    def test_rewrite_metadata_absent_in_old_receipts_remains_absent(self):
        for raw in (fixture(), self.wire_fixture()):
            self.assertNotIn("rewrite", module.filter_snapshot(raw)["traces"][0])
            raw["traces"][0]["rewrite"] = None
            self.assertIsNone(module.filter_snapshot(raw)["traces"][0]["rewrite"])

    def test_rewrite_metadata_redacts_unknown_strings_and_rejects_bad_timings(self):
        value = dict(protocolVersion="PRIVATE_SENTINEL", configuredBudgetMs=5000,
                     outcome="PRIVATE_SENTINEL", fallbackReason="PRIVATE_SENTINEL",
                     errorCode="PRIVATE_SENTINEL", durationMs=200)
        clean = module.filter_rewrite(value)
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))
        self.assertIsNone(clean["errorCode"])
        for field, invalid in (("configuredBudgetMs", True), ("configuredBudgetMs", 0),
                               ("configuredBudgetMs", 120001), ("durationMs", -1),
                               ("durationMs", float("nan")), ("durationMs", "private text")):
            with self.subTest(field=field, invalid=invalid), self.assertRaises(module.ExportError):
                module.filter_rewrite({**value, field: invalid})

    def test_rewrite_error_codes_keep_provider_and_parser_causes_distinct(self):
        for code in ("rewrite_timeout", "authentication_failed", "rewrite_service_error",
                     "invalid_source_mapping", "incomplete_source_coverage", "truncated_output"):
            clean = module.filter_rewrite(dict(protocolVersion="mode-contract-v1", configuredBudgetMs=5000,
                outcome="fallback", fallbackReason="invalidOutput", errorCode=code, durationMs=100))
            self.assertEqual(clean["errorCode"], code)

    def test_admission_and_semantic_rejections_keep_fixed_labels_without_private_evidence(self):
        for code in ("rewrite_admission_rejected", "rewrite_semantic_rejected"):
            with self.subTest(code=code):
                raw = self.wire_fixture()
                row = raw["traces"][0]
                row.update(outcome="ready", errorCode=code, rewrite={
                    "protocolVersion": "fluent-dictation-v1+semantic-review-compact-v2",
                    "configuredBudgetMs": 20000, "outcome": "fallback", "fallbackReason": "meaningRisk",
                    "errorCode": code, "durationMs": 10400,
                    "numericKeys": ["PRIVATE_SENTINEL"], "sourceQuote": "PRIVATE_SENTINEL",
                    "draftQuote": "PRIVATE_SENTINEL", "reviewResponse": "PRIVATE_SENTINEL"})
                clean = module.filter_snapshot(raw)["traces"][0]
                self.assertEqual(clean["errorCode"], code)
                self.assertEqual(clean["rewrite"]["errorCode"], code)
                self.assertEqual(clean["rewrite"]["fallbackReason"], "meaningRisk")
                self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))
                row["errorCode"] = row["rewrite"]["errorCode"] = code + ":PRIVATE_SENTINEL"
                clean = module.filter_snapshot(raw)["traces"][0]
                self.assertIsNone(clean["errorCode"])
                self.assertIsNone(clean["rewrite"]["errorCode"])
                self.assertNotIn("PRIVATE_SENTINEL", json.dumps(clean))

    def test_fallback_stage_is_exported_without_claiming_validation(self):
        raw = fixture()
        raw['traces'][0]['marks'] = [{'stage': 'rewriteStarted', 'offsetMs': 100}, {'stage': 'rewriteFallbackSelected', 'offsetMs': 200}]
        clean = module.filter_snapshot(raw)
        stages = [mark['stage'] for mark in clean['traces'][0]['marks']]
        self.assertEqual(stages, ['rewriteStarted', 'rewriteFallbackSelected'])
        self.assertNotIn('rewriteValidated', stages)

    def test_fixed_target_capture_codes_preserved_without_arbitrary_details(self):
        for code in ('stop_anchor_unavailable','stop_focus_changed','target_not_editable',
                     'target_capability_unsupported','target_inspection_timeout'):
            raw = fixture()
            raw['traces'][0]['errorCode'] = code
            raw['traces'][0]['targetText'] = 'PRIVATE_SENTINEL'
            value = module.filter_snapshot(raw)
            self.assertEqual(value['traces'][0]['errorCode'], code)
            self.assertNotIn('PRIVATE_SENTINEL', json.dumps(value))

    def setUp(self):
        self.exe = (module.ROOT / "output/releases/fake-test/SpeakMode.exe").resolve()

    def wire_fixture(self):
        raw = fixture()
        raw.update(schemaVersion=2, appVersion="0.1.7", timingScope=module.TIMING_SCOPE_V2)
        row = raw["traces"][0]
        row["errorCode"] = "asr_incomplete"
        row["marks"].append({"stage": "asrFinishWritten", "offsetMs": 4100})
        row["durations"]["stopToFinishWrittenMs"] = 12
        count = {"frames": 189, "realSamples": 302160, "paddingSamples": 240}
        row["capture"] = {"inputSamples": 906480, "inputSampleRate": 48000,
            "emittedFrames": 189, "complete": True, "cancelled": False}
        row["bridge"] = {"observed": count.copy(), "enqueued": count.copy(),
            "lastFrameEnqueuedMs": 4089, "finishEnqueuedMs": 4090}
        row["asr"] = {"dequeued": count.copy(), "socketWritten": count.copy(),
            "lastFrameWrittenMs": 4095, "finishDequeuedMs": 4096, "finishExpectedFrames": 189,
            "finishWrittenMs": 4100, "sessionFinishedReceivedMs": 4120,
            "pendingItems": 1, "finalItems": 9, "observedEvents": 34, "terminated": True,
            "eventCounts": [{"kind": "partial", "count": 24}, {"kind": "final", "count": 9},
                            {"kind": "sessionFinished", "count": 1}],
            "eventTail": [{"sequence": 34, "kind": "sessionFinished", "offsetMs": 4120,
                "itemOrdinal": None, "previousItemOrdinal": None, "pendingItems": 1, "finalItems": 9}]}
        return raw

    def test_v2_preserves_client_boundaries_and_unfinished_terminal_metadata(self):
        value = module.filter_snapshot(self.wire_fixture())
        row = value["traces"][0]
        self.assertEqual(value["schemaVersion"], 2)
        self.assertEqual(row["asr"]["socketWritten"]["realSamples"], 302160)
        self.assertEqual(row["asr"]["finishWrittenMs"], 4100)
        self.assertEqual(row["asr"]["pendingItems"], 1)
        self.assertEqual(row["errorCode"], "asr_incomplete")
        self.assertEqual(row["capture"]["inputSampleRate"], 48000)

    def test_conditioning_metadata_is_optional_and_contains_only_typed_scalars(self):
        raw = self.wire_fixture()
        self.assertIsNone(module.filter_snapshot(raw)["traces"][0]["capture"]["conditioning"])
        levels = dict(profile="softVoice", version="soft-voice-v1", sampleRateHz=16000,
                      realSamples=1600, inputRmsDbfs=-48.2, outputRmsDbfs=-40.1,
                      inputPeakDbfs=-39, outputPeakDbfs=-30, meanGainDb=8.1, maxGainDb=12,
                      inputClippedSamples=0, outputClippedSamples=0, peakLimitedSamples=2,
                      nonfiniteSamples=0, processingUs=72, pcm=["PRIVATE_SENTINEL"], transcript="PRIVATE_SENTINEL")
        raw["traces"][0]["capture"]["conditioning"] = levels
        result = module.filter_snapshot(raw)
        clean = result["traces"][0]["capture"]["conditioning"]
        self.assertEqual(clean["inputRmsDbfs"], -48.2)
        self.assertEqual(clean["realSamples"], 1600)
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(result))
        for field, invalid in (("realSamples", -1), ("processingUs", True),
                               ("inputRmsDbfs", float("nan")), ("maxGainDb", "PRIVATE_SENTINEL"),
                               ("sampleRateHz", 48000), ("profile", "PRIVATE_SENTINEL")):
            bad = copy.deepcopy(raw)
            bad["traces"][0]["capture"]["conditioning"][field] = invalid
            with self.subTest(field=field), self.assertRaises(module.ExportError) as error:
                module.filter_snapshot(bad)
            self.assertNotIn("PRIVATE_SENTINEL", str(error.exception))

    def test_native_receipt_durations_are_optional_for_old_v1_and_v2(self):
        for raw in (fixture(), self.wire_fixture()):
            value = module.filter_snapshot(raw)
            durations = value["traces"][0]["durations"]
            self.assertIsNone(durations["readyToNativeDeliveryReceiptMs"])
            self.assertIsNone(durations["stopToNativeDeliveryReceiptMs"])

    def test_native_receipt_elapsed_times_never_upgrade_ready_to_delivery_success(self):
        for raw in (fixture(), self.wire_fixture()):
            row = raw["traces"][0]
            row.update(outcome="ready", deliveryState="unconfirmed")
            row["marks"].extend([
                {"stage": "stopIntentReceived", "offsetMs": 4000},
                {"stage": "readyCommitted", "offsetMs": 4500},
                {"stage": "nativeDeliveryReceipt", "offsetMs": 4700},
            ])
            row["durations"].update(readyToNativeDeliveryReceiptMs=200,
                                    stopToNativeDeliveryReceiptMs=700)
            clean = module.filter_snapshot(raw)["traces"][0]
            self.assertEqual(clean["durations"]["readyToNativeDeliveryReceiptMs"], 200)
            self.assertEqual(clean["durations"]["stopToNativeDeliveryReceiptMs"], 700)
            self.assertEqual(clean["outcome"], "ready")
            self.assertNotIn("deliveryState", clean)
            self.assertNotIn("deliveryConfirmed", clean)
            self.assertNotIn("success", json.dumps(clean))

    def test_native_receipt_timings_remain_typed_and_duplicate_receipts_rejected(self):
        for field in ("readyToNativeDeliveryReceiptMs", "stopToNativeDeliveryReceiptMs"):
            for invalid in (float("nan"), float("inf"), -1, True, "PRIVATE_SENTINEL"):
                raw = self.wire_fixture()
                raw["traces"][0]["durations"][field] = invalid
                with self.subTest(field=field, invalid=invalid), self.assertRaises(module.ExportError) as error:
                    module.filter_snapshot(raw)
                self.assertNotIn("PRIVATE_SENTINEL", str(error.exception))
        raw = self.wire_fixture()
        raw["traces"][0]["marks"].extend([
            {"stage": "nativeDeliveryReceipt", "offsetMs": 4500},
            {"stage": "nativeDeliveryReceipt", "offsetMs": 4600},
        ])
        with self.assertRaisesRegex(module.ExportError, "duplicate"):
            module.filter_snapshot(raw)

    def test_v2_drops_nested_payload_and_provider_identifiers(self):
        raw = self.wire_fixture()
        row = raw["traces"][0]
        row.update(transcript="PRIVATE_SENTINEL", apiKey="PRIVATE_SENTINEL")
        row["bridge"]["observed"]["pcm"] = ["PRIVATE_SENTINEL"]
        row["asr"]["eventTail"][0].update(providerItemId="PRIVATE_SENTINEL", text="PRIVATE_SENTINEL")
        row["asr"]["responseBody"] = "PRIVATE_SENTINEL"
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(module.filter_snapshot(raw)))

    def test_v2_keeps_known_asr_failure_codes_and_redacts_unknown_codes(self):
        for code in ("asr_config_timeout", "audio_disconnected", "late_asr_event", "asr_service_error",
                     "dictionary_invalid", "missing_predecessor", "unknown_order", "ambiguous_order", "unexpected_finish"):
            raw = self.wire_fixture()
            raw["traces"][0]["errorCode"] = code
            self.assertEqual(module.filter_snapshot(raw)["traces"][0]["errorCode"], code)
        raw["traces"][0]["errorCode"] = "PRIVATE_SENTINEL"
        self.assertIsNone(module.filter_snapshot(raw)["traces"][0]["errorCode"])

    def test_v2_rejects_unknown_event_type_without_printing_payload(self):
        for collection in ("eventCounts", "eventTail"):
            raw = self.wire_fixture()
            raw["traces"][0]["asr"][collection][0]["kind"] = "PRIVATE_SENTINEL"
            with self.assertRaises(module.ExportError) as error:
                module.filter_snapshot(raw)
            self.assertNotIn("PRIVATE_SENTINEL", str(error.exception))

    def test_dictionary_metadata_keeps_counts_and_drops_private_terms(self):
        value = self.wire_fixture()
        value["traces"][0]["asr"]["dictionary"] = {
            "version": 8, "entryCount": 2, "vocabularyCount": 2, "contextChunks": 0,
            "support": "vocabularyAndContext", "socketWritten": True,
            "effectivenessVerified": True, "terms": ["PRIVATE_TERM"], "sourceText": "PRIVATE_SOURCE"}
        clean = module.filter_snapshot(value)
        dictionary = clean["traces"][0]["asr"]["dictionary"]
        self.assertEqual(dictionary["version"], 8)
        self.assertEqual(dictionary["entryCount"], 2)
        self.assertTrue(dictionary["socketWritten"])
        self.assertFalse(dictionary["effectivenessVerified"])
        self.assertNotIn("PRIVATE", json.dumps(clean))

    def test_v2_bounds_event_arrays_and_rejects_duplicate_event_kinds(self):
        for collection, limit in (("eventTail", 32), ("eventCounts", 14)):
            raw = self.wire_fixture()
            entry = raw["traces"][0]["asr"][collection][0]
            raw["traces"][0]["asr"][collection] = [entry.copy() for _ in range(limit + 1)]
            with self.assertRaises(module.ExportError):
                module.filter_snapshot(raw)
        raw = self.wire_fixture()
        raw["traces"][0]["asr"]["eventCounts"] *= 2
        with self.assertRaises(module.ExportError):
            module.filter_snapshot(raw)

    def test_v2_rejects_event_order_and_boolean_counts(self):
        raw = self.wire_fixture()
        raw["traces"][0]["asr"]["eventTail"] *= 2
        with self.assertRaises(module.ExportError):
            module.filter_snapshot(raw)
        raw = self.wire_fixture()
        raw["traces"][0]["bridge"]["enqueued"]["frames"] = True
        with self.assertRaises(module.ExportError):
            module.filter_snapshot(raw)

    def test_v2_accepts_unstarted_capture_and_cancel_before_finish(self):
        raw = self.wire_fixture()
        raw["traces"][0]["capture"] = None
        raw["traces"][0]["asr"].update(finishWrittenMs=None, sessionFinishedReceivedMs=None)
        value = module.filter_snapshot(raw)
        self.assertIsNone(value["traces"][0]["capture"])
        self.assertIsNone(value["traces"][0]["asr"]["finishWrittenMs"])

    def test_v2_rejects_nonfinite_socket_timing_and_invalid_flags(self):
        for key, value in (("finishWrittenMs", float("nan")), ("terminated", 1)):
            raw = self.wire_fixture()
            raw["traces"][0]["asr"][key] = value
            with self.assertRaises(module.ExportError):
                module.filter_snapshot(raw)

    def valid_args(self, name="fresh-test", **overrides):
        args = {"pid": 123, "port": 19224, "executable": self.exe, "receipt_name": name}
        args.update(overrides)
        with patch.object(Path, "is_file", return_value=True), patch.object(Path, "exists", return_value=False):
            return module.validate_args(**args)

    def test_fresh_name_is_contained_without_creating_any_file(self):
        exe, target = self.valid_args()
        self.assertEqual(exe, self.exe)
        self.assertEqual(target, module.ROOT / "evidence/voice-diagnostics/fresh-test.json")

    def test_traversal_reserved_names_extensions_and_shell_fragments_are_rejected(self):
        for unsafe in ("../escape", "C:/escape", "\\host\\x", "a.json", "NUL", "con", "com1", "lpt9",
                       "test:stream", "folder/name", "x.", "a b", "$(cmd)", "x;cmd", "a" * 65):
            with self.subTest(unsafe=unsafe), self.assertRaises(module.ExportError):
                self.valid_args(unsafe)

    def test_bad_pid_port_and_executable_are_rejected(self):
        for override in ({"pid": 0}, {"pid": True}, {"port": 80}, {"port": 65536},
                         {"executable": module.ROOT.parent / "sibling/SpeakMode.exe"},
                         {"executable": module.ROOT / "output/Other.exe"}):
            with self.subTest(override=override), self.assertRaises(module.ExportError):
                self.valid_args(**override)

    def test_existing_receipt_is_not_overwritten(self):
        with patch.object(Path, "is_file", return_value=True), patch.object(Path, "exists", return_value=True):
            with self.assertRaisesRegex(module.ExportError, "fresh receipt"):
                module.validate_args(123, 19224, self.exe, "old-test")

    def test_writer_uses_exclusive_create_and_cannot_write_elsewhere(self):
        target = module.ROOT / "evidence/voice-diagnostics/write-boundary.json"
        handle = mock_open()
        with patch.object(Path, "mkdir") as mkdir, patch.object(module.os, "open", return_value=55) as open_file, \
             patch.object(module.os, "fdopen", handle):
            module.write_receipt(target, {"snapshot": {"traces": []}})
            self.assertEqual(open_file.call_args.args[1], module.os.O_CREAT | module.os.O_EXCL | module.os.O_WRONLY)
            self.assertEqual(open_file.call_args.args[0], target)
            handle().write.assert_called_once()
            mkdir.assert_called_once_with(parents=True, exist_ok=True)
        with patch.object(Path, "mkdir") as mkdir, self.assertRaises(module.ExportError):
            module.write_receipt(module.ROOT / "runtime/private.json", {})
        mkdir.assert_not_called()

    def test_redirected_output_directory_is_rejected(self):
        original = Path.resolve
        output = module.ROOT / "evidence/voice-diagnostics"
        def redirect(path, *args, **kwargs):
            if path == output:
                return module.ROOT.parent / "outside"
            return original(path, *args, **kwargs)
        with patch.object(Path, "resolve", redirect), self.assertRaisesRegex(module.ExportError, "escaped"):
            self.valid_args()

    def test_snapshot_keeps_only_typed_metadata(self):
        raw = fixture()
        for obj in (raw, raw["traces"][0], raw["traces"][0]["marks"][0], raw["traces"][0]["durations"]):
            obj.update({"text": "private-sentence", "apiKey": "sk-private", "rules": "private-prompt"})
        clean = module.filter_snapshot(raw)
        encoded = json.dumps(clean)
        for private in ("private-sentence", "sk-private", "private-prompt", "apiKey", "rules"):
            self.assertNotIn(private, encoded)
        self.assertEqual(clean["traces"][0]["sourceCharCount"], 36)
        self.assertEqual(clean["traces"][0]["durations"]["intentToFirstCaptionMs"], 150.2)
        self.assertIsNone(clean["traces"][0]["durations"]["stopToReadyMs"])

    def test_free_text_metadata_values_are_redacted(self):
        raw = fixture()
        for field in ("provider", "region", "asrModel", "rewriteModel", "outcome", "errorCode"):
            raw["traces"][0][field] = "sk-private"
        clean = module.filter_snapshot(raw)
        self.assertNotIn("sk-private", json.dumps(clean))
        self.assertIsNone(clean["traces"][0]["errorCode"])

    def test_invalid_schema_uuid_timestamp_and_bounds_never_export(self):
        for key, value in (("schemaVersion", True), ("schemaVersion", 2), ("inMemoryOnly", False),
                           ("droppedEvents", -1), ("evictedTraces", True), ("timingScope", "private-text")):
            raw = fixture(); raw[key] = value
            with self.subTest(key=key), self.assertRaises(module.ExportError):
                module.filter_snapshot(raw)
        for key, value in (("runId", "private-id"), ("modeId", "private-rules"),
                           ("generation", True), ("startedAt", "private-text"), ("sourceCharCount", -1)):
            raw = fixture(); raw["traces"][0][key] = value
            with self.subTest(key=key), self.assertRaises(module.ExportError):
                module.filter_snapshot(raw)
        for value in (float("nan"), float("inf"), -1, True):
            raw = fixture(); raw["traces"][0]["marks"][0]["offsetMs"] = value
            with self.subTest(value=value), self.assertRaises(module.ExportError):
                module.filter_snapshot(raw)
        raw = fixture(); raw["traces"] *= 65
        with self.assertRaises(module.ExportError):
            module.filter_snapshot(raw)

    def test_old_ambiguous_stage_names_are_rejected(self):
        for stage in ("firstPartialReceived", "workerReleased", "private-text"):
            raw = fixture(); raw["traces"][0]["marks"][0]["stage"] = stage
            with self.subTest(stage=stage), self.assertRaises(module.ExportError):
                module.filter_snapshot(raw)

    def test_fixed_main_root_and_own_origin_are_required(self):
        main = SimpleNamespace(url=module.MAIN_URL)
        recorder = SimpleNamespace(url=module.MAIN_URL + "?window=recorder")
        self.assertIs(module.select_main([main, recorder]), main)
        for pages in ([], [recorder], [main, main], [main, SimpleNamespace(url="https://example.com/")],
                      [SimpleNamespace(url=module.MAIN_URL + "?window=main")],
                      [main, SimpleNamespace(url="http://tauri.localhost/other.html")],
                      [main, SimpleNamespace(url="http://tauri.localhost/index.html?window=main")],
                      [main, SimpleNamespace(url=module.RECORDER_URL + "&extra=1")],
                      [main, SimpleNamespace(url="http://private@tauri.localhost/")]):
            with self.subTest(pages=pages), self.assertRaises(module.ExportError):
                module.select_main(pages)

    def test_actual_tauri_recorder_config_url_is_allowed_only_as_recorder(self):
        config = json.loads((module.ROOT / "app/src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
        configured = next(window for window in config["app"]["windows"] if window["label"] == "recording")
        self.assertEqual(configured["url"], "index.html?window=recorder")
        main = SimpleNamespace(url=module.MAIN_URL)
        recorder = SimpleNamespace(url=module.MAIN_URL + configured["url"])
        self.assertEqual(recorder.url, module.RECORDER_URL)
        self.assertIs(module.select_main([main, recorder]), main)
        with self.assertRaises(module.ExportError):
            module.select_main([recorder])

    def test_listener_requires_exact_pid_executable_ancestry_and_creation_identity(self):
        owner = {"matched": True, "chain": [
            {"pid": 789, "parent": 123, "path": "D:/edge/msedgewebview2.exe", "created": "2026-10-07T02:00:01+00:00"},
            {"pid": 123, "parent": 100, "path": str(self.exe), "created": "2026-10-07T02:00:00+00:00"}]}
        def runner(*args, **kwargs):
            return SimpleNamespace(stdout=json.dumps(owner))
        clean = module.ownership(123, 19224, self.exe, runner)
        self.assertEqual(clean["listenerAncestry"][0]["pid"], 789)
        self.assertNotIn("path", clean["listenerAncestry"][0])
        for mutate in (lambda o: o.update(matched=False),
                       lambda o: o["chain"][-1].update(path=str(module.ROOT / "wrong/SpeakMode.exe")),
                       lambda o: o["chain"][-1].update(pid=124),
                       lambda o: o["chain"][0].update(parent=999),
                       lambda o: o["chain"][-1].update(created="private-text")):
            bad = copy.deepcopy(owner); mutate(bad)
            with self.assertRaises(module.ExportError):
                module.ownership(123, 19224, self.exe, lambda *a, **k: SimpleNamespace(stdout=json.dumps(bad)))


if __name__ == "__main__":
    unittest.main()
