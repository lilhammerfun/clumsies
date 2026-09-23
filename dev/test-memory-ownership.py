#!/usr/bin/env python3
"""Exercise Project/Org ownership with four real sessions on this local Dev Instance."""
import importlib.util
from pathlib import Path
import secrets

spec = importlib.util.spec_from_file_location("playground", Path(__file__).with_name("seed-review-playground.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def head(client, scope="project"):
    prefix = f"projects/{client.project}" if scope == "project" else "org"
    return client.call("GET", f"/api/v1/{prefix}/commit-state")["ref"]["commit_id"]


def etag(commit):
    return {"If-Match": '"' + (commit or "ref-none") + '"'}


def proposal(client, path, body, target=None):
    base = head(client)
    resource = {"scope": "project", "path": path, "id": target}
    return client.call("POST", "/api/v1/drafts", {
        "daemon_installation_id": "ownership-acceptance", "project_id": client.project,
        "base_commit_id": base, "title": path, "resource": resource,
        "operations": [{"action": "update" if target else "create", "resource": resource,
                        "content": {"content": body}}]})


def review(client, draft, contribution_path=None):
    source = draft["draft"]
    body = {"drafts": [{"draft_id": source["draft_id"], "expected_draft_version": source["version"]}]}
    if contribution_path:
        body["org_contribution"] = [{"draft_id": source["draft_id"], "path": contribution_path}]
    return client.call("POST", "/api/v1/reviews", body, etag(source["base_commit_id"]))


def merge(client, detail):
    item = detail["review"]
    return client.call("POST", f'/api/v1/reviews/{item["review_id"]}/merges',
                       {"expected_review_version": item["version"]}, etag(head(client, item["scope"])))


def denied(action):
    try:
        action()
    except RuntimeError as error:
        assert ": 403:" in str(error), str(error)
    else:
        raise AssertionError("unauthorized publication succeeded")


def main():
    owner = module.Playground()
    owner.login()
    owner.ensure_test_accounts()
    clients = {"owner": owner}
    for account in ("project-admin", "member-a", "member-b"):
        client = module.Playground()
        client.login(account)
        assert client.me["user"]["role"] == "member"
        clients[account] = client
    assert len({c.me["user"]["user_id"] for c in clients.values()}) == 4
    tag = secrets.token_hex(5)
    project = owner.call("POST", "/api/v1/projects", {"name": "Memory ownership " + tag})["project_id"]
    for account, client in clients.items():
        client.project = project
        if account != "owner":
            owner.call("POST", f"/api/v1/admin/projects/{project}/members", {
                "user_id": client.me["user"]["user_id"], "role": "admin" if account == "project-admin" else "member"})
    admin, alice, bob = (clients[key] for key in ("project-admin", "member-a", "member-b"))
    path = "guide.md"
    body = "# Guide\n\n## Build\n\nBuild v1.\n\n## Test\n\nTest v1.\n"
    org_before = head(owner, "org")
    first = review(alice, proposal(alice, path, body), "ownership-" + tag + "/guide.md")
    denied(lambda: merge(bob, first))
    published = merge(admin, first)
    assert head(owner, "org") == org_before
    assert head(bob) == published["commit_id"]
    memories = bob.call("GET", f"/api/v1/projects/{project}/memories")["items"]
    memory = next(m for m in memories if m["path"] == path)
    assert memory["scope"] == "project"
    resource = memory["memory_id"]
    assert bob.call("GET", f"/api/v1/projects/{project}/memories/{resource}")["content"] == body
    assert any(n["kind"] == "shared_update" and n["project_id"] == project
               for n in bob.call("GET", "/api/v1/me/inbox")["items"])
    review_id = first["review"]["review_id"]
    source = alice.call("GET", f"/api/v1/reviews/{review_id}")
    contribution = source["review"]["org_contribution"]
    assert contribution["source_commit_id"] == published["commit_id"]
    linked = contribution["org_review_id"]
    assert linked, contribution
    for _ in range(2):
        retried = alice.call("POST", f"/api/v1/reviews/{review_id}/org-contribution", {})
        assert retried["review"]["org_contribution"]["org_review_id"] == linked
    org_review = owner.call("GET", f"/api/v1/reviews/{linked}")
    assert org_review["review"]["scope"] == "org"
    assert org_review["review"]["project_source"]["commit_id"] == published["commit_id"]
    denied(lambda: merge(admin, org_review))
    merge(owner, org_review)
    assert head(bob) == published["commit_id"], "Org contribution must not auto-select itself"
    print("PASS: Project member → Project admin → member snapshot + inbox; separate Org owner merge; idempotent link")

    # Two different files of work on the same shared resource: clean vs overlapping text.
    conflict = proposal(bob, path, body.replace("Build v1.", "Build by Bob."), resource)
    clean = proposal(alice, path, body.replace("Test v1.", "Test by Alice."), resource)
    updated_body = body.replace("Build v1.", "Build by maintainer.")
    merge(admin, review(admin, proposal(admin, path, updated_body, resource)))
    for client, original, should_conflict in ((bob, conflict, True), (alice, clean, False)):
        draft = original["draft"]
        updated = client.call("POST", f'/api/v1/drafts/{draft["draft_id"]}/auto-rebases',
                              {"expected_draft_version": draft["version"]})
        if should_conflict:
            assert updated["draft"]["version"] == draft["version"]
            assert updated["draft"]["base_commit_id"] == draft["base_commit_id"]
            assert updated["operations"] == original["operations"]
            assert updated["draft"]["coordination"]["reconciliation"] == "conflicts"
            assert any(n["kind"] == "draft_conflict" and n["target_id"] == draft["draft_id"]
                       for n in client.call("GET", "/api/v1/me/inbox")["items"])
        else:
            assert updated["draft"]["base_commit_id"] == head(client)
            assert updated["draft"]["version"] > draft["version"]
    print("PASS: clean draft advances; conflicting draft keeps base/operations and notifies its author")
    print("Dev instance:", owner.instance, "project:", project, "Project PR:", review_id, "Org PR:", linked)


if __name__ == "__main__":
    main()
