"""Mise à jour automatique des voisins : TomBox et Xbox2Windows (VIctory n'est pas suivi : third_party/victory est notre code).

    python -I tools/sync_voisins.py [--appliquer] [--rapport FICHIER] [--message FICHIER]

Sans --appliquer : lecture seule, compare les versions épinglées aux dernières versions publiées.
Avec --appliquer : avance les sous-modules (TomBox), recopie sans modification les fichiers déjà copiés dans
third_party/xbox2windows depuis le dernier commit, et change les numéros de commit
dans leurs README et dans THIRD_PARTY.md. Rien n'est fusionné : le workflow voisins.yml pousse la branche
auto/voisins, que la CI teste ; un mainteneur la relit et ouvre la pull request. Bibliothèque standard seulement ; le code téléchargé n'est jamais exécuté.

Un voisin n'est pas mis à jour (et le rapport le dit) si sa licence a changé ou si un fichier copié a
disparu en amont. Les fichiers portés à la main (Rust, C++) ne sont jamais touchés : le rapport signale
seulement les commits qui les modifient.
"""
import ast
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

FUSEAU = "Europe/Paris"
RACINE = Path(__file__).resolve().parents[1]
TIMEOUT = 60
TAILLE_MAX = 8 * 1024 * 1024
COMMITS_LISTES = 12
FICHIERS_MAX_API = 300                                  # l'API compare ne liste pas plus de fichiers
DEPENDANCES_MAX = 64                                    # garde-fou : nombre de fichiers d'une copie
MOIS = ("janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre",
        "novembre", "décembre")
COMMIT_README = re.compile(r"commit `([0-9a-f]{7,40})`( \([^)]*\))?")


@dataclass(frozen=True)
class Voisin:
    nom: str
    depot: str
    dossier: str
    sous_module: bool = False
    prefixe: str = ""                                   # dossier amont des fichiers copiés
    portes: tuple = ()                                  # fichiers amont portés à la main
    branche: str = ""                                   # branche suivie ; vide = branche par défaut du dépôt


VOISINS = (
    Voisin("TomBox", "omonkeyz/xbox-emulator", "third_party/tombox", sous_module=True),
    Voisin("Xbox2Windows", "R0Y00/Xbox2Windows", "third_party/xbox2windows",
           portes=("src/era_heap.cpp", "src/era_heap.h", "src/era_query.cpp", "tools/bc7_blocks.py",
                   "tools/bc7_tiles.py")),
    # VIctory n'est pas suivi : third_party/victory est notre code, sans amont.
)


@dataclass
class Bilan:
    voisin: Voisin
    ancien: str = ""
    nouveau: str = ""
    lignes: list = field(default_factory=list)
    bloque: str = ""                                    # raison de ne pas appliquer
    erreur: str = ""
    applique: bool = False

    @property
    def a_jour(self):
        return bool(self.ancien) and self.nouveau.startswith(self.ancien)


# --- fonctions pures ------------------------------------------------------------------------------

def commit_epingle(texte):
    match = COMMIT_README.search(texte)
    if not match:
        raise ValueError("aucun « commit `…` » dans le README de la copie")
    return match.group(1)


def date_fr(moment):
    return f"{moment.day} {MOIS[moment.month - 1]} {moment.year}, {moment.hour} h"


def remplacer_commit(texte, ancien, nouveau, date_txt=None):
    """Remplace le numéro de commit (même longueur que l'ancien) et, si date_txt, la date qui le suit."""
    court = nouveau[:len(ancien)]

    def remplace(match):
        if match.group(1) != ancien:
            return match.group(0)
        suite = f" ({date_txt})" if date_txt and match.group(2) else (match.group(2) or "")
        return f"commit `{court}`{suite}"
    return COMMIT_README.sub(remplace, texte)


def chemin_amont(voisin, relatif):
    return relatif if relatif == "LICENSE" else voisin.prefixe + relatif


def fichiers_copies(dossier):
    return sorted(p.relative_to(dossier).as_posix() for p in dossier.rglob("*")
                  if p.is_file() and p.name != "README.md" and "__pycache__" not in p.parts)


