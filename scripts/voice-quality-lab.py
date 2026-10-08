"""Offline preregistration and scoring. Never calls a provider or reads app state.

Python standard library only. Local references and result text are necessary
inputs; aggregate reports deliberately contain neither text nor local paths.
"""
import argparse
from collections import Counter, defaultdict
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import re
import unicodedata
import wave


VERSION = 1
SCORING = "speakmode-quality-v1"
LATENCIES = ("speechToCaption", "stopToAsr", "stopToReady", "stopToDispatch")
LABEL = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,95}\Z")
HASH = re.compile(r"[0-9a-f]{64}\Z")


class ValidationError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise ValidationError(message)


def shape(value, keys, name):
    require(isinstance(value, dict) and set(value) == set(keys),
            f"{name}: missing or unknown fields")
    return value


def label(value, name):
    require(isinstance(value, str) and LABEL.fullmatch(value) is not None,
            f"{name}: expected an anonymous ASCII label")
    return value


def string(value, name, allow_empty=False):
    require(isinstance(value, str) and len(value) <= 100_000 and
            (allow_empty or value.strip()), f"{name}: invalid text")
    return value


def number(value, name, nullable=True):
    require((nullable and value is None) or
            (type(value) in (int, float) and math.isfinite(value) and value >= 0),
            f"{name}: expected a finite nonnegative number or null")


def boolean(value, name, nullable=False):
    require(type(value) is bool or (nullable and value is None), f"{name}: invalid boolean")


def reject_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, "JSON has duplicate object keys")
        result[key] = value
    return result


def read_json(path):
    raw = Path(path).read_bytes()
    require(len(raw) <= 8 * 1024 * 1024, "JSON file exceeds 8 MiB")
    try:
        value = json.loads(raw.decode("utf-8-sig"), object_pairs_hook=reject_duplicate_keys,
                           parse_constant=lambda _: (_ for _ in ()).throw(ValidationError("nonfinite JSON")))
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ValidationError("invalid UTF-8 JSON") from error
    return value, sha(raw)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def digest(value):
    return sha(json.dumps(value, ensure_ascii=False, sort_keys=True,
                          separators=(",", ":"), allow_nan=False).encode("utf-8"))


def write_new(path, value):
    path = Path(path)
    # Exclusive creation: a repeated run can never replace the original receipt.
    with path.open("x", encoding="utf-8", newline="\n") as output:
        json.dump(value, output, ensure_ascii=False, indent=2, allow_nan=False)
        output.write("\n")


def norm(text):
    return unicodedata.normalize("NFKC", text).casefold()


def cer_tokens(text):
    # All characters, including Latin/digits, for zh/mixed. No number conversion.
    return [c for c in norm(text) if not c.isspace() and not unicodedata.category(c).startswith("P")]


def wer_tokens(text, mixed=False):
    # English words/digits; punctuation splits tokens. Mixed English WER is an
    # explicitly limited ASCII projection, not a Chinese word-segmentation score.
    return re.findall(r"[a-z0-9_]+" if mixed else r"\w+", norm(text), re.UNICODE)


def distance(reference, hypothesis):
    previous = list(range(len(hypothesis) + 1))
    for i, expected in enumerate(reference, 1):
        current = [i]
        for j, actual in enumerate(hypothesis, 1):
            current.append(min(current[-1] + 1, previous[j] + 1,
                               previous[j - 1] + (expected != actual)))
        previous = current
    return previous[-1]


def term_present(term, text):
    term, text = norm(term), norm(text)
    left = r"(?<![a-z0-9_])" if term[0].isascii() and term[0].isalnum() else ""
    right = r"(?![a-z0-9_])" if term[-1].isascii() and term[-1].isalnum() else ""
    return re.search(left + re.escape(term) + right, text) is not None


