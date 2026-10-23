"""Owned-fixture live probe; never writes folder associations or user files."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import winreg

ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / 'artifacts/shell-bridge-experiment'
EXE = OUTPUT / 'shell-bridge.exe'
RE = ROOT / 'dist/RustExplorer-default-browser-preview/rust-explorer.exe'


def registry_snapshot():
    result = {}
    def read(path):
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path) as key:
                values = []
                for i in range(winreg.QueryInfoKey(key)[1]):
                    name, value, kind = winreg.EnumValue(key, i)
                    values.append([name, kind, value.hex() if isinstance(value, bytes) else value])
                result[path] = sorted(values)
                for i in range(winreg.QueryInfoKey(key)[0]):
                    read(path + '\\' + winreg.EnumKey(key, i))
        except FileNotFoundError:
            result[path] = None
    for name in ('Directory', 'Drive', 'Folder'):
        read('Software\\Classes\\' + name)
    return result


def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else 'immediate'
    if mode not in ('immediate', 'delayed', 'no-selection', 'aborted'):
        raise ValueError('Use immediate, delayed, no-selection, or aborted')
    OUTPUT.mkdir(parents=True, exist_ok=True)
    before = registry_snapshot()
    (OUTPUT / 'registry-before.json').write_text(json.dumps(before, ensure_ascii=False, indent=2), encoding='utf-8')
    fixture = Path(tempfile.mkdtemp(prefix='RE-shell-bridge-'))
    folder = fixture / '中文 空格目录'
    folder.mkdir()
    target = folder / '选中 文件,01.txt'
    target.write_text('Owned shell bridge test fixture.\n', encoding='utf-8')
    environment = dict(os.environ)
    environment['LOCALAPPDATA'] = str(fixture / 'profile')
    environment['PATH'] = r'C:\msys64\ucrt64\bin;' + environment['PATH']
    process = subprocess.Popen([str(EXE), str(folder), str(RE)], stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, env=environment, creationflags=subprocess.CREATE_NO_WINDOW)
    try:
        ready = process.stdout.readline().strip()
        if ready != 'READY':
            raise RuntimeError(f'Bridge did not register: {ready}; {process.stderr.read()}')
        print('Bridge registered; mode=' + mode, flush=True)
        started = time.perf_counter()
        if mode == 'delayed':
            time.sleep(1.2)
        command = [str(OUTPUT / 'probe.exe'), str(folder), str(target)]
        if mode == 'aborted':
            process.kill()
            process.wait(timeout=5)
            time.sleep(0.3)
        if mode in ('no-selection', 'aborted'):
            command.append('--find-only')
        probe = subprocess.run(command,
            capture_output=True, text=True, env=environment, timeout=15,
            creationflags=subprocess.CREATE_NO_WINDOW)
        print(probe.stdout, flush=True)
        if probe.returncode:
            raise RuntimeError(f'Native probe failed: {probe.stderr}')
        stdout, stderr = process.communicate(timeout=15)
        after_probe = subprocess.run([str(OUTPUT / 'probe.exe'), str(folder), str(target), '--find-only'],
            capture_output=True, text=True, env=environment, timeout=10,
            creationflags=subprocess.CREATE_NO_WINDOW)
        result = dict(mode=mode, fixture=str(fixture), folder=str(folder), target=str(target),
            profile=environment['LOCALAPPDATA'], elapsed_ms=round((time.perf_counter()-started)*1000),
            exit_code=process.returncode, events=[ready] + stdout.splitlines(), stderr=stderr,
            probe=probe.stdout, after_probe=after_probe.stdout,
            registration_revoked='FIND class=1 hr=00000001' in after_probe.stdout,
            registry_unchanged=registry_snapshot() == before)
        (OUTPUT / f'result-{mode}.json').write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
        print(json.dumps(result, ensure_ascii=False, indent=2))
        expected = 'NO_SELECTION' if mode == 'no-selection' else 'SELECTED'
        if ((mode != 'aborted' and (process.returncode or expected not in stdout)) or not result['registry_unchanged']
                or not result['registration_revoked']):
            raise RuntimeError('Live shell selection probe failed; inspect result-' + mode + '.json')
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        after = registry_snapshot()
        (OUTPUT / 'registry-after.json').write_text(json.dumps(after, ensure_ascii=False, indent=2), encoding='utf-8')
        if after != before:
            raise RuntimeError('Folder association values changed during read-only probe')


if __name__ == '__main__':
    main()