def resume_comparaison(comparaison, voisin):
    """Lignes du rapport tirées de l'API compare de GitHub, et raison de blocage éventuelle."""
    if comparaison is None:
        return ["- comparaison indisponible (API GitHub) : relire les commits à la main"], ""
    lignes, bloque = [], ""
    if comparaison.get("status") == "diverged":
        lignes.append(f"- ⚠ historique réécrit en amont ({comparaison.get('behind_by', '?')} commit(s) "
                      "disparus de leur branche) : vérifier que rien d'utile n'a été perdu")
    commits = comparaison.get("commits", [])
    lignes.append(f"- {comparaison.get('ahead_by', len(commits))} nouveau(x) commit(s)")
    for commit in commits[-COMMITS_LISTES:][::-1]:
        lignes.append(f"  - `{commit['sha'][:7]}` {commit['commit']['message'].splitlines()[0]}")
    touches = {f["filename"] for f in comparaison.get("files", [])}
    if len(touches) >= FICHIERS_MAX_API:
        lignes.append(f"- liste des fichiers modifiés tronquée par GitHub ({FICHIERS_MAX_API}) : licence et "
                      "fichiers portés vérifiés en partie seulement")
    licences = sorted(n for n in touches if Path(n).name.upper().startswith(("LICENSE", "COPYING")))
    if licences:
        bloque = "licence modifiée en amont (" + ", ".join(licences) + ") : mise à jour manuelle"
    portes = sorted(touches.intersection(voisin.portes))
    if portes:
        lignes.append("- à porter à la main (code repris en Rust/C++) : " + ", ".join(f"`{p}`" for p in portes))
    return lignes, bloque


def imports_manquants(contenu, deja_copies):
    """Modules importés par un fichier Python copié qui ne sont ni standard ni déjà copiés."""
    try:
        arbre = ast.parse(contenu)
    except SyntaxError:
        return []
    noms = set()
    for noeud in ast.walk(arbre):
        if isinstance(noeud, ast.Import):
            noms.update(alias.name.split(".")[0] for alias in noeud.names)
        elif isinstance(noeud, ast.ImportFrom) and noeud.module and not noeud.level:
            noms.add(noeud.module.split(".")[0])
    locaux = {Path(p).stem for p in deja_copies}
    return sorted(n for n in noms if n not in sys.stdlib_module_names and n not in locaux)


# --- accès au réseau et à git ---------------------------------------------------------------------

def git(*args, cwd=RACINE):
    result = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, timeout=TIMEOUT)
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} : {result.stderr.strip()}")
    return result.stdout.strip()


def reference_distante(branche):
    """Référence interrogée sur le dépôt distant : la branche suivie, sinon HEAD."""
    return f"refs/heads/{branche}" if branche else "HEAD"


def dernier_commit(depot, branche=""):
    reference = reference_distante(branche)
    sortie = git("ls-remote", f"https://github.com/{depot}", reference)
    if not sortie:
        raise RuntimeError(f"{depot} : {reference} introuvable")
    return sortie.split()[0]


def _ouvrir(url, api=False):
    entetes = {"User-Agent": "anyxboxx-sync-voisins"}
    jeton = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    if api and jeton:
        entetes["Authorization"] = f"Bearer {jeton}"
    with urllib.request.urlopen(urllib.request.Request(url, headers=entetes), timeout=TIMEOUT) as reponse:
        donnees = reponse.read(TAILLE_MAX + 1)
    if len(donnees) > TAILLE_MAX:
        raise RuntimeError(f"{url} : plus de {TAILLE_MAX} octets")
    return donnees


def telecharger(depot, commit, chemin):
    """Octets du fichier au commit donné ; None s'il n'existe plus."""
    try:
        return _ouvrir(f"https://raw.githubusercontent.com/{depot}/{commit}/{chemin}")
    except urllib.error.HTTPError as erreur:
        if erreur.code == 404:
            return None
        raise RuntimeError(f"{depot}/{chemin} : HTTP {erreur.code}") from erreur
    except urllib.error.URLError as erreur:
        raise RuntimeError(f"{depot}/{chemin} : {erreur.reason}") from erreur


