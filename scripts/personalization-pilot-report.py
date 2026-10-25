"""Offline pilot accounting. Reads only the supplied CSV and explicit text files.

Outputs aggregate metadata, never participant text. No app DB, network or models.
Missing trials and manual targets remain UNKNOWN/NOT_RUN, never zero edits.
"""
import argparse
import csv
import json
from collections import Counter
from pathlib import Path

UNKNOWN = {'', 'UNKNOWN', 'NOT_RUN'}
OUTCOMES = {'passed', 'sourceFallback', 'failed', 'cancelled', 'UNKNOWN', 'NOT_RUN'}
DELIVERED = {'passed', 'sourceFallback'}
FIELDS = [
    'participant_id', 'task_id', 'scene', 'profile_version', 'provider', 'model',
    'recording_outcome', 'stop_to_ready_ms', 'host_received',
    'base_outcome', 'personal_outcome', 'base_text_path', 'personal_text_path',
    'base_final_path', 'personal_final_path', 'applicable_preferences', 'executed_preferences',
    'personal_added_serious_errors', 'personal_scope_violations', 'preference_vote',
    'blind_order_recorded', 'reason',
]


def count(value):
    if value in UNKNOWN:
        return None
    number = int(value)
    if number < 0:
        raise ValueError('Counts must be nonnegative')
    return number


def edit_distance(before, after):
    # Python strings iterate Unicode code points, including non-BMP characters.
    # Strip shared edges without changing the minimum Levenshtein distance.
    prefix = 0
    while prefix < min(len(before), len(after)) and before[prefix] == after[prefix]:
        prefix += 1
    before, after = before[prefix:], after[prefix:]
    suffix = 0
    while suffix < min(len(before), len(after)) and before[-suffix-1] == after[-suffix-1]:
        suffix += 1
    if suffix:
        before, after = before[:-suffix], after[:-suffix]
    if len(before) < len(after):
        before, after = after, before
    if not after:
        return len(before)
    if len(before) * len(after) > 16_000_000:
        raise ValueError('Manual text difference exceeds offline computation budget')
    row = list(range(len(after) + 1))
    for i, a in enumerate(before, 1):
        next_row = [i]
        for j, b in enumerate(after, 1):
            next_row.append(min(row[j] + 1, next_row[-1] + 1, row[j-1] + (a != b)))
        row = next_row
    return row[-1]


def read_text(base, name):
    if name in UNKNOWN:
        return None
    path = Path(name)
    if not path.is_absolute():
        path = base / path
    if path.stat().st_size > 1_048_576:
        raise ValueError('Explicit text file exceeds 1 MiB')
    # Preserve intentional line breaks; do not normalize or strip user text.
    with path.open(encoding='utf-8', newline='') as stream:
        return stream.read()


