"""Manually export metadata from an already running, explicitly identified SpeakMode.

Does not launch or show the app, capture audio, call a provider, inspect a profile,
or change any app state. The only app command is main-only diagnostics.get.
Receipts are exclusively created under evidence/voice-diagnostics and never replaced.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]
WORKSPACE = ROOT
MAIN_URL = "http://tauri.localhost/"
RECORDER_URL = "http://tauri.localhost/index.html?window=recorder"
# The root-relative recorder was used by earlier native QA fixtures.
LEGACY_RECORDER_URL = "http://tauri.localhost/?window=recorder"
OWN_PAGE_URLS = frozenset((MAIN_URL, RECORDER_URL, LEGACY_RECORDER_URL))
TIMING_SCOPE = "Rust monotonic offsets; UI render receipt is a proxy, not optical visibility or host confirmation"
TIMING_SCOPE_V2 = "Rust monotonic offsets; socketWritten is a client write, not provider ACK; capture input samples use the native sample rate; bridge/asr samples use 16 kHz; UI render receipt is a proxy, not optical visibility or host confirmation"
MAX_SAFE_INT = (1 << 53) - 1
STAGES = frozenset("""
intentReceived captureFirstAccepted providerReadyReceived providerSpeechStartedReceived
firstCaptionReceived captionRenderReceiptProxy stopIntentReceived captureCutoff
captureDrained finalAsrReceived sourceCheckpointCommitted rewriteStarted
rewriteValidated rewriteFallbackSelected readyCommitted deliveryClaim nativeDeliveryReceipt cancelIntent
processingWorkerReleased asrFinishWritten asrSessionFinishedReceived
""".split())
DURATION_FIELDS = (
    "intentToCaptureMs", "intentToFirstCaptionMs", "captionToRenderReceiptProxyMs",
    "stopToFinalAsrMs", "rewriteRoundTripMs", "stopToReadyMs",
    "stopToFinishWrittenMs", "finishWrittenToSessionFinishedMs",
    # A native receipt includes skipped/failed/unconfirmed dispatch outcomes.
    # These are elapsed times, never evidence of successful host insertion.
    "readyToNativeDeliveryReceiptMs", "stopToNativeDeliveryReceiptMs",
)
ORIGINS = frozenset(("uiCommand", "hotkeyHook", "asrRetry", "rewriteRetry", "saveRetry"))
MODE_IDS = frozenset(("auto", "light", "structure"))
REGIONS = frozenset(("beijing", "singapore"))
# The current MVP models. Other values are redacted, never copied as arbitrary strings.
ASR_MODELS = frozenset(("qwen3-asr-flash-realtime", "qwen3-asr-flash-realtime-2025-10-27", "qwen3-asr-flash-realtime-2026-02-10"))
REWRITE_MODELS = frozenset(("qwen-flash", "qwen-plus", "qwen3.8-flash", "qwen3.7-plus", "qwen3.7-plus-2026-05-26"))
REWRITE_PROTOCOLS = frozenset(("mode-contract-v1", "mode-contract-v3", "cleanup-contract-v1", "automatic-dictation-v3", "natural-cleanup-v1", "natural-cleanup-v2", "fluent-dictation-v1+semantic-review-compact-v2", "fluent-dictation-v2+protected-long-v3+semantic-review-compact-v3", "minimum-edit-v3+minimum-review-v3+residue-hint-v2+local-repair-v2"))
REWRITE_OUTCOMES = frozenset(("validated", "fallback", "failed", "cancelled"))
REWRITE_FALLBACKS = frozenset(("timeout", "serviceUnavailable", "invalidOutput", "meaningRisk"))
REWRITE_ERROR_CODES = frozenset("""
rewrite_timeout rewrite_network network_setup rewrite_service_error authentication_failed rate_limited
invalid_response invalid_output invalid_local_repair stale_rewrite empty_output response_too_large truncated_output invalid_source_mapping
incomplete_source_coverage rewrite_meaning_risk rewrite_admission_rejected rewrite_semantic_rejected rewrite_literal_changed protected_terms_missing protected_terms_new_latin protected_terms_marker_unknown protected_terms_marker_invalid protected_terms_limit protected_terms_empty protected_terms_nonce_invalid protected_terms_nonce_collision stale_protected_draft
cancelled source_incomplete invalid_source invalid_mode
unsupported_model unsupported_rewrite_model unsupported_provider unsupported_region invalid_config invalid_workspace
""".split())
ERROR_CODES = frozenset("""
ack_timeout asr_closed asr_connect_failed asr_connect_timeout asr_disconnected
asr_finish_timeout asr_limit asr_receive_failed asr_retry_unavailable asr_send_failed
asr_send_timeout audio_frame_limit audio_incomplete audio_unavailable cache_overflow
cancel_cleanup_failed cancelled capture_busy capture_cleanup_failed capture_overflow
capture_unavailable conflicting_final conflicting_order credential_unavailable
delivery_receipt_save_failed empty_output empty_recording empty_recovery forbidden
incomplete_source incomplete_source_coverage invalid_asr_event invalid_clock
invalid_endpoint invalid_final invalid_item_id invalid_key invalid_mode invalid_order
invalid_output invalid_local_repair stale_rewrite invalid_payload invalid_response invalid_source invalid_source_mapping
invalid_workspace key_required missing_item_id missing_order mode_frozen native_busy
network_setup no_output no_session notFound processing_busy recording_limit
response_too_large rewrite_network rewrite_timeout rewrite_unavailable revisionConflict
save_expired save_interrupted serialization_failed session_busy settings_busy
settings_rollback_failed shutdown_busy shutdown_failed source_incomplete stale_attempt
stale_session storage_busy storage_protocol storage_unavailable system_cleanup_busy
system_cleanup_failed system_resume_failed system_unavailable unsupported_model
unsupported_provider unsupported_region invalid_config unauthorized rate_limited
provider_error rewrite_http rewrite_rejected rewrite_incomplete asr_incomplete
rewrite_meaning_risk rewrite_admission_rejected rewrite_semantic_rejected rewrite_literal_changed protected_terms_missing protected_terms_new_latin protected_terms_marker_unknown protected_terms_marker_invalid protected_terms_limit protected_terms_empty protected_terms_nonce_invalid protected_terms_nonce_collision stale_protected_draft target_higher_integrity
stop_anchor_unavailable stop_focus_changed target_not_editable
target_capability_unsupported target_inspection_timeout
asr_config_timeout audio_disconnected late_asr_event asr_service_error dictionary_invalid
missing_predecessor unknown_order ambiguous_order unexpected_finish
unsupported_asr_model unsupported_rewrite_model unsupported_asr_language
asr_adapter_not_implemented asr_adapter_mismatch audio_capture_stalled
audio_first_sample_timeout audio_capture_device_error audio_capture_queue_overflow audio_capture_malformed
""".split())


class ExportError(ValueError):
    """A fixed, non-payload error safe to display on the console."""


def unsigned(value, field):
    if type(value) is not int or not 0 <= value <= MAX_SAFE_INT:
        raise ExportError(f"Invalid numeric metadata field: {field}")
    return value


def elapsed(value, field, nullable=True):
    if value is None and nullable:
        return None
    if type(value) not in (int, float) or not math.isfinite(value) or not 0 <= value <= MAX_SAFE_INT:
        raise ExportError(f"Invalid timing metadata field: {field}")
    return value


def identity(value, field, nullable=False):
    if nullable and value is None:
        return None
    if type(value) is not str or len(value) != 36:
        raise ExportError(f"Invalid UUID metadata field: {field}")
    try:
        parsed = uuid.UUID(value)
    except ValueError:
        raise ExportError(f"Invalid UUID metadata field: {field}") from None
    if str(parsed) != value.lower():
        raise ExportError(f"Invalid UUID metadata field: {field}")
    return str(parsed)


def choice(value, allowed):
    return value if type(value) is str and value in allowed else None


def timestamp(value):
    if type(value) is not str or not 20 <= len(value) <= 40 or not re.fullmatch(r"[0-9TZ+:.\-]+", value):
        raise ExportError("Invalid startedAt metadata")
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        raise ExportError("Invalid startedAt metadata") from None
    if parsed.tzinfo is None:
        raise ExportError("Expected timezone-aware startedAt metadata")
    return parsed.isoformat()


def filter_snapshot(raw):
    """Construct a new typed whitelist; never serialize or log raw RPC data."""
    if type(raw) is not dict or type(raw.get("schemaVersion")) is not int or raw.get("schemaVersion") not in (1, 2):
        raise ExportError("Unsupported diagnostics schema")
    schema = raw["schemaVersion"]
    scope = TIMING_SCOPE if schema == 1 else TIMING_SCOPE_V2
    if raw.get("inMemoryOnly") is not True or raw.get("timingScope") != scope:
        raise ExportError("Unexpected diagnostics storage or timing scope")
    version = raw.get("appVersion")
    if type(version) is not str or not re.fullmatch(r"[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}(?:-[a-zA-Z0-9.-]{1,20})?", version):
        raise ExportError("Invalid appVersion metadata")
    rows = raw.get("traces")
    if type(rows) is not list or len(rows) > 64:
        raise ExportError("Diagnostics traces exceed schema bounds")
    traces = []
    for row in rows:
        if type(row) is not dict:
            raise ExportError("Invalid diagnostic trace")
        origin = choice(row.get("origin"), ORIGINS)
        if origin is None:
            raise ExportError("Unknown diagnostic origin")
        mode = row.get("modeId")
        if type(mode) is not str or mode not in MODE_IDS:
            mode = identity(mode, "modeId")
        marks = row.get("marks")
        durations = row.get("durations")
        if type(marks) is not list or len(marks) > len(STAGES) or type(durations) is not dict:
            raise ExportError("Invalid diagnostic marks or durations")
        clean_marks, seen = [], set()
        for mark in marks:
            if type(mark) is not dict:
                raise ExportError("Invalid diagnostic mark")
            stage = choice(mark.get("stage"), STAGES)
            if stage is None or stage in seen:
                raise ExportError("Unknown or duplicate diagnostic stage")
            seen.add(stage)
            clean_marks.append({"stage": stage, "offsetMs": elapsed(mark.get("offsetMs"), "offsetMs", False)})
        traces.append({
            "runId": identity(row.get("runId"), "runId"),
            "sessionId": identity(row.get("sessionId"), "sessionId"),
            "generation": unsigned(row.get("generation"), "generation"),
            "origin": origin, "startedAt": timestamp(row.get("startedAt")),
            "provider": choice(row.get("provider"), frozenset(("aliyun",))),
            "region": choice(row.get("region"), REGIONS),
            "asrModel": choice(row.get("asrModel"), ASR_MODELS),
            "rewriteModel": choice(row.get("rewriteModel"), REWRITE_MODELS),
            "modeId": mode, "modeRevision": unsigned(row.get("modeRevision"), "modeRevision"),
            "asrAttemptId": identity(row.get("asrAttemptId"), "asrAttemptId", True),
            "captureDurationMs": None if row.get("captureDurationMs") is None else unsigned(row["captureDurationMs"], "captureDurationMs"),
            "sourceCharCount": None if row.get("sourceCharCount") is None else unsigned(row["sourceCharCount"], "sourceCharCount"),
            "bridgeQueuePeakFrames": unsigned(row.get("bridgeQueuePeakFrames"), "bridgeQueuePeakFrames"),
            "outcome": choice(row.get("outcome"), frozenset(("ready", "failed", "cancelled"))),
            "errorCode": choice(row.get("errorCode"), ERROR_CODES),
            "marks": clean_marks,
            "durations": {field: elapsed(durations.get(field), field) for field in DURATION_FIELDS},
        })
        if schema == 2:
            traces[-1].update(filter_transport(row))
        # Additive optional metadata: preserve old schema 1/2 receipts without
        # inventing an outcome, budget or cause when the producer did not report it.
        if "rewrite" in row:
            traces[-1]["rewrite"] = filter_rewrite(row["rewrite"])
    return {"schemaVersion": schema, "appVersion": version, "inMemoryOnly": True,
            "timingScope": scope, "droppedEvents": unsigned(raw.get("droppedEvents"), "droppedEvents"),
            "evictedTraces": unsigned(raw.get("evictedTraces"), "evictedTraces"), "traces": traces}


ASR_EVENT_KINDS = frozenset(("sessionCreated", "sessionUpdated", "speechStarted", "speechStopped",
    "committed", "itemCreated", "partial", "final", "sessionFinished", "serviceError",
    "transcriptionFailed", "unknown", "missingType", "invalidJson"))


def metadata_object(value):
    if type(value) is not dict:
        raise ExportError("Invalid transport diagnostic object")
    return value


def filter_rewrite(value):
    """Whitelist rewrite decisions independently of the eventual ready/delivery result."""
    if value is None:
        return None
    value = metadata_object(value)
    budget = unsigned(value.get("configuredBudgetMs"), "configuredBudgetMs")
    if not 0 < budget <= 120000:
        raise ExportError("Invalid rewrite budget metadata")
    result = {"protocolVersion": choice(value.get("protocolVersion"), REWRITE_PROTOCOLS),
            "configuredBudgetMs": budget,
            "outcome": choice(value.get("outcome"), REWRITE_OUTCOMES),
            "fallbackReason": choice(value.get("fallbackReason"), REWRITE_FALLBACKS),
            "errorCode": choice(value.get("errorCode"), REWRITE_ERROR_CODES),
            "durationMs": elapsed(value.get("durationMs"), "rewrite.durationMs")}
    if "requests" in value:
        result["requests"] = filter_rewrite_requests(value["requests"])
    return result


def filter_rewrite_requests(value):
    if type(value) is not list or len(value) > 4:
        raise ExportError("Invalid rewrite request list")
    stages = frozenset(("generation", "review", "repair", "finalReview"))
    outcomes = frozenset(("responseReceived", "failed", "cancelled", "timedOut", "interrupted"))
    result, seen = [], set()
    for raw in value:
        raw = metadata_object(raw)
        stage = choice(raw.get("stage"), stages)
        if stage in seen:
            raise ExportError("Duplicate rewrite request stage")
        seen.add(stage)
        start = elapsed(raw.get("startMs"), "request.startMs")
        end = elapsed(raw.get("endMs"), "request.endMs")
        duration = elapsed(raw.get("durationMs"), "request.durationMs")
        remaining = unsigned(raw.get("remainingBudgetMs"), "remainingBudgetMs")
        if start is None or remaining > 120000 or ((end is None) != (duration is None)):
            raise ExportError("Invalid rewrite request timing")
        if end is not None and (end < start or abs((end - start) - duration) > 0.01):
            raise ExportError("Inconsistent rewrite request timing")
        result.append({"stage": stage, "startMs": start, "endMs": end, "durationMs": duration,
                       "remainingBudgetMs": remaining, "outcome": choice(raw.get("outcome"), outcomes)})
    return result


def flag(value):
    if type(value) is not bool:
        raise ExportError("Invalid diagnostic boolean")
    return value


def counts(value):
    value = metadata_object(value)
    return {field: unsigned(value.get(field), field) for field in ("frames", "realSamples", "paddingSamples")}


def filter_conditioning(value):
    """Optional audio-quality scalars only; never export sample arrays or free text."""
    if value is None:
        return None
    value = metadata_object(value)
    profile = choice(value.get("profile"), frozenset(("bypass", "softVoice", "fixedGain6Db")))
    version = value.get("version")
    if profile is None or version != "soft-voice-v1":
        raise ExportError("Invalid conditioning profile metadata")
    # The algorithm version is a fixed implementation label, not user content.
    clean = {"profile": profile, "version": version}
    for field in ("sampleRateHz", "realSamples", "inputClippedSamples", "outputClippedSamples",
                  "peakLimitedSamples", "nonfiniteSamples", "processingUs"):
        clean[field] = unsigned(value.get(field), field)
    if clean["sampleRateHz"] != 16000:
        raise ExportError("Invalid conditioning sample rate")
    for field in ("inputRmsDbfs", "outputRmsDbfs", "inputPeakDbfs", "outputPeakDbfs", "meanGainDb", "maxGainDb"):
        number = value.get(field)
        if type(number) not in (int, float) or not math.isfinite(number) or not -120 <= number <= 120:
            raise ExportError("Invalid conditioning level metadata")
        clean[field] = number
    return clean


def filter_transport(row):
    capture = row.get("capture")
    if capture is not None:
        capture = metadata_object(capture)
        capture = {**{field: unsigned(capture.get(field), field) for field in ("inputSamples", "inputSampleRate", "emittedFrames")},
                   "complete": flag(capture.get("complete")), "cancelled": flag(capture.get("cancelled")),
                   "conditioning": filter_conditioning(capture.get("conditioning"))}
    bridge = metadata_object(row.get("bridge"))
    clean_bridge = {"observed": counts(bridge.get("observed")), "enqueued": counts(bridge.get("enqueued")),
                   **{field: elapsed(bridge.get(field), field) for field in ("lastFrameEnqueuedMs", "finishEnqueuedMs")}}
    asr = metadata_object(row.get("asr"))
    event_counts, tail = asr.get("eventCounts"), asr.get("eventTail")
    if type(event_counts) is not list or len(event_counts) > 14 or type(tail) is not list or len(tail) > 32:
        raise ExportError("Transport event metadata exceeds bounds")
    clean_counts, seen = [], set()
    for event in event_counts:
        event = metadata_object(event)
        kind = choice(event.get("kind"), ASR_EVENT_KINDS)
        if kind is None or kind in seen:
            raise ExportError("Invalid or duplicate ASR event kind")
        seen.add(kind)
        clean_counts.append({"kind": kind, "count": unsigned(event.get("count"), "count")})
    clean_tail, previous_sequence = [], 0
    for event in tail:
        event = metadata_object(event)
        kind = choice(event.get("kind"), ASR_EVENT_KINDS)
        sequence = unsigned(event.get("sequence"), "sequence")
        if kind is None or sequence <= previous_sequence:
            raise ExportError("Invalid ASR event order or kind")
        previous_sequence = sequence
        clean_tail.append({"kind": kind, "sequence": sequence,
            "offsetMs": elapsed(event.get("offsetMs"), "offsetMs"),
            **{field: None if event.get(field) is None else unsigned(event[field], field)
               for field in ("itemOrdinal", "previousItemOrdinal")},
            **{field: unsigned(event.get(field), field) for field in ("pendingItems", "finalItems")}})
    clean_asr = {"dequeued": counts(asr.get("dequeued")), "socketWritten": counts(asr.get("socketWritten")),
        **{field: elapsed(asr.get(field), field) for field in ("lastFrameWrittenMs", "finishDequeuedMs", "finishWrittenMs", "sessionFinishedReceivedMs")},
        "finishExpectedFrames": None if asr.get("finishExpectedFrames") is None else unsigned(asr["finishExpectedFrames"], "finishExpectedFrames"),
        **{field: unsigned(asr.get(field), field) for field in ("pendingItems", "finalItems", "observedEvents")},
        "terminated": flag(asr.get("terminated")), "eventCounts": clean_counts, "eventTail": clean_tail}
    dictionary = asr.get("dictionary")
    if dictionary is not None:
        dictionary = metadata_object(dictionary)
        clean_asr["dictionary"] = {
            **{field: unsigned(dictionary.get(field), field) for field in ("version", "entryCount", "vocabularyCount", "contextChunks")},
            "support": choice(dictionary.get("support"), frozenset(("legacyCorpusUnverified", "vocabularyAndContext", "vocabulary"))),
            "socketWritten": flag(dictionary.get("socketWritten")),
            "effectivenessVerified": False}
    return {"capture": capture, "bridge": clean_bridge, "asr": clean_asr}


def validate_args(pid, port, executable, receipt_name, root=ROOT):
    if type(pid) is not int or not 0 < pid <= 0xFFFFFFFF or type(port) is not int or not 1024 <= port <= 65535:
        raise ExportError("Expected a positive own-app PID and a valid local debug port")
    root = root.resolve()
    if root != WORKSPACE:
        raise ExportError("Exporter must remain in the explicitly selected SpeakMode project")
    executable = Path(executable).resolve()
    if not executable.is_relative_to(root) or executable.name.lower() != "speakmode.exe" or not executable.is_file():
        raise ExportError("Expected an existing SpeakMode.exe inside this project")
    if type(receipt_name) is not str or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", receipt_name):
        raise ExportError("Receipt name must be 1-64 lowercase ASCII letters, numbers, underscore or hyphen")
    if re.fullmatch(r"(?:con|prn|aux|nul|com[1-9]|lpt[1-9])", receipt_name, re.IGNORECASE):
        raise ExportError("Windows reserved receipt name")
    output_dir = root / "evidence" / "voice-diagnostics"
    target = output_dir / (receipt_name + ".json")
    if output_dir.resolve() != output_dir or target.resolve() != target or not target.resolve().is_relative_to(output_dir):
        raise ExportError("Diagnostic output escaped its intended evidence directory")
    if target.exists():
        raise ExportError("Preserve earlier receipt; supply a fresh receipt name")
    return executable, target


def ownership(pid, port, expected_exe, runner=subprocess.run):
    # Only checked integers are interpolated; neither a path nor renderer choice enters PowerShell.
    unsigned(pid, "pid"); unsigned(port, "port")
    script = f"""