def comparer(depot, ancien, nouveau):
    try:
        return json.loads(_ouvrir(f"https://api.github.com/repos/{depot}/compare/{ancien}...{nouveau}", api=True))
    except (urllib.error.URLError, json.JSONDecodeError, RuntimeError):
        return None


# --- mise à jour ----------------------------------------------------------------------------------

def epingle(voisin, racine):
    if voisin.sous_module:
        return git("ls-tree", "HEAD", voisin.dossier, cwd=racine).split()[2]
    return commit_epingle((racine / voisin.dossier / "README.md").read_text(encoding="utf-8"))


def _lf(donnees):
    """Fins de ligne normalisées : une copie Windows (core.autocrlf) a des CRLF, GitHub des LF."""
    return donnees.replace(b"\r\n", b"\n")


def recopier(voisin, racine, nouveau, charger=telecharger):
    """Télécharge tous les fichiers copiés ; n'écrit rien si l'un d'eux manque. Renvoie (bloque, notes)."""
    dossier = racine / voisin.dossier
    relatifs = fichiers_copies(dossier)
    contenus = {}
    for relatif in relatifs:
        donnees = charger(voisin.depot, nouveau, chemin_amont(voisin, relatif))
        if donnees is None:
            return f"`{chemin_amont(voisin, relatif)}` a disparu en amont : mise à jour manuelle", []
        contenus[relatif] = donnees
    licence = contenus.get("LICENSE")
    if licence is not None and _lf(licence) != _lf((dossier / "LICENSE").read_bytes()):
        return "licence modifiée en amont : mise à jour manuelle", []
    notes = ajouter_dependances(voisin, dossier, nouveau, contenus, charger)
    for relatif, donnees in contenus.items():
        (dossier / relatif).parent.mkdir(parents=True, exist_ok=True)
        (dossier / relatif).write_bytes(donnees)
    return "", notes


def ajouter_dependances(voisin, dossier, nouveau, contenus, charger):
    """Copie aussi les modules que les fichiers copiés se mettent à importer, s'ils sont à côté en amont."""
    notes, a_voir = [], [r for r in contenus if r.endswith(".py")]
    while a_voir and len(contenus) < DEPENDANCES_MAX:
        relatif = a_voir.pop()
        ancien = dossier / relatif
        avant = set(imports_manquants(ancien.read_bytes(), contenus)) if ancien.is_file() else set()
        for module in sorted(set(imports_manquants(contenus[relatif], contenus)) - avant):
            voisin_rel = (Path(relatif).parent / f"{module}.py").as_posix()
            donnees = charger(voisin.depot, nouveau, chemin_amont(voisin, voisin_rel))
            if donnees is None:
                notes.append(f"- `{relatif}` importe désormais `{module}`, introuvable à côté en amont : à vérifier")
                continue
            contenus[voisin_rel] = donnees
            a_voir.append(voisin_rel)
            notes.append(f"- nouvelle dépendance copiée : `{voisin_rel}` (importée par `{relatif}`), "
                         "à ajouter au tableau du README de la copie")
    return notes


def mettre_a_jour_commits(voisin, racine, ancien, nouveau, maintenant):
    readme = racine / voisin.dossier / "README.md"
    fichiers = [racine / "THIRD_PARTY.md"] + ([] if voisin.sous_module else [readme])
    for chemin in fichiers:
        with open(chemin, encoding="utf-8", newline="") as fichier:     # garde les fins de ligne
            texte = fichier.read()
        date_txt = date_fr(maintenant) if chemin == readme else None
        nouveau_texte = remplacer_commit(texte, ancien, nouveau, date_txt)
        if nouveau_texte != texte:
            with open(chemin, "w", encoding="utf-8", newline="") as fichier:
                fichier.write(nouveau_texte)


