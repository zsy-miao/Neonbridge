from contextlib import redirect_stdout
import io
import json
import unittest
from unittest.mock import patch

from tools.library_gaps import gap_rows, main


class LibraryGapTests(unittest.TestCase):
    def setUp(self):
        self.status = {'libraries': [{'name': 'Example.dll', 'export_coverage': {
            'binary_sha256': 'sample', 'total': 3, 'verified': ['Alias', '#3']}}]}
        self.inventory = {'libraries': [{'name': 'example.dll', 'sha256': 'sample',
            'exports': {'entries': [
                {'ordinal': 1, 'names': ['First', 'Alias'], 'kind': 'code'},
                {'ordinal': 2, 'names': ['Second'], 'kind': 'forwarder'},
                {'ordinal': 3, 'names': [], 'kind': 'data'}]}}],
            'title': {'imports': [
                {'module': 'EXAMPLE.dll', 'table': 'imports', 'symbols': ['First', '#1', 'Missing']},
                {'module': 'example.dll', 'table': 'delay_imports', 'symbols': ['Second']}]}}

    def test_aliases_and_ordinal_imports_resolve_to_same_slot(self):
        report = gap_rows(self.status, self.inventory, 'example.dll')
        first, second, third = report['exports']
        self.assertTrue(first['verified'])
        self.assertEqual(first['imports'], ['imports:#1', 'imports:First'])
        self.assertFalse(second['verified'])
        self.assertEqual(second['imports'], ['delay_imports:Second'])
        self.assertTrue(third['verified'])
        self.assertEqual(report['unresolved_imports'], ['Missing'])

    def test_reference_build_must_match_and_verified_slots_must_be_unique(self):
        self.inventory['libraries'][0]['sha256'] = 'different'
        with self.assertRaisesRegex(ValueError, 'hashes differ'):
            gap_rows(self.status, self.inventory, 'Example.dll')
        self.inventory['libraries'][0]['sha256'] = 'sample'
        self.status['libraries'][0]['export_coverage']['verified'] = ['First', '#1']
        with self.assertRaisesRegex(ValueError, 'duplicate verified'):
            gap_rows(self.status, self.inventory, 'Example.dll')

    def test_inventoried_dll_can_be_queried_outside_the_active_map(self):
        self.status['libraries'] = []
        report = gap_rows(self.status, self.inventory, 'EXAMPLE.dll')
        self.assertFalse(report['reviewed'])
        self.assertFalse(any(entry['verified'] for entry in report['exports']))
        self.assertEqual(report['exports'][1]['imports'], ['delay_imports:Second'])

    def test_cli_defaults_to_title_imports_and_can_return_the_full_table(self):
        for options, scope in [([], 'title_imports'), (['--all'], 'all_exports')]:
            output = io.StringIO()
            with patch('sys.argv', ['library_gaps.py', 'xcrdapi.dll', '--json', *options]), redirect_stdout(output):
                main()
            report, = json.loads(output.getvalue())
            self.assertEqual(report['scope'], scope)
            if not options:
                self.assertEqual([row['names'] for row in report['exports']], [['DStorageGetFactory']])
            else:
                self.assertTrue(any(not row['imports'] for row in report['exports']))

    def test_external_decoder_with_no_static_imports_retains_runtime_source(self):
        self.status['libraries'] = []
        self.status['external_libraries'] = [{'name': 'Example.dll', 'source': 'Packaged decoder'}]
        self.inventory['title']['imports'] = []
        report = gap_rows(self.status, self.inventory, 'example.dll', imports_only=True)
        self.assertEqual(report['source'], 'Packaged decoder')
        self.assertEqual(report['exports'], [])

    def test_provider_name_and_ordinal_are_checked_separately(self):
        with patch('tools.library_gaps.export_table', return_value=({1: {'Other'}, 8: {'First'}, 3: set()}, set())):
            rows = gap_rows(self.status, self.inventory, 'Example.dll', 'provider.dll')['exports']
        self.assertTrue(rows[0]['provider_name'])
        self.assertFalse(rows[0]['provider_ordinal'])
        self.assertFalse(rows[1]['provider_name'])
        self.assertTrue(rows[2]['provider_ordinal'])


if __name__ == '__main__':
    unittest.main()