def validate_reference(ref, fid):
    shape(ref, ("schemaVersion", "fixtureId", "language", "transcript", "deletableFillers",
                "facts", "positiveTerms", "negativeTerms", "modeCriteria"), "reference")
    require(type(ref["schemaVersion"]) is int and ref["schemaVersion"] == VERSION,
            "reference version unsupported")
    require(ref["fixtureId"] == fid, "reference fixtureId mismatch")
    require(ref["language"] in ("zh", "en", "mixed"), "reference language unsupported")
    transcript = string(ref["transcript"], "transcript")
    require(isinstance(ref["deletableFillers"], list), "deletableFillers must be a list")
    for filler in ref["deletableFillers"]:
        string(filler, "filler")
        require(filler in transcript, "deletable filler absent from transcript")
    require(isinstance(ref["facts"], list) and ref["facts"], "reference needs factual rubric")
    fact_ids = set()
    for fact in ref["facts"]:
        shape(fact, ("id", "description", "sourceStart", "sourceEnd", "disposition", "critical", "kind"), "fact")
        label(fact["id"], "fact.id")
        require(fact["id"] not in fact_ids, "duplicate fact.id")
        fact_ids.add(fact["id"])
        string(fact["description"], "fact.description")
        require(type(fact["sourceStart"]) is int and type(fact["sourceEnd"]) is int and
                0 <= fact["sourceStart"] < fact["sourceEnd"] <= len(transcript), "fact source offsets invalid")
        require(fact["disposition"] in ("retained", "withdrawn"), "fact disposition invalid")
        boolean(fact["critical"], "fact.critical")
        require(fact["kind"] in ("number", "negation", "condition", "entity", "tail", "other"), "fact kind invalid")
    require(any(f["critical"] and f["disposition"] == "retained" for f in ref["facts"]),
            "reference needs a critical retained fact")
    all_terms = set()
    for key in ("positiveTerms", "negativeTerms"):
        require(isinstance(ref[key], list), f"{key} must be a list")
        for term in ref[key]:
            string(term, key)
            require(norm(term) not in all_terms, "duplicate or overlapping terms")
            all_terms.add(norm(term))
            require(term_present(term, transcript) == (key == "positiveTerms"),
                    "term expectation disagrees with transcript")
    shape(ref["modeCriteria"], ("light", "structure"), "modeCriteria")
    for mode, criteria in ref["modeCriteria"].items():
        require(isinstance(criteria, list) and criteria, f"{mode}: criteria required")
        ids = set()
        for criterion in criteria:
            shape(criterion, ("id", "description"), "criterion")
            label(criterion["id"], "criterion.id")
            require(criterion["id"] not in ids, "duplicate criterion.id")
            ids.add(criterion["id"])
            string(criterion["description"], "criterion.description")
    return ref


def validate_manifest(manifest):
    shape(manifest, ("schemaVersion", "experimentId", "sourceCommit", "arms", "fixtures"), "manifest")
    require(type(manifest["schemaVersion"]) is int and manifest["schemaVersion"] == VERSION,
            "manifest version unsupported")
    label(manifest["experimentId"], "experimentId")
    require(isinstance(manifest["sourceCommit"], str) and
            re.fullmatch(r"[0-9a-f]{40}", manifest["sourceCommit"]), "sourceCommit needs full lowercase SHA")
    require(isinstance(manifest["arms"], list) and 1 <= len(manifest["arms"]) <= 12, "arms must contain 1..12 entries")
    arm_ids = set()
    for arm in manifest["arms"]:
        shape(arm, ("id", "task", "mode", "config"), "arm")
        label(arm["id"], "arm.id")
        require(arm["id"] not in arm_ids, "duplicate arm.id")
        arm_ids.add(arm["id"])
        require(arm["task"] in ("asr", "rewrite", "pipeline"), "arm task invalid")
        require(arm["mode"] in (("none",) if arm["task"] == "asr" else ("light", "structure")), "arm mode invalid")
        config = shape(arm["config"], ("asrModel", "rewriteModel", "adapter", "region", "language",
                                     "dictionarySha256", "promptSha256", "parameters"), "arm.config")
        for key in ("asrModel", "rewriteModel"):
            if config[key] is not None:
                label(config[key], key)
        require((arm["task"] == "rewrite" or config["asrModel"] is not None) and
                (arm["task"] == "asr" or config["rewriteModel"] is not None), "task model missing")
        label(config["adapter"], "adapter")
        label(config["region"], "region")
        require(config["language"] in ("auto", "zh", "en"), "language invalid")
        for key in ("dictionarySha256", "promptSha256"):
            require(isinstance(config[key], str) and HASH.fullmatch(config[key]), f"{key} required")
        require(isinstance(config["parameters"], dict), "parameters must be an explicit object")
    require(isinstance(manifest["fixtures"], list) and 1 <= len(manifest["fixtures"]) <= 1000,
            "fixtures must contain 1..1000 entries")
    fixture_ids, speakers = set(), {}
    for fixture in manifest["fixtures"]:
        shape(fixture, ("id", "speakerId", "split", "sourceKind", "category", "tags",
                        "collectionStatus", "audioPath", "referencePath"), "fixture")
        for key in ("id", "speakerId", "category"):
            label(fixture[key], key)
        require(fixture["id"] not in fixture_ids, "duplicate fixture.id")
        fixture_ids.add(fixture["id"])
        require(fixture["split"] in ("dev", "test"), "split must be dev or test")
        require(fixture["sourceKind"] in ("human", "synthetic"), "sourceKind invalid")
        speaker_key = (fixture["sourceKind"], fixture["speakerId"])
        require(speakers.get(speaker_key, fixture["split"]) == fixture["split"],
                "one speaker cannot appear in both development and blind test")
        speakers[speaker_key] = fixture["split"]
        require(isinstance(fixture["tags"], list) and len(set(fixture["tags"])) == len(fixture["tags"]), "tags invalid")
        for tag in fixture["tags"]:
            label(tag, "tag")
        require(fixture["collectionStatus"] in ("missing", "ready"), "collectionStatus invalid")
        if fixture["collectionStatus"] == "missing":
            require(fixture["audioPath"] is None and fixture["referencePath"] is None,
                    "uncollected fixture must have null audio/reference paths")
        else:
            for key in ("audioPath", "referencePath"):
                require(isinstance(fixture[key], str) and Path(fixture[key]).is_absolute(), f"{key} must be absolute")
    return manifest


