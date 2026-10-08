"""Freeze a bounded fixed-TTS matrix, then evaluate immutable cloud receipts.

Only synthetic fixture text and experiment outputs are read. No user profile,
credential, ambient audio, or production database is accessed by this script.
"""
import argparse
import hashlib
import json
import statistics
import unicodedata
import wave
from datetime import datetime, timezone
from pathlib import Path

TERMS = ['SpeakMode', 'Typeless', 'Superwhisper', 'WASAPI', 'Rubato']


def write_new(path, data):
    with path.open('x', encoding='utf-8') as output:
        json.dump(data, output, ensure_ascii=False, indent=2)


def normalize(text):
    return ''.join(c for c in unicodedata.normalize('NFKC', text).casefold()
                   if not c.isspace() and not unicodedata.category(c).startswith('P'))


def distance(a, b):
    previous = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        current = [i]
        for j, cb in enumerate(b, 1):
            current.append(min(current[-1] + 1, previous[j] + 1,
                               previous[j - 1] + (ca != cb)))
        previous = current
    return previous[-1]


def commit_sha(value):
    if len(value) != 40 or any(c not in '0123456789abcdefABCDEF' for c in value):
        raise argparse.ArgumentTypeError('--source-commit must be a complete 40-character hexadecimal commit SHA')
    return value.lower()


def freeze(root, source_commit):
    # Bind the preregistration to an explicitly selected candidate rather than
    # silently attributing a later build to the original 0.1.5 baseline.
    source_commit = commit_sha(source_commit)
    texts = json.loads((root / 'fixture-texts.json').read_text(encoding='utf-8'))
    fixtures = root / 'fixtures'
    with wave.open(str(fixtures / 'C1.wav'), 'rb') as first, wave.open(str(fixtures / 'C2.wav'), 'rb') as last:
        for reader in (first, last):
            assert (reader.getnchannels(), reader.getframerate(), reader.getsampwidth()) == (1, 16000, 2)
        c1 = first.readframes(first.getnframes())
        c2 = last.readframes(last.getnframes())
    cpath = fixtures / 'C.wav'
    assert not cpath.exists()
    with wave.open(str(cpath), 'wb') as output:
        output.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
        output.writeframes(c1 + b'\0\0' * (16000 * 30) + c2)
    references = {'A': texts['A'], 'B': texts['B'], 'C': texts['C1'] + texts['C2']}
    details = {}
    for fid in references:
        path = fixtures / (fid + '.wav')
        with wave.open(str(path), 'rb') as reader:
            assert (reader.getnchannels(), reader.getframerate(), reader.getsampwidth()) == (1, 16000, 2)
            details[fid] = {'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                            'samples': reader.getnframes(), 'durationSeconds': reader.getnframes() / 16000,
                            'path': str(path.resolve()), 'reference': references[fid]}
    runs = []
    for repeat, conditions in [(1, ['off', 'on']), (2, ['on', 'off'])]:
        for fid in references:
            for condition in conditions:
                runs.append({'runId': f'{fid}-{condition}-r{repeat}', 'fixtureId': fid,
                             'wavPath': details[fid]['path'], 'dictionaryTerms': TERMS if condition == 'on' else [],
                             'language': 'auto', 'baseModes': ['light', 'structure']})
    total = sum(details[run['fixtureId']]['durationSeconds'] for run in runs)
    assert len(runs) == 12 and total <= 600
    prereg = {
        'frozenAt': datetime.now(timezone.utc).isoformat(),
        'sourceCommit': source_commit,
        'voice': 'Microsoft Huihui Desktop - Chinese (Simplified)', 'voiceRate': 0,
        'fixtures': details, 'dictionaryTerms': TERMS, 'totalAsrAudioSeconds': total,
        'scoring': {'cer': 'NFKC + casefold; remove Unicode punctuation and whitespace; character Levenshtein/reference length. No spelling or number normalization.',
                    'terms': 'Casefold exact ASCII word substring; Chinese phonetics do not count.',
                    'meaningB': ['下周开发范围', '最终预算3500且明确不是5000', '时间未确认这一条件', '先不要发邀请', '周五试用仍可能且时间未确定'],
                    'meaningC': ['后段保存结果到本地', '尾句不要删除原始录音'],
                    'manual': 'Automated keywords are aids only. Fixed rubric manual review overrides neither CER nor raw outputs.'},
        'boundaries': ['Synthetic TTS is not human voice accuracy.', 'Cloud module fixture injection bypasses microphone, coordinator, UI, SQLite and hotkey.',
                       'Hardware capture is local-only metadata; no microphone PCM uploaded or saved.', 'Owned target test readback is distinct from production Unconfirmed status.',
                       'Small sample: report per-run, median and range, no population P95/significance or stability percentage.'],
        'stopRules': ['Credential or quota/permission failure stops remaining.', 'Three consecutive transport failures stop remaining (tool guard).',
                      'No automatic retry or fixture regeneration after first API request.'],
    }
    scope = {'kind': 'FIXED_TTS_REAL_CLOUD_PRODUCTION_MODULES', 'microphone': 'BYPASSED', 'appCoordinator': 'BYPASSED',
             'sourceCommit': prereg['sourceCommit'], 'preRegistrationSha256': None}
    write_new(root / 'preregistration.json', prereg)
    scope['preRegistrationSha256'] = hashlib.sha256((root / 'preregistration.json').read_bytes()).hexdigest()
    write_new(root / 'cloud-manifest.json', {'scope': scope, 'asrRuns': runs})
    print(json.dumps({'runs': len(runs), 'totalAudioSeconds': total, 'fixtures': {k: v['durationSeconds'] for k, v in details.items()}}, ensure_ascii=False))


