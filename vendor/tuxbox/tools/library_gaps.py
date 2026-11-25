#!/usr/bin/env python3
"""Inspect Frosty's direct and delay imports and the libraries that supply them."""
import argparse
import json
from pathlib import Path
import sys

import pefile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import export_table


def gap_rows(status, inventory, name, provider=None, *, imports_only=False):
    """Compare occupied reference slots, preserving aliases and import spellings."""
    key = name.casefold()
    library = next((row for row in status['libraries'] if row['name'].casefold() == key), None)
    external = next((row for row in status.get('external_libraries', []) if row['name'].casefold() == key), None)
    image = next((row for row in inventory['libraries'] if row['name'].casefold() == key), None)
    if image is None:
        raise ValueError(f'{name}: missing reference inventory')
    coverage = library['export_coverage'] if library else None
    entries = image['exports']['entries']
    if not image.get('sha256') or (coverage and coverage.get('binary_sha256') != image['sha256']):
        raise ValueError(f'{name}: reference hashes differ; rescan before comparing')
    if coverage and coverage['total'] != len(entries):
        raise ValueError(f'{name}: reference export count differs')
    by_spelling = {}
    for entry in entries:
        for spelling in [f"#{entry['ordinal']}", *entry['names']]:
            if spelling in by_spelling:
                raise ValueError(f'{name}: duplicate reference export {spelling}')
            by_spelling[spelling] = entry['ordinal']
    verified = set()
    for spelling in (coverage['verified'] if coverage else []):
        if spelling not in by_spelling:
            raise ValueError(f'{name}: unknown verified export {spelling}')
        ordinal = by_spelling[spelling]
        if ordinal in verified:
            raise ValueError(f'{name}: duplicate verified export ordinal {ordinal}')
        verified.add(ordinal)
    imported = {}
    unresolved = []
    for imp in inventory['title']['imports']:
        if imp['module'].casefold() != key:
            continue
        for spelling in imp['symbols']:
            ordinal = by_spelling.get(spelling)
            if ordinal is None:
                unresolved.append(spelling)
            else:
                imported.setdefault(ordinal, set()).add(f"{imp['table']}:{spelling}")
    provider_slots = export_table(provider)[0] if provider is not None else None
    provider_names = {n for aliases in provider_slots.values() for n in aliases} if provider_slots else set()
    rows = []
    for entry in entries:
        ordinal, aliases = entry['ordinal'], entry['names']
        if imports_only and ordinal not in imported:
            continue
        row = {'ordinal': ordinal, 'names': aliases, 'kind': entry['kind'],
               'verified': ordinal in verified, 'imports': sorted(imported.get(ordinal, set()))}
        if provider_slots is not None:
            actual = provider_slots.get(ordinal)
            row['provider_name'] = any(alias in provider_names for alias in aliases)
            row['provider_ordinal'] = actual is not None and (not aliases or bool(set(aliases) & actual))
        rows.append(row)
    source = library.get('provider') if library else None
    if external:
        source = external['source']
    return {'library': image['name'], 'reviewed': library is not None, 'reference_sha256': image['sha256'],
            'provider': str(provider) if provider is not None else None,
            'source': source,
            'scope': 'title_imports' if imports_only else 'all_exports',
            'title_import_chain': image.get('title_import_chain', []),
            'unresolved_imports': sorted(set(unresolved)), 'exports': rows}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('libraries', nargs='+', help='catalogued DLL names, e.g. xmem.dll')
    parser.add_argument('--provider', type=Path, help='compare one built provider PE export table')
    parser.add_argument('--all', '--all-exports', action='store_true',
                        help='inspect the entire reference export table, including unused ordinals')
    parser.add_argument('--json', action='store_true', help='write the selected scope as JSON')
    args = parser.parse_args()
    if args.provider and len(args.libraries) != 1:
        parser.error('--provider requires exactly one library')
    try:
        status = json.loads((ROOT / 'evidence/library-status.json').read_text())
        inventory = json.loads((ROOT / 'evidence/frosty-xbox-library-inventory.json').read_text())
        reports = [gap_rows(status, inventory, n, args.provider, imports_only=not args.all) for n in args.libraries]
    except (ValueError, OSError, KeyError, pefile.PEFormatError) as exc:
        parser.exit(1, f'{exc}\n')
    if args.json:
        print(json.dumps(reports, indent=2))
        return
    for report in reports:
        scope = 'Full reference PE export table' if args.all else 'Frosty direct and delay imports'
        print(f"{report['library']} · {scope} · reference {report['reference_sha256'][:12]}")
        if report['source']:
            print(report['source'])
        if not args.all and not (report['exports'] or report['unresolved_imports']):
            chain = report['title_import_chain']
            if chain:
                print('Packaged import chain · ' + ' → '.join(chain))
            else:
                print("Frosty's executable has no direct or delay imports from this DLL.")
            if report['source']:
                print('See the runtime paths with python tuxbox.py status --libraries.')
        if report['provider']:
            print('Provider name and ordinal columns compare PE export tables.')
        for row in report['exports']:
            label = ', '.join(row['names']) or '(ordinal only)'
            state = row['kind']
            if report['reviewed']:
                state += ', TuxBox export checked' if row['verified'] else ', no recorded TuxBox export check'
            bindings = [name.replace('delay_imports:', 'delay import ', 1)
                        if name.startswith('delay_imports:') else name.replace('imports:', 'import ', 1)
                        for name in row['imports']]
            imported = f"  {', '.join(bindings)}" if bindings else ''
            linked = ''
            if report['provider']:
                linked = f"  provider name={'yes' if row['provider_name'] else 'no'}, ordinal={'yes' if row['provider_ordinal'] else 'no'}"
            print(f"  #{row['ordinal']:<3} {label}  [{state}]{imported}{linked}")
        if report['unresolved_imports']:
            print('  Imports absent from reference inventory: ' + ', '.join(report['unresolved_imports']))


if __name__ == '__main__':
    main()
