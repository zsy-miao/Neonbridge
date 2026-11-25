"""Mise à jour automatique des voisins (tools/sync_voisins.py), sans réseau : amont simulé."""
import sys
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import sync_voisins as sv  # noqa: E402

# Voisin factice : copie de fichiers sous « tools/ », comme le faisait Leonida avant sa suppression.
COPIE = sv.Voisin("Factice", "exemple/factice", "third_party/factice", prefixe="tools/",
                  portes=("runtime/src/gfx10_descriptors.h", "tools/gfx10_formats.json"))
X2W = sv.VOISINS[1]
ANCIEN, NOUVEAU = "1628210170", "aa2988cc555bad7ecd508b48a702adbb32fe7c95"
README = f"Copie depuis Factice, dossier `tools/`, commit `{ANCIEN}` (22 novembre 2026, 20 h). Licence GPL.\n"
SOIR = datetime(2026, 11, 22, 23, 5)


def _copie(racine, fichiers, readme=README):
    dossier = racine / COPIE.dossier
    dossier.mkdir(parents=True)
    (dossier / "README.md").write_text(readme, encoding="utf-8")
    for nom, contenu in fichiers.items():
        (dossier / nom).write_bytes(contenu)
    (racine / "THIRD_PARTY.md").write_text(f"| Factice | commit `{ANCIEN}` | GPL |\r\n", encoding="utf-8",
                                           newline="")
    return dossier


def _amont(fichiers):
    appels = []

    def charger(depot, commit, chemin):
        appels.append((depot, commit, chemin))
        return fichiers.get(chemin)
    return charger, appels


def test_pinned_commit_and_replacement_keep_length_and_update_date():
    assert sv.commit_epingle(README) == ANCIEN
    texte = sv.remplacer_commit(README, ANCIEN, NOUVEAU, sv.date_fr(SOIR))
    assert f"commit `{NOUVEAU[:10]}` (22 novembre 2026, 23 h)" in texte
    assert sv.remplacer_commit("commit `ff7f68a` ici", "ff7f68a", NOUVEAU) == f"commit `{NOUVEAU[:7]}` ici"
    assert sv.remplacer_commit("commit `abcdef0`", ANCIEN, NOUVEAU) == "commit `abcdef0`"   # autre commit


def test_copy_downloads_every_file_from_upstream_prefix(tmp_path):
    dossier = _copie(tmp_path, {"LICENSE": b"GPL\r\n", "shader_ir.py": b"x = 1\r\n"})
    charger, appels = _amont({"LICENSE": b"GPL\n", "tools/shader_ir.py": b"x = 2\n"})
    assert sv.recopier(COPIE, tmp_path, NOUVEAU, charger) == ("", [])
    assert (dossier / "shader_ir.py").read_bytes() == b"x = 2\n"
    assert {c[2] for c in appels} == {"LICENSE", "tools/shader_ir.py"}
    assert all(c[1] == NOUVEAU for c in appels)


def test_changed_licence_or_missing_file_blocks_without_writing(tmp_path):
    dossier = _copie(tmp_path, {"LICENSE": b"GPL\n", "a.py": b"x = 1\n"})
    bloque, _ = sv.recopier(COPIE, tmp_path, NOUVEAU, _amont({"LICENSE": b"MIT\n", "tools/a.py": b"y\n"})[0])
    assert "licence" in bloque
    bloque, _ = sv.recopier(COPIE, tmp_path, NOUVEAU, _amont({"LICENSE": b"GPL\n"})[0])
    assert "tools/a.py" in bloque and "disparu" in bloque
    assert (dossier / "a.py").read_bytes() == b"x = 1\n"


def test_new_sibling_import_is_copied_but_old_optional_imports_are_ignored(tmp_path):
    dossier = _copie(tmp_path, {"LICENSE": b"L\n", "a.py": b"import numpy\n"})
    charger, _ = _amont({"LICENSE": b"L\n", "tools/a.py": b"import numpy\nimport gate\nimport absent\n",
                         "tools/gate.py": b"import json\n"})
    bloque, notes = sv.recopier(COPIE, tmp_path, NOUVEAU, charger)
    assert bloque == ""
    assert (dossier / "gate.py").read_bytes() == b"import json\n"
    texte = "\n".join(notes)
    assert "`gate.py`" in texte and "`absent`" in texte and "numpy" not in texte