def audio_metadata(path):
    path = Path(path)
    require(path.stat().st_size <= 16000 * 2 * 600 + 1024 * 1024, "audio exceeds 10-minute bound")
    audio_hash = sha(path.read_bytes())
    try:
        with wave.open(str(path), "rb") as reader:
            require((reader.getnchannels(), reader.getframerate(), reader.getsampwidth(), reader.getcomptype()) ==
                    (1, 16000, 2, "NONE"), "audio must be uncompressed PCM16 mono 16000Hz WAV")
            frames = reader.getnframes()
            require(0 < frames <= 16_000 * 600, "audio frame count invalid")
            pcm = reader.readframes(frames)
            require(len(pcm) == frames * 2, "audio truncated PCM")
    except (wave.Error, EOFError) as error:
        raise ValidationError("invalid PCM WAV") from error
    return {"sha256": audio_hash, "pcmSha256": sha(pcm), "sampleRate": 16000,
            "channels": 1, "sampleWidthBytes": 2, "frames": frames, "durationSeconds": frames / 16000}


def freeze(manifest_path, lock_path):
    manifest_path = Path(manifest_path).resolve()
    manifest, manifest_hash = read_json(manifest_path)
    validate_manifest(manifest)
    frozen = {}
    for fixture in manifest["fixtures"]:
        if fixture["collectionStatus"] == "missing":
            frozen[fixture["id"]] = {"audio": None, "referenceSha256": None}
        else:
            ref, ref_hash = read_json(fixture["referencePath"])
            validate_reference(ref, fixture["id"])
            frozen[fixture["id"]] = {"audio": audio_metadata(fixture["audioPath"]), "referenceSha256": ref_hash}
    lock = {"schemaVersion": VERSION, "scoringVersion": SCORING, "manifestPath": str(manifest_path),
            "manifestSha256": manifest_hash, "manifest": manifest, "frozenFixtures": frozen,
            "frozenAt": datetime.now(timezone.utc).isoformat()}
    write_new(lock_path, lock)
    return lock


def verify_lock(path):
    lock, _ = read_json(path)
    shape(lock, ("schemaVersion", "scoringVersion", "manifestPath", "manifestSha256", "manifest",
                 "frozenFixtures", "frozenAt"), "lock")
    require(type(lock["schemaVersion"]) is int and lock["schemaVersion"] == VERSION and
            lock["scoringVersion"] == SCORING, "unsupported frozen scoring version")
    manifest, manifest_hash = read_json(lock["manifestPath"])
    validate_manifest(manifest)
    require(manifest_hash == lock["manifestSha256"] and manifest == lock["manifest"], "manifest changed since freeze")
    require(set(lock["frozenFixtures"]) == {f["id"] for f in manifest["fixtures"]}, "frozen fixture set differs")
    refs = {}
    for fixture in manifest["fixtures"]:
        frozen = shape(lock["frozenFixtures"][fixture["id"]], ("audio", "referenceSha256"), "frozen fixture")
        if fixture["collectionStatus"] == "missing":
            require(frozen == {"audio": None, "referenceSha256": None}, "missing fixture has frozen evidence")
            continue
        ref, ref_hash = read_json(fixture["referencePath"])
        require(ref_hash == frozen["referenceSha256"], "reference changed since freeze")
        validate_reference(ref, fixture["id"])
        require(audio_metadata(fixture["audioPath"]) == frozen["audio"], "audio changed since freeze")
        refs[fixture["id"]] = ref
    return lock, refs


def registrations(lock):
    # Hash tuple avoids ambiguous IDs when labels themselves contain separators.
    return {digest([f["id"], a["id"]])[:24]: (f, a)
            for f in lock["manifest"]["fixtures"] for a in lock["manifest"]["arms"]}


def bindings(lock, fixture):
    frozen = lock["frozenFixtures"][fixture["id"]]
    return {"lockSha256": digest(lock), "manifestSha256": lock["manifestSha256"],
            "audioSha256": frozen["audio"]["sha256"] if frozen["audio"] else None,
            "referenceSha256": frozen["referenceSha256"]}


RECEIPT_KEYS = ("schemaVersion", "runId", "registrationId", "purpose", "diagnosticOf", "status",
                "asrText", "outputText", "protocolComplete", "latencyMs", "cost", "errorCode",
                "executedConfigSha256", "actualAsrVersion", "actualRewriteVersion")


