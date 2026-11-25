#!/usr/bin/env python3
"""Run a foreground research command with an isolated, temporary XG proxy."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
from tempfile import NamedTemporaryFile

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from project_tuxbox.inspect import digest
from tools.prepare_frosty import POINTER, XG_PATH, publish, verify_staged_files

STATE = 'xg-reference-trace.json'
BACKUP = 'xg-reference-trace-backup.bin'
MANIFEST_BACKUP = 'xg-reference-trace-providers.json'


def atomic_copy(source, dest, expected):
    tmp = None
    try:
        with source.open('rb') as src, NamedTemporaryFile(dir=dest.parent, delete=False) as out:
            tmp = Path(out.name)
            shutil.copyfileobj(src, out)
            out.flush()
            os.fsync(out.fileno())
        if digest(tmp) != expected:
            raise ValueError(f'hash mismatch copying {source}')
        os.replace(tmp, dest)
    finally:
        if tmp:
            tmp.unlink(missing_ok=True)


def recover(work):
    state_path = work / STATE
    state = json.loads(state_path.read_text())
    dest = work / XG_PATH
    reference = dest.parent / 'xg_reference.dll'
    backup = work / BACKUP
    manifest_backup = work / MANIFEST_BACKUP
    if not backup.is_file() or digest(backup) != state['tuxbox_sha256']:
        raise ValueError('verified TuxBox XG backup is missing or changed; manual recovery required')
    if not manifest_backup.is_file() or digest(manifest_backup) != state['providers_sha256']:
        raise ValueError('verified provider manifest backup is missing or changed; manual recovery required')
    if dest.exists() and digest(dest) not in {state['tuxbox_sha256'], state['proxy_sha256']}:
        raise ValueError('staged XG changed during trace; manual recovery required')
    if reference.exists() and digest(reference) != state['reference_sha256']:
        raise ValueError('staged reference changed during trace; manual recovery required')
    atomic_copy(backup, dest, state['tuxbox_sha256'])
    reference.unlink(missing_ok=True)
    atomic_copy(manifest_backup, work / 'providers.json', state['providers_sha256'])
    verify_staged_files(work)
    state['status'] = 'restored'
    publish(work / 'xg-reference-trace-last.json', json.dumps(state, indent=2) + '\n')
    state_path.unlink()
    backup.unlink()
    manifest_backup.unlink()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--work', type=Path, required=True)
    parser.add_argument('--proxy-dir', type=Path)
    parser.add_argument('--recover', action='store_true')
    parser.add_argument('command', nargs=argparse.REMAINDER,
                        help='foreground command after --; do not use the ordinary launcher, which also locks')
    args = parser.parse_args()
    work = args.work.resolve(strict=True)
    if POINTER.is_file() and Path(POINTER.read_text().strip()).resolve() == work:
        parser.error('choose a separate prepared work directory, not the selected default')
    if not (work / 'launch.lock').is_file():
        parser.error('expected an existing prepared work directory')
    with (work / 'launch.lock').open('a+b') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if args.recover:
            if args.command or args.proxy_dir:
                parser.error('--recover accepts neither a command nor proxy directory')
            recover(work)
            print('Restored and verified TuxBox XG.')
            return
        command = args.command[1:] if args.command[:1] == ['--'] else args.command
        if not args.proxy_dir or not command:
            parser.error('--proxy-dir and a foreground command after -- are required')
        if any((work / name).exists() for name in (STATE, BACKUP, MANIFEST_BACKUP)):
            raise ValueError('unfinished trace detected; use --recover')
        if json.loads((work / 'inputs.json').read_text()).get('xg_provider') != 'tuxbox':
            raise ValueError('work directory must be prepared with TuxBox XG')
        verify_staged_files(work)
        source = args.proxy_dir.resolve(strict=True)
        provenance = json.loads((source / 'provenance.json').read_text())
        proxy, reference = source / 'xg_xs.dll', source / 'xg_reference.dll'
        if digest(proxy) != provenance['proxy_sha256'] or digest(reference) != provenance['reference_sha256']:
            raise ValueError('proxy or reference does not match its provenance')
        dest = work / XG_PATH
        staged_ref = dest.parent / 'xg_reference.dll'
        if staged_ref.exists():
            raise ValueError('reference filename already exists in stage')
        original_hash = digest(dest)
        state = {'status': 'temporary reference proxy in use', 'work': str(work),
                 'proxy_dir': str(source), 'tuxbox_sha256': original_hash,
                 'providers_sha256': digest(work / 'providers.json'),
                 'proxy_sha256': provenance['proxy_sha256'],
                 'reference_sha256': provenance['reference_sha256'],
                 'reference_source': provenance['reference_source'],
                 'command': command}
        atomic_copy(dest, work / BACKUP, original_hash)
        atomic_copy(work / 'providers.json', work / MANIFEST_BACKUP, state['providers_sha256'])
        publish(work / STATE, json.dumps(state, indent=2) + '\n')
        old_sigterm = signal.getsignal(signal.SIGTERM)
        def terminate(_signum, _frame):
            raise KeyboardInterrupt('interrupted')
        signal.signal(signal.SIGTERM, terminate)
        try:
            atomic_copy(reference, staged_ref, state['reference_sha256'])
            atomic_copy(proxy, dest, state['proxy_sha256'])
            files = json.loads((work / MANIFEST_BACKUP).read_text())
            files[str(XG_PATH)] = {'origin': 'Research XG trace proxy', 'sha256': state['proxy_sha256']}
            files[str(staged_ref.relative_to(work))] = {'origin': 'Packaged Xbox XG reference for research',
                                                       'sha256': state['reference_sha256']}
            publish(work / 'providers.json', json.dumps(files, indent=2) + '\n')
            print('Research XG reference proxy active; TuxBox XG will be restored.', flush=True)
            process = subprocess.Popen(command, start_new_session=True)
            try:
                returncode = process.wait()
            except KeyboardInterrupt:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                raise
        finally:
            signal.signal(signal.SIGTERM, old_sigterm)
            recover(work)
            print('Restored and verified TuxBox XG.', flush=True)
        sys.exit(returncode)


if __name__ == '__main__':
    main()
