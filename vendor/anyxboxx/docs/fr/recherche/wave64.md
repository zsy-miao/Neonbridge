> English: [wave64.md](../../research/wave64.md)

# Programmes wave64 sur des GPU à waves de 32 lignes (issue #70)

## Pourquoi

Mesuré (24 novembre 2026, voir [victory-xcl-fivestar.md](victory-xcl-fivestar.md)) : la couche de débogage D3D12
refuse 119 shaders de calcul du jeu parce qu'ils demandent des waves de 64 lignes alors que le pilote NVIDIA en
accepte 32 au plus. RDNA 2 exécute un programme de calcul en mode wave32 ou wave64 (`CS_W32_EN` dans l'état de
dispatch) ; un programme wave64 est écrit pour 64 lignes et ne peut pas être traduit comme s'il en avait 32.

## État d'un wave64

La documentation RDNA 2 d'AMD (chapitre 3, « Kernel State ») garde les mêmes registres et élargit les masques
de lignes :

| État | wave32 | wave64 |
| --- | --- | --- |
| Lignes | 0-31 | 0-63 |
| EXEC | `exec_lo` = s126 | `exec_lo` = s126 (lignes 0-31), `exec_hi` = s127 (lignes 32-63) |
| VCC | `vcc_lo` = s106 | `vcc_lo` = s106, `vcc_hi` = s107 |
| Masque de lignes dans un SGPR | un SGPR | une paire alignée `s[n:n+1]` |
| VGPR | 32 valeurs | 64 valeurs |

Familles d'instructions dont le résultat dépend de la taille du wave :

- **Comparaisons** (VOPC, comparaisons VOP3, comparaisons SDWA) : le masque écrit dans VCC, EXEC (`v_cmpx`) ou un
  SGPR fait 64 bits ; les lignes inactives donnent 0.
- **Retenues** (`v_add_co_u32`, `v_add_co_ci_u32`… en VOP2 et VOP3b, `v_mad_u64_u32`) : la retenue d'entrée et
  celle de sortie sont des masques de 64 bits.
- **`v_cndmask_b32`** : le sélecteur (VCC, ou paire de SGPR en VOP3) fait 64 bits.
- **Opérations scalaires sur les masques** : le compilateur emploie les formes `_b64` (`s_and_saveexec_b64`,
  `s_andn2_b64 exec`, `s_or_b64 exec`, `s_mov_b64`, `s_bcnt1_i32_b64`, `s_ff1_i32_b64`…). Leur sémantique ne
  dépend pas de la taille du wave ; seuls les registres lus en dépendent.
- **Branchements** : `s_cbranch_execz`/`vccz` testent tout EXEC/VCC sur 64 bits.
- **Accès à une ligne** : `v_readlane_b32`/`v_writelane_b32` prennent le numéro de ligne dans les bits [5:0] de
  SSRC1 au lieu de [4:0] ; `v_readfirstlane_b32` prend la plus basse ligne active parmi 64 (ligne 0 si EXEC est
  nul, comme en wave32).
- **Numéros de ligne** : `v_mbcnt_lo_u32_b32` compte les bits de S0 sous la ligne dans les bits [31:0] du masque
  (les 32 pour les lignes 32-63), `v_mbcnt_hi_u32_b32` compte ceux des bits [63:32] (aucun pour les lignes 0-31).
  La suite habituelle `v_mbcnt_hi(-1, v_mbcnt_lo(-1, 0))` donne le numéro de ligne 0-63.
- **Données entre lignes** : les rangées DPP restent de 16 lignes (row_mask compte 4 rangées en wave64), DPP8 reste
  sur 8 lignes. `ds_bpermute_b32` ne permute **qu'à l'intérieur de chaque moitié de 32 lignes** en wave64 sur GFX10 :
  le compilateur ACO de Mesa émule une permutation sur tout le wave en « GFX10-11.5 wave64 mode »
  (`aco_select_nir_intrinsics.cpp`, lu statiquement ; non mesuré sur matériel). L'interpréteur suit cette règle :
  ligne source = sa moitié + `((adresse + décalage) / 4) & 31`.

## Interpréteur de référence

`rdna2_exec.Wave(lanes=64)` ; 32 reste la valeur par défaut et tous les programmes existants tournent sans
changement. Le wave garde son nombre de lignes, `wave.exec` et `wave.vcc` lisent des masques de 64 bits (paires),
et toute écriture de masque passe par `write_mask`, qui écrit un SGPR en wave32 et une paire en wave64. Seules les
instructions ci-dessus changent.

## Stratégie HLSL

Trois façons d'exécuter un programme de 64 lignes là où le wave matériel en a 32 :

1. **`[WaveSize(64)]` avec les intrinsèques de wave SM 6.6.** Refusé par D3D12 sur NVIDIA (l'échec mesuré).
2. **Deux moitiés sur deux waves matériels de 32 lignes** (approche de xcl, décrite dans leur
   `docs/wave64-on-wave32.md`, lue comme conception seulement : GPL-2.0-only ; mode `Wave32` de la PR #189 de
   VIctory). Rapide, mais suppose que les threads `32k..32k+31` d'un groupe tombent dans un même wave matériel, dans
   l'ordre des lignes, ce que D3D12 ne garantit pas (la PR #117 a vu NVIDIA regrouper les pixels de deux triangles
   dans un même warp ; mesuré : WARP exécute un groupe de 64 threads en 16 waves de 4 lignes) : chaque opération
   entre lignes demanderait de
   toute façon un test `WaveGetLaneCount()` à l'exécution et une solution de repli.
