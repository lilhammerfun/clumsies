#!/usr/bin/env python3
"""Temporarily fail this worktree's Local Dev HTTP server; restore it on exit."""

import argparse
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import runpy
import signal
import subprocess
import time


def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.PIPE).strip()


def exercise(mode, seconds):
    repo = Path(__file__).resolve().parent.parent
    lab = runpy.run_path(str(repo / "dev/seed-review-playground.py"))["Playground"]()
    root = lab.root
    lock = root.parent / ".locks" / lab.instance
    # Use the lifecycle runner's ownership checks before touching any process.
    run("sh", str(repo / "dev/dev-instance.sh"), "status")
    lock.mkdir(mode=0o700)
    (lock / "pid").write_text(str(os.getpid()))
    runtime = lab.runtime
    job = f"gui/{os.getuid()}/" + runtime["identities"]["server_launch_agent_label"]
    plist = runtime["paths"]["server_launch_agent_plist"]
    stopped = False
    http = None

    def interrupt(*_):
        raise KeyboardInterrupt

    def restore():
        run("launchctl", "bootstrap", f"gui/{os.getuid()}", plist)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            match = re.search(r"^\s*pid = (\d+)\s*$", run("launchctl", "print", job), re.M)
            if match:
                pid = match[1]
                if run("ps", "-p", pid, "-o", "command=") == runtime["paths"]["server_binary"]:
                    runtime["processes"] = {"server_pid": int(pid),
                        "server_start_identity": run("ps", "-p", pid, "-o", "lstart=")}
                    runtime["updated_at"] = datetime.now(timezone.utc).isoformat()
                    temporary = root / "runtime.fault.tmp"
                    temporary.write_text(json.dumps(runtime, indent=2) + "\n")
                    temporary.chmod(0o600)
                    temporary.replace(root / "runtime.json")
                    try:
                        run("sh", str(repo / "dev/dev-instance.sh"), "status")
                        print("Restored: original Server healthy; App and daemon kept running.", flush=True)
                        return
                    except subprocess.CalledProcessError:
                        pass
            time.sleep(0.2)
        raise RuntimeError("Server recovery did not complete. Run just dev-macos.")

    try:
        # Revalidate under the shared lifecycle lock, including PID start identity.
        run("sh", str(repo / "dev/dev-instance.sh"), "status")
        signal.signal(signal.SIGINT, interrupt)
        signal.signal(signal.SIGTERM, interrupt)
        run("launchctl", "bootout", job)
        stopped = True
        deadline = time.monotonic() + 10
        while subprocess.run(["kill", "-0", str(runtime["processes"]["server_pid"])],
                             stderr=subprocess.DEVNULL).returncode == 0:
            if time.monotonic() > deadline:
                raise RuntimeError("Owned Server did not stop; refusing to replace its listener.")
            time.sleep(0.1)
        if mode != "offline":
            status = int(mode)

            class Failure(BaseHTTPRequestHandler):
                def do_GET(self):
                    body = json.dumps({"error": {"code": "qa_failure",
                        "message": "QA_RAW_ERROR_MUST_NOT_APPEAR",
                        "request_id": "req_qa_private"}}).encode()
                    self.send_response(status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.send_header("Connection", "close")
                    self.end_headers()
                    self.wfile.write(body)
                    self.close_connection = True

                do_POST = do_PUT = do_PATCH = do_DELETE = do_GET

                def log_message(self, *_):
                    pass  # Never log credentials, bodies, or user-controlled request paths.

            http = ThreadingHTTPServer(("127.0.0.1", runtime["ports"]["server"]), Failure)
            http.timeout = 0.2
        print(f"Fault active: {mode}; instance {lab.instance}; {seconds}s or Ctrl-C to restore.", flush=True)
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if http:
                http.handle_request()
            else:
                time.sleep(0.2)
    except KeyboardInterrupt:
        pass
    finally:
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        try:
            if http:
                http.server_close()
            if stopped:
                restore()
        finally:
            (lock / "pid").unlink()
            lock.rmdir()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["offline", "400", "404", "500"])
    parser.add_argument("--seconds", type=int, default=60)
    args = parser.parse_args()
    if not 1 <= args.seconds <= 600:
        parser.error("--seconds must be between 1 and 600")
    exercise(args.mode, args.seconds)
