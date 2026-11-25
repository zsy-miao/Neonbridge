> English: [README.md](../../../research/package/README.md)

# Notes de rétro-ingénierie du paquet Scarlett étudié

Notes rédigées par Codex pendant l'analyse statique du paquet Xbox Series X placé à côté du dépôt. Elles décrivent des formats observés sur cette copie (tables RDS5 et ETPF, en-têtes RPF, configuration du paquet). Elles ne contiennent aucun fichier du jeu.

| Note | Contenu |
| --- | --- |
| [ANALYSE_JEU.md](ANALYSE_JEU.md) | Organisation du paquet sur disque, conteneurs TTPF, archives de scripts, lanceur Windows, configuration binaire. |
| [ANALYSE_ETPF.md](ANALYSE_ETPF.md) | Index ETPF, correspondance avec les blocs RDS5, dimensions candidates, familles de blocs, essais d'agencement de pixels. |
| [RPF_RECHERCHE.md](RPF_RECHERCHE.md) | Champs observés des en-têtes RPF et candidats internes. |
| [REA_RECHERCHE.md](REA_RECHERCHE.md) | Inspection avec REA : lanceur .NET, `Catalog000.bin`, bibliothèques FidelityFX. |
| [../plan-compatibilite-paquet.md](../plan-compatibilite-paquet.md) | Plan de travail pour la compatibilité, et ses limites. |
| [../pistes-premier-executable.md](../pistes-premier-executable.md) | Diagnostic du futur exécutable, limites du rapport d'imports et pistes de tests. |

Les commandes `python .\analyse_paquet.py`, `inspect_etpf.py`, `inspect_rpf.py`, `stats_etpf.py`, `comparer_*.py` et `inspect_zlib_padding.py` citées dans ces notes se lancent depuis le dossier du paquet, où se trouvent ces scripts. Ils ne sont pas versionnés ici. Les chiffres et empreintes ne valent que pour la copie examinée, dont la provenance n'a pas été authentifiée.