3. **Un groupe de 64 threads est le wave virtuel** (mode `Emulate` de la PR #189 de VIctory, GPL-3.0-or-later).
   La ligne `n` est le thread de `SV_GroupIndex = n` ; les masques de lignes sont construits en mémoire
   `groupshared` par `InterlockedOr` entre barrières de groupe, et les lectures de ligne passent par un tableau
   `groupshared` de 64 entrées.

**Choix : 3.** C'est exactement ce que fait déjà notre traduction wave32 pour 32 threads (`Ballot`, `ReadLane`) :
elle ne s'appuie que sur ce que D3D12 garantit (`GroupMemoryBarrierWithGroupSync`, `SV_GroupIndex`), tourne sur
toute taille de wave matériel (NVIDIA 32, AMD 32/64, WARP), n'a besoin ni de l'attribut `WaveSize` ni d'un
modèle de shader au-delà de 6.0, et se vérifie au bit près contre l'interpréteur sur la RTX comme sur WARP. Les
barrières restent correctes car le contrôle de flux RDNA 2 est scalaire : chaque thread garde une copie identique
des SGPR, de SCC, d'EXEC et de VCC, donc chaque branchement est uniforme dans le groupe et chaque thread atteint
chaque barrière.

Contrepartie : chaque comparaison, retenue ou lecture de ligne coûte trois barrières de groupe au lieu d'une
instruction de wave. C'est le coût de notre chemin wave32 actuel ; l'option 2 pourra s'ajouter plus tard comme
chemin rapide quand un shader mesuré en aura besoin, l'option 3 restant la référence contre laquelle la vérifier.

Forme du HLSL en wave64 : `[numthreads(64, 1, 1)]`, `s[127]` = `exec_hi` initialisé à `0xFFFFFFFF`, une macro
`LANE_ON` (`Bit64(s[126], s[127], lane)`) au lieu de `(s[126] >> lane) & 1`, `Ballot64` qui renvoie les deux
mots du masque, `ReadLane64` avec `& 63`. Les instructions scalaires 64 bits (`s_mov_b64`, logiques, `saveexec`,
`wrexec`, `s_bcnt*_b64`, `s_ff*_b64`, `s_cselect_b64`, `s_cmp_*_u64`) sont ajoutées pour les deux tailles.

## Pris en charge en wave64, et refusé

Traduit en wave64 et vérifié au bit près contre l'interpréteur (`tests/test_rdna2_hlsl_wave64.py`) : opérations
scalaires 64 bits sur les masques (`s_and_saveexec_b64`, `s_andn2_b64`, `s_or_b64`, `s_mov_b64`, `s_bcnt1_i32_b64`,
`s_ff1_i32_b64`, `s_xor_b64`, `s_cmp_lg_u64`), boucles et if/else par `s_cbranch_execz`/`vccnz`, comparaisons vers
VCC, EXEC (`v_cmpx`) ou une paire de SGPR, un ballot dont le masque couvre les deux moitiés relu comme donnée,
retenues VOP2 et VOP3, `v_cndmask_b32` (VOP2 et VOP3), `v_mad_u64_u32`, `v_readlane_b32`, `v_readfirstlane_b32`,
`v_writelane_b32`, `v_mbcnt_lo/hi_u32_b32`, `ds_bpermute_b32` (par moitié), LDS et mémoire globale. Mesuré :
identique sur une RTX 4060 Laptop GPU, dont `WaveGetLaneCount()` vaut 32 (les threads 32-63 du groupe sont les
lignes 0-31 d'un second warp), et avec WARP (waves de 4 lignes). Les autres formes scalaires 64 bits (variantes
`saveexec`/`wrexec`, `s_not_b64`, `s_nor_b64`, `s_bcnt0`, `s_ff0`, `s_cmov_b64`, `s_cmp_eq_u64`) sont vérifiées de
la même façon dans un programme wave32.

Délégué sans changement à la traduction wave32, avec `LANE_ON` comme test d'activité : toute autre instruction ALU
ligne à ligne, 64 bits ou mémoire (chargements buffer, scalaires et image compris) ; elles ne dépendent pas de la
taille du wave.

Refusé par `Untranslatable` en nommant l'instruction : DPP16/DPP8 (entre lignes, pas encore écrit pour 64 lignes),
comparaisons SDWA et `v_cndmask_b32` SDWA (masques de 32 bits), `v_mad_i64_i32` avec retenue (déjà refusé en
wave32), et toute instruction dont la traduction wave32 lit directement EXEC ou VCC ou échange entre lignes hors
de la liste ci-dessus : le traducteur wave64 relit le texte qu'il délègue au code wave32 et refuse tout ce qui
lit encore `s[106]`/`s[126]` bit à bit ou utilise `Ballot`, `ReadLane` ou `lane` (lire VCC ou EXEC comme une
valeur, par exemple `v_mov_b32 v1, vcc_lo`, reste permis).

Non couvert : la taille du wave n'est pas encore lue dans l'état de dispatch ; l'appelant passe `wave_size=64`.