$ErrorActionPreference='Stop'
$taskListener=@(Get-NetTCPConnection -State Listen -LocalPort {port})
if($taskListener.Count -ne 1){{throw 'Expected one own-app debug listener'}}
$taskAncestor=[int]$taskListener[0].OwningProcess
$taskChain=@()
for($taskDepth=0;$taskDepth -lt 20 -and $taskAncestor -gt 0;$taskDepth++){{
  $taskProcess=Get-CimInstance Win32_Process -Filter "ProcessId=$taskAncestor"
  if(-not $taskProcess){{throw 'Debug listener ancestry disappeared'}}
  $taskChain+=@{{pid=[int]$taskProcess.ProcessId;parent=[int]$taskProcess.ParentProcessId;path=$taskProcess.ExecutablePath;created=$taskProcess.CreationDate.ToUniversalTime().ToString('o')}}
  if($taskAncestor -eq {pid}){{break}}
  $taskAncestor=[int]$taskProcess.ParentProcessId
}}
@{{matched=($taskAncestor -eq {pid});chain=$taskChain}}|ConvertTo-Json -Depth 5 -Compress
"""
    try:
        result = runner(["powershell.exe", "-NoProfile", "-Command", script], capture_output=True,
                        text=True, encoding="utf-8", check=True, timeout=30)
        owner = json.loads(result.stdout.lstrip("\ufeff"))
    except Exception:
        raise ExportError("Could not verify own-app debug listener ancestry") from None
    if type(owner) is not dict or owner.get("matched") is not True:
        raise ExportError("Debug listener is not owned by the supplied SpeakMode PID")
    chain = owner.get("chain")
    if type(chain) is not list or not 1 <= len(chain) <= 20:
        raise ExportError("Invalid own-app listener ancestry")
    clean, seen = [], set()
    for item in chain:
        if type(item) is not dict:
            raise ExportError("Invalid own-app process identity")
        process = unsigned(item.get("pid"), "pid")
        parent = unsigned(item.get("parent"), "parent")
        if process == 0 or process in seen:
            raise ExportError("Cyclic or invalid own-app listener ancestry")
        seen.add(process)
        clean.append({"pid": process, "parent": parent, "created": timestamp(item.get("created"))})
    if any(chain[index]["parent"] != chain[index + 1]["pid"] for index in range(len(chain) - 1)):
        raise ExportError("Broken own-app listener ancestry")
    if clean[-1]["pid"] != pid or type(chain[-1].get("path")) is not str or Path(chain[-1]["path"]).resolve() != expected_exe:
        raise ExportError("Debug listener does not descend from the supplied executable and PID")
    return {"pid": pid, "executable": str(expected_exe), "port": port, "listenerAncestry": clean}


def select_main(pages):
    if not pages:
        raise ExportError("No own-app WebView pages were found")
    for page in pages:
        if page.url not in OWN_PAGE_URLS:
            raise ExportError("Debug root contains a page outside the fixed own-app URL allowlist")
    main = [page for page in pages if page.url == MAIN_URL]
    if len(main) != 1:
        raise ExportError("Expected exactly one fixed main WebView root")
    return main[0]


def read_diagnostics(port):
    # Lazy import leaves pure boundary tests independent of a browser/runtime.
    from playwright.sync_api import sync_playwright
    with sync_playwright() as playwright:
        browser = playwright.chromium.connect_over_cdp(f"http://127.0.0.1:{port}", timeout=10000)
        main = select_main([page for context in browser.contexts for page in context.pages])
        main.wait_for_function("location.href==='http://tauri.localhost/' && typeof window.__TAURI_INTERNALS__?.invoke==='function'", timeout=5000)
        raw = main.evaluate("""async () => {
          if(location.href!=='http://tauri.localhost/') throw new Error('Own main root changed');
          return window.__TAURI_INTERNALS__.invoke('app_command',
            {request:{name:'diagnostics.get',payload:{requestId:crypto.randomUUID()}}});
        }""")
        if main.url != MAIN_URL:
            raise ExportError("Own main WebView root changed during export")
        return filter_snapshot(raw)


def write_receipt(target, value):
    intended = ROOT / "evidence" / "voice-diagnostics"
    if target.parent != intended or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}\.json", target.name):
        raise ExportError("Diagnostic output escaped its intended evidence directory")
    encoded = (json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n").encode("utf-8")
    target.parent.mkdir(parents=True, exist_ok=True)
    # Revalidate after directory creation; never overwrite an earlier receipt.
    if target.parent.resolve() != target.parent or target.resolve() != target:
        raise ExportError("Diagnostic output changed during export")
    descriptor = os.open(target, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "wb") as handle:
        handle.write(encoded)


def main():
    global ROOT, WORKSPACE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, required=True)
    parser.add_argument("--executable", type=Path, required=True)
    parser.add_argument("--port", type=int, default=19224)
    parser.add_argument("--receipt-name", required=True, help="Fresh stem without .json; previous receipts are never overwritten")
    parser.add_argument("--project-root", type=Path, default=ROOT, help="Explicit project containing the executable and evidence directory")
    args = parser.parse_args()
    try:
        ROOT = WORKSPACE = args.project_root.resolve()
        if not (ROOT / "app/src-tauri/tauri.conf.json").is_file():
            raise ExportError("Selected project is missing its SpeakMode application config")
        exe, target = validate_args(args.pid, args.port, args.executable, args.receipt_name, root=ROOT)
        before = ownership(args.pid, args.port, exe)
        snapshot = read_diagnostics(args.port)
        after = ownership(args.pid, args.port, exe)
        if before != after:
            raise ExportError("Own-app process identity changed during export")
        with exe.open("rb") as handle:
            digest = hashlib.file_digest(handle, "sha256").hexdigest()
        receipt = {"recordedAtUtc": datetime.now(timezone.utc).isoformat(),
                   "scope": "Manual own-app main diagnostics.get; metadata only. No audio, provider, profile, window or app-state operations.",
                   "ownership": {**after, "executableSha256": digest}, "snapshot": snapshot,
                   "filtering": "Unknown fields discarded; only typed schema-v1/v2 metadata, bounded transport counts and event ordinals, known model/region/origin/stage/outcome/error-code values exported."}
        write_receipt(target, receipt)
    except ExportError as error:
        print(str(error), file=sys.stderr)
        return 1
    except Exception:
        # Browser/RPC exception text can contain arbitrary payload; never print it.
        print("Export failed; no diagnostic payload was printed. Use a fresh receipt name if retrying.", file=sys.stderr)
        return 1
    print(json.dumps({"receipt": str(target), "traces": len(snapshot["traces"]),
                      "droppedEvents": snapshot["droppedEvents"], "evictedTraces": snapshot["evictedTraces"]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
