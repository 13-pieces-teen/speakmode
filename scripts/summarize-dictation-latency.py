"""Summarize a bounded synthetic latency receipt without hiding rejected flows."""
from __future__ import annotations
import argparse
import hashlib
import json
from pathlib import Path
import statistics

def median(rows, field):
    return statistics.median(row[field] for row in rows) if rows else None

def summarize(path: Path):
    raw = path.read_bytes()
    receipt = json.loads(raw)
    assert receipt['tool'] == 'speakmode-dictation-latency-lab'
    assert receipt['status'] == 'completed'
    rows = receipt['results']
    reviews = [r for r in rows if r['kind'] == 'review']
    flows = [r for r in rows if r['kind'] == 'flow']
    assert receipt['requests'] == len(reviews) + sum(r['trace']['calls'] for r in flows)
    assert receipt['requests'] <= receipt['maximumRequests']
    result = {'scope': receipt['scope'], 'receiptSha256': hashlib.sha256(raw).hexdigest(),
              'requests': receipt['requests'], 'reviews': {}, 'flowPairs': [], 'knownFailures': []}
    for arm in ('named', 'ordered'):
        subset = [r for r in reviews if r['arm'] == arm]
        result['reviews'][arm] = {'n': len(subset), 'parsed': sum(r['parsed'] for r in subset),
            'matchedExpected': sum(r['expectationMet'] for r in subset), 'medianMs': median(subset, 'elapsedMs'),
            'medianCompletionTokens': statistics.median(r['response']['usage']['completion_tokens'] for r in subset)}
        result['knownFailures'].extend({'kind': 'review', 'caseId': r['caseId'], 'arm': arm}
                                      for r in subset if not r['expectationMet'])
    groups = {}
    for row in reviews:
        pair = groups.setdefault(row['pair'], {})
        assert row['arm'] not in pair
        pair[row['arm']] = row
    result['reviewDecisionAgreement'] = {'matched': sum(pair['named']['accepted'] == pair['ordered']['accepted']
                                                      for pair in groups.values()), 'pairs': len(groups)}
    groups = {}
    for row in flows:
        pair = groups.setdefault(row['pair'], {})
        assert row['arm'] not in pair
        pair[row['arm']] = row
    for pair in groups.values():
        a, b = pair['named'], pair['ordered']
        assert a['caseId'] == b['caseId'] and a['sourceSha256'] == b['sourceSha256']
        both = a['validated'] and b['validated']
        result['flowPairs'].append({'caseId': a['caseId'], 'bothValidated': both,
            'namedMs': a['elapsedMs'], 'orderedMs': b['elapsedMs'],
            'namedValidated': a['validated'], 'orderedValidated': b['validated'],
            'namedError': a['errorCode'], 'orderedError': b['errorCode']})
        if not both:
            result['knownFailures'].append({'kind': 'flow', 'caseId': a['caseId'],
                                            'namedError': a['errorCode'], 'orderedError': b['errorCode']})
    comparable = [r for r in result['flowPairs'] if r['bothValidated']]
    a, b = median(comparable, 'namedMs'), median(comparable, 'orderedMs')
    result['validatedPipeline'] = {'comparablePairs': len(comparable), 'totalPairs': len(groups),
        'namedMedianMs': a, 'orderedMedianMs': b, 'medianReductionPercent': 100 * (a - b) / a if a else None,
        'fasterPairs': sum(r['orderedMs'] < r['namedMs'] for r in comparable)}
    result['microphoneAsrAndHostDelivery'] = 'NOT_RUN'
    return result

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--receipt', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    with args.out.open('x', encoding='utf-8') as target:
        json.dump(summarize(args.receipt), target, ensure_ascii=False, indent=2)
