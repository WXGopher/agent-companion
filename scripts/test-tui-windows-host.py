#!/usr/bin/env python3
"""Native standard-user CI host; never used by the installed launchers.

Called through WMI outside the runner job on an ephemeral GitHub Windows VM.
Load the disposable user's profile explicitly and launch with its primary token,
avoiding the process-tree job used by the Secondary Logon/profile launcher.
"""
import ctypes
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import subprocess
import sys
import traceback


def start(root):
    if os.name != "nt":
        raise RuntimeError("This host is only for disposable GitHub Windows runners")
    import msvcrt

    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    security = ctypes.WinDLL("advapi32", use_last_error=True)
    profiles = ctypes.WinDLL("userenv", use_last_error=True)

    class Profile(ctypes.Structure):
        _fields_ = [("size", w.DWORD), ("flags", w.DWORD), ("user", w.LPWSTR),
                    ("path", w.LPWSTR), ("default", w.LPWSTR),
                    ("server", w.LPWSTR), ("policy", w.LPWSTR), ("handle", w.HANDLE)]

    class Startup(ctypes.Structure):
        _fields_ = [("size", w.DWORD), ("reserved", w.LPWSTR), ("desktop", w.LPWSTR),
                    ("title", w.LPWSTR), ("x", w.DWORD), ("y", w.DWORD),
                    ("width", w.DWORD), ("height", w.DWORD), ("chars_x", w.DWORD),
                    ("chars_y", w.DWORD), ("fill", w.DWORD), ("flags", w.DWORD),
                    ("show", w.WORD), ("reserved_size", w.WORD),
                    ("reserved_data", ctypes.POINTER(ctypes.c_byte)),
                    ("stdin", w.HANDLE), ("stdout", w.HANDLE), ("stderr", w.HANDLE)]

    class Process(ctypes.Structure):
        _fields_ = [("process", w.HANDLE), ("thread", w.HANDLE),
                    ("pid", w.DWORD), ("tid", w.DWORD)]

    class Luid(ctypes.Structure):
        _fields_ = [("low", w.DWORD), ("high", w.LONG)]

    class Privilege(ctypes.Structure):
        _fields_ = [("count", w.DWORD), ("luid", Luid), ("attributes", w.DWORD)]

    kernel.GetCurrentProcess.restype = w.HANDLE
    kernel.IsProcessInJob.argtypes = [w.HANDLE, w.HANDLE, ctypes.POINTER(w.BOOL)]
    kernel.IsProcessInJob.restype = w.BOOL
    kernel.CloseHandle.argtypes = [w.HANDLE]
    kernel.WaitForSingleObject.argtypes = [w.HANDLE, w.DWORD]
    kernel.GetExitCodeProcess.argtypes = [w.HANDLE, ctypes.POINTER(w.DWORD)]
    security.LogonUserW.argtypes = [w.LPCWSTR, w.LPCWSTR, w.LPCWSTR, w.DWORD,
                                   w.DWORD, ctypes.POINTER(w.HANDLE)]
    security.LogonUserW.restype = w.BOOL
    profiles.LoadUserProfileW.argtypes = [w.HANDLE, ctypes.POINTER(Profile)]
    profiles.LoadUserProfileW.restype = w.BOOL
    profiles.UnloadUserProfile.argtypes = [w.HANDLE, w.HANDLE]
    profiles.CreateEnvironmentBlock.argtypes = [ctypes.POINTER(ctypes.c_void_p), w.HANDLE, w.BOOL]
    profiles.CreateEnvironmentBlock.restype = w.BOOL
    profiles.DestroyEnvironmentBlock.argtypes = [ctypes.c_void_p]
    security.CreateProcessAsUserW.argtypes = [w.HANDLE, w.LPCWSTR, w.LPWSTR,
                                            ctypes.c_void_p, ctypes.c_void_p, w.BOOL,
                                            w.DWORD, ctypes.c_void_p, w.LPCWSTR,
                                            ctypes.POINTER(Startup), ctypes.POINTER(Process)]
    security.CreateProcessAsUserW.restype = w.BOOL
    security.OpenProcessToken.argtypes = [w.HANDLE, w.DWORD, ctypes.POINTER(w.HANDLE)]
    security.LookupPrivilegeValueW.argtypes = [w.LPCWSTR, w.LPCWSTR, ctypes.POINTER(Luid)]
    security.AdjustTokenPrivileges.argtypes = [w.HANDLE, w.BOOL, ctypes.POINTER(Privilege),
                                             w.DWORD, ctypes.c_void_p, ctypes.c_void_p]
    security.CreateProcessWithTokenW.argtypes = [w.HANDLE, w.DWORD, w.LPCWSTR, w.LPWSTR,
                                                w.DWORD, ctypes.c_void_p, w.LPCWSTR,
                                                ctypes.POINTER(Startup), ctypes.POINTER(Process)]
    security.CreateProcessWithTokenW.restype = w.BOOL

    def checked(value, operation):
        if not value:
            raise ctypes.WinError(ctypes.get_last_error(), operation)

    credential = root / "credential.json"
    login = json.loads(credential.read_text(encoding="utf-8"))
    credential.unlink()
    launch = json.loads((root / "launch.json").read_text(encoding="utf-8"))
    if launch.get("github_actions") != "true":
        raise RuntimeError("This host requires a disposable GitHub runner manifest")
    user = login["user"].split("\\", 1)[-1]
    domain = login["user"].split("\\", 1)[0]
    token = w.HANDLE()
    profile = Profile(size=ctypes.sizeof(Profile), flags=1, user=user)
    native_environment = ctypes.c_void_p()
    child = Process()
    loaded = False
    try:
        in_job = w.BOOL()
        checked(kernel.IsProcessInJob(kernel.GetCurrentProcess(), None, ctypes.byref(in_job)), "IsProcessInJob(host)")
        print(f"Native CI host in job: {bool(in_job.value)}", flush=True)
        # Enable only rights already granted to the CI service's admin token.
        # No machine policy or runner job is modified.
        caller = w.HANDLE()
        checked(security.OpenProcessToken(kernel.GetCurrentProcess(), 0x28, ctypes.byref(caller)), "OpenProcessToken")
        try:
            for name in ("SeBackupPrivilege", "SeRestorePrivilege", "SeIncreaseQuotaPrivilege",
                         "SeAssignPrimaryTokenPrivilege", "SeImpersonatePrivilege"):
                privilege = Privilege(count=1, attributes=2)
                checked(security.LookupPrivilegeValueW(None, name, ctypes.byref(privilege.luid)), "LookupPrivilegeValueW")
                ctypes.set_last_error(0)
                checked(security.AdjustTokenPrivileges(caller, False, ctypes.byref(privilege), 0, None, None), "AdjustTokenPrivileges")
                if ctypes.get_last_error() == 1300:
                    print(f"CI token has no {name}", flush=True)
        finally:
            kernel.CloseHandle(caller)
        checked(security.LogonUserW(user, domain, login["password"], 2, 0, ctypes.byref(token)), "LogonUserW")
        login = None
        checked(profiles.LoadUserProfileW(token, ctypes.byref(profile)), "LoadUserProfileW")
        loaded = True
        checked(profiles.CreateEnvironmentBlock(ctypes.byref(native_environment), token, False), "CreateEnvironmentBlock")
        environment = {}
        address = native_environment.value
        while True:
            item = ctypes.wstring_at(address)
            if not item:
                break
            key, value = item.split("=", 1)
            environment[key] = value
            address += (len(item) + 1) * ctypes.sizeof(ctypes.c_wchar)
        temp = root / "tmp"
        temp.mkdir()
        environment = {key: value for key, value in environment.items()
                       if key.upper() not in {"PATH", "TEMP", "TMP", "PYTHONIOENCODING", "GITHUB_ACTIONS"}}
        environment.update(Path=launch["path"], TEMP=str(temp), TMP=str(temp),
                           PYTHONIOENCODING="utf-8", GITHUB_ACTIONS="true")
        block = ctypes.create_unicode_buffer("\0".join(f"{key}={value}" for key, value in sorted(environment.items(), key=lambda pair: pair[0].upper())) + "\0\0")
        command_text = subprocess.list2cmdline([
            launch["python"], launch["script"], "--companion", launch["companion"],
            "--work-dir", launch["fixture"]] + (["--preflight-only"] if launch.get("preflight_only") else []))
        command = ctypes.create_unicode_buffer(command_text)
        with open(os.devnull, "rb") as stdin, (root / "stdout.log").open("wb") as stdout, (root / "stderr.log").open("wb") as stderr:
            handles = [msvcrt.get_osfhandle(file.fileno()) for file in (stdin, stdout, stderr)]
            for handle in handles:
                os.set_handle_inheritable(handle, True)
            startup = Startup(size=ctypes.sizeof(Startup), flags=0x100,
                              stdin=handles[0], stdout=handles[1], stderr=handles[2])
            # The caller owns no restrictive job. Explicit breakaway also makes
            # any unexpected CI host containment fail before package downloads.
            flags = subprocess.CREATE_BREAKAWAY_FROM_JOB | subprocess.DETACHED_PROCESS | 0x400
            created = security.CreateProcessAsUserW(token, launch["python"], command,
                                                     None, None, True, flags, block, str(root),
                                                     ctypes.byref(startup), ctypes.byref(child))
            if not created and ctypes.get_last_error() == 1314:
                # Admin tokens may have impersonation but no assign-primary
                # right. The profile is already loaded; do not request the
                # Secondary Logon profile-lifetime job here.
                print("Using CreateProcessWithTokenW with the already-loaded profile", flush=True)
                # This API creates a new console by default. DETACHED_PROCESS
                # cannot be combined with that flag. The caller is outside a
                # job already; use the documented ordinary console creation.
                assert not in_job.value, "Native token host must be outside a process job"
                command = ctypes.create_unicode_buffer(command_text)
                created = security.CreateProcessWithTokenW(token, 0, launch["python"], command,
                                                          0x400 | subprocess.CREATE_NEW_CONSOLE, block, str(root),
                                                          ctypes.byref(startup), ctypes.byref(child))
            checked(created, "Create standard-user process")
            checked(kernel.IsProcessInJob(child.process, None, ctypes.byref(in_job)), "IsProcessInJob(standard user)")
            print(f"Native standard-user process in job: {bool(in_job.value)}", flush=True)
            if kernel.WaitForSingleObject(child.process, 24 * 60 * 1000) != 0:
                raise TimeoutError("Native standard-user acceptance did not finish")
            result = w.DWORD()
            checked(kernel.GetExitCodeProcess(child.process, ctypes.byref(result)), "GetExitCodeProcess")
            return result.value
    finally:
        for handle in (child.thread, child.process):
            if handle:
                kernel.CloseHandle(handle)
        if native_environment:
            profiles.DestroyEnvironmentBlock(native_environment)
        if loaded:
            profiles.UnloadUserProfile(token, profile.handle)
        if token:
            kernel.CloseHandle(token)


def main():
    root = Path(sys.argv[1]).resolve(strict=True)
    result = 1
    with (root / "acceptance.log").open("w", encoding="utf-8") as log:
        sys.stdout = sys.stderr = log
        try:
            result = start(root)
        except Exception:
            traceback.print_exc()
        finally:
            pending = root / "result.pending"
            pending.write_text(str(result), encoding="ascii")
            pending.replace(root / "result")
    return result


if __name__ == "__main__":
    sys.exit(main())
