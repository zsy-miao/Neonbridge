# Composants tiers

AnyXboxX est distribué sous [GNU GPL 3.0 ou ultérieure](LICENSE). Les composants suivants gardent leur propre licence.

Jusqu'au 21 novembre 2026, AnyXboxX était sous LGPL 2.1 ou ultérieure. La clause « ou ultérieure » permet de passer à la GPL 3.0, et l'auteur du projet en était le seul contributeur. Ce passage permet de reprendre du code GPL-3.0, comme celui de Leonida. Les composants ci-dessous sont tous compatibles avec la GPL 3.0 : MIT, Apache 2.0, BSD 3 clauses et LGPL 2.1 ou ultérieure.

| Composant | Origine | Licence | Utilisation dans AnyXboxX |
| --- | --- | --- | --- |
| Module `Task` de libHttpClient | [microsoft/libHttpClient](https://github.com/microsoft/libHttpClient) | MIT | File de tâches du runtime GDK (`runtime/third_party/libhttpclient-task`) |
| Tests d'encodage AMDGPU de LLVM | [llvm/llvm-project](https://github.com/llvm/llvm-project), `llvm/test/MC/AMDGPU` | Apache 2.0 avec exception LLVM | Tables générées `PrototypeScarlett/emu/rdna2_ops.py` et `rdna2_shapes.py` (outils `tools/rdna2_*.py`) |
| `AMDGPUInstPrinter::printSwizzle`, `printExpSrcN`, `printExpTgt`, `Exp::getTgtName` et constantes `Swizzle`/`Exp` (`SIDefines.h`) de LLVM | [llvm/llvm-project](https://github.com/llvm/llvm-project), `llvm/lib/Target/AMDGPU` | Apache 2.0 avec exception LLVM | Ports en Python : `PrototypeScarlett/emu/rdna2_swizzle.py` (offset de `ds_swizzle_b32`) et `rdna2_exp.py` (exports EXP) |
| TomBox | [drycheetah84/TomBox](https://github.com/drycheetah84/TomBox) | voir le dépôt | Projet d'origine de la suite ci-dessous |
| Suite de TomBox (DLL console en Rust, `tombox-xshader`) | [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator) | GPL-3.0 ou ultérieure depuis leur commit `51f5319` du 22 novembre 2026 (MIT ou Apache-2.0 avant) ; le décodeur `xma` reste en LGPL-2.1 ou ultérieure | Code et observations sur Direct3D 12.x, les bibliothèques console et les shaders RDNA 2 ; voir [docs/fr/recherche/tombox.md](docs/fr/recherche/tombox.md) |
| Xbox2Windows | [R0Y00/Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) | MIT | Décodage BC7 et agencement GFX10 SW_4K_S portés en Rust (`rust/anyxboxx-texture`) ; attributs `XALLOC_ATTRIBUTES` et `XMemVirtualQuery` (`src/era_heap.cpp`, `src/era_query.cpp`) portés en C++ dans `runtime/xmem/era_heap.*` et `era_query.*`, avis MIT dans `era_heap.h` ; voir [docs/fr/recherche/xbox2windows.md](docs/fr/recherche/xbox2windows.md) |
| Descripteurs GFX10 (T#, S#) de Leonida, `runtime/src/gfx10_descriptors.h`, et sa table `tools/gfx10_formats.json` | [ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida) | GPL-3.0 | Positions des champs T#/S# portées en Python (`PrototypeScarlett/emu/rdna2_image.py`) ; table des formats copiée (`PrototypeScarlett/emu/data/gfx10_formats.json`) |
| Traducteur RDNA 2 → SPIR-V de Leonida (`tools/rdna2spirv.py` et ses modules), commit `1cc6e3c121` | [ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida) | GPL-3.0 | Copié sans modification dans `third_party/leonida/` (voir son README), appelé par `PrototypeScarlett/emu/rdna2_spirv.py` |
| Sémantique des accès mémoire typés mesurée par Leonida (`rdna2spirv.py` : `FORMATS`, `decode_format`, `norm_bits`, `buffer_load_format`, `buffer_store`) | [ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida) | GPL-3.0 | Règles reprises (arrondi UNORM/SNORM, NaN float16, saturation des écritures entières, DST_SEL, V# de FORMAT INVALID en MTBUF) et méthode du quotient répété réécrite en Python et en HLSL : `PrototypeScarlett/emu/rdna2_buffer_format.py`, `rdna2_hlsl_format.py` |
| Retarget du DXIL console de Xbox2Windows (`tools/dxil_retarget.py` et ses dépendances, test `dxil_retarget_check.py`), commit `708f4ab` | [R0Y00/Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) | MIT | Copié sans modification dans `third_party/xbox2windows/` (voir son README), test lancé par `tests/test_dxil_retarget.py` |
| Registres de Mesa (`src/amd/registers/gfx10-rsrc.json`, `ac_descriptors.c`), source des champs de Leonida, et `src/amd/common/ac_formats.c` (`ac_translate_buffer_dataformat`) | [Mesa](https://gitlab.freedesktop.org/mesa/mesa) | MIT | Positions et valeurs des champs T#/S# (via Leonida) ; champs du V# (`SQ_BUF_RSRC_WORD1/3` : FORMAT, DST_SEL, ADD_TID_ENABLE, SWIZZLE_ENABLE) et ordre des composantes des formats tampons (`PrototypeScarlett/emu/rdna2_buffer_format.py`) |
| Encodage MIMG gfx10 (`SIInstrFormats.td`, classe `MIMGe_gfx10`) de LLVM | [llvm/llvm-project](https://github.com/llvm/llvm-project) | Apache 2.0 avec exception LLVM | Champs de l'instruction `image_sample` (`rdna2_image.instruction_fields`) |
| Encodages gfx10 `MUBUFe_gfx10`, `MTBUFe_gfx10` (`BUFInstructions.td`), `FLAT_Real_gfx10` (`FLATInstructions.td`), syntaxe `getMUBUFAsmOps`/`getMTBUFAsmOps`, `AMDGPUInstPrinter` (`printOffset`, `printFlatOffset`, `printCPol`, `printFormat`, `printDim`), calcul des registres d'image (`AMDGPUDisassembler::convertMIMGInst`, `getAddrSizeMIMGOp`, dimensions et variantes de `MIMGInstructions.td`) de LLVM | [llvm/llvm-project](https://github.com/llvm/llvm-project), `llvm/lib/Target/AMDGPU` | Apache 2.0 avec exception LLVM | Ports en Python : `PrototypeScarlett/emu/rdna2_operands_vmem.py` (tampons, FLAT/GLOBAL/SCRATCH) et `rdna2_operands_mimg.py` (images) ; tests du désassembleur (`llvm/test/MC/Disassembler/AMDGPU/gfx10_*.txt`) pour quelques encodages de `tests/test_rdna2.py` |
| Textes de licence ajoutés en amont par TomBox (commit `f89e410`) : portage de `rocjitsu` (`v_log_f32`, `v_exp_f32`, `v_sin_f32`, `v_cos_f32`, `crates/tombox-xshader/src/trans32/`) et correctifs de brcly/TuxBox (`tools/reference/tombox-*.patch`) | [ROCm/rocm-systems](https://github.com/ROCm/rocm-systems) (AMD), [brcly/TuxBox](https://github.com/brcly/TuxBox) | MIT (rocjitsu), Apache 2.0 (TuxBox) | Présents dans le sous-module `third_party/tombox`, compilé tel quel par `tools/build_tombox.py` ; textes dans `crates/tombox-xshader/src/trans32/LICENSE-rocjitsu.txt` et `THIRD_PARTY/LICENSE-TuxBox-APACHE-2.0.txt` |
| Décodeur rANS `_bcpack_rans` de Xbox2Windows, copie de bcpack-rs (Ahmet Dedeler) et de `silicon-rans` (Microsoft) | [R0Y00/Xbox2Windows](https://github.com/R0Y00/Xbox2Windows), `tools/_bcpack_rans` | MIT ou Apache 2.0 (bcpack-rs), MIT (silicon-rans) | Non copié dans AnyXboxX : aucun fichier de `third_party/xbox2windows` ne l'importe ; mentionné pour mémoire, ses textes de licence (`LICENSE-MIT`, `LICENSE-APACHE`) restent en amont |
| Décodeur XMA2 / WMA Pro `xma` de TomBox, portage Rust de FFmpeg `libavcodec/wmaprodec.c` | [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator), [FFmpeg](https://ffmpeg.org) | LGPL-2.1 ou ultérieure | `rust/anyxboxx-audio` (son console) ; avis d'origine dans `third_party/tombox/crates/compat/xma/NOTICE` |
| Luminances HDR de repli de DXVK (`NormalizeDisplayMetadata`, `src/wsi/wsi_edid.h`, commit `ff55077`) | [doitsujin/dxvk](https://github.com/doitsujin/dxvk) | zlib | Valeurs 0,01 / 1 499 / 799 nits reprises (pas de code copié) dans `runtime/xgameruntime/hdr_luminance.h`, quand l'écran n'annonce pas de métadonnées HDR statiques ([#191](https://github.com/LooperSalty/AnyXboxX/issues/191)) |
| Banc GPU de Xbox2Windows (`tools/gpu-check/Run-GpuCheck.ps1`), commit `bcdce96` | [R0Y00/Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) | MIT | Modèle de `tools/Run-GpuCheck.ps1` (tests lancés sur un adaptateur choisi, rapport Markdown par carte) ; avis MIT dans l'en-tête du script |
| VIctory, base Neptune (VIctory Edit.) 2026-11-25c, reprise de plusieurs dépôts | [Marvisak/VIctory](https://github.com/Marvisak/VIctory) et des forks, voir [Sources de `third_party/victory`](#sources-de-third_partyvictory) | GPL-3.0 ou ultérieure (VIctory), GPL-3.0 (forks) | Copié dans `third_party/victory`, devenu notre code ; compilé et lancé par `tools/lancer_victory.ps1` |
| Arrondi `softfloat_roundPackToF32` de Berkeley SoftFloat 3e (John R. Hauser) | [ucb-bar/berkeley-softfloat-3](https://github.com/ucb-bar/berkeley-softfloat-3) | BSD 3 clauses | Réécrit en HLSL, réduit à l'arrondi au plus proche pair, dans `FmaRoundPack` (`PrototypeScarlett/emu/rdna2_hlsl_fma.py`, FMA f32 arrondie une seule fois) |

## Sources de `third_party/victory`

`third_party/victory` est notre code, issu de VIctory. Il ne suit aucun autre dépôt : rien n'est synchronisé ni épinglé, et un dépôt d'origine peut disparaître sans rien casser ici. Un changement repris ailleurs entre comme un commit à nous, avec en fin de message le dépôt d'origine et l'auteur :

```
Source: <dépôt> <PR ou commit>
Co-authored-by: Nom <e-mail>
```

Depuis le 25 novembre au soir, la base est **Neptune (VIctory Edit.) 2026-11-25c** : la source correspondante livrée avec son paquet (`neptune-ve-source.zip`, GPL-3.0 ou ultérieure), committée telle quelle (commit « feat: VIctory base moved to Neptune (VIctory Edit.) 2026-11-25c »), puis nos changements, chacun dans son commit. Aucune dépendance git ni chemin hors de l'arbre : `Cargo.toml` et `Cargo.lock` ne prennent que crates.io.

| Dépôt d'origine | Licence | Dans la base Neptune (d'après son `NEPTUNE-README.txt`) | Repris après |
| --- | --- | --- | --- |
| Neptune (VIctory Edit.), paquet du 2026-11-25 au soir (auteur sans contact public) | GPL-3.0 ou ultérieure | L'assemblage et ses propres changements : temps d'image, souris et clavier, Rich Presence Discord, éclairage ambiant de remplacement, file de textures et mémoire de la carte, attente de pipelines bornée à 2,5 s, `play.ps1` (jeu de réglages repris dans `tools/lancer_victory.ps1`) | Rich Presence Discord éteinte sauf `VICTORY_DISCORD=1` |
| [Marvisak/VIctory](https://github.com/Marvisak/VIctory) (Marvisak et ses contributeurs) | GPL-3.0 ou ultérieure | Le runtime d'origine | |
| [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (fxceless ; PR de Jonirulah, Flyander, AscilCH) | GPL-3.0 | La base : niveaux à la demande, synchronisation du tas de descripteurs (`a26859f`, `9376fd6`) | |
| [myokex/victory-amd](https://github.com/myokex/victory-amd) (myokex) | GPL-3.0 | La base, avec fxceless/victory-fork | |
| [soldsawn/victory-nvidia-8gb](https://github.com/soldsawn/victory-nvidia-8gb) (soldsawn) | GPL-3.0 | Le traducteur de shaders (programmes wave64 appariés sur les cartes NVIDIA à 32 lanes) | |
| Belobug, [reddxd-byte/ShakiraRED-dt](https://github.com/reddxd-byte/ShakiraRED-dt) | GPL-3.0 | La passe de lumière ambiante | |
| [ZSlayerHQ/victory-world-runtime](https://github.com/ZSlayerHQ/victory-world-runtime) (ZSlayerHQ) | GPL-3.0 | Écritures vues entre vues, lectures noyau, dispatchs en parties | |
| [JoshuuDev/victory-amd](https://github.com/JoshuuDev/victory-amd) (JoshuuDev) | GPL-3.0 | Verrou de remappage par plage ; corrections de blend et de descripteurs | |
| zm0kie | GPL-3.0 | Overlay (F10), envois de fenêtres, lignes mémoire, `tools/` (`start-neptune.ps1`, `memwatch.ps1`) | |
| [skornuta/victory-fork](https://github.com/skornuta/victory-fork) (skornuta) | GPL-3.0 | Export des temps d'image (`VICTORY_FRAMETIME_DUMP`) | |
| Trexies | GPL-3.0 | Petits tampons regroupés | |
| Le correctif de redirection FidelityFX reçu pour DexNom Runtime | GPL-3.0 | `VICTORY_FSR_DLL` (mode Series S, non utilisé par le lanceur) | |
| [LooperSalty/victory](https://github.com/LooperSalty/victory) (Looper_Salty) | GPL-3.0 ou ultérieure | | Les quatre commits `xg:` (`19feca8`, `1f0fe1d`, `36955ee`, `fb021f7`) : `xg_xs.dll` du GDK de l'utilisateur comme oracle ou service (`VICTORY_XG_DLL`, `VICTORY_XG_MODE`, jamais copiée dans le dépôt) et HLE qui répond comme la DLL de Microsoft |

### Base précédente (jusqu'au 25 novembre au soir)

Le détail par changement est dans `git log -- third_party/victory`. La copie initiale (commit « chore: intégrer VIctory dans third_party/victory au lieu d'un sous-module ») n'a pas un commit par changement : le tableau garde ce qu'elle contenait, puis ce qui est venu après, par dépôt d'origine.

| Dépôt d'origine | Licence | Dans la copie initiale | Repris après |
| --- | --- | --- | --- |
| [Marvisak/VIctory](https://github.com/Marvisak/VIctory) (Marvisak et ses contributeurs) | GPL-3.0 ou ultérieure | `main` jusqu'à `ffe310e`, base de tout le reste | Portages des PR #1722 (Impre-visible, bibliothèque de pipelines sous vkd3d-proton), #1718 (tjthebigbearcat-hub), #1696 (Calvix2k) et du commit `86a5ed6` (Marvisak) |
| [fxceless/victory-fork](https://github.com/fxceless/victory-fork) (fxceless, anciennement victory-gta6) | GPL-3.0 | Le fork à `7b0f736`, dont la PR #1 (ZazaJr24) ; ses PR #63, #32 (arminkhaleghirod), #71 (MartinIndra02), #82 (Kaivyyy), #83 (ryotdev), #81 (undeadhuman37), #77 (m335671), #11 (cxchhh), #349, #355, #353 (AscilCH), #396, #386 (JordanFableFur), #358 (HailLucifer666), #374 (davutsimsekk), #282 (t4l03, porté) ; deux commits de Yorivel (`e49864a`, `bf870de`, `VICTORY_DSTORAGE_NOTIFY`) | Le commit `1684989` (fxceless, nettoyage des shaders), la PR #14 (oddiums) ; portages des PR #127 (BitWuehler), #9 (MartinIndra02), #134 (triippywhale), #115 (Spexivus, avec la correction de joroyt), #141 (adritoyalser), #135 (alexdeliu), #191 (gdawid9407), #206 (AscilCH), #114 (Kaivyyy), #98 (sillynocodecat), #150 (wildheathen) |
| [LooperSalty/victory](https://github.com/LooperSalty/victory) (Looper_Salty) | GPL-3.0 ou ultérieure | L'assemblage du fork et de ses PR (`80d58f8`), l'adaptation de #282, les commits `xg:` (`1f0fe1d`, reprises de `4466d2e` et `a16f040`) qui font répondre le HLE de `xg_xs.dll` comme la DLL de Microsoft, et `19feca8` qui charge la `xg_xs.dll` du GDK de l'utilisateur (jamais copiée dans le dépôt) | |
| [myokex/victory-amd](https://github.com/myokex/victory-amd) (myokex) | GPL-3.0 | | Douze commits : textures revérifiées après envoi, mips partielles, dispatchs de plus de 65 535 groupes, vues de textures placées, liste des pipelines précoces, attentes de pipelines au chargement, vues de tampons constants de 64 Kio, file de textures, diagnostics, tableaux de pages DirectStorage |
| [JoshuuDev/victory-amd](https://github.com/JoshuuDev/victory-amd) (Joshua Odom) | GPL-3.0 | | Lectures noyau dans la mémoire du titre faites à côté puis recopiées (`10ea38e`) |
| [PastiiJay/klbf-lab](https://github.com/PastiiJay/klbf-lab), branche `klbf-amd` (PastiiJay, KLBF) | GPL-3.0 | | Portages : demi-vagues wave32 sur un hôte à 64 lanes, garde NaN |
| [rorodev6/victory](https://github.com/rorodev6/victory) (rorodev6) | GPL-3.0 ou ultérieure | | Compteur de fps en haut à gauche (`fps_overlay.rs`, commit `4ddc0c1`), porté avec une taille réglable (`VICTORY_FPS_OVERLAY_SIZE`) |

Les autres commits de cette base précédente étaient écrits pour AnyXboxX (Proton, réductions de write-watch, gardes, vues, diagnostics) ; leur auteur est celui du commit. Ils ne sont pas dans la base Neptune.

## Exceptions

Registre des données propriétaires indispensables à un format, admises par décision du mainteneur, sur le modèle de `docs/asset-policy.md` de VIctory (tables BCPack). Chaque entrée est une table constante sans laquelle le format ne peut pas être décodé et qu'on ne peut pas recalculer. **La licence du dépôt (GPL-3.0 ou ultérieure) ne couvre pas ces données.** Tout le reste (assets, shaders extraits, textures, sons, archives, sauvegardes, dumps, clés, exécutables) reste interdit.

| Fichier | Format | Contenu | Décision du mainteneur |
| --- | --- | --- | --- |
| `third_party/victory/crates/victory-formats/src/bcpack-tables.bin` (environ 78 Kio, venu avec la copie de VIctory, identique octet pour octet dans la base Neptune) | BCPack | Tables constantes du format : modèles d'entropie, tables de rang des indices, codes de contrôle, dictionnaires par format (voir `third_party/victory/docs/asset-policy.md`) | **Admise par décision du mainteneur le 2026-11-25** (#199) ; admise dans VIctory par décision de son propriétaire le 2026-11-23 |

Aucun fichier de jeu, de console ou de SDK n'est inclus dans le dépôt.
