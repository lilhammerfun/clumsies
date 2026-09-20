#!/usr/bin/env python3
"""Seed only this worktree's loopback Dev Instance and its labelled dashboard fixture."""
import argparse
import base64
import collections
import concurrent.futures
import datetime as dt
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import random
import secrets
import subprocess
import time
import urllib.error
import urllib.parse
import urllib.request


def sample(now=None):
    rng = random.Random(271828)
    now = now or dt.datetime.now().astimezone()
    today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    start = today - dt.timedelta(days=89)
    directories = ["knowledge", "procedures", "experience", "skills", "decisions", "reference"]
    topics = ["Authentication", "Memory sync", "Swift concurrency", "Search ranking", "Release workflow",
              "Schema migration", "Review lifecycle", "Workspace isolation", "Observability", "Accessibility",
              "Offline recovery", "API contracts", "Index lifecycle", "Testing strategy", "Security boundaries"]
    titles = ["Project architecture", "Swift frontend conventions", "Memory publishing guide",
              "Local development workflow", "Retrieval troubleshooting", "Database migration checklist"]
    docs, active, days, recalls, changes = [], [], [], [], []

    def add(day):
        index = len(docs)
        category = index % 6 if index < 6 else rng.choices(range(6), [28, 21, 17, 15, 11, 8])[0]
        title = titles[index] if index < 6 else f"{topics[index % len(topics)]} — {['guide', 'decision', 'checklist', 'notes'][index % 4]} {index // 60 + 1}"
        doc = {"id": f"sample-{index}", "title": title,
               "path": f"{directories[category]}/{index:03d}-{topics[index % len(topics)].lower().replace(' ', '-')}.md"}
        docs.append(doc)
        active.append(doc)
        if day is not None:
            changes.append({"date": day.timestamp(), "resourceID": doc["id"], "kind": "added"})

    for _ in range(215):
        add(None)
    for index in range(90):
        date = start + dt.timedelta(days=index)
        weekend = date.weekday() >= 5
        additions = rng.randint(0, 1) if weekend else rng.randint(1, 4)
        if index in [18, 42, 67, 81]:
            additions += rng.randint(12, 23)
        for _ in range(additions):
            add(date)
        if index > 10 and index % 11 == 0:
            for _ in range(rng.randint(2, 5)):
                removed = active.pop(rng.randrange(8, len(active)))
                changes.append({"date": date.timestamp(), "resourceID": removed["id"], "kind": "deleted"})
        for doc in rng.sample(active, rng.randint(0, 2) if weekend else rng.randint(3, 10)):
            changes.append({"date": date.timestamp(), "resourceID": doc["id"], "kind": "updated"})
        requests = max(0, round((15 if weekend else 49) + index * .55 + rng.gauss(0, 10)))
        if index in [43, 68, 82]:
            requests += 50
        if index == 74:
            requests = 9
        if index == 89:
            requests = round(requests * max(.15, (now.hour + now.minute / 60) / 24))
        empty = failed = 0
        hits = collections.Counter()
        eligible = [d for d in active if int(d['id'].split('-')[1]) < 6 or
                    (int(d['id'].split('-')[1]) % 5 < 3 and (index < 60 or int(d['id'].split('-')[1]) % 7 < 4))]
        for _ in range(requests):
            draw = rng.random()
            if draw < (.21 if index in [61, 62] else .012):
                failed += 1
                continue
            if draw < (.34 if 38 <= index <= 42 else .12):
                empty += 1
                continue
            selected = set()
            for _ in range(rng.randint(1, 4)):
                doc = active[int(rng.random() ** 1.8 * 6)] if rng.random() < .55 else rng.choice(eligible)
                selected.add(doc["id"])
            hits.update(selected)
        recalls.extend({"date": date.timestamp(), "resourceID": resource, "count": count} for resource, count in hits.items())
        days.append({"date": date.timestamp(), "memoryCount": len(active),
                     "returned": requests - empty - failed, "empty": empty, "failed": failed})
    data = {"projectID": None, "projectName": "Clumsies · Dashboard Lab", "generatedAt": now.timestamp(),
            "historyStart": start.timestamp(), "historyComplete": True, "resources": active,
            "days": days, "recalls": recalls, "changes": changes, "openDrafts": 9, "submittedDrafts": 5}
    assert len(active) == 215 + sum(c['kind'] == 'added' for c in changes) - sum(c['kind'] == 'deleted' for c in changes)
    assert all(d['returned'] >= 0 and d['empty'] >= 0 and d['failed'] >= 0 for d in days)
    assert len({d['id'] for d in active}) == len(active)
    return data


