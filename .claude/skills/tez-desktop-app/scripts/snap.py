"""Capture ONLY tez-app's own window into a PNG with PrintWindow (PW_RENDERFULLCONTENT). Windows only.

Read-only: it never clicks, types, moves, focuses or raises anything, and it refuses to capture
unless a visible, non-minimised window titled exactly "tez" belongs to a tez-app.exe process.
Needs Pillow.

Usage: python -I snap.py <out.png>
"""
import ctypes
import ctypes.wintypes as wt
import subprocess
import sys

from PIL import Image

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32", use_last_error=True)
# Physical pixels. Without this Windows reports scaled-down window sizes and the capture is cropped.
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # PER_MONITOR_AWARE_V2
user32.GetDC.restype = wt.HDC
user32.GetDC.argtypes = [wt.HWND]
user32.ReleaseDC.argtypes = [wt.HWND, wt.HDC]
user32.PrintWindow.argtypes = [wt.HWND, wt.HDC, wt.UINT]
user32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(wt.RECT)]
user32.IsIconic.argtypes = [wt.HWND]
user32.IsWindowVisible.argtypes = [wt.HWND]
user32.GetWindowThreadProcessId.argtypes = [wt.HWND, ctypes.POINTER(wt.DWORD)]
user32.GetWindowTextW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
gdi32.CreateCompatibleDC.restype = wt.HDC
gdi32.CreateCompatibleDC.argtypes = [wt.HDC]
gdi32.CreateCompatibleBitmap.restype = wt.HBITMAP
gdi32.CreateCompatibleBitmap.argtypes = [wt.HDC, ctypes.c_int, ctypes.c_int]
gdi32.SelectObject.restype = wt.HGDIOBJ
gdi32.SelectObject.argtypes = [wt.HDC, wt.HGDIOBJ]
gdi32.DeleteObject.argtypes = [wt.HGDIOBJ]
gdi32.DeleteDC.argtypes = [wt.HDC]
gdi32.GetDIBits.argtypes = [wt.HDC, wt.HBITMAP, wt.UINT, wt.UINT, ctypes.c_void_p, ctypes.c_void_p, wt.UINT]

PW_RENDERFULLCONTENT = 2


class BIH(ctypes.Structure):
    _fields_ = [("biSize", wt.DWORD), ("biWidth", wt.LONG), ("biHeight", wt.LONG), ("biPlanes", wt.WORD),
                ("biBitCount", wt.WORD), ("biCompression", wt.DWORD), ("biSizeImage", wt.DWORD),
                ("biXPelsPerMeter", wt.LONG), ("biYPelsPerMeter", wt.LONG), ("biClrUsed", wt.DWORD),
                ("biClrImportant", wt.DWORD)]


def tez_pids():
    out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq tez-app.exe", "/FO", "CSV", "/NH"],
                         capture_output=True, text=True, creationflags=0x0800_0000).stdout
    return {int(line.split('","')[1]) for line in out.splitlines() if line.startswith('"tez-app.exe"')}


def find_window(pids):
    found = []
    enum_proc = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def check(hwnd, _):
        pid = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if pid.value in pids and user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(64)
            user32.GetWindowTextW(hwnd, buf, 64)
            if buf.value == "tez":
                found.append(hwnd)
        return True

    user32.EnumWindows(enum_proc(check), 0)
    return found[0] if found else None


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: python -I snap.py <out.png>")
    pids = tez_pids()
    hwnd = find_window(pids) if pids else None
    if not hwnd:
        sys.exit("no visible tez window; nothing captured")
    if user32.IsIconic(hwnd):
        sys.exit("the tez window is minimised; nothing captured")
    r = wt.RECT()
    user32.GetWindowRect(hwnd, ctypes.byref(r))
    w, h = r.right - r.left, r.bottom - r.top
    wdc = user32.GetDC(hwnd)
    mdc = gdi32.CreateCompatibleDC(wdc)
    bmp = gdi32.CreateCompatibleBitmap(wdc, w, h)
    gdi32.SelectObject(mdc, bmp)
    ok = user32.PrintWindow(hwnd, mdc, PW_RENDERFULLCONTENT)
    bih = BIH(ctypes.sizeof(BIH), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bih), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mdc)
    user32.ReleaseDC(hwnd, wdc)
    Image.frombuffer("RGBA", (w, h), buf.raw, "raw", "BGRA", 0, 1).convert("RGB").save(sys.argv[1])
    print(f"captured {w}x{h} (PrintWindow ok={bool(ok)})")


if __name__ == "__main__":
    main()
