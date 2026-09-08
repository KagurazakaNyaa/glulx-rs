"""Check that Windows can extract both shell icon sizes from the release exe."""

import ctypes
from ctypes import wintypes
import pathlib
import sys


def main():
    binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
    shell32 = ctypes.WinDLL("shell32", use_last_error=True)
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    extract = shell32.ExtractIconExW
    extract.argtypes = [
        wintypes.LPCWSTR,
        ctypes.c_int,
        ctypes.POINTER(wintypes.HICON),
        ctypes.POINTER(wintypes.HICON),
        wintypes.UINT,
    ]
    extract.restype = wintypes.UINT
    destroy = user32.DestroyIcon
    destroy.argtypes = [wintypes.HICON]
    destroy.restype = wintypes.BOOL
    large, small = wintypes.HICON(), wintypes.HICON()
    try:
        count = extract(str(binary), 0, ctypes.byref(large), ctypes.byref(small), 1)
        if count != 1 or not large.value or not small.value:
            raise SystemExit(f"Missing embedded large/small application icons: {binary}")
        print(f"Windows extracted large and small application icons: {binary}")
    finally:
        for icon in (large, small):
            if icon.value:
                destroy(icon)


if __name__ == "__main__":
    main()
