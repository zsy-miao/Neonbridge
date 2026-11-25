"""Outil de veille (tools/veille.py) : date de départ et rapport, sans appel réseau (gh simulé)."""
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import veille  # noqa: E402

NOW = datetime(2026, 11, 21, 20, 0, tzinfo=timezone.utc)


def test_start_date_comes_from_option_then_state_then_one_day():
    assert veille._since(["--depuis", "2026-11-01"], {}, NOW) == datetime(2026, 11, 1, tzinfo=timezone.utc)
    assert veille._since([], {"derniere": "2026-11-20T10:00:00+00:00"}, NOW).day == 20
    assert veille._since([], {}, NOW) == NOW - timedelta(days=1)


def test_section_lists_news_and_our_contributions(monkeypatch):
    answers = {                                                   # « search » avant « issues »
        "search": {"items": [{"number": 9, "state": "closed", "title": "Patch", "comments": 2,
                              "pull_request": {"merged_at": "2026-11-21T19:00:00Z"}}]},
        "commits": [{"sha": "abcdef123", "commit": {"message": "gpu: fix\n\ncorps"}}],
        "issues": [{"number": 5, "state": "open", "title": "Bug"},
                   {"number": 9, "state": "closed", "title": "Patch", "pull_request": {}}],
    }
    monkeypatch.setattr(veille, "gh", lambda path: next(v for k, v in answers.items() if k in path))
    text = "\n".join(veille.section("x/y", "2026-11-21T00:00:00Z", "moi"))
    assert "1 commit(s)" in text and "`abcdef1` gpu: fix" in text
    assert "1 issue(s) et 1 pull request(s)" in text
    assert "PR #9 [closed (fusionnée)] Patch — 2 commentaire(s)" in text
