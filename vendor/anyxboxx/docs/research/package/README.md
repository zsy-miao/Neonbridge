> Français : [README.md](../../fr/recherche/paquet/README.md)

# Reverse-engineering notes on the studied Scarlett package

Notes written by Codex during the static analysis of the Xbox Series X package placed next to the repository. They describe formats observed on this copy (RDS5 and ETPF tables, RPF headers, package configuration). They contain no game file.

| Note | Content |
| --- | --- |
| [GAME_ANALYSIS.md](GAME_ANALYSIS.md) | Package layout on disk, TTPF containers, script archives, Windows launcher, binary configuration. |
| [ETPF_ANALYSIS.md](ETPF_ANALYSIS.md) | ETPF index, mapping to RDS5 blocks, candidate dimensions, block families, pixel layout experiments. |
| [RPF_RESEARCH.md](RPF_RESEARCH.md) | Observed RPF header fields and internal candidates. |
| [REA_RESEARCH.md](REA_RESEARCH.md) | Inspection with REA: .NET launcher, `Catalog000.bin`, FidelityFX libraries. |
| [../plan-compatibilite-paquet.md](../package-compatibility-plan.md) | Work plan for compatibility, and its limits. |
| [../pistes-premier-executable.md](../first-executable-leads.md) | Diagnosis of the future executable, limits of the import report and test leads. |

The commands `python .\analyse_paquet.py`, `inspect_etpf.py`, `inspect_rpf.py`, `stats_etpf.py`, `comparer_*.py` and `inspect_zlib_padding.py` cited in these notes are run from the package folder, where these scripts are located. They are not versioned here. The figures and fingerprints only hold for the examined copy, whose origin has not been authenticated.
