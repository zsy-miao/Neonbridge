# Xbox2Windows : retarget du DXIL console (copie)

Fichiers copiés **sans modification** depuis [R0Y00/Xbox2Windows](https://github.com/R0Y00/Xbox2Windows),
commit `708f4ab` (23 novembre 2026, 15 h). Licence : MIT (fichier `LICENSE`). Tout le mérite de ce code
revient aux contributeurs de Xbox2Windows.

| Fichier | Rôle |
| --- | --- |
| `tools/dxil_retarget.py` | réécrit un module DXIL console (`dxil-ms-xb2`) en DXIL du PC (`dxil-ms-dx`), sans repasser par HLSL, puis re-signe le conteneur ; mode `--pe` pour les conteneurs embarqués dans un exécutable |
| `tools/resource_inventory.py`, `tools/verify_build.py` | lecture des parties d'un conteneur DXBC, dépendances du précédent |
| `tools/call_evidence.py`, `tools/probe_roots.py`, `tools/pe_inventory.py` | inventaire des conteneurs de shaders d'un PE et validation par le validateur DXIL du PC (chargés à la demande par `--pe` et le contrôle sur GPU) |
| `tools/xbox_intrinsic_gate.py` | refuse un module qui appelle encore des intrinsèques `xbox.*` non résolues avant la soumission d'un PSO |
| `tests/dxil_retarget_check.py`, `tests/shader_probe_check.py` | leur test : modules DXIL fabriqués en mémoire, contrôles positifs et négatifs ; le second sert au contrôle sur GPU |

Utilisation : `python third_party/xbox2windows/tools/dxil_retarget.py <entrée.dxbc> <sortie.dxbc>`, ou
`--pe <jeu.exe> --output-dir <dossier>` sur un exécutable console. AnyXboxX lance leur test dans sa suite
(`tests/test_dxil_retarget.py`). Pour mettre à jour : recopier ces fichiers depuis un commit plus récent.
