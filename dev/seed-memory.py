#!/usr/bin/env python3
"""Put real Memory into a Project, through the product's own path.

Publishing is not a write: a proposal becomes a Draft, a Draft becomes a
Review, and only an authorized merge changes the published Ref. This script
walks that path for a local Dev Server so a client has something real to show,
and it goes through the daemon exactly as the desktop client does.

    python3 dev/seed-memory.py --project-id prj_...

It needs a daemon that is already signed in (dev/dev-login.py) and a Server
whose organization exists.
"""

import argparse
import json
import socket
import struct
import sys
import time
import urllib.error
import urllib.request

MAX_FRAME_BYTES = 64 * 1024 * 1024

DOCUMENTS = [
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
        "Architecture",
        """---
title: Client architecture
kind: knowledge
---

# Client architecture

Each platform has its own native client, and they share one local engine
instead of one user interface: **macOS keeps its Swift app, Windows and Linux
share the Rust client.**

## Why

The engine — drafts, synchronization, retrieval — is where the product's
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


def socket_path():
    import os

    if value := os.environ.get("CLUMSIES_DAEMON_SOCKET"):
        return value
    if value := os.environ.get("CLUMSIES_DAEMON_ROOT"):
        return os.path.join(value, "daemon.sock")
    home = os.path.expanduser("~")
    data = os.environ.get("XDG_DATA_HOME") or os.path.join(home, ".local", "share")
    return os.path.join(data, "ai.clumsies", "daemon.sock")


def call(method, payload):
    body = json.dumps({"method": method, "payload": payload}).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(120)
        connection.connect(socket_path())
        connection.sendall(struct.pack(">I", len(body)) + body)
        header = connection.recv(4)
        if len(header) < 4:
            raise SystemExit("the daemon closed the connection without replying")
        length = struct.unpack(">I", header)[0]
        if length > MAX_FRAME_BYTES:
            raise SystemExit(f"implausible reply of {length} bytes")
        body = b""
        while len(body) < length:
            chunk = connection.recv(length - len(body))
            if not chunk:
                break
            body += chunk
    reply = json.loads(body)
    if not reply.get("ok"):
        error = reply.get("error") or {}
        raise SystemExit(f"{method} failed: {error.get('code')} {error.get('message')}")
    return reply.get("payload")


def server(method, path, body=None, headers=None):
    """Every Server call goes through the daemon, which holds the session and
    forwards these headers as given, content type included."""
    headers = dict(headers or {})
    if body is not None:
        headers.setdefault("content-type", "application/json")
    payload = call(
        "server_request",
        {
            "method": method,
            "path": path,
            "headers": headers,
            "body": json.dumps(body) if body is not None else None,
        },
    )
    status = payload.get("status")
    text = payload.get("body") or ""
    if status >= 400:
        raise SystemExit(f"{method} {path} -> {status}: {text[:400]}")
    return json.loads(text) if text else {}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project-id", required=True)
    arguments = parser.parse_args()
    project_id = arguments.project_id

    existing = call("project_checkout", {"project_id": project_id})
    if existing["resources"]:
        print(f"this Project already publishes {len(existing['resources'])} documents")
        return

    drafts = [
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
            ],
        }
        for path, description, content in DOCUMENTS
    ]
    # One call, so the set arrives as one proposal rather than three.
    combined = {
        "project_id": project_id,
        "base_commit_id": None,
        "operations": [operation for draft in drafts for operation in draft["operations"]],
    }
    # Rerunnable: a previous run may have landed the Drafts already, and the
    # Server is the authority on which of them exist.
    page = server("GET", f"/api/v1/drafts?project_id={project_id}")
    server_drafts = page.get("items") or []
    if server_drafts:
        print(f"reusing {len(server_drafts)} Draft(s) already on the Server")
    else:
        call("desktop_create_memory_drafts", combined)
        print(f"created {len(DOCUMENTS)} Drafts locally")
        # The daemon pushes Drafts to the Server on its own schedule.
        print("waiting for the Drafts to reach the Server", end="", flush=True)
        for _ in range(40):
            time.sleep(2)
            page = server("GET", f"/api/v1/drafts?project_id={project_id}")
            server_drafts = page.get("items") or []
            if len(server_drafts) >= len(DOCUMENTS):
                break
            print(".", end="", flush=True)
        print()
    if not server_drafts:
        raise SystemExit("the Drafts never reached the Server; is the daemon signed in?")
    print(f"  {len(server_drafts)} Draft(s) on the Server")

    review = server(
        "POST",
        "/api/v1/reviews",
        {
            "drafts": [
                {"draft_id": draft["draft_id"], "expected_draft_version": draft["version"]}
                for draft in server_drafts
            ],
            "title": "Seed project Memory",
            "description": "The first Memory for this Project.",
        },
        {"if-match": '"ref-none"'},
    )["review"]
    print(f"opened review {review['review_id']} (version {review['version']})")

    merged = server(
        "POST",
        f"/api/v1/reviews/{review['review_id']}/merges",
        {"expected_review_version": review["version"]},
        {"if-match": f'"{review["ref_etag"]}"' if review.get("ref_etag") else '"ref-none"'},
    )
    print(f"merged: commit {merged.get('commit_id')}")

    print("waiting for the daemon to publish the new commit", end="", flush=True)
    # The daemon syncs the Project it has selected, so select this one before
    # waiting: a Project nobody selected is a Project nobody syncs.
    call("select_project", {"project_id": project_id})
    for _ in range(30):
        time.sleep(2)
        checkout = call("project_checkout", {"project_id": project_id})
        if checkout["resources"]:
            print()
            for resource in checkout["resources"]:
                print(f"  {resource['path']}")
            print(f"published {len(checkout['resources'])} documents")
            return
        print(".", end="", flush=True)
    raise SystemExit("the daemon never picked up the published commit")


if __name__ == "__main__":
    main()
