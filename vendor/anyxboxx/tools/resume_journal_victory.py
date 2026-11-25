"""Résume le journal d'un lancement de VIctory : où va le temps d'une image du monde.

VIctory écrit toutes les ~5 s une ligne d'images (`victory_gpu::host`) :
« 3.9 fps, 255.9 ms/frame (92.1 preparing: copies 49.5 ms 126.8 MB, textures 7.4 ms 28.5 MB; 146.2 on the GPU;
165.2 copying back 62.3 MB), 5218 draws/frame, dropped 348 without a host pipeline (2 being made) and … ».
Ce script garde les lignes du monde (au moins `--draws-min` draws par image ; les menus en ont moins), en donne la
médiane, le minimum et le maximum par colonne, et compte les avertissements qui expliquent une image noire ou une
perte du périphérique. Avec deux journaux, il les compare colonne par colonne (médianes, écart en %), pour juger un
changement sur la partie de l'image qu'il vise plutôt que sur la seule cadence.

    python tools/resume_journal_victory.py victory.log
    python tools/resume_journal_victory.py avant.log apres.log --markdown

Le journal sort sur stderr : `victory.exe run … 2> victory.log`, ou `*> victory.log` en PowerShell (UTF-16 accepté).
Seuls des comptes et des temps sont écrits : rien du jeu.
"""
import argparse
import re
import statistics
import sys
from pathlib import Path

NOMBRE = r"([0-9.]+|inf|NaN)"
LIGNE_IMAGE = re.compile(
    rf"{NOMBRE} fps, {NOMBRE} ms/frame \({NOMBRE} preparing: copies {NOMBRE} ms {NOMBRE} MB, "
    rf"textures {NOMBRE} ms {NOMBRE} MB; {NOMBRE} on the GPU; {NOMBRE} copying back {NOMBRE} MB\), "
    rf"{NOMBRE} draws/frame, dropped {NOMBRE} without a host pipeline \((\d+) being made\) and {NOMBRE} indirect")
# (clé, libellé), dans l'ordre des groupes de LIGNE_IMAGE.
COLONNES = (
    ("fps", "fps"), ("image", "ms/frame"), ("preparation", "preparing (ms)"), ("copies", "copies (ms)"),
    ("copies_mo", "copies (MB)"), ("textures", "textures (ms)"), ("textures_mo", "textures (MB)"),
    ("gpu", "on the GPU (ms)"), ("retour", "copying back (ms)"), ("retour_mo", "copying back (MB)"),
    ("draws", "draws/frame"), ("perdus", "dropped without a host pipeline"), ("en_cours", "pipelines being made"),
    ("indirects", "dropped indirect"),
)
# Avertissements comptés : (libellé, motif cherché dans la ligne).
AVERTISSEMENTS = (
    ("shader does not translate", "does not translate"),
    ("host pipeline refused", "host pipeline refused"),
    ("pipeline method not implemented", "pipeline creation method that is not implemented"),
    ("host GPU device removed", "device removed"),
)
ANSI = re.compile(r"\x1b\[[0-9;]*m")
NIVEAU_ALERTE = re.compile(r"\b(WARN|ERROR)\b")   # niveau tracing de la ligne
DRAWS_MIN = 100                                  # sous ce seuil, une ligne vient des menus ou d'un chargement


