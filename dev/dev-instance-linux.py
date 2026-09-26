#!/usr/bin/env python3
"""The Linux counterpart of dev/dev-instance.sh: one isolated instance per worktree.

Same conventions, same layout, same commands. An instance is addressed by the
hash of its worktree path and owns everything it needs: ports, a database, a
Server, a daemon root, a cache, its secrets and its logs. The Setup Code is
generated here and read back from the instance's compose.env, so it is never
something a developer has to know.

This is a separate file rather than a branch of dev-instance.sh because that
script is macOS mechanics end to end -- launchd, Keychain, a signed .app,
Xcode. The two share the layout and dev/dev-server.sh, and differ only in what
supervises the processes, which is the part that cannot be shared.

    dev/dev-instance-linux.py up [--seed-memory] [--no-client]
    dev/dev-instance-linux.py sign-in [--server-url URL] [--setup-code CODE]
    dev/dev-instance-linux.py status | logs | down | reset

Containers may need elevated rights (Omarchy keeps users out of the docker
group on purpose). When that is the case, up prints the one command to run and
waits for the ports instead of failing.
"""

import argparse
import base64
import hashlib
import http.cookiejar
import json
import os
import secrets
import shutil
import signal
import socket
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAX_FRAME_BYTES = 64 * 1024 * 1024

SEED_DOCUMENTS = [
    (
        "CLUMSIES.md",
        "Memory Guidelines",
        """---
title: Memory Guidelines
kind: guidelines
---

# Memory Guidelines

This Project keeps its durable knowledge in Clumsies. Keep facts, constraints
and procedures that a future task would otherwise have to rediscover.

## What belongs here

- Decisions with their reasons, not just their outcome.
- Constraints that come from outside the code, such as a release window.
- Procedures a new teammate could follow without asking.

## What does not

- Anything the repository already states, such as build commands.
- Task state. That belongs to the task, not to the Project.
""",
    ),
    (
        "knowledge/architecture.md",
        "Client architecture",
        """---
title: Client architecture
kind: knowledge
---

# Client architecture

Each platform has its own native client, and they share one local engine
instead of one user interface: **macOS keeps its Swift app, Windows and Linux
share the Rust client.**

## Why

The engine - drafts, synchronization, retrieval - is where the product's
complexity lives, and it is already portable Rust. A user interface is the part
that must feel native, and a single cross-platform toolkit gives that up on
every platform at once.

## The seam

Clients never talk to the organization Server directly. They ask the local
engine over a local socket, and the engine holds the session. A client that
crashes cannot leak a token it never had.
""",
    ),
    (
        "procedures/release.md",
        "Release checklist",
        """---
title: Release checklist
kind: procedure
---

# Release checklist

1. Confirm the version to publish.
2. Freeze the release pipeline.
3. Run the packaged tests, not the unit tests.
4. Publish, then watch the error rate for five minutes.
5. Record the release and the person who ran it.

> When in doubt, stop. A delayed release costs less than a bad one.
""",
    ),
]


# --- the instance ---------------------------------------------------------