def validate_receipt(row, lock, stored=True):
    shape(row, RECEIPT_KEYS + (("bindings", "recordedAt", "resultSha256") if stored else ()), "receipt")
    require(type(row["schemaVersion"]) is int and row["schemaVersion"] == VERSION, "receipt version unsupported")
    label(row["runId"], "runId")
    require(row["registrationId"] in registrations(lock), "unknown registrationId")
    fixture, arm = registrations(lock)[row["registrationId"]]
    require(row["purpose"] in ("primary", "diagnostic"), "receipt purpose invalid")
    if row["purpose"] == "primary":
        require(row["diagnosticOf"] is None, "primary cannot be a diagnostic retry")
    else:
        label(row["diagnosticOf"], "diagnosticOf")
        require(row["diagnosticOf"] != row["runId"], "diagnostic cannot refer to itself")
    require(row["status"] in ("missing", "failed", "succeeded"), "status invalid")
    require(fixture["collectionStatus"] == "ready" or row["status"] == "missing", "uncollected fixture cannot be run")
    if row["status"] != "missing":
        require(row["executedConfigSha256"] == digest(arm["config"]), "executed configuration differs from preregistration")
        require(arm["config"]["parameters"].get("templateOnly") is not True,
                "collection template configuration must be replaced before real runs")
    else:
        require(row["executedConfigSha256"] is None, "missing run cannot claim execution configuration")
    for key in ("actualAsrVersion", "actualRewriteVersion"):
        if row[key] is not None:
            label(row[key], key)
    for key in ("asrText", "outputText"):
        if row[key] is not None:
            string(row[key], key, allow_empty=True)
    if row["status"] == "succeeded":
        required_text = "asrText" if arm["task"] == "asr" else "outputText"
        string(row[required_text], required_text)
        if arm["task"] == "pipeline":
            string(row["asrText"], "asrText")
    boolean(row["protocolComplete"], "protocolComplete", nullable=True)
    shape(row["latencyMs"], LATENCIES, "latencyMs")
    for key, value in row["latencyMs"].items():
        number(value, key)
    shape(row["cost"], ("currency", "amount"), "cost")
    number(row["cost"]["amount"], "cost.amount")
    require(row["cost"]["currency"] in (None, "CNY", "USD"), "cost currency unsupported")
    require(row["cost"]["amount"] is None or row["cost"]["currency"] is not None, "known cost needs currency")
    if row["errorCode"] is not None:
        label(row["errorCode"], "errorCode")
    if row["status"] == "missing":
        require(row["asrText"] is None and row["outputText"] is None and
                row["protocolComplete"] is None and all(v is None for v in row["latencyMs"].values()) and
                row["cost"] == {"currency": None, "amount": None} and
                row["actualAsrVersion"] is None and row["actualRewriteVersion"] is None,
                "missing run cannot claim measured evidence")
    if stored:
        require(row["bindings"] == bindings(lock, fixture), "receipt not bound to this frozen input")
        string(row["recordedAt"], "recordedAt")
        require(row["resultSha256"] == digest({k: v for k, v in row.items() if k != "resultSha256"}),
                "recorded result changed; create a diagnostic run rather than editing it")
    return fixture, arm


def load_receipts(directory, lock):
    rows, run_ids, primary = [], set(), set()
    directory = Path(directory)
    if directory.exists():
        for path in sorted(directory.glob("*.json")):
            row, _ = read_json(path)
            validate_receipt(row, lock)
            require(row["runId"] not in run_ids, "duplicate runId; original receipt must remain immutable")
            run_ids.add(row["runId"])
            if row["purpose"] == "primary":
                require(row["registrationId"] not in primary, "duplicate primary; use a diagnostic run instead")
                primary.add(row["registrationId"])
            rows.append(row)
    by_id = {r["runId"]: r for r in rows}
    for row in rows:
        if row["purpose"] == "diagnostic":
            parent = by_id.get(row["diagnosticOf"])
            require(parent is not None and parent["purpose"] == "primary" and
                    parent["registrationId"] == row["registrationId"], "diagnostic needs the original same-registration primary")
    return rows


def record(lock_path, input_path, directory):
    lock, _ = verify_lock(lock_path)
    row, _ = read_json(input_path)
    fixture, _ = validate_receipt(row, lock, stored=False)
    prior = load_receipts(directory, lock)
    require(all(r["runId"] != row["runId"] for r in prior), "runId already exists")
    if row["purpose"] == "primary":
        require(all(r["purpose"] != "primary" or r["registrationId"] != row["registrationId"] for r in prior),
                "primary already recorded; diagnostic reruns never replace it")
    else:
        require(any(r["runId"] == row["diagnosticOf"] and r["purpose"] == "primary" and
                    r["registrationId"] == row["registrationId"] for r in prior), "diagnostic parent not found")
    row.update(bindings=bindings(lock, fixture), recordedAt=datetime.now(timezone.utc).isoformat())
    row["resultSha256"] = digest(row)
    Path(directory).mkdir(parents=True, exist_ok=True)
    write_new(Path(directory) / (row["runId"] + ".json"), row)
    return row


REVIEW_KEYS = ("schemaVersion", "runId", "reviewerId", "role", "blind", "factVerdicts", "modeVerdicts", "addedFacts")


