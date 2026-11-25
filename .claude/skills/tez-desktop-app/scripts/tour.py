"""Drive tez-app through its own `go` port and capture its window between steps. Windows only.

No clicks or typing: each `go:<target>` is a line sent to the app's single-instance port
(127.0.0.1:7978), which `navigate()` in app/src/main.rs handles. Captures use snap.py.

Usage: python -I tour.py <out_dir> <step>...
  go:<target>     e.g. go:services:redis, go:theme:light, go:site:hello:logs
  wait:<seconds>  let the app re-render or a switch settle
  snap:<name>     write <out_dir>/<name>.png

Some targets change real state (flip:..., theme:...). See SKILL.md before using them.
"""
import os
import socket
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PORT = 7978


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    t0 = time.time()
    for step in sys.argv[2:]:
        kind, _, arg = step.partition(":")
        if kind == "go":
            with socket.create_connection(("127.0.0.1", PORT), timeout=2) as s:
                s.sendall(f"go {arg}".encode())
            t0 = time.time()
        elif kind == "wait":
            time.sleep(float(arg))
        elif kind == "snap":
            path = os.path.join(out, f"{arg}.png")
            r = subprocess.run([sys.executable, "-I", os.path.join(HERE, "snap.py"), path],
                               capture_output=True, text=True)
            if r.returncode != 0:
                sys.exit(f"{arg}: {r.stderr.strip() or r.stdout.strip()}")
            print(f"{arg}: {path} ({time.time() - t0:.2f}s after the last go)")
        else:
            sys.exit(f"unknown step {step!r}; use go:, wait: or snap:")


if __name__ == "__main__":
    main()
