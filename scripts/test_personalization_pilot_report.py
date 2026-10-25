"""Metric boundary tests with synthetic local data only."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('pilot_report', Path(__file__).with_name('personalization-pilot-report.py'))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


def planned(task='T1'):
    row = dict.fromkeys(report.FIELDS, 'NOT_RUN')
    row.update(participant_id='P001', task_id=task, scene='chat')
    return row


class PilotReportTests(unittest.TestCase):
    def test_six_planned_tasks_are_not_six_attempts_or_zero_edits(self):
        result = report.summarize([planned(f'T{i}') for i in range(1, 7)], Path('.'))
        self.assertEqual(result['planned_tasks'], 6)
        self.assertEqual(result['attempted_recordings'], 0)
        self.assertIsNone(result['edit_distance']['base_total'])
        self.assertIsNone(result['preferences']['observed_rate'])
        self.assertEqual(set(result['gates'].values()), {'UNKNOWN'})
        self.assertFalse(result['release_accepted'])

    def test_unicode_distance_counts_code_points_and_preserves_whitespace(self):
        self.assertEqual(report.edit_distance('A😀B', 'A🚀B'), 1)
        self.assertEqual(report.edit_distance('A B', 'AB'), 1)
        self.assertEqual(report.edit_distance('a\r\nb', 'a\nb'), 1)
        self.assertEqual(report.edit_distance('', '中文'), 2)

    def test_fallback_and_host_failure_remain_in_denominators(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, text in [('base', '嗯，好'), ('personal', '好'), ('final', '好')]:
                (root / name).write_text(text, encoding='utf-8')
            row = planned()
            row.update(recording_outcome='passed', host_received='no', base_outcome='sourceFallback',
                       personal_outcome='passed', base_text_path='base', personal_text_path='personal',
                       base_final_path='final', personal_final_path='final', applicable_preferences='1',
                       executed_preferences='1', personal_added_serious_errors='0', personal_scope_violations='0',
                       preference_vote='personal', blind_order_recorded='yes')
            result = report.summarize([row], root)
            self.assertEqual(result['host_received']['no'], 1)
            self.assertEqual(result['arm_outcomes']['base']['sourceFallback'], 1)
            self.assertEqual(result['edit_distance']['base_total'], 2)
            self.assertEqual(result['gates']['edit_burden_reduction_15pct'], 'PASS')
            self.assertEqual(result['gates']['reliability_no_regression'], 'UNKNOWN')
            self.assertFalse(result['release_accepted'])

    def test_incomplete_pairs_cannot_pass_from_one_favourable_observation(self):
        row = planned()
        row.update(recording_outcome='passed', base_outcome='passed', personal_outcome='failed',
                   applicable_preferences='1', executed_preferences='0', personal_added_serious_errors='1',
                   personal_scope_violations='0', preference_vote='personal', blind_order_recorded='yes')
        result = report.summarize([row, planned('T2')], Path('.'))
        self.assertEqual(result['gates']['serious_added_errors'], 'FAIL')
        self.assertEqual(result['gates']['edit_burden_reduction_15pct'], 'UNKNOWN')
        self.assertEqual(result['paired_blind_votes'], {})
        self.assertEqual(result['arm_outcomes']['personal']['failed'], 1)

    def test_duplicates_negative_counts_and_impossible_preferences_are_rejected(self):
        with self.assertRaises(ValueError):
            report.summarize([planned(), planned()], Path('.'))
        with self.assertRaises(ValueError):
            report.count('-1')
        row = planned()
        row.update(applicable_preferences='1', executed_preferences='2')
        with self.assertRaises(ValueError):
            report.summarize([row], Path('.'))

    def test_missing_manual_targets_and_zero_baseline_do_not_create_improvement(self):
        row = planned()
        row.update(base_outcome='passed', personal_outcome='passed')
        result = report.summarize([row], Path('.'))
        self.assertEqual(result['edit_distance']['missing_manual_targets'], 1)
        self.assertIsNone(result['edit_distance']['observed_reduction_fraction'])
        self.assertEqual(result['gates']['edit_burden_reduction_15pct'], 'UNKNOWN')

    def test_partial_fact_judgment_cannot_hide_a_known_serious_error(self):
        row = planned()
        row.update(personal_outcome='passed', personal_added_serious_errors='1')
        result = report.summarize([row], Path('.'))
        self.assertEqual(result['added_serious_errors'], 1)
        self.assertEqual(result['gates']['serious_added_errors'], 'FAIL')
        self.assertIsNone(result['scope_violations'])

    def test_zero_baseline_is_not_a_fictitious_percentage_gain(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'text').write_text('already usable', encoding='utf-8')
            row = planned()
            row.update(base_outcome='passed', personal_outcome='passed',
                       base_text_path='text', personal_text_path='text', base_final_path='text', personal_final_path='text')
            result = report.summarize([row], root)
            self.assertEqual(result['edit_distance']['base_total'], 0)
            self.assertIsNone(result['edit_distance']['observed_reduction_fraction'])
            self.assertEqual(result['gates']['edit_burden_reduction_15pct'], 'UNKNOWN')

    def test_thirty_imbalanced_tasks_are_not_five_people_with_six_tasks(self):
        rows=[]
        for participant, tasks in enumerate([26, 1, 1, 1, 1], 1):
            for task in range(tasks):
                row=planned(f'T{task}')
                row.update(participant_id=f'P{participant}', recording_outcome='passed')
                rows.append(row)
        result=report.summarize(rows, Path('.'))
        self.assertEqual(result['attempted_recordings'], 30)
        self.assertEqual(result['participants_with_six_completed_recordings'], 1)
        self.assertEqual(result['gates']['five_people_thirty_recordings'], 'UNKNOWN')

    def test_cancelled_attempts_remain_visible_without_becoming_completed_recordings(self):
        rows=[]
        for participant in range(5):
            for task in range(6):
                row=planned(f'T{task}')
                row.update(participant_id=f'P{participant}', recording_outcome='cancelled')
                rows.append(row)
        result=report.summarize(rows, Path('.'))
        self.assertEqual(result['attempted_recordings'], 30)
        self.assertEqual(result['completed_recordings'], 0)
        self.assertEqual(result['gates']['five_people_thirty_recordings'], 'UNKNOWN')

    def test_five_complete_people_do_not_by_themselves_accept_quality_or_release(self):
        rows=[]
        for participant in range(5):
            for task in range(6):
                row=planned(f'T{task}')
                row.update(participant_id=f'P{participant}', recording_outcome='passed')
                rows.append(row)
        result=report.summarize(rows, Path('.'))
        self.assertEqual(result['gates']['five_people_thirty_recordings'], 'PASS')
        self.assertEqual(result['gates']['edit_burden_reduction_15pct'], 'UNKNOWN')
        self.assertFalse(result['release_accepted'])


if __name__ == '__main__':
    unittest.main()
