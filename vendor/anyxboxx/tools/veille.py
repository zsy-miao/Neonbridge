"""Veille des projets voisins : nouveautés depuis le dernier passage, et suivi de nos contributions.

    python tools/veille.py [--depuis AAAA-MM-JJ]

Lecture seule, par l'API GitHub (`gh` connecté). Pour chaque dépôt suivi : commits, issues et pull
requests mis à jour depuis le dernier passage (date gardée dans %LOCALAPPDATA%\\AnyXboxX\\veille.json),
puis l'état des issues et pull requests ouvertes par AnyXboxX chez eux.
"""
import json
import os
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

WATCHED = ("omonkeyz/xbox-emulator", "R0Y00/Xbox2Windows")
STATE = Path(os.environ.get("LOCALAPPDATA", Path.home())) / "AnyXboxX" / "veille.json"
FIRST_LOOKBACK = timedelta(days=1)
MAX_LISTED = 15


def gh(path):
    result = subprocess.run(["gh", "api", path], capture_output=True, text=True, encoding="utf-8", timeout=60)
    if result.returncode != 0:
        raise RuntimeError(f"gh api {path} : {result.stderr.strip()}")
    return json.loads(result.stdout or "null")


def section(repo, since, login):
    lines = [f"## {repo}"]
    commits = gh(f"repos/{repo}/commits?since={since}&per_page=100")
    lines.append(f"- {len(commits)} commit(s) depuis {since[:10]}")
    lines += [f"  - `{c['sha'][:7]}` {c['commit']['message'].splitlines()[0]}" for c in commits[:MAX_LISTED]]
    items = gh(f"repos/{repo}/issues?state=all&since={since}&per_page=100&sort=updated")
    pulls = [i for i in items if "pull_request" in i]
    issues = [i for i in items if "pull_request" not in i]
    lines.append(f"- {len(issues)} issue(s) et {len(pulls)} pull request(s) mises à jour")
    for item in (issues + pulls)[:MAX_LISTED]:
        kind = "PR" if "pull_request" in item else "issue"
        lines.append(f"  - {kind} #{item['number']} [{item['state']}] {item['title']}")
    ours = gh(f"search/issues?q=repo:{repo}+author:{login}&per_page=50")["items"]
    if ours:
        lines.append("- Nos contributions :")
        for item in ours:
            kind = "PR" if "pull_request" in item else "issue"
            merged = " (fusionnée)" if kind == "PR" and item["pull_request"].get("merged_at") else ""
            lines.append(f"  - {kind} #{item['number']} [{item['state']}{merged}] {item['title']} — "
                         f"{item['comments']} commentaire(s)")
    return lines


def _since(argv, state, now):
    if "--depuis" in argv and argv.index("--depuis") + 1 < len(argv):
        return datetime.fromisoformat(argv[argv.index("--depuis") + 1]).replace(tzinfo=timezone.utc)
    return datetime.fromisoformat(state["derniere"]) if "derniere" in state else now - FIRST_LOOKBACK


def main(argv):
    state = json.loads(STATE.read_text(encoding="utf-8")) if STATE.is_file() else {}
    now = datetime.now(timezone.utc)
    since = _since(argv, state, now).strftime("%Y-%m-%dT%H:%M:%SZ")
    try:
        login = gh("user")["login"]
        lines = ["# Veille des projets voisins", ""]
        for repo in WATCHED:
            lines += section(repo, since, login) + [""]
    except (RuntimeError, subprocess.TimeoutExpired, json.JSONDecodeError, KeyError) as error:
        print(f"Erreur : {error}", file=sys.stderr)
        return 1
    print("\n".join(lines))
    STATE.parent.mkdir(parents=True, exist_ok=True)
    STATE.write_text(json.dumps({"derniere": now.isoformat()}), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
