> English: [neighbors.md](../../research/neighbors.md)

# Projets voisins : où ils sont en avance et comment les rattraper

État au 21 novembre 2026. AnyXboxX est en GPL-3.0 ou ultérieure : le code des trois projets peut
être repris avec crédit dans [THIRD_PARTY.md](../../../THIRD_PARTY.md). Aucun des trois ne démarre
encore un jeu commercial Series X (TomBox atteint le salon de Fortnite, d'après leurs notes).
Détails par projet : [tombox.md](tombox.md), [xbox2windows.md](xbox2windows.md).

| Projet | Licence | Taille du travail | Leur meilleure avance |
| --- | --- | --- | --- |
| [Leonida](https://github.com/ItsNotRexed/Leonida) | GPL-3.0 | traducteur RDNA 2 → SPIR-V de 9 722 lignes (22 novembre) | shaders : toutes les familles d'un jeu, contrôle de flux structuré, descripteurs GFX10, émulation wave64 |
| [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (TomBox) | GPL-3.0 ou ultérieure (MIT ou Apache 2.0 avant le 22 novembre) | `d3d12_xs` de 32 400 lignes (Rust) | D3D12.X (194/200 méthodes), traduction RDNA 2 → HLSL, bibliothèques console |
| [Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) | MIT | environ 100 tests ctest | mémoire ERA et alias GPU, recompilation CPU, retarget DXIL, banc GPU |

## Ce qu'il faut reprendre, par ordre d'impact

| # | Domaine | Source | Ce que nous avons | Issue | Effort |
| --- | --- | --- | --- | --- | --- |
| 1 | Textures : descripteurs T#/S#, formats GFX10, `image_sample` | Leonida `runtime/src/gfx10_descriptors.h`, `tools/gfx10_formats.json` ; LLVM ; Mesa | BC7 et SW_4K_S en Rust | #21 | 15-25 h |
| 2 | D3D12.X complet (device, listes de commandes, ESRAM) | TomBox `compat/d3d12_xs`, `d3d12_x` (lien de DLL, déjà en place pour `d3d12_xs`) | sérialisation des root signatures | #18, #22 | 8-15 h |
| 3 | Mémoire ERA, alias CPU/GPU, table d'adresses titre, `XMemVirtualQuery` | Xbox2Windows `src/era_*.cpp` | `XMemAllocDefault` sans attributs | #13, #22 | 12-18 h |
| 4 | Sémantique RDNA 2 manquante (SDWA, `v_perm_b32`, f16, `v_cvt_pkrtz`, `v_mul_legacy`…) et fixtures `.asm` | Leonida `docs/design.md`, `runtime/test/shaders/` | 520/1 444 encodages exécutés | #21 | 60-100 h |
| 5 | Contrôle de flux structuré et masques de lanes (compilation dxc rapide, waves de pixels) | Leonida `tools/cfg_structure.py`, `tools/lane_masks.py` | `while` + `switch (pc)` | #68 | 12-35 h |
| 6 | Second traducteur de comparaison | TomBox `tombox-xshader` (pont Rust existant) | pont du décodeur | #21 | 10-20 h |
| 7 | Retarget du DXIL console vers le DXIL du PC, inventaire des PSO | Xbox2Windows `tools/dxil_retarget.py`, `pso_readiness.py` | aucun | #69 | 10-16 h |
| 8 | Bibliothèques console supplémentaires et audio (`xaudio2_9`, `xapu`, `opus`) | TomBox | `xg_xs`, `xmem`, `xma` liés | #26 | 9-18 h |
| 9 | Banc GPU partagé (BVH, DXR, wave64, rapport par carte) | Xbox2Windows `tools/gpu-check` | `--banc`, `anyxboxx_gpu_run.exe` | #70 | 14-20 h |
| 10 | Recompilation statique du code CPU (AOT) | Xbox2Windows `tools/cpu_*_aot.py` | exécution native et Unicorn | #71 | 25-40 h |

Le point 10 n'est utile que si l'exécution native échoue, puisque le processeur de la console a
le même jeu d'instructions que le PC.

## Leonida au 22 novembre 2026 (commit `bb2e18421d`)

Notre copie de `third_party/leonida/` passe de `8bd0087d2d` à `bb2e18421d` : le traducteur grossit
de 6 580 à 9 722 lignes, sans changer l'appel `translate(chemin)` qu'utilise `emu/rdna2_spirv.py`.
D'après leur README à ce commit :

| Domaine | Leur état | Pour AnyXboxX |
| --- | --- | --- |
| RDNA 2 → SPIR-V | les 26 503 shaders du jeu traduits et acceptés par `spirv-val` ; 21 545 sont wave64 | récupéré par la copie mise à jour |
| Wave64 sur GPU à sous-groupes de 32 (NVIDIA, Intel) | exact en compute et mesh (`--wave64-emulate`), approché en VS/PS/GS | option du traducteur, pas encore exposée par `scarlett.py --spirv` |
| Tests GPU du traducteur | plus de 30 couples GPU et pilote (NVIDIA, AMD GCN3 à RDNA4, Intel, Apple, llvmpipe) | à comparer avec notre banc `--banc` (#70) |
| D3D12 Xbox → Vulkan | file, listes de commandes, barrières, copies, requêtes ; pas encore de pipelines ni de draws | AnyXboxX vise D3D12 natif via TomBox (#18, #22) |
| Manettes | shim GameInput sur SDL3 (vibrations, gâchettes DualSense) | candidat pour une issue GameInput |
| Textures BCPack | décodeur Python « clean-room » de BC1 à BC7, une table d'historique manque | provenance à vérifier avant toute reprise (règle 2) |

## TomBox au 22 novembre 2026 (commit `51f5319`)

Le sous-module `third_party/tombox` passe de `838f43f` à `51f5319` (15 commits). Le workspace Rust
d'AnyXboxX et les 7 DLL servies par `ANYXBOXX_TOMBOX_DIR` (`tools/build_tombox.py`) compilent sans
changement.

| Domaine | Leur avancée | Pour AnyXboxX |
| --- | --- | --- |
| Licence | passage de MIT ou Apache 2.0 à GPL-3.0 ou ultérieure ; `xma` reste en LGPL-2.1 ou ultérieure | compatible avec la GPL-3.0 d'AnyXboxX ; mentions mises à jour |
| `xg_x`, `xg_xs` | agencement des textures réécrit d'après le modèle console (0 écart sur 3 builds), `XGEncodeDCC` exact sur 1 000 cas | récupéré par les DLL compilées (`xg_xs`) |
| Bibliothèques GDK | 37 défauts corrigés (fin de `XTaskQueue`, ordre de `XVersion`, codes d'erreur GDK, enveloppe du device D3D12, `xmem`) ; 34 retours fixes identiques à la console | récupéré pour `xmem`, `d3d12_xs`, `d3d12_x` |
| `d3d12_xs` | ABI par version de l'OS console | récupéré par la DLL compilée |
| Bac à sable | correction d'une évasion par lettre de lecteur dans `fs::resolve` (50 000 cas de fuzz) | non utilisé : AnyXboxX a son propre lanceur |
| Outils de débogage console (`xbtp`, `xtf*`) | sessions et clients réimplémentés | hors périmètre pour l'instant |

## Xbox2Windows au 22 novembre 2026 (commit `0f8e7fa`)

Notre copie de `third_party/xbox2windows/` passe de `bc24dfd7eb` à `0f8e7fa` (338 commits chez eux,
5 sur le retarget). Le retarget re-signe désormais le conteneur DXIL réécrit et refuse les opérandes
VBR de plus de 64 bits ; trois modules génériques s'ajoutent (`call_evidence.py`, `probe_roots.py`,
`pe_inventory.py`) avec `tests/shader_probe_check.py`.

| Domaine | Leur avancée | Pour AnyXboxX |
| --- | --- | --- |
| Pixel shaders (AOT vers DXIL) | 851 des 12 101 pixel shaders du jeu compilés et identiques sur RTX 4060 et WARP ; DPP16/DPP8, SDWA, conversions, `v_sin`/`v_cos` | comparer nos traductions HLSL à leurs cas (#21) |
| Compute et mesh | 12 compute shaders exacts ; un mesh shader indexé rendu comme la référence CPU | à reprendre dans `--banc` (#70) |
| Décodage RDNA 2 | VOP3P décodé, corrections EXP, VINTRP, SMEM, MIMG et littéraux | à confronter à notre désassembleur (oracle LLVM, règle 4) |
| Recompilation CPU | FMA3, AES-NI, PCLMULQDQ, SSE4.1, AVX2, x87 abaissés en C++ | utile seulement si l'exécution native échoue (#71) |

## Mise à jour du 22 novembre au soir

- **TomBox a réécrit son historique** : `51f5319`, épinglé le matin, n'a plus d'ancêtre commun avec
  leur `main` et pourrait disparaître de GitHub. Le sous-module passe à `99f48e3`, qui reprend les
  PR de contributeurs (`d3d12_xs` : plus de fausse réponse « Xbox One » mise en cache avant le
  chargement de `d3d12_x.dll`, files Direct enregistrées ; formats MTBUF gfx7 traduits en formats
  unifiés RDNA 2 ; crédits regroupés dans `THIRD_PARTY/`). Le workspace Rust, les 7 DLL et la
  suite de tests passent sans changement.
- **Leonida** : copie à `a0acd33d77` (9 722 → 10 704 lignes : `ds_wrxchg*_rtn_b64`, tampons typés
  SINT et `32_32_32_UINT`, analyse wave64 des shaders graphiques dans le nouveau `wave64_halves.py`).
- **Xbox2Windows** (17 commits) : recompilation CPU (SSE2 à SSE4.2, x87), DPP et SDWA dans leur
  recompilateur de pixel shaders ; rien de nouveau à copier pour AnyXboxX.

## Mise à jour du 22 novembre, 18 h

- **TomBox** `99f48e3` → `dbe89c8` (61 commits, historique intact) : `d3d12_xs` (diagnostics DRED,
  capture des blocages, fermer la fenêtre arrête le jeu, lectures protégées des textures en option),
  `tombox-xshader` (atomiques d'image avec `glc`, transcendantes f16 exactes au bit près, index de
  viewport). Rust, les 7 DLL et la suite de tests passent sans changement.
- **Leonida** `a0acd33d77` → `49475796c2` : accès image cohérents (`glc`), offsets scalaires constants.
- **Xbox2Windows** `0f8e7fa` → `a97ba9f` : le retarget DXIL signale les intrinsèques `xbox.*`.

## Mise à jour du 22 novembre, 20 h

- **TomBox** `dbe89c8` → `a541bbb` (historique intact) : mesh shaders à 256 threads et sorties par primitive,
  constantes de root signature sur les SGPR utilisateur (Scarlett), vue réseau hors ligne.
- **Leonida** `49475796c2` → `162821017e` : `v_cmp(x)_class_f32` préserve les NaN de signalisation.
- **Xbox2Windows** `a97ba9f` → `ff7f68a` : retarget DXIL et ses dépendances recopiés.
- Contrôle : 51 des 52 shaders de `XboxBvhBuilder.dll` traduits, comme avant (`tools/fssm_mesure.py`).

## Mise à jour du 22 novembre, 23 h, et suivi automatique

- **Suivi automatique** : le workflow `.github/workflows/voisins.yml` lance `tools/sync_voisins.py` toutes les
  heures. Il avance le sous-module TomBox, recopie sans modification les fichiers de `third_party/leonida` et
  `third_party/xbox2windows` (et les modules qu'ils se mettent à importer), change les numéros de commit, puis
  force-pousse la seule branche `auto/voisins`, que la CI teste. Il n'ouvre pas de pull request (le réglage
  qui autoriserait Actions à créer et approuver des PR reste désactivé) : un mainteneur relit la branche, la
  complète si besoin sur une branche `chore/voisins-<date>` et ouvre la PR. Il ne fusionne rien et s'arrête
  si une licence change ou si un fichier copié disparaît. Les commits qui touchent du code porté à la main
  (BC7, tuiles, `era_heap`, descripteurs GFX10) sont seulement signalés dans le rapport du passage.
- **TomBox** `a541bbb` → `30a1b53` : flux de PSO Scarlett (sous-objets 23 à 30 renumérotés), protection contre
  un périphérique retiré, préchauffage des PSO, course corrigée dans le balayage de `xmem`, service audio XVM.
- **Leonida** `162821017e` → `aa2988cc55` (2 531 commits en quatre heures) : chargements de tampons typés
  par une seule table de tailles ; nouvelle dépendance copiée, `wave64_emulate_opt.py`. `buffer_store_short`
  (réservé par AnyXboxX sur leur #2300) n'est toujours pas traité chez eux.
- **Xbox2Windows** `ff7f68a` → `6f48648` : le retarget DXIL refuse les appels `xbox.*` non résolus avant un PSO ;
  nouvelle dépendance copiée, `tools/xbox_intrinsic_gate.py`.

## Ce qui n'est pas repris

Tout ce qui est propre à un jeu : formats d'archives et de ressources (RPF, AWC, TTPF, XBCH,
FSSM), noms de fichiers, hachages, inventaires de shaders, modules en ligne de jeux précis,
cas Bink et FFX de Xbox2Windows. AnyXboxX reste une plateforme générique (règle 1 d'AGENTS.md).

## Dépôt complet de Leonida (23 novembre)

Le dépôt amont de Leonida a été supprimé le 2026-11-24 et son sous-module retiré. Voir [leonida-gta6.md](leonida-gta6.md) pour la comparaison antérieure des fonctions GDK des deux runtimes.

Premiers lancements de l'exécutable 1.0.2.1 sous `gta6rt` (arrêts mesurés dans l'ordre, pull requests de Leonida qui les traitent, liste de travaux qui en découle pour AnyXboxX) : [premier-demarrage-1021.md](premier-demarrage-1021.md).

Le même exécutable mené jusqu'à la boucle de frames sous `gta6rt` de Leonida (arrêts, quatre causes mesurées des images noires, constats statiques, nos erreurs, ce que le proxy TomBox doit encore servir) : [demarrage-leonida-2026-11-23.md](demarrage-leonida-2026-11-23.md).

## Licences modifiées en amont (23 novembre, après-midi)

Le workflow avait refusé deux mises à jour parce qu'un fichier de licence changeait. Relues à la main, elles sont compatibles avec la GPL 3.0 ou ultérieure :

- **TomBox** `30a1b53` → `f89e410` : `THIRD_PARTY/LICENSE-TuxBox-APACHE-2.0.txt` (Apache 2.0, correctifs de brcly/TuxBox repris avec crédit) et `crates/tombox-xshader/src/trans32/LICENSE-rocjitsu.txt` (MIT, AMD, modèles `v_log/exp/sin/cos_f32`). Apache 2.0 et MIT sont compatibles GPL 3.0 ; le texte de TomBox lui-même reste GPL-3.0 ou ultérieure. Le sous-module avance sans modification.
- **Xbox2Windows** `6f48648` → `708f4ab` : `tools/_bcpack_rans/LICENSE-MIT` et `LICENSE-APACHE` (bcpack-rs, MIT ou Apache 2.0, plus silicon-rans de Microsoft, MIT). Ce dossier n'est pas copié chez nous et aucun fichier copié ne l'importe ; la licence de dépôt (`LICENSE`, MIT) est inchangée. Les cinq fichiers copiés qui ont changé (`dxil_retarget.py`, `pe_inventory.py`, `probe_roots.py`, `xbox_intrinsic_gate.py`, `dxil_retarget_check.py`) sont recopiés sans modification.


## Nouveaux voisins depuis le 24 novembre

VIctory, xcl et five-star-recomp, avec nos mesures de VIctory du 2026-11-24 (menus à 30 fps, 351 programmes non traduits dans le monde, 119 compute shaders refusés pour wave64) : [victory-xcl-fivestar.md](victory-xcl-fivestar.md).

Nouveau aussi : [Leonida Continued](https://github.com/ubiquitarianism/leonida) (ubiquitarianism/leonida, GPL-3.0), une suite de la traduction RDNA 2 → SPIR-V après la suppression du projet Leonida d'origine. Listé d'après sa description publique ; nous n'avons pas encore comparé son code au nôtre.