def validate_review(review, row, ref, arm, stored=True):
    shape(review, REVIEW_KEYS + (("resultSha256",) if stored else ()), "review")
    require(type(review["schemaVersion"]) is int and review["schemaVersion"] == VERSION, "review version unsupported")
    require(review["runId"] == row["runId"], "review run mismatch")
    if stored:
        require(review["resultSha256"] == row["resultSha256"], "human rating refers to a different result revision")
    label(review["reviewerId"], "reviewerId")
    require(review["role"] in ("reviewer", "adjudicator"), "review role invalid")
    require(review["blind"] is True, "review must declare model-blind human rating")
    boolean(review["addedFacts"], "addedFacts")
    expected_facts = {f["id"] for f in ref["facts"]}
    expected_modes = {c["id"] for c in ref["modeCriteria"].get(arm["mode"], [])}
    for field, keys in (("factVerdicts", expected_facts), ("modeVerdicts", expected_modes)):
        require(isinstance(review[field], dict) and set(review[field]) == keys, f"{field}: every rubric item must be rated")
        require(all(v in ("correct", "incorrect", "uncertain") for v in review[field].values()), "review verdict invalid")


def load_reviews(directory, rows, refs, lock):
    by_id = {row["runId"]: row for row in rows}
    reviews, seen = defaultdict(list), set()
    directory = Path(directory)
    if directory.exists():
        for path in sorted(directory.glob("*.json")):
            review, _ = read_json(path)
            require(isinstance(review, dict) and review.get("runId") in by_id, "review run not found")
            row = by_id[review["runId"]]
            fixture, arm = registrations(lock)[row["registrationId"]]
            require(fixture["id"] in refs and row["status"] == "succeeded", "review requires a succeeded, referenced run")
            validate_review(review, row, refs[fixture["id"]], arm)
            key = (review["runId"], review["reviewerId"])
            require(key not in seen, "duplicate reviewer; a human rating cannot be overwritten")
            seen.add(key)
            reviews[review["runId"]].append(review)
    for run_reviews in reviews.values():
        raters = [r for r in run_reviews if r["role"] == "reviewer"]
        judges = [r for r in run_reviews if r["role"] == "adjudicator"]
        require(len(raters) <= 2 and len(judges) <= 1, "exactly two independent reviewers and at most one adjudicator")
        require(not judges or len(raters) == 2, "adjudication requires two original human ratings")
    return reviews


def reviewed_verdict(reviews):
    raters = [r for r in reviews if r["role"] == "reviewer"]
    judges = [r for r in reviews if r["role"] == "adjudicator"]
    if len(raters) != 2:
        return None
    if judges:
        verdict = judges[0]
    elif all(raters[0][k] == raters[1][k] for k in ("factVerdicts", "modeVerdicts", "addedFacts")):
        verdict = raters[0]
    else:
        return None
    if "uncertain" in list(verdict["factVerdicts"].values()) + list(verdict["modeVerdicts"].values()):
        return None
    return verdict


def review_record(lock_path, receipts_dir, input_path, reviews_dir):
    lock, refs = verify_lock(lock_path)
    rows = load_receipts(receipts_dir, lock)
    prior = load_reviews(reviews_dir, rows, refs, lock)
    review, _ = read_json(input_path)
    require(isinstance(review, dict) and review.get("runId") in {r["runId"] for r in rows}, "review run not found")
    row = next(r for r in rows if r["runId"] == review["runId"])
    fixture, arm = registrations(lock)[row["registrationId"]]
    require(row["status"] == "succeeded" and fixture["id"] in refs, "review requires succeeded run")
    validate_review(review, row, refs[fixture["id"]], arm, stored=False)
    require(all(r["reviewerId"] != review["reviewerId"] for r in prior[review["runId"]]), "reviewer already recorded")
    raters = [r for r in prior[review["runId"]] if r["role"] == "reviewer"]
    judges = [r for r in prior[review["runId"]] if r["role"] == "adjudicator"]
    require(len(raters) < 2 if review["role"] == "reviewer" else len(raters) == 2 and not judges,
            "reviewer/adjudicator slot not available")
    review["resultSha256"] = row["resultSha256"]
    Path(reviews_dir).mkdir(parents=True, exist_ok=True)
    write_new(Path(reviews_dir) / (review["runId"] + "--" + review["reviewerId"] + ".json"), review)


def ratio(numerator, denominator, measured=True):
    return {"numerator": numerator, "denominator": denominator,
            "rate": numerator / denominator if denominator and measured else None}


def percentile(values, percent):
    return sorted(values)[max(0, math.ceil(percent * len(values)) - 1)] if values else None


def cost_summary(rows, usable=None, audio_seconds=None):
    attempted = [r for r in rows if r["status"] != "missing"]
    unknown = sum(r["cost"]["amount"] is None for r in attempted)
    currencies = {r["cost"]["currency"] for r in attempted if r["cost"]["amount"] is not None}
    subtotals = {currency: sum(r["cost"]["amount"] for r in attempted
                              if r["cost"]["currency"] == currency and r["cost"]["amount"] is not None)
                 for currency in sorted(currencies)}
    total = next(iter(subtotals.values())) if not unknown and len(currencies) == 1 else None
    return {"attemptedRuns": len(attempted), "unknownCostRuns": unknown, "knownSubtotals": subtotals,
            "total": total, "currency": next(iter(currencies)) if len(currencies) == 1 else None,
            "perUsableDraft": total / usable if total is not None and usable else None,
            "attemptedAudioMinutes": audio_seconds / 60 if audio_seconds is not None else None,
            "perAudioMinute": total / (audio_seconds / 60) if total is not None and audio_seconds else None,
            "state": "NOT_RUN" if not attempted else "UNKNOWN" if unknown else "MIXED_CURRENCY" if len(currencies) > 1 else "KNOWN"}


