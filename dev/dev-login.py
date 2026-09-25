#!/usr/bin/env python3
"""Sign the local daemon in to a Server, the way a desktop client does.

The macOS Dev Instance signs in through the App, which keeps the session in the
Keychain. Linux and Windows have no Keychain: the daemon owns an owner-only
credential file, so this script is the equivalent of that flow. It performs the
same authorization a client performs and then hands the tokens to the daemon
over its local socket; it never writes the credential file itself, so where a
session lives stays a daemon decision.

A Server that has never been configured requires a one-time setup before any
product login. Pass the deployment's Setup Code with --setup-code (or
CLUMSIES_SETUP_CODE) and this script completes it, using the same three calls
the App makes.

    sh dev/server.sh                                      # another terminal
    python3 dev/dev-login.py --setup-code clumsies-local-setup-code-00000001

Against a configured deployment, drop --setup-code and pass --server-url.
"""

import argparse
import base64
import hashlib
import http.cookiejar
import json
import os
import secrets
import socket
import struct
import sys
import urllib.error
import urllib.parse
import urllib.request

CALLBACK_PATH = "/callback"
SOCKET_ENV = "CLUMSIES_DAEMON_SOCKET"
ROOT_ENV = "CLUMSIES_DAEMON_ROOT"
MAX_FRAME_BYTES = 64 * 1024 * 1024


class Callback(Exception):
    """Raised when the browser flow reaches our loopback callback."""


class CaptureRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, msg, headers, new_url):
        if urllib.parse.urlparse(new_url).path == CALLBACK_PATH:
            raise Callback(new_url)
        return super().redirect_request(request, fp, code, msg, headers, new_url)


def socket_path():
    if value := os.environ.get(SOCKET_ENV):
        return value
    if value := os.environ.get(ROOT_ENV):
        return os.path.join(value, "daemon.sock")
    home = os.path.expanduser("~")
    data = os.environ.get("XDG_DATA_HOME") or os.path.join(home, ".local", "share")
    return os.path.join(data, "ai.clumsies", "daemon.sock")


def daemon_call(method, payload):
    """One request, one reply, framed with a 4-byte big-endian length."""
    body = json.dumps({"method": method, "payload": payload}).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(30)
        connection.connect(socket_path())
        connection.sendall(struct.pack(">I", len(body)) + body)
        header = connection.recv(4)
        if len(header) < 4:
            raise SystemExit("the daemon closed the connection without replying")
        length = struct.unpack(">I", header)[0]
        if length > MAX_FRAME_BYTES:
            raise SystemExit(f"the daemon announced an implausible reply of {length} bytes")
        body = b""
        while len(body) < length:
            chunk = connection.recv(length - len(body))
            if not chunk:
                break
            body += chunk
    reply = json.loads(body)
    if not reply.get("ok"):
        error = reply.get("error") or {}
        raise SystemExit(
            f"the daemon refused {method}: {error.get('code')} {error.get('message')}"
        )
    return reply.get("payload")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server-url", default="http://127.0.0.1:18080")
    parser.add_argument(
        "--setup-code",
        default=os.environ.get("CLUMSIES_SETUP_CODE"),
        help="deployment Setup Code, for a Server that has never been configured",
    )
    parser.add_argument("--org-name", default="Clumsies Local")
    parser.add_argument("--project-name", default="clumsies")
    parser.add_argument("--email-domain", default="clumsies.local")
    parser.add_argument("--redirect-port", type=int, default=49199)
    arguments = parser.parse_args()

    origin = arguments.server_url.rstrip("/")
    redirect = f"http://127.0.0.1:{arguments.redirect_port}{CALLBACK_PATH}"
    opener = urllib.request.build_opener(
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()), CaptureRedirect()
    )

    def request(path, method="GET", body=None, csrf=None):
        headers = {"content-type": "application/json"}
        if csrf:
            headers["x-csrf-token"] = csrf
        prepared = urllib.request.Request(
            origin + path,
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
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest())
        .rstrip(b"=")
        .decode()
    )
    state = secrets.token_urlsafe(24)
    parameters = {
        "redirect_uri": redirect,
        "state": state,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
    }

    if request("/api/v1/setup").get("state") == "setup_required":
        if not arguments.setup_code:
            raise SystemExit(
                "this Server has never been configured; pass --setup-code "
                "(the deployment's CLUMSIES_SETUP_CODE)"
            )
        session = request(
            "/api/v1/setup/sessions", "POST", {"setup_code": arguments.setup_code}
        )
        csrf = session["csrf_token"]
        request(
            "/api/v1/setup/configuration",
            "PUT",
            {
                "org_name": arguments.org_name,
                "default_project_name": arguments.project_name,
                "allowed_email_domains": [arguments.email_domain],
            },
            csrf,
        )
        authorization = request(
            "/api/v1/setup/oidc-authorizations", "POST", parameters, csrf
        )["authorization_url"]
        print(f"configured {origin} as {arguments.org_name!r}")
    else:
        authorization = origin + "/oauth2/authorization/oidc?" + urllib.parse.urlencode(
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

    tokens = request(
        "/api/v1/auth/token",
        "POST",
        {
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect,
            "code_verifier": verifier,
        },
    )

    # The daemon owns the credential file; a client only hands it the session.
    daemon_call(
        "replace_project_config",
        {
            "server_url": origin,
            "project_id": None,
            "access_token": tokens["access_token"],
            "refresh_token": tokens.get("refresh_token"),
        },
    )

    user = tokens.get("user") or {}
    org = tokens.get("org") or {}
    print(f"signed in to {origin}")
    print(f"  user: {user.get('email') or user.get('user_id')}")
    print(f"  org:  {org.get('name')} ({org.get('org_id')})")
    print(f"  daemon: {socket_path()}")


if __name__ == "__main__":
    try:
        main()
    except (ConnectionError, FileNotFoundError) as error:
        raise SystemExit(f"could not reach the daemon at {socket_path()}: {error}")