def test_commit_numbers_updated_and_line_endings_kept(tmp_path):
    dossier = _copie(tmp_path, {})
    sv.mettre_a_jour_commits(COPIE, tmp_path, ANCIEN, NOUVEAU, SOIR)
    assert f"commit `{NOUVEAU[:10]}` (22 novembre 2026, 23 h)" in (dossier / "README.md").read_text(encoding="utf-8")
    assert (tmp_path / "THIRD_PARTY.md").read_bytes() == f"| Factice | commit `{NOUVEAU[:10]}` | GPL |\r\n".encode()


def test_comparison_summary_flags_rewritten_history_licence_and_ported_files():
    comparaison = {"status": "diverged", "behind_by": 3, "ahead_by": 2,
                   "commits": [{"sha": "1" * 40, "commit": {"message": "premier\n\ncorps"}},
                               {"sha": "2" * 40, "commit": {"message": "second"}}],
                   "files": [{"filename": "LICENSE"}, {"filename": "src/era_heap.cpp"}]}
    lignes, bloque = sv.resume_comparaison(comparaison, X2W)
    texte = "\n".join(lignes)
    assert "historique réécrit" in texte and "2 nouveau(x) commit(s)" in texte
    assert texte.index("`2222222` second") < texte.index("`1111111` premier")   # plus récent d'abord
    assert "`src/era_heap.cpp`" in texte and "licence" in bloque
    assert sv.resume_comparaison(None, X2W)[1] == ""


def test_report_and_commit_message():
    a_jour = sv.Bilan(sv.VOISINS[0], ancien="a541bbb823", nouveau="a541bbb823ffff")
    applique = sv.Bilan(COPIE, ancien=ANCIEN, nouveau=NOUVEAU, applique=True, lignes=["- 1 nouveau(x) commit(s)"])
    erreur = sv.Bilan(X2W, erreur="réseau")
    texte = sv.rapport([a_jour, applique, erreur])
    assert "à jour (`a541bbb823`)" in texte and "mis à jour" in texte and "❌ erreur : réseau" in texte
    message = sv.message_commit([a_jour, applique, erreur])
    assert message.startswith("chore: voisins à jour automatiquement (Factice aa2988c)")
    assert "TomBox" not in message


def test_every_tracked_submodule_is_declared_in_gitmodules_with_its_upstream_url():
    import configparser
    declares = configparser.ConfigParser()
    declares.read(sv.RACINE / ".gitmodules", encoding="utf-8")
    urls = {declares[s]["path"]: declares[s]["url"] for s in declares.sections()}
    sous_modules = [v for v in sv.VOISINS if v.sous_module]
    assert {v.dossier for v in sous_modules} >= {"third_party/tombox"}
    for voisin in sous_modules:
        assert urls.get(voisin.dossier, "").removesuffix(".git").endswith(voisin.depot), voisin.nom



def test_victory_is_pinned_by_hand_and_a_tracked_branch_is_queried_by_name(monkeypatch):
    assert all(v.nom != "VIctory" for v in sv.VOISINS)      # épinglé sur le fork, jamais avancé automatiquement
    victory = sv.Voisin("Factice", "exemple/factice", "third_party/factice", sous_module=True, branche="anyxboxx/stable")
    assert all(v.branche == "" for v in sv.VOISINS)
    appels = []

    def faux_git(*args, **_):
        appels.append(args)
        return "a" * 40 + "	" + args[-1]
    monkeypatch.setattr(sv, "git", faux_git)
    assert sv.dernier_commit(victory.depot, victory.branche) == "a" * 40
    assert appels[-1][-1] == "refs/heads/anyxboxx/stable"
    sv.dernier_commit("exemple/factice")
    assert appels[-1][-1] == "HEAD"


def test_missing_branch_is_an_explicit_error(monkeypatch):
    monkeypatch.setattr(sv, "git", lambda *args, **_: "")
    try:
        sv.dernier_commit("exemple/factice", "absente")
    except RuntimeError as erreur:
        assert "refs/heads/absente" in str(erreur)
    else:
        raise AssertionError("une branche absente doit lever une erreur")