def summarize_group(entries, refs, primary, reviews, arm, lock):
    status = Counter()
    metrics = {k: {"edits": 0, "referenceUnits": 0, "scoredRegistrations": 0} for k in ("cer", "wer", "mixedEnglishWer")}
    positive_hit = positive_total = negative_hit = negative_total = negative_observed = 0
    critical_correct = critical_total = consensus = usable = protocol = attempts = 0
    mode_correct = mode_total = 0
    fact_kinds = defaultdict(lambda: {"numerator": 0, "denominator": 0})
    latency = {key: [] for key in LATENCIES}
    primary_rows = []
    for rid, fixture in entries:
        row = primary.get(rid)
        state = row["status"] if row else "missing"
        status[state] += 1
        if row:
            primary_rows.append(row)
        attempts += state != "missing"
        ref = refs.get(fixture["id"])
        if ref is None:
            continue
        # First failures and absent runs are scored as empty hypotheses; they
        # never disappear from aggregate CER/WER or positive-term denominators.
        hypothesis = row["asrText"] or "" if row else ""
        if arm["task"] in ("asr", "pipeline"):
            types = [("cer", cer_tokens)] if ref["language"] in ("zh", "mixed") else [("wer", wer_tokens)]
            if ref["language"] == "mixed":
                types.append(("mixedEnglishWer", lambda text: wer_tokens(text, mixed=True)))
            for metric_name, tokenize in types:
                rt, ht = tokenize(ref["transcript"]), tokenize(hypothesis)
                if rt:
                    metrics[metric_name]["edits"] += distance(rt, ht)
                    metrics[metric_name]["referenceUnits"] += len(rt)
                    metrics[metric_name]["scoredRegistrations"] += 1
        term_text = hypothesis if arm["task"] == "asr" else row["outputText"] or "" if row else ""
        positive_total += len(ref["positiveTerms"])
        positive_hit += sum(term_present(t, term_text) for t in ref["positiveTerms"])
        negative_total += len(ref["negativeTerms"])
        if row and state == "succeeded":
            negative_observed += len(ref["negativeTerms"])
            negative_hit += sum(term_present(t, term_text) for t in ref["negativeTerms"])
        verdict = reviewed_verdict(reviews.get(row["runId"], [])) if row else None
        critical = [f for f in ref["facts"] if f["critical"]]
        critical_total += len(critical)
        mode_total += len(ref["modeCriteria"].get(arm["mode"], []))
        for fact in ref["facts"]:
            fact_kinds[fact["kind"]]["denominator"] += 1
        if verdict:
            consensus += 1
            critical_correct += sum(verdict["factVerdicts"][f["id"]] == "correct" for f in critical)
            mode_correct += sum(v == "correct" for v in verdict["modeVerdicts"].values())
            for fact in ref["facts"]:
                fact_kinds[fact["kind"]]["numerator"] += verdict["factVerdicts"][fact["id"]] == "correct"
        if row and state == "succeeded":
            protocol += row["protocolComplete"] is True
            for key in LATENCIES:
                if row["latencyMs"][key] is not None:
                    latency[key].append(row["latencyMs"][key])
            # All fact rubric items are required, even when not critical.
            usable += bool(arm["task"] != "asr" and verdict and not verdict["addedFacts"] and
                           all(v == "correct" for v in verdict["factVerdicts"].values()) and
                           all(v == "correct" for v in verdict["modeVerdicts"].values()) and
                           (arm["task"] == "rewrite" or row["protocolComplete"] is True))
    for metric in metrics.values():
        metric["rate"] = metric["edits"] / metric["referenceUnits"] if attempts and metric["referenceUnits"] else None
    return {"registered": len(entries), "collected": sum(f["id"] in refs for _, f in entries),
            "state": "NOT_RUN" if not attempts else "PARTIAL" if status["missing"] else "MEASURED",
            "statuses": {k: status[k] for k in ("missing", "failed", "succeeded")},
            "usableDraft": ratio(usable, len(entries), measured=bool(attempts and arm["task"] != "asr")),
            "humanConsensusRuns": consensus, "unreviewedOrDisputedSuccesses": status["succeeded"] - consensus,
            "criticalFacts": ratio(critical_correct, critical_total, measured=bool(attempts)),
            "factsByKind": {kind: ratio(count["numerator"], count["denominator"], measured=bool(attempts))
                            for kind, count in sorted(fact_kinds.items())},
            "modeCriteria": ratio(mode_correct, mode_total, measured=bool(attempts)),
            "protocolComplete": ratio(protocol, len(entries), measured=bool(attempts and arm["task"] != "rewrite")),
            "alignment": metrics,
            "positiveTerms": ratio(positive_hit, positive_total, measured=bool(attempts)),
            "negativeTermIntrusions": {**ratio(negative_hit, negative_observed, measured=bool(attempts)),
                                       "registeredTermChecks": negative_total,
                                       "unobservedTermChecks": negative_total - negative_observed},
            "latencyMs": {key: {"n": len(values), "p50": percentile(values, .50), "p95": percentile(values, .95),
                                "unmeasuredRegistered": len(entries) - len(values),
                                "smallSample": len(values) < 20} for key, values in latency.items()},
            "cost": cost_summary(primary_rows, usable, attempted_audio_seconds(primary_rows, lock))}