def statistics_fixtures(data):
    """Precompute the production DTOs; the App never aggregates mock events."""
    now = dt.datetime.fromtimestamp(data['generatedAt']).astimezone()
    today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    resources = data['resources']
    ids = {r['id'] for r in resources}
    result = []
    for period in (7, 30, 90):
        bounds = [(today + dt.timedelta(days=offset)).timestamp() for offset in range(1-period, 2)]
        recency = [(today - dt.timedelta(days=d)).timestamp() for d in (6, 29, 89)]
        events = [e for e in data['changes'] if bounds[0] <= e['date'] <= data['generatedAt']]
        def changed(kind, start, end):
            return len({e['resourceID'] for e in events if e['kind'] == kind and start <= e['date'] < end})
        stride = 1 if period == 7 else 7
        buckets = [{'date': bounds[i], 'kind': k, 'count': changed(k, bounds[i], bounds[min(i+stride, period)])}
                   for i in range(0, period, stride) for k in ('added', 'updated', 'deleted')]
        days = [d for d in data['days'] if bounds[0] <= d['date'] <= data['generatedAt']]
        by_day = {d['date']: d for d in days}
        memory = dict(generatedAt=data['generatedAt'], dayBounds=bounds, recencyStarts=recency,
                      projectIds=[data['projectID']] if data['projectID'] else [], resources=resources,
                      memoryCount=len(resources), addedCount=changed('added', bounds[0], bounds[-1]),
                      updatedCount=changed('updated', bounds[0], bounds[-1]), deletedCount=changed('deleted', bounds[0], bounds[-1]),
                      days=[{'date': d, 'memoryCount': by_day.get(d, {}).get('memoryCount')} for d in bounds[:-1]],
                      changeBuckets=buckets, openDrafts=data['openDrafts'], submittedDrafts=data['submittedDrafts'])
        hits = collections.Counter()
        latest = {}
        for e in data['recalls']:
            if e['resourceID'] not in ids or not recency[2] <= e['date'] <= data['generatedAt']:
                continue
            latest[e['resourceID']] = max(latest.get(e['resourceID'], 0), e['date'])
            if e['date'] >= bounds[0]:
                hits[e['resourceID']] += e['count']
        directories = {}
        recent = [0, 0, 0, 0]
        for r in resources:
            directory = r['path'].split('/')[0] + '/' if '/' in r['path'] else 'Root'
            bar = directories.setdefault(directory, dict(id=directory, label=directory, value=0, total=0))
            bar['total'] += 1
            bar['value'] += int(hits[r['id']] > 0)
            date = latest.get(r['id'])
            recent[3 if date is None else 0 if date >= recency[0] else 1 if date >= recency[1] else 2] += 1
        top = [dict(id=r['id'], label=r['title'], value=hits[r['id']]) for r in resources if hits[r['id']]]
        retrieval = dict(retrievals=sum(d['returned']+d['empty']+d['failed'] for d in days),
                         recalledCount=len(hits), coverage=len(hits)/len(resources) if resources else 0,
                         days=[{k:d[k] for k in ('date','returned','empty','failed')} for d in days],
                         directories=sorted(directories.values(), key=lambda b:(-b['total'], b['id'])),
                         topResources=sorted(top, key=lambda b:(-b['value'], b['id']))[:6],
                         recency=[dict(id=str(i), label=label, value=recent[i]) for i,label in enumerate(
                             ('Last 7 days','8–30 days','31–90 days','Not observed'))],
                         historyStart=data['historyStart'], retentionPerProject=500)
        assert sum(recent) == len(resources)
        assert sum(b['value'] for b in directories.values()) == len(hits)
        result.append(dict(projectId=data['projectID'], projectName=data['projectName'], period=period,
                           memory=memory, retrieval=retrieval))
    return result


class Callback(Exception):
    pass


class CaptureRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urllib.parse.urlparse(newurl).path == '/callback':
            raise Callback(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def seed(runtime_path):
    runtime_path = runtime_path.resolve()
    runtime = json.loads(runtime_path.read_text())
    worktree = Path(__file__).resolve().parent.parent
    instance = hashlib.sha256(str(worktree).encode()).hexdigest()[:12]
    assert runtime['instance_id'] == instance and runtime['worktree_path'] == str(worktree), 'Wrong Dev Instance'
    assert runtime['mode'] == 'local', 'Only a local Dev Instance can be seeded'
    origin = runtime['server_url']
    parsed = urllib.parse.urlparse(origin)
    assert parsed.scheme == 'http' and parsed.hostname == '127.0.0.1' and parsed.port, 'Loopback only'
    root = runtime_path.parent
    fixture_path = root / 'fixtures/dashboard.json'
    secrets_map = dict(line.split('=', 1) for line in (root / 'compose.env').read_text().splitlines() if '=' in line)
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()), CaptureRedirect())

    def setup_request(path, method='GET', body=None, csrf=None):
        headers = {'Content-Type': 'application/json'}
        if csrf:
            headers['x-csrf-token'] = csrf
        request = urllib.request.Request(origin + path, data=json.dumps(body).encode() if body is not None else None,
                                         method=method, headers=headers)
        with opener.open(request, timeout=60) as response:
            return json.load(response)

    verifier = secrets.token_urlsafe(48)
    state = secrets.token_urlsafe(24)
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).decode().rstrip('=')
    redirect = 'http://127.0.0.1:49199/callback'
    parameters = {'redirect_uri': redirect, 'state': state, 'code_challenge': challenge, 'code_challenge_method': 'S256'}
    if setup_request('/api/v1/setup')['state'] == 'setup_required':
        session = setup_request('/api/v1/setup/sessions', 'POST', {'setup_code': secrets_map['CLUMSIES_SETUP_CODE']})
        csrf = session['csrf_token']
        setup_request('/api/v1/setup/configuration', 'PUT', {'org_name': 'Clumsies Dashboard Lab',
                      'default_project_name': 'clumsies', 'allowed_email_domains': ['clumsies.local']}, csrf)
        authorization = setup_request('/api/v1/setup/oidc-authorizations', 'POST', parameters, csrf)['authorization_url']
    else:
        authorization = origin + '/oauth2/authorization/oidc?' + urllib.parse.urlencode(dict(parameters, client_kind='desktop'))
    try:
        opener.open(authorization, timeout=60)
        raise RuntimeError('The local OIDC provider did not return a callback')
    except Callback as callback:
        values = urllib.parse.parse_qs(urllib.parse.urlparse(str(callback)).query)
        assert values.get('state') == [state], 'OIDC state mismatch'
        code = values['code'][0]
    token = setup_request('/api/v1/auth/token', 'POST', {'grant_type': 'authorization_code', 'code': code,
                          'redirect_uri': redirect, 'code_verifier': verifier})

    def api(path, method='GET', body=None, ref=None):
        headers = {'Authorization': 'Bearer ' + token['access_token'], 'Content-Type': 'application/json'}
        if method == 'POST' and path == '/api/v1/projects':
            headers['Idempotency-Key'] = 'dashboard-empty-' + instance
        if ref is not None:
            headers['If-Match'] = '"' + (ref or 'ref-none') + '"'
        request = urllib.request.Request(origin + path, data=json.dumps(body).encode() if body is not None else None,
                                         method=method, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'{method} {path}: {error.code} {error.read().decode()[:500]}') from None

    me = api('/api/v1/me')
    project = me['default_project_id'] or me['projects'][0]['project_id']
    exported = api('/api/v1/admin/memory-export')
    dataset = sample()
    dataset['projectID'] = project
    dataset['projectName'] = 'clumsies'
    base = api('/api/v1/org/commit-state')['ref']['commit_id']

    def draft(doc, update=False):
        resource = {'type': 'memory', 'scope': 'org', 'id': doc['id'] if update else None, 'path': doc['path']}
        body = (f"# {doc['title']}\n\nThis document is part of the isolated Dashboard demo dataset.\n\n"
                f"## Context\n\nThe team maintains this guidance in `{doc['path'].split('/')[0]}/`. "
                "Changes are proposed as drafts and reviewed before publication.\n\n"
                "## Working agreement\n\n- Verify the current project and source version before making changes.\n"
                "- Preserve local edits during refresh and project switching.\n"
                "- Run the focused regression checks and record the observed result.\n\n"
                "## Validation\n\nConfirm the expected behavior in the local Dev Instance, including recovery after a failed request.\n")
        if update:
            body += '\n## Proposed update\n\nAdd the verification steps observed during the latest release rehearsal.\n'
        result = api('/api/v1/drafts', 'POST', {'daemon_installation_id': 'dashboard-demo-' + instance,
            'project_id': project, 'base_commit_id': base, 'title': doc['title'],
            'description': 'Isolated dashboard demonstration', 'resource': resource,
            'operations': [{'action': 'update' if update else 'create', 'resource': resource, 'content': {'content': body}}]})
        result = result.get('draft', result)
        return {'draft_id': result['draft_id'], 'expected_draft_version': result['version']}

    if exported['memories']:
        assert {m['path'] for m in exported['memories']} == {m['path'] for m in dataset['resources']}, 'Only this demo dataset may be resumed'
        assert all('isolated Dashboard demo dataset' in m['body'] for m in exported['memories']), 'Non-demo documents exist'
    else:
        print(f"Creating {len(dataset['resources'])} demo documents through the local Draft/Review APIs…", flush=True)
        checkpoint = root / 'dashboard-proposals.json'
        if checkpoint.exists():
            proposals = json.loads(checkpoint.read_text())
            assert len(proposals) == len(dataset['resources']), 'Incomplete seed checkpoint'
        else:
            assert not api('/api/v1/drafts?project_id=' + urllib.parse.quote(project))['items'], 'Unexpected existing drafts'
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                proposals = list(pool.map(draft, dataset['resources']))
            checkpoint.write_text(json.dumps(proposals))
        review = api('/api/v1/reviews', 'POST', {'drafts': proposals, 'title': 'Seed Dashboard demonstration knowledge'}, ref=base or '')
        review = review.get('review', review)
        merged = api(f"/api/v1/reviews/{review['review_id']}/merges", 'POST', {'expected_review_version': review['version']}, ref=base or '')
        base = merged['commit_id']
        exported = api('/api/v1/admin/memory-export')
    identifiers = {m['path']: m['memory_id'] for m in exported['memories']}
    mapping = {r['id']: identifiers[r['path']] for r in dataset['resources']}
    for resource in dataset['resources']:
        resource['id'] = mapping[resource['id']]
    for record in dataset['recalls'] + dataset['changes']:
        record['resourceID'] = mapping.get(record['resourceID'], 'historical-' + record['resourceID'])
    current_drafts = [d for d in api('/api/v1/drafts?project_id=' + urllib.parse.quote(project))['items'] if d['status'] in ['open', 'submitted']]
    if current_drafts:
        assert collections.Counter(d['status'] for d in current_drafts) == {'open': 9, 'submitted': 5}, 'Unexpected demo draft state'
    else:
        open_proposals = [draft(doc, update=True) for doc in dataset['resources'][:14]]
        api('/api/v1/reviews', 'POST', {'drafts': open_proposals[9:], 'title': 'Refine release and retrieval guidance'}, ref=base or '')
    empty_project = api('/api/v1/projects', 'POST', {'name': 'Empty project', 'description': 'Dashboard empty-state verification'})
    empty_id = empty_project.get('project', empty_project)['project_id']
    empty = dict(dataset, projectID=empty_id, projectName='Empty project', resources=[], days=[], recalls=[], changes=[],
                 openDrafts=0, submittedDrafts=0)
    fixture_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    fixture_path.write_text(json.dumps(statistics_fixtures(dataset) + statistics_fixtures(empty), ensure_ascii=False))
    fixture_path.chmod(0o600)
    config = {'server_url': origin, 'project_id': project, 'access_token': token['access_token'], 'refresh_token': token['refresh_token']}
    helper = root / 'bin/dashboard-session'
    subprocess.run(['xcrun', 'swiftc', str(worktree / 'dev/dev-login.swift'), '-o', str(helper)], check=True)
    deadline = time.monotonic() + 600
    while subprocess.run(['launchctl', 'print', f"gui/{os.getuid()}/{runtime['identities']['mach_service']}"],
                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        if time.monotonic() >= deadline:
            raise RuntimeError('Dev daemon did not start within ten minutes')
        time.sleep(1)
    subprocess.run([str(helper), instance], input=json.dumps(config), text=True, check=True)
    subprocess.run(['defaults', 'write', runtime['identities']['bundle_id'], 'ClumsiesAgentSetupCompleted', '-bool', 'true'], check=True)
    print(f"Seeded {len(dataset['resources'])} documents, 14 drafts, 90 days of sample statistics and an empty project.")
    print(f"Fixture: {fixture_path}")


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runtime', nargs='?', type=Path)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    if args.check:
        data = sample(dt.datetime(2026, 9, 20, 18, tzinfo=dt.timezone(dt.timedelta(hours=8))))
        assert len(data['days']) == 90 and len(data['resources']) > 300
        assert any(day['failed'] > 10 for day in data['days'])
        assert data == sample(dt.datetime(2026, 9, 20, 18, tzinfo=dt.timezone(dt.timedelta(hours=8))))
        fixtures = statistics_fixtures(data)
        assert [f['period'] for f in fixtures] == [7, 30, 90]
        assert fixtures[-1]['retrieval']['retrievals'] >= fixtures[0]['retrieval']['retrievals']
        print('Dashboard fixture invariants passed.')
    elif args.runtime:
        seed(args.runtime)
    else:
        parser.error('Provide this worktree Dev Instance runtime.json or --check')
