"""Live Shell bridge probe on owned fixtures; does not change folder associations.

Usage: python scripts/test_shell_bridge.py PATH_TO_EXE [immediate|delayed|no-selection]
Requires the native probe built by experiments/shell-bridge/build.ps1.
"""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('probe_helpers',ROOT/'experiments/shell-bridge/test_bridge.py')
helpers=importlib.util.module_from_spec(spec)
spec.loader.exec_module(helpers)
exe=Path(sys.argv[1])
mode=sys.argv[2] if len(sys.argv)>2 else 'immediate'
if mode not in ('immediate','delayed','no-selection'):
    raise ValueError('Unknown test mode')
fixture=Path(tempfile.mkdtemp(prefix='RE-integrated-shell-'))
folder=fixture/'中文 空格目录'
folder.mkdir()
target=folder/'选中 文件,01.txt'
target.write_text('Owned integrated bridge fixture.\n',encoding='utf-8')
environment=dict(os.environ)
environment['LOCALAPPDATA']=str(fixture/'profile')
environment['PATH']=r'C:\msys64\ucrt64\bin;'+environment['PATH']
before=helpers.registry_snapshot()
output=ROOT/'artifacts/shell-bridge-integration'
output.mkdir(parents=True,exist_ok=True)
probe=ROOT/'artifacts/shell-bridge-experiment/probe.exe'
start=time.perf_counter()
process=subprocess.Popen([str(exe),'--open-folder',str(folder)],env=environment,
    stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
result={}
try:
    registered=False
    for _ in range(30):
        found=subprocess.run([str(probe),str(folder),str(target),'--find-only'],env=environment,
            capture_output=True,text=True,timeout=5,creationflags=subprocess.CREATE_NO_WINDOW)
        if 'FIND class=1 hr=00000000' in found.stdout:
            registered=True
            break
        time.sleep(.05)
    if not registered:
        raise RuntimeError('Integrated bridge did not register')
    if mode=='delayed':
        time.sleep(1.2)
    if mode!='no-selection':
        selection=subprocess.run([str(probe),str(folder),str(target)],env=environment,
            capture_output=True,text=True,timeout=15,creationflags=subprocess.CREATE_NO_WINDOW)
        if selection.returncode:
            raise RuntimeError(selection.stderr)
    try:
        stdout,stderr=process.communicate(timeout=12)
    except subprocess.TimeoutExpired as error:
        print('Timeout: process status=',process.poll(),'stderr=',error.stderr,flush=True)
        raise
    after=subprocess.run([str(probe),str(folder),str(target),'--find-only'],env=environment,
        capture_output=True,text=True,timeout=5,creationflags=subprocess.CREATE_NO_WINDOW)
    result=dict(mode=mode,exe=str(exe),folder=str(folder),target=str(target),profile=environment['LOCALAPPDATA'],
        exit_code=process.returncode,elapsed_ms=round((time.perf_counter()-start)*1000),stdout=stdout,stderr=stderr,
        registry_unchanged=helpers.registry_snapshot()==before,
        registration_revoked='FIND class=1 hr=00000001' in after.stdout)
    if process.returncode or not result['registry_unchanged'] or not result['registration_revoked']:
        raise RuntimeError(result)
finally:
    if process.poll() is None:
        process.kill()
        process.wait(timeout=5)
    (output/f'{mode}.json').write_text(json.dumps(result,ensure_ascii=False,indent=2),encoding='utf-8')
print(json.dumps(result,ensure_ascii=False,indent=2))