def attempted_audio_seconds(rows, lock):
    seconds = 0
    matrix = registrations(lock)
    for row in rows:
        if row["status"] != "missing":
            fixture, _ = matrix[row["registrationId"]]
            seconds += lock["frozenFixtures"][fixture["id"]]["audio"]["durationSeconds"]
    return seconds


def evaluate(lock_path, receipts_dir, reviews_dir):
    lock, refs = verify_lock(lock_path)
    rows = load_receipts(receipts_dir, lock)
    reviews = load_reviews(reviews_dir, rows, refs, lock)
    primary = {r["registrationId"]: r for r in rows if r["purpose"] == "primary"}
    groups = defaultdict(list)
    arms = {a["id"]: a for a in lock["manifest"]["arms"]}
    for rid, (fixture, arm) in registrations(lock).items():
        groups[(fixture["sourceKind"], fixture["split"], arm["id"])].append((rid, fixture))
    summary, entry_scores = [], []
    for (kind, split, arm_id), entries in sorted(groups.items()):
        summary.append({"sourceKind": kind, "split": split, "armId": arm_id,
                        **summarize_group(entries, refs, primary, reviews, arms[arm_id], lock)})
        for rid, fixture in entries:
            score = summarize_group([(rid, fixture)], refs, primary, reviews, arms[arm_id], lock)
            row = primary.get(rid)
            entry_scores.append({"registrationId": rid, "fixtureId": fixture["id"], "armId": arm_id,
                                 "speakerId": fixture["speakerId"], "sourceKind": kind, "split": split,
                                 "category": fixture["category"], "collectionStatus": fixture["collectionStatus"],
                                 "primaryRunId": row["runId"] if row else None,
                                 "status": row["status"] if row else "missing",
                                 "humanConsensus": bool(score["humanConsensusRuns"]),
                                 "usableDraft": score["usableDraft"], "alignment": score["alignment"],
                                 "positiveTerms": score["positiveTerms"],
                                 "negativeTermIntrusions": score["negativeTermIntrusions"],
                                 "criticalFacts": score["criticalFacts"]})
    diagnostic = [r for r in rows if r["purpose"] == "diagnostic"]
    return {"schemaVersion": VERSION, "scoringVersion": SCORING, "experimentId": lock["manifest"]["experimentId"],
            "sourceCommit": lock["manifest"]["sourceCommit"], "lockSha256": digest(lock),
            "arms": [{"id": a["id"], "task": a["task"], "mode": a["mode"], "configSha256": digest(a["config"]),
                      **{k: a["config"][k] for k in ("asrModel", "rewriteModel", "adapter", "region", "language")}}
                     for a in lock["manifest"]["arms"]],
            "groups": summary, "registrations": entry_scores,
            "diagnostics": {"excludedFromPrimaryMetrics": True, "runs": len(diagnostic),
                            "statuses": dict(Counter(r["status"] for r in diagnostic)),
                            "cost": cost_summary(diagnostic, audio_seconds=attempted_audio_seconds(diagnostic, lock))},
            "allCallsCost": cost_summary(rows, sum(g["usableDraft"]["numerator"] for g in summary),
                                        attempted_audio_seconds(rows, lock)),
            "limitations": ["Offline statistics; no microphone, provider, delivery, or population acceptance proven.",
                            "Human ratings are declarations. Independence and blindness require an external operator.",
                            "Nearest-rank P95 with n<20 is exploratory, not a population tail estimate."]}