def summarize(rows, base):
    seen = set()
    states = {arm: Counter() for arm in ['base', 'personal']}
    recording, hosts, votes = Counter(), Counter(), Counter()
    participants, attempts, completed_by_participant = set(), 0, Counter()
    edit_pairs, base_edits, personal_edits, missing_targets = 0, 0, 0, 0
    applicable, executed, preference_rows, missing_preferences = 0, 0, 0, 0
    severe, scope, fact_rows, severe_rows, scope_rows = 0, 0, 0, 0, 0
    observed_votes = Counter()
    for row in rows:
        identity = (row['participant_id'], row['task_id'])
        if not all(identity) or identity in seen:
            raise ValueError('Each participant/task must be present exactly once')
        seen.add(identity)
        outcome = row['recording_outcome']
        if outcome not in OUTCOMES:
            raise ValueError('Invalid recording outcome')
        recording[outcome] += 1
        if outcome not in UNKNOWN:
            attempts += 1
            participants.add(identity[0])
        if outcome in DELIVERED:
            completed_by_participant[identity[0]] += 1
        host = row['host_received']
        if host not in {'yes', 'no', 'UNKNOWN', 'NOT_RUN', ''}:
            raise ValueError('Host receipt must be yes/no/UNKNOWN/NOT_RUN')
        hosts[host or 'UNKNOWN'] += 1
        for arm in states:
            state = row[f'{arm}_outcome']
            if state not in OUTCOMES:
                raise ValueError('Invalid comparison arm outcome')
            states[arm][state] += 1
        paired = all(row[f'{arm}_outcome'] in DELIVERED for arm in states)
        if paired:
            texts = [read_text(base, row[name]) for name in
                     ['base_text_path', 'personal_text_path', 'base_final_path', 'personal_final_path']]
            if any(text is None for text in texts):
                missing_targets += 1
            else:
                base_edits += edit_distance(texts[0], texts[2])
                personal_edits += edit_distance(texts[1], texts[3])
                edit_pairs += 1
        applicable_row, executed_row = count(row['applicable_preferences']), count(row['executed_preferences'])
        if applicable_row is not None and executed_row is not None:
            if executed_row > applicable_row:
                raise ValueError('Executed preferences exceed applicable preferences')
            applicable += applicable_row
            executed += executed_row
            preference_rows += 1
        elif row['personal_outcome'] not in UNKNOWN:
            missing_preferences += 1
        severe_row = count(row['personal_added_serious_errors'])
        scope_row = count(row['personal_scope_violations'])
        if severe_row is not None:
            severe += severe_row
            severe_rows += 1
        if scope_row is not None:
            scope += scope_row
            scope_rows += 1
        if severe_row is not None and scope_row is not None:
            fact_rows += 1
        vote = row['preference_vote']
        if vote not in {'base', 'tie', 'personal', 'UNKNOWN', 'NOT_RUN', ''}:
            raise ValueError('Invalid preference vote')
        if vote not in UNKNOWN:
            observed_votes[vote] += 1
            if paired and row['blind_order_recorded'] == 'yes':
                votes[vote] += 1
    total = len(rows)
    complete_pairs = sum(states['base'][s] for s in DELIVERED) == total and sum(states['personal'][s] for s in DELIVERED) == total
    preference_rate = executed / applicable if applicable else None
    reduction = 1 - personal_edits / base_edits if base_edits else None
    full_edits = bool(total) and edit_pairs == total and complete_pairs
    personal_complete = bool(total) and sum(states['personal'][s] for s in DELIVERED) == total
    full_facts = personal_complete and fact_rows == total
    full_preferences = personal_complete and preference_rows == total and missing_preferences == 0 and applicable > 0
    def gate(complete, condition):
        return 'UNKNOWN' if not complete else ('PASS' if condition else 'FAIL')
    gates = {
        'serious_added_errors': 'FAIL' if severe else gate(full_facts, True),
        'scope_violations': 'FAIL' if scope else gate(full_facts, True),
        'preference_execution_90pct': gate(full_preferences, preference_rate is not None and preference_rate >= .9),
        'edit_burden_reduction_15pct': gate(full_edits and base_edits > 0, reduction is not None and reduction >= .15),
        'blind_preference_wins': gate(bool(total) and sum(votes.values()) == total, votes['personal'] > votes['base']),
        'five_people_thirty_recordings': gate(sum(n >= 6 for n in completed_by_participant.values()) >= 5, True),
        # These need frozen four-arm model data or paired end-to-end evidence.
        'reliability_no_regression': 'UNKNOWN', 'occupation_increment': 'UNKNOWN', 'online_latency': 'UNKNOWN',
    }
    return {
        'schema': 'personalization-pilot-report-v1', 'scope': 'human_reported_pilot_only',
        'planned_tasks': total, 'attempted_recordings': attempts, 'participants_with_attempts': len(participants),
        'participants_with_six_completed_recordings': sum(n >= 6 for n in completed_by_participant.values()),
        'completed_recordings': sum(completed_by_participant.values()),
        'recording_outcomes': dict(recording), 'host_received': dict(hosts),
        'arm_outcomes': {arm: dict(value) for arm, value in states.items()},
        'edit_distance': {'complete_pairs': edit_pairs, 'missing_manual_targets': missing_targets,
                          'base_total': base_edits if edit_pairs else None,
                          'personal_total': personal_edits if edit_pairs else None,
                          'observed_reduction_fraction': reduction, 'covers_entire_plan': full_edits},
        'preferences': {'executed': executed, 'applicable': applicable,
                        'observed_rate': preference_rate, 'evaluated_rows': preference_rows,
                        'missing_rows': missing_preferences},
        'added_serious_errors': severe if severe_rows else None, 'scope_violations': scope if scope_rows else None,
        'fact_evaluated_rows': fact_rows, 'observed_votes': dict(observed_votes), 'paired_blind_votes': dict(votes),
        'gates': gates, 'release_accepted': False,
        'release_note': 'Pilot accounting alone cannot accept the complete personalization PRD.',
        'participant_text_in_report': False, 'network_requests': 0, 'app_database_opened': False,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--csv', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    if args.out.exists():
        parser.error('Use a fresh output path; evidence is never overwritten')
    with args.csv.open(encoding='utf-8-sig', newline='') as stream:
        reader = csv.DictReader(stream)
        if not set(FIELDS).issubset(reader.fieldnames or []):
            parser.error('Missing columns from personalization-pilot-protocol v2')
        report = summarize(list(reader), args.csv.parent)
    with args.out.open('x', encoding='utf-8') as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2)
        stream.write('\n')
    print(json.dumps({'output': str(args.out), 'attempted_recordings': report['attempted_recordings'],
                      'gates': report['gates']}, ensure_ascii=False))


if __name__ == '__main__':
    main()
