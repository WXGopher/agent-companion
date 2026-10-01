"""Validate a vendor ZIP before ditto extraction. No installation side effects."""
import pathlib
import posixpath
import stat
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    members = archive.infolist()
    if not members or len(members) > 200000 or sum(item.file_size for item in members) > 8 * 1024 ** 3:
        raise ValueError("Invalid application archive size")
    for item in members:
        path = pathlib.PurePosixPath(item.filename)
        if path.is_absolute() or ".." in path.parts or "\\" in item.filename or not path.parts:
            raise ValueError("Unsafe archive path")
        if not path.parts[0].endswith(".app") and path.parts[0] != "__MACOSX":
            raise ValueError("Unexpected archive root")
        if stat.S_ISLNK(item.external_attr >> 16):
            target = archive.read(item).decode("utf-8")
            resolved = pathlib.PurePosixPath(posixpath.normpath(str(path.parent / target)))
            if resolved.is_absolute() or ".." in resolved.parts or resolved.parts[0] != path.parts[0]:
                raise ValueError("Archive symlink leaves application")