def init_collection(directory, source_commit):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    categories = ["natural", "correction", "negation", "numbers", "mixed", "multi-item", "pause-tail", "weak-fast"]
    prompts = ["今天先整理会议记录，下午再把摘要发给小组。",
               "预算五千，不对，改成三千五百，日期还是周六。",
               "如果时间没确认，先不要发邀请，可能周五试用。",
               "版本是一点七，费用三点五万元，目标降低百分之三。",
               "请检查 SpeakMode 的 WASAPI 输入，再看 Rubato 重采样。",
               "第一项已经完成，第二项等确认，第三项今天先不做。",
               "先保留原始记录。停顿后继续说：结果保存到本地，不要删除原始录音。",
               "如果还没收到回复，今天不要自动提交，明天再确认。"]
    empty_hash = sha(b"")
    config = {"asrModel": "qwen3-asr-flash-realtime", "rewriteModel": "qwen-flash", "adapter": "existing-realtime",
              "region": "beijing", "language": "auto", "dictionarySha256": empty_hash,
              "promptSha256": empty_hash, "parameters": {"templateOnly": True}}
    fixtures, tasks = [], []
    for i in range(24):
        category = categories[i // 3]
        split = "dev" if i % 3 == 0 else "test"
        fid = f"pilot-{i + 1:02}"
        fixtures.append({"id": fid, "speakerId": f"speaker-{split}-{1 + (i // 3) % 2:02}", "split": split,
                         "sourceKind": "human", "category": category, "tags": [], "collectionStatus": "missing",
                         "audioPath": None, "referencePath": None})
        tasks.append({"fixtureId": fid, "prompt": prompts[i // 3], "status": "missing",
                      "instructions": "真人自然口述；逐字标注实际说法，不能把采集提示当参考逐字稿。" +
                      ("保留30至60秒实际静音，后续尾句不能裁去。" if category == "pause-tail" else ""),
                      "annotationRequired": ["verbatim", "final-and-withdrawn-facts", "source-offsets",
                                             "positive-and-negative-terms", "light-and-structure-rubrics"]})
    manifest = {"schemaVersion": VERSION, "experimentId": "human-pilot-24", "sourceCommit": source_commit,
                "arms": [{"id": "baseline-light", "task": "pipeline", "mode": "light", "config": config}], "fixtures": fixtures}
    validate_manifest(manifest)
    write_new(directory / "manifest.json", manifest)
    write_new(directory / "collection-tasks.json", {"schemaVersion": VERSION, "state": "NOT_RECORDED", "tasks": tasks})
    return {"tasks": 24, "collected": 0, "state": "NOT_RECORDED", "configMustBeFilledBeforeRealRun": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    init = sub.add_parser("init", help="create 24 unrecorded human collection tasks")
    init.add_argument("--directory", type=Path, required=True)
    init.add_argument("--source-commit", required=True)
    frozen = sub.add_parser("freeze", help="freeze manifest, references and actual PCM SHA/format")
    frozen.add_argument("--manifest", type=Path, required=True)
    frozen.add_argument("--output", type=Path, required=True)
    verify = sub.add_parser("verify", help="detect audio/reference/manifest drift")
    verify.add_argument("--lock", type=Path, required=True)
    listing = sub.add_parser("registrations", help="list registered fixture/arm IDs without transcript")
    listing.add_argument("--lock", type=Path, required=True)
    recording = sub.add_parser("record", help="append an immutable local result receipt")
    recording.add_argument("--lock", type=Path, required=True)
    recording.add_argument("--input", type=Path, required=True)
    recording.add_argument("--receipts", type=Path, required=True)
    reviewing = sub.add_parser("review", help="append a complete blinded human rating")
    reviewing.add_argument("--lock", type=Path, required=True)
    reviewing.add_argument("--input", type=Path, required=True)
    reviewing.add_argument("--receipts", type=Path, required=True)
    reviewing.add_argument("--reviews", type=Path, required=True)
    evaluation = sub.add_parser("evaluate", help="score every preregistered entry; never prints raw text")
    evaluation.add_argument("--lock", type=Path, required=True)
    evaluation.add_argument("--receipts", type=Path, required=True)
    evaluation.add_argument("--reviews", type=Path, required=True)
    evaluation.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        if args.command == "init":
            result = init_collection(args.directory, args.source_commit)
        elif args.command == "freeze":
            lock = freeze(args.manifest, args.output)
            result = {"state": "FROZEN", "registered": len(registrations(lock)), "lockSha256": digest(lock)}
        elif args.command in ("verify", "registrations"):
            lock, refs = verify_lock(args.lock)
            result = {"state": "VERIFIED", "collected": len(refs), "registered": len(registrations(lock))}
            if args.command == "registrations":
                result["registrations"] = [{"registrationId": rid, "fixtureId": f["id"], "armId": a["id"],
                                            "collectionStatus": f["collectionStatus"], "configSha256": digest(a["config"])}
                                           for rid, (f, a) in registrations(lock).items()]
        elif args.command == "record":
            row = record(args.lock, args.input, args.receipts)
            result = {k: row[k] for k in ("runId", "registrationId", "purpose", "status")}
        elif args.command == "review":
            review_record(args.lock, args.receipts, args.input, args.reviews)
            result = {"state": "HUMAN_RATING_RECORDED"}
        else:
            result = evaluate(args.lock, args.receipts, args.reviews)
            if args.output:
                write_new(args.output, result)
        print(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False))
    except (ValidationError, OSError, KeyError, TypeError) as error:
        # OS exceptions can contain a local path; keep CLI errors path/text-free.
        message = str(error) if isinstance(error, ValidationError) else "local I/O or schema error; no output overwritten"
        parser.exit(2, f"voice-quality-lab: {message}\n")


if __name__ == "__main__":
    main()
