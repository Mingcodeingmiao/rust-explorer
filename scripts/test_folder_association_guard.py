"""Windows process integration tests. Debug-only isolated registry; never live Classes."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import uuid
import winreg


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--runtime-dir", type=Path, required=True)
    args = parser.parse_args()
    workspace = Path(tempfile.mkdtemp(prefix="RustExplorer-association-guard-"))
    registry_root = "Software\\RustExplorer\\AssociationTests\\guard-" + uuid.uuid4().hex
    env = os.environ.copy()
    env["LOCALAPPDATA"] = str(workspace / "profile")
    env["RUST_EXPLORER_ASSOCIATION_TEST_ROOT"] = registry_root
    app = workspace / "app"
    app.mkdir()
    exe = app / "RustExplorer-0.8.11-windows-x64.exe"
    shutil.copy2(args.exe, exe)
    for name in ("libgcc_s_seh-1.dll", "libwinpthread-1.dll", "Everything64.dll"):
        source = args.runtime_dir / name
        if source.is_file():
            shutil.copy2(source, app / name)
    recovery = workspace / "profile" / "RustExplorer" / "folder-association"

    def run(*arguments, executable=None, success=True):
        result = subprocess.run([str(executable or exe), *arguments], env=env,
                                capture_output=True, timeout=20,
                                creationflags=subprocess.CREATE_NO_WINDOW)
        if success and result.returncode:
            raise AssertionError(result.stderr.decode("utf-8", "replace"))
        return result

    def read(class_name):
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, registry_root + "\\" + class_name + "\\shell") as key:
                return winreg.QueryValueEx(key, "")
        except FileNotFoundError:
            return None

    def assert_original():
        assert read("Directory") == ("OriginalBrowser", winreg.REG_EXPAND_SZ)
        assert read("Drive") is None

    def remove_test_key(path):
        assert path.startswith(registry_root) and registry_root.startswith("Software\\RustExplorer\\AssociationTests\\guard-")
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path, 0, winreg.KEY_READ | winreg.KEY_WRITE) as key:
                while True:
                    try:
                        child = winreg.EnumKey(key, 0)
                    except OSError:
                        break
                    remove_test_key(path + "\\" + child)
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, path)
        except FileNotFoundError:
            pass

    tested_scope = False
    try:
        diagnostics = json.loads(run("--folder-association-status").stdout)
        assert diagnostics["registry_scope"] == registry_root, "Refusing to run: executable does not support isolated debug registry"
        tested_scope = True
        with winreg.CreateKey(winreg.HKEY_CURRENT_USER, registry_root + "\\Directory\\shell") as key:
            winreg.SetValueEx(key, "", 0, winreg.REG_EXPAND_SZ, "OriginalBrowser")
        original = read("Directory")
        started = time.monotonic()
        run("--test-folder-browser", "30")
        assert read("Directory")[0] == "RustExplorer.Open", f"Trial inactive after {time.monotonic()-started:.2f}s; pending={list(recovery.glob('*'))}; value={read('Directory')}"
        helper = recovery / "recovery" / "rust-explorer.exe"
        assert helper.is_file() and (recovery / "restore.reg").is_file()
        # The launching CLI is already gone; even removing its owned EXE cannot
        # prevent the independently copied process from restoring associations.
        unavailable = exe.with_suffix(".unavailable")
        exe.rename(unavailable)
        deadline = time.monotonic() + 40
        while (recovery / "trial.json").exists() and time.monotonic() < deadline:
            time.sleep(0.2)
        assert not (recovery / "trial.json").exists(), "Guard failed to restore expired trial"
        assert_original()
        unavailable.rename(exe)
        print("PASS: launching process exited + original EXE unavailable -> automatic recovery")

        run("--test-folder-browser", "12")
        deadline = json.loads((recovery / "trial.json").read_bytes())["deadline"]
        run("--confirm-folder-browser")
        assert not (recovery / "trial.json").exists()
        time.sleep(max(0, deadline - time.time()) + 1)
        diagnostics = json.loads(run("--folder-association-status").stdout)
        assert not diagnostics["trial"]
        for class_name in ("Directory", "Drive"):
            assert read(class_name)[0] == "RustExplorer.Open"
            command_key = registry_root + "\\" + class_name + "\\shell\\RustExplorer.Open\\command"
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, command_key) as key:
                command = winreg.QueryValueEx(key, "")[0]
                assert Path(command.split('"')[1]).resolve() == exe.resolve()
        run("--restore-folder-browser", executable=helper)
        assert_original()
        run("--test-folder-browser", "12")
        run("--confirm-folder-browser")
        run("--restore-folder-browser", executable=helper)
        assert_original()
        print("PASS: versioned EXE stays associated past deadline; independent restore and re-enable")

        (recovery / "backup.json").write_bytes(b"broken backup")
        assert run("--test-folder-browser", "8", success=False).returncode != 0
        assert_original()
        (recovery / "backup.json").unlink()
        print("PASS: corrupt backup fails closed")
        assert original == read("Directory")
    finally:
        if tested_scope:
            helper = recovery / "recovery" / "rust-explorer.exe"
            if (recovery / "backup.json").exists() and helper.is_file():
                result = run("--restore-folder-browser", executable=helper, success=False)
                if result.returncode:
                    raise RuntimeError(f"Recovery requires attention; fixtures retained: {workspace}")
            remove_test_key(registry_root)
        # Remove only the resolved directory created by tempfile above.
        assert workspace.resolve().parent == Path(tempfile.gettempdir()).resolve()
        assert workspace.name.startswith("RustExplorer-association-guard-")
        for _ in range(20):
            try:
                shutil.rmtree(workspace)
                break
            except PermissionError:
                time.sleep(0.2)
        else:
            raise RuntimeError(f"Helper still in use; owned fixtures retained: {workspace}")


if __name__ == "__main__":
    main()