def lire(chemin):
    """Texte d'un journal : UTF-16 si une marque d'ordre l'annonce (redirection PowerShell 5), UTF-8 sinon."""
    brut = Path(chemin).read_bytes()
    if brut[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return brut.decode("utf-16", errors="replace")
    return brut.decode("utf-8-sig", errors="replace")


def analyser(texte, draws_min=DRAWS_MIN):
    """{"images": [dict par ligne du monde], "lignes": lignes d'images en tout, "avertissements": {libellé: n}}."""
    images, lignes = [], 0
    avertissements = {libelle: 0 for libelle, _ in AVERTISSEMENTS}
    for ligne in texte.splitlines():
        ligne = ANSI.sub("", ligne)
        if NIVEAU_ALERTE.search(ligne):
            for libelle, motif in AVERTISSEMENTS:
                avertissements[libelle] += motif in ligne
        trouve = LIGNE_IMAGE.search(ligne)
        if not trouve:
            continue
        lignes += 1
        valeurs = dict(zip((cle for cle, _ in COLONNES), (float(v) for v in trouve.groups())))
        if valeurs["draws"] >= draws_min:
            images.append(valeurs)
    return {"images": images, "lignes": lignes, "avertissements": avertissements}


def resumer(images):
    """{clé: (médiane, minimum, maximum)} sur les lignes du monde ; {} sans ligne."""
    if not images:
        return {}
    return {cle: (statistics.median(i[cle] for i in images), min(i[cle] for i in images),
                  max(i[cle] for i in images)) for cle, _ in COLONNES}


def _nombre(valeur):
    return f"{valeur:.0f}" if abs(valeur) >= 100 else f"{valeur:.1f}"


def tableau_seul(analyse, markdown=False):
    """Lignes de texte : médiane (min-max) par colonne, puis les avertissements."""
    resume = resumer(analyse["images"])
    tete = f"{len(analyse['images'])} lignes du monde sur {analyse['lignes']} lignes d'images"
    if not resume:
        return [tete + " : rien à résumer."]
    sortie = [tete, ""]
    if markdown:
        sortie += ["| | median | range |", "|---|---:|---:|"]
    for cle, libelle in COLONNES:
        med, bas, haut = resume[cle]
        plage = f"{_nombre(bas)}-{_nombre(haut)}"
        sortie.append(f"| {libelle} | {_nombre(med)} | {plage} |" if markdown else f"{libelle:<34}{_nombre(med):>9}"
                      f"   ({plage})")
    return sortie + _avertissements(analyse, markdown)


def tableau_compare(avant, apres, markdown=False):
    """Lignes de texte : médianes des deux journaux et écart relatif, par colonne."""
    ra, rb = resumer(avant["images"]), resumer(apres["images"])
    tete = f"A : {len(avant['images'])} lignes du monde ; B : {len(apres['images'])} lignes du monde"
    if not ra or not rb:
        return [tete + " : un des journaux n'a aucune ligne du monde."]
    sortie = [tete, ""]
    if markdown:
        sortie += ["| median | A | B | B vs A |", "|---|---:|---:|---:|"]
    for cle, libelle in COLONNES:
        a, b = ra[cle][0], rb[cle][0]
        ecart = f"{(b - a) / a * 100:+.0f} %" if a else "n/a"
        sortie.append(f"| {libelle} | {_nombre(a)} | {_nombre(b)} | {ecart} |" if markdown
                      else f"{libelle:<34}{_nombre(a):>9}{_nombre(b):>9}{ecart:>9}")
    return sortie


def _avertissements(analyse, markdown):
    lignes = ["", "Warnings:" if markdown else "Avertissements :"]
    for libelle, n in analyse["avertissements"].items():
        lignes.append(f"- {libelle}: {n}" if markdown else f"  {libelle:<32}{n:>7}")
    return lignes


def main(argv=None):
    parseur = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parseur.add_argument("journaux", nargs="+", help="un journal à résumer, ou deux à comparer (A puis B)")
    parseur.add_argument("--draws-min", type=float, default=DRAWS_MIN,
                         help=f"draws par image à partir desquels une ligne compte comme du monde ({DRAWS_MIN})")
    parseur.add_argument("--markdown", action="store_true", help="tableaux Markdown, à coller dans une issue")
    args = parseur.parse_args(argv)
    if len(args.journaux) > 2:
        parseur.error("un ou deux journaux")
    analyses = [analyser(lire(j), args.draws_min) for j in args.journaux]
    sortie = tableau_seul(analyses[0], args.markdown) if len(analyses) == 1 else tableau_compare(*analyses,
                                                                                                markdown=args.markdown)
    print("\n".join(sortie))
    return 0 if all(a["images"] for a in analyses) else 1


if __name__ == "__main__":
    sys.exit(main())