def examiner(voisin, racine, appliquer, maintenant, charger=telecharger):
    bilan = Bilan(voisin)
    try:
        bilan.ancien = epingle(voisin, racine)
        bilan.nouveau = dernier_commit(voisin.depot, voisin.branche)
        if bilan.a_jour:
            return bilan
        bilan.lignes, bilan.bloque = resume_comparaison(comparer(voisin.depot, bilan.ancien, bilan.nouveau), voisin)
        if not appliquer or bilan.bloque:
            return bilan
        if voisin.sous_module:
            git("update-index", "--cacheinfo", f"160000,{bilan.nouveau},{voisin.dossier}", cwd=racine)
        else:
            bilan.bloque, notes = recopier(voisin, racine, bilan.nouveau, charger)
            bilan.lignes += notes
            if bilan.bloque:
                return bilan
        mettre_a_jour_commits(voisin, racine, bilan.ancien, bilan.nouveau, maintenant)
        bilan.applique = True
    except (RuntimeError, ValueError, OSError, IndexError, subprocess.TimeoutExpired) as erreur:
        bilan.erreur = str(erreur)
    return bilan


# --- rapport --------------------------------------------------------------------------------------

def rapport(bilans):
    lignes = ["# Voisins : nouveautés", ""]
    for bilan in bilans:
        v = bilan.voisin
        titre = f"## {v.nom} ([{v.depot}](https://github.com/{v.depot}))"
        if bilan.erreur:
            lignes += [titre, f"- ❌ erreur : {bilan.erreur}", ""]
            continue
        if bilan.a_jour:
            lignes += [titre, f"- à jour (`{bilan.ancien[:10]}`)", ""]
            continue
        etat = "mis à jour" if bilan.applique else ("⛔ non appliqué : " + bilan.bloque if bilan.bloque else "en retard")
        lignes += [titre, f"- `{bilan.ancien[:10]}` → `{bilan.nouveau[:10]}` : {etat}",
                   f"- [voir les changements](https://github.com/{v.depot}/compare/{bilan.ancien}...{bilan.nouveau})"]
        lignes += bilan.lignes + [""]
    return "\n".join(lignes)


def message_commit(bilans):
    appliques = [b for b in bilans if b.applique]
    resume = ", ".join(f"{b.voisin.nom} {b.nouveau[:7]}" for b in appliques)
    corps = [f"- {b.voisin.nom} {b.ancien[:10]} -> {b.nouveau[:10]}" for b in appliques]
    return "\n".join([f"chore: voisins à jour automatiquement ({resume})", "", *corps,
                      "", "Copies sans modification ; la CI vérifie la suite complète.", ""])


def _option(argv, nom):
    if nom in argv and argv.index(nom) + 1 < len(argv):
        return Path(argv[argv.index(nom) + 1])
    return None


def heure_de_paris():
    try:
        return datetime.now(ZoneInfo(FUSEAU))
    except ZoneInfoNotFoundError:                       # Windows sans le paquet tzdata : heure locale
        return datetime.now(timezone.utc).astimezone()


def main(argv, racine=RACINE):
    appliquer = "--appliquer" in argv
    maintenant = heure_de_paris()
    bilans = [examiner(v, racine, appliquer, maintenant) for v in VOISINS]
    texte = rapport(bilans)
    if hasattr(sys.stdout, "reconfigure"):              # console Windows en cp1252, même avec python -I
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    print(texte)
    if (chemin := _option(argv, "--rapport")) is not None:
        chemin.write_text(texte + "\n", encoding="utf-8")
    change = any(b.applique for b in bilans)
    if change and (chemin := _option(argv, "--message")) is not None:
        chemin.write_text(message_commit(bilans), encoding="utf-8")
    if sortie := os.environ.get("GITHUB_OUTPUT"):
        with open(sortie, "a", encoding="utf-8") as fichier:
            fichier.write(f"changed={'true' if change else 'false'}\n")
    if all(b.erreur for b in bilans):
        print("Erreur : aucun voisin n'a pu être examiné", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