def evaluate(root):
    prereg = json.loads((root / 'preregistration.json').read_text(encoding='utf-8'))
    receipt = json.loads((root / 'cloud.json').read_text(encoding='utf-8'))
    # Parsing deliberately follows the tool's recorded schema, never transcript contents.
    rows = []
    for asr in [row for row in receipt['results'] if row['kind'] == 'asr']:
        fid = asr['fixtureId']
        run_id = asr['runId']
        text = '\n'.join(item['text'] for item in asr.get('source', {}).get('items', []))
        reference = prereg['fixtures'][fid]['reference']
        normal = normalize(text)
        edit = distance(normalize(reference), normal)
        registered = next(row for row in receipt['preregisteredRuns'] if row['runId'] == run_id)
        rows.append({'runId': run_id, 'fixtureId': fid, 'dictionary': 'on' if registered['dictionaryTerms'] else 'off',
                     'asrSucceeded': asr.get('status') == 'passed', 'sourceText': text,
                     'cer': edit / len(normalize(reference)), 'editDistance': edit,
                     'termHits': [term for term in TERMS if term.casefold() in text.casefold()],
                     'unrelatedTermIntrusions': [term for term in TERMS if fid != 'A' and term.casefold() in text.casefold()],
                     'rewrites': [row for row in receipt['results'] if row['kind'] == 'rewrite' and row['runId'] == run_id]})
    write_new(root / 'scores-automatic.json', {'scope': 'ASSISTIVE_SCORING_REQUIRES_MANUAL_MEANING_REVIEW', 'rows': rows})
    print(json.dumps({'runs': len(rows), 'successfulAsr': sum(row['asrSucceeded'] for row in rows)}, ensure_ascii=False))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('command', choices=['freeze', 'evaluate'])
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--source-commit', type=commit_sha,
                        help='Required for freeze: exact 40-character source commit for this candidate')
    args = parser.parse_args()
    if args.command == 'freeze' and args.source_commit is None:
        parser.error('freeze requires --source-commit with the exact candidate commit SHA')
    assert args.root.is_absolute()
    if args.command == 'freeze':
        freeze(args.root, args.source_commit)
    else:
        evaluate(args.root)