class Instance:
    def __init__(self):
        repo_root = os.path.realpath(REPO_ROOT)
        self.instance_id = hashlib.sha256(repo_root.encode()).hexdigest()[:12]
        default_root = os.path.join(
            os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")),
            "ai.clumsies.dev",
        )
        self.dev_root = os.environ.get("CLUMSIES_DEV_ROOT", default_root)
        if not os.path.isabs(self.dev_root):
            raise SystemExit("CLUMSIES_DEV_ROOT must be absolute")
        self.worktree = repo_root
        self.root = os.path.join(self.dev_root, "instances", self.instance_id)
        self.compose_env = os.path.join(self.root, "compose.env")
        self.runtime = os.path.join(self.root, "runtime.json")
        self.ready = os.path.join(self.root, "server-ready.json")
        self.logs = os.path.join(self.root, "logs")
        self.daemon_root = os.path.join(self.root, "daemon")
        self.cache = os.path.join(self.root, "cache")
        self.bin = os.path.join(self.root, "bin")
        self.server_pid = os.path.join(self.root, "server.pid")
        self.daemon_pid = os.path.join(self.root, "daemon.pid")
        self.compose_project = f"clumsies-dev-{self.instance_id}"
        self.server_binary = os.path.join(self.bin, "clumsies-server")
        self.daemon_binary = os.path.join(REPO_ROOT, "target", "debug", "clumsiesd")
        self.client_binary = os.path.join(REPO_ROOT, "target", "debug", "clumsies-desktop")
        self.client_pid = os.path.join(self.root, "client.pid")

    # -- files

    def create(self):
        for path in (self.root, self.logs, self.daemon_root, self.cache, self.bin):
            os.makedirs(path, mode=0o700, exist_ok=True)

    def env(self):
        values = {}
        if os.path.exists(self.compose_env):
            with open(self.compose_env) as handle:
                for line in handle:
                    if "=" in line:
                        key, value = line.strip().split("=", 1)
                        values[key] = value
        for key, make in (
            ("CLUMSIES_DB_PASSWORD", lambda: secrets.token_hex(24)),
            ("CLUMSIES_SETUP_CODE", lambda: secrets.token_hex(24)),
            ("CLUMSIES_DB_PORT", free_port),
            ("CLUMSIES_OIDC_PORT", free_port),
        ):
            values.setdefault(key, make())
        values["CLUMSIES_HOST_BIND_ADDRESS"] = "127.0.0.1"
        values["CLUMSIES_DB_NAME"] = "clumsies"
        values["CLUMSIES_DB_USER"] = "clumsies"
        return values

    def write_env(self, values):
        write_private(
            self.compose_env,
            "".join(f"{key}={value}\n" for key, value in values.items()),
        )

    def write_runtime(self, values, server_port):
        write_private(
            self.runtime,
            json.dumps(
                {
                    "instance_id": self.instance_id,
                    "worktree_path": self.worktree,
                    "server_url": f"http://127.0.0.1:{server_port}",
                    "ports": {
                        "server": server_port,
                        "postgres": int(values["CLUMSIES_DB_PORT"]),
                        "oidc": int(values["CLUMSIES_OIDC_PORT"]),
                    },
                    "daemon_root": self.daemon_root,
                },
                indent=2,
            )
            + "\n",
        )

    # -- processes

    def containers(self, values):
        command = [
            "docker", "compose",
            "--env-file", self.compose_env,
            "-p", self.compose_project,
            "up", "-d", "--wait", "postgres", "fake-oidc",
        ]
        if docker_available():
            run(command)
            return
        ports = [int(values["CLUMSIES_DB_PORT"]), int(values["CLUMSIES_OIDC_PORT"])]
        if wait_for_ports(ports, timeout=1):
            return
        # Omarchy keeps users out of the docker group on purpose, so this is a
        # normal state, not a failure. Say exactly what to run and stop, rather
        # than waiting on a command only the developer can issue.
        raise SystemExit(
            "Docker needs elevated rights here. Run this, then run \"up\" again:\n\n"
            "  sudo " + " ".join(command) + "\n"
        )

    def build_server(self):
        run(["cargo", "build", "-p", "server", "--bin", "clumsies-server"], cwd=REPO_ROOT)
        # Replacing a running executable fails on Linux, and "up" means "make
        # this instance current", so the old Server stops before the swap and
        # start_server brings it back.
        stop(self.server_pid)
        shutil.copy2(
            os.path.join(REPO_ROOT, "target", "debug", "clumsies-server"), self.server_binary
        )
        os.chmod(self.server_binary, 0o755)

    def start_server(self, values, server_port):
        if running(self.server_pid):
            return
        if os.path.exists(self.ready):
            os.remove(self.ready)
        # dev/dev-server.sh is the launcher the macOS instance uses too: it reads
        # the ports, the database password and the Setup Code from compose.env.
        spawn(
            [
                os.path.join(REPO_ROOT, "dev", "dev-server.sh"),
                self.compose_env,
                self.server_binary,
                f"127.0.0.1:{server_port}",
                self.ready,
            ],
            log=os.path.join(self.logs, "server.log"),
            pid_file=self.server_pid,
        )
        print("waiting for the Server", end="", flush=True)
        for _ in range(90):
            if os.path.exists(self.ready):
                print()
                return
            print(".", end="", flush=True)
            time.sleep(1)
        raise SystemExit(f"\nthe Server never became ready; see {self.logs}/server.log")

    def build_daemon(self):
        run(["cargo", "build", "-p", "clumsiesd", "--bin", "clumsiesd"], cwd=REPO_ROOT)

    def start_daemon(self):
        if running(self.daemon_pid):
            return
        environment = {
            "CLUMSIES_DAEMON_ROOT": self.daemon_root,
            "CLUMSIES_DAEMON_CACHE_DIR": self.cache,
        }
        spawn(
            [self.daemon_binary],
            log=os.path.join(self.logs, "daemon.log"),
            pid_file=self.daemon_pid,
            environment=environment,
        )
        print("waiting for the daemon", end="", flush=True)
        for _ in range(60):
            if os.path.exists(os.path.join(self.daemon_root, "daemon.sock")):
                print()
                return
            print(".", end="", flush=True)
            time.sleep(1)
        raise SystemExit(f"\nthe daemon never opened its socket; see {self.logs}/daemon.log")

    def start_client(self):
        """The macOS instance opens the App as part of "up"; this is the same
        promise, so a developer sees the product rather than a description of
        it."""
        if running(self.client_pid):
            return
        run(["cargo", "build", "-p", "desktop"], cwd=REPO_ROOT)
        spawn(
            [self.client_binary],
            log=os.path.join(self.logs, "client.log"),
            pid_file=self.client_pid,
            environment={"CLUMSIES_DAEMON_ROOT": self.daemon_root},
        )

    # -- the session

    def sign_in(self, server_url=None, setup_code=None):
        server_url = (server_url or self.server_url()).rstrip("/")
        tokens = authorize(server_url, setup_code or discover_setup_code())
        call(
            self.daemon_root,
            "replace_project_config",
            {
                "server_url": server_url,
                "project_id": None,
                "access_token": tokens["access_token"],
                "refresh_token": tokens.get("refresh_token"),
            },
        )
        user = tokens.get("user") or {}
        org = tokens.get("org") or {}
        print(f"signed in to {server_url}")
        print(f"  user: {user.get('email') or user.get('user_id')}")
        print(f"  org:  {org.get('name')} ({org.get('org_id')})")

    def server_url(self):
        with open(self.runtime) as handle:
            return json.load(handle)["server_url"]

    def seed_memory(self):
        """A client needs something to show, and publishing is not a write:
        a proposal becomes a Draft, a Draft becomes a Review, and only an
        authorized merge changes the published Ref."""
        project_id = self.project_id()
        if not project_id:
            raise SystemExit("the account has no Project to seed")
        checkout = call(self.daemon_root, "project_checkout", {"project_id": project_id})
        if checkout["resources"]:
            print(f"this Project already publishes {len(checkout['resources'])} documents")
            return

        page = self.server("GET", f"/api/v1/drafts?project_id={project_id}")
        drafts = page.get("items") or []
        if not drafts:
            call(
                self.daemon_root,
                "desktop_create_memory_drafts",
                {
                    "project_id": project_id,
                    "base_commit_id": None,
                    "operations": [
                        {
                            "create": {
                                "path": path,
                                "content": {"content": content},
                                "description": description,
                            },
                            "update": None,
                            "rename": None,
                            "delete": None,
                            "discard": None,
                        }
                        for path, description, content in SEED_DOCUMENTS
                    ],
                },
            )
            print(f"created {len(SEED_DOCUMENTS)} Drafts locally")
            print("waiting for the Drafts to reach the Server", end="", flush=True)
            for _ in range(40):
                time.sleep(2)
                page = self.server("GET", f"/api/v1/drafts?project_id={project_id}")
                drafts = page.get("items") or []
                if len(drafts) >= len(SEED_DOCUMENTS):
                    break
                print(".", end="", flush=True)
            print()

        review = self.server(
            "POST",
            "/api/v1/reviews",
            {
                "drafts": [
                    {"draft_id": draft["draft_id"], "expected_draft_version": draft["version"]}
                    for draft in drafts
                ],
                "title": "Seed project Memory",
                "description": "The first Memory for this Project.",
            },
            {"if-match": '"ref-none"'},
        )["review"]
        print(f"opened review {review['review_id']}")
        self.server(
            "POST",
            f"/api/v1/reviews/{review['review_id']}/merges",
            {"expected_review_version": review["version"]},
            {"if-match": f'"{review["ref_etag"]}"' if review.get("ref_etag") else '"ref-none"'},
        )
        print("merged; waiting for the daemon to publish the new commit", end="", flush=True)
        for _ in range(30):
            time.sleep(2)
            checkout = call(self.daemon_root, "project_checkout", {"project_id": project_id})
            if checkout["resources"]:
                print()
                for resource in checkout["resources"]:
                    print(f"  {resource['path']}")
                return
            print(".", end="", flush=True)
        raise SystemExit("\nthe daemon never picked up the published commit")

    def project_id(self):
        page = self.server("GET", "/api/v1/projects")
        items = page.get("items") or []
        return items[0]["project_id"] if items else None

    def server(self, method, path, body=None, headers=None):
        """Every Server call goes through the daemon, which holds the session
        and forwards these headers as given, content type included."""
        headers = dict(headers or {})
        if body is not None:
            headers.setdefault("content-type", "application/json")
        response = call(
            self.daemon_root,
            "server_request",
            {
                "method": method,
                "path": path,
                "headers": headers,
                "body": json.dumps(body) if body is not None else None,
            },
        )
        status = response.get("status")
        text = response.get("body") or ""
        if status >= 400:
            raise SystemExit(f"{method} {path} -> {status}: {text[:400]}")
        return json.loads(text) if text else {}

    # -- lifecycle

    def up(self, seed, with_client=True):
        self.create()
        values = self.env()
        self.write_env(values)
        self.containers(values)
        self.build_server()
        server_port = self.port_of("server")
        self.write_runtime(values, server_port)
        self.start_server(values, server_port)
        self.build_daemon()
        self.start_daemon()
        self.sign_in()
        call(
            self.daemon_root,
            "select_project",
            {"project_id": self.project_id()},
        )
        if seed:
            self.seed_memory()
        if with_client:
            print("starting the client", end="", flush=True)
            self.start_client()
            print()
        print()
        print(f"instance {self.instance_id} is up")
        print(f"  server: http://127.0.0.1:{server_port}")
        print(f"  daemon: {self.daemon_root}/daemon.sock")
        print(f"  logs:   {self.logs}")
        print(f"  seeded: {'yes' if seed else 'no (pass --seed-memory)'}")

    def port_of(self, name):
        if os.path.exists(self.runtime):
            with open(self.runtime) as handle:
                return json.load(handle)["ports"][name]
        return free_port()

    def status(self):
        if not os.path.exists(self.runtime):
            raise SystemExit("no instance here yet")
        with open(self.runtime) as handle:
            print(json.dumps(json.load(handle), indent=2))
        print(f"server: {'running' if running(self.server_pid) else 'stopped'}")
        print(f"daemon: {'running' if running(self.daemon_pid) else 'stopped'}")

    def show_logs(self):
        for name in ("server.log", "daemon.log"):
            path = os.path.join(self.logs, name)
            print(f"--- {path}")
            if os.path.exists(path):
                with open(path) as handle:
                    sys.stdout.write("".join(handle.readlines()[-40:]))

    def down(self):
        for pid_file in (self.client_pid, self.daemon_pid, self.server_pid):
            stop(pid_file)
        if docker_available():
            subprocess.run(
                [
                    "docker", "compose", "--env-file", self.compose_env,
                    "-p", self.compose_project, "down",
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
        print(f"instance {self.instance_id} stopped")

    def reset(self):
        self.down()
        shutil.rmtree(self.root, ignore_errors=True)
        print(f"instance {self.instance_id} deleted")


# --- small shared pieces ---------------------------------------------------


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def write_private(path, text):
    temporary = f"{path}.{os.getpid()}.tmp"
    with open(temporary, "w") as handle:
        handle.write(text)
    os.chmod(temporary, 0o600)
    os.replace(temporary, path)


def docker_available():
    return subprocess.run(
        ["docker", "info"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False
    ).returncode == 0


def run(command, cwd=None):
    result = subprocess.run(command, cwd=cwd)
    if result.returncode != 0:
        raise SystemExit(f"{' '.join(command)} failed")


def spawn(command, log, pid_file, environment=None):
    full_environment = dict(os.environ)
    full_environment.update(environment or {})
    with open(log, "ab") as handle:
        process = subprocess.Popen(
            command,
            stdout=handle,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
            env=full_environment,
        )
    write_private(pid_file, str(process.pid))


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except (ProcessLookupError, PermissionError):
        return False


def running(pid_file):
    if not os.path.exists(pid_file):
        return False
    try:
        return alive(int(open(pid_file).read().strip()))
    except ValueError:
        return False


def stop(pid_file):
    """Asks a process to stop and waits for it, because the caller usually
    wants to replace the executable it is running from."""
    if not os.path.exists(pid_file):
        return
    try:
        pid = int(open(pid_file).read().strip())
    except ValueError:
        os.remove(pid_file)
        return
    if alive(pid):
        try:
            os.kill(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        for _ in range(50):
            if not alive(pid):
                break
            time.sleep(0.1)
        else:
            # A Server holds a database and a daemon holds draft state, so this
            # is the last resort, not the first move.
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            time.sleep(0.5)
    os.remove(pid_file)


def wait_for_ports(ports, timeout):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if all(
            socket.socket().connect_ex(("127.0.0.1", port)) == 0 for port in ports
        ):
            return True
        time.sleep(2)
    return False


def call(daemon_root, method, payload):
    body = json.dumps({"method": method, "payload": payload}).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(120)
        connection.connect(os.path.join(daemon_root, "daemon.sock"))
        connection.sendall(struct.pack(">I", len(body)) + body)
        header = connection.recv(4)
        if len(header) < 4:
            raise SystemExit("the daemon closed the connection without replying")
        length = struct.unpack(">I", header)[0]
        if length > MAX_FRAME_BYTES:
            raise SystemExit(f"implausible reply of {length} bytes")
        data = b""
        while len(data) < length:
            chunk = connection.recv(length - len(data))
            if not chunk:
                break
            data += chunk
    reply = json.loads(data)
    if not reply.get("ok"):
        error = reply.get("error") or {}
        raise SystemExit(f"{method} failed: {error.get('code')} {error.get('message')}")
    return reply.get("payload")


class Callback(Exception):
    """Raised when the authorization flow reaches our loopback callback."""


class CaptureRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, new_url):
        if urllib.parse.urlparse(new_url).path == "/callback":
            raise Callback(new_url)
        return super().redirect_request(request, fp, code, msg, headers, new_url)


def discover_setup_code():
    """The Setup Code is deployment data, never something a developer types.

    In order: the environment, this worktree's Dev Instance (whose compose.env
    holds the code the instance generated), then the shared dev Server's own
    default in dev/server.sh. The last one is why signing in against
    `sh dev/server.sh` needs no argument: the value is already in the
    repository, so asking a human for it asks them to retype a file they can
    read."""
    if value := os.environ.get("CLUMSIES_SETUP_CODE"):
        return value
    instance = Instance()
    if os.path.exists(instance.compose_env):
        found = env_value(instance.compose_env, "CLUMSIES_SETUP_CODE")
        if found:
            return found
    return env_value(os.path.join(REPO_ROOT, "dev", "server.sh"), "CLUMSIES_SETUP_CODE")


def env_value(path, key):
    try:
        with open(path) as handle:
            for line in handle:
                line = line.strip()
                for prefix in (f"{key}=", f"export {key}="):
                    if line.startswith(prefix):
                        return line[len(prefix):].rstrip("\\").strip().strip('"')
    except OSError:
        return None
    return None


def authorize(server_url, setup_code=None, redirect_port=49199):
    """The same PKCE authorization a desktop client performs, plus the one-time
    Server setup when the deployment still needs it."""
    redirect = f"http://127.0.0.1:{redirect_port}/callback"
    opener = urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()), CaptureRedirect()
    )

    def request(path, method="GET", body=None, csrf=None):
        headers = {"content-type": "application/json"}
        if csrf:
            headers["x-csrf-token"] = csrf
        prepared = urllib.request.Request(
            server_url + path,
            data=json.dumps(body).encode() if body is not None else None,
            method=method,
            headers=headers,
        )
        try:
            with opener.open(prepared, timeout=60) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read(400).decode("utf-8", "replace")
            raise SystemExit(f"{method} {path} failed: {error.code} {detail}")

    verifier = base64.urlsafe_b64encode(secrets.token_bytes(48)).rstrip(b"=").decode()
    challenge = (
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    )
    state = secrets.token_urlsafe(24)
    parameters = {
        "redirect_uri": redirect,
        "state": state,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
    }

    if request("/api/v1/setup").get("state") == "setup_required":
        if not setup_code:
            raise SystemExit(
                "this Server has never been configured and no Setup Code was found; "
                "pass --setup-code or set CLUMSIES_SETUP_CODE"
            )
        session = request("/api/v1/setup/sessions", "POST", {"setup_code": setup_code})
        csrf = session["csrf_token"]
        request(
            "/api/v1/setup/configuration",
            "PUT",
            {
                "org_name": "Clumsies Dev",
                "default_project_name": "clumsies",
                "allowed_email_domains": ["clumsies.local"],
            },
            csrf,
        )
        authorization = request("/api/v1/setup/oidc-authorizations", "POST", parameters, csrf)[
            "authorization_url"
        ]
        print(f"configured {server_url}")
    else:
        authorization = server_url + "/oauth2/authorization/oidc?" + urllib.parse.urlencode(
            dict(parameters, client_kind="desktop")
        )

    try:
        opener.open(authorization, timeout=60)
        raise SystemExit("the identity provider never returned a callback")
    except Callback as callback:
        values = urllib.parse.parse_qs(urllib.parse.urlparse(str(callback)).query)
        if values.get("error"):
            raise SystemExit(f"authorization failed: {values['error'][0]}")
        if values.get("state") != [state]:
            raise SystemExit("the callback state does not match the request")
        code = values["code"][0]

    return request(
        "/api/v1/auth/token",
        "POST",
        {
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect,
            "code_verifier": verifier,
        },
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    up = commands.add_parser("up", help="start containers, Server, daemon, client; sign in")
    up.add_argument("--seed-memory", action="store_true", help="also publish starter Memory")
    up.add_argument("--no-client", action="store_true", help="leave the client alone")
    sign_in = commands.add_parser("sign-in", help="sign the daemon in to a Server")
    sign_in.add_argument("--server-url", default=None)
    sign_in.add_argument("--setup-code", default=None)
    for name in ("status", "logs", "down", "reset"):
        commands.add_parser(name)
    arguments = parser.parse_args()

    instance = Instance()
    if arguments.command == "up":
        instance.up(seed=arguments.seed_memory, with_client=not arguments.no_client)
    elif arguments.command == "sign-in":
        instance.sign_in(arguments.server_url, arguments.setup_code)
    elif arguments.command == "status":
        instance.status()
    elif arguments.command == "logs":
        instance.show_logs()
    elif arguments.command == "down":
        instance.down()
    elif arguments.command == "reset":
        instance.reset()


if __name__ == "__main__":
    main()
