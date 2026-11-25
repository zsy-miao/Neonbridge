> English: [tombox.md](../../research/tombox.md)

# TomBox et sa suite : ce que nous reprenons

[TomBox](https://github.com/drycheetah84/TomBox) (drycheetah84) et sa suite [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator) (licence GPL-3.0 ou ultérieure depuis le 22 novembre 2026, MIT ou Apache-2.0 avant) réimplémentent en Rust la couche plateforme de la Xbox Series X|S. Chaque DLL console (`d3d12_xs`, `xg_xs`, `xmem`, `xma`…) y devient une DLL de remplacement, et `tombox-xshader` traduit les shaders RDNA 2 en HLSL. AnyXboxX reprend leur code et leurs observations publiées, en les citant ; aucun fichier de jeu ou de console n'est repris.

État relevé le 21 novembre 2026 (commit `fef9a6e` de omonkeyz/xbox-emulator), d'après leurs fichiers `README.md`, `docs/MILESTONES.md`, `docs/BLOCKERS.md` et `docs/DEVELOPMENT_LOG.md`.

## Ce qu'ils ont établi

| Domaine | Leur état | Pour AnyXboxX |
| --- | --- | --- |
| Fonctions système importées (Fortnite) | 671 sur 671 couvertes | modèle pour nos #12 et #13 |
| Méthodes Direct3D 12.x console | 185 sur 200, emplacements de vtable mesurés | débloque nos #18 (périphérique) et #19 (PresentX) |
| Instructions RDNA 2 vérifiées | 573 sur 721 | à croiser avec notre banc de 34 948 encodages LLVM |
| Étages de shaders | 6 sur 8 (mesh et amplification incomplets) | au-delà de notre recompilateur de calcul (#21) |
| Shaders d'un titre traduits | 105 506 sur 105 506 (Fortnite) | à valider avec notre interpréteur de référence |
| Bibliothèques console | 2 076 sur 2 141 exports | nos #13, #14, #26 |
| Titre lancé | Fortnite : démarrage, connexion, sélection du mode, salon Battle Royale | aucun titre console chez nous |

## Observations sur le paquet GTA VI 1.0.2.1

Ils ont examiné le même paquet que celui étudié par AnyXboxX et arrivent à la même conclusion : il ne peut pas être lancé, l'exécutable est absent.

- 26 503 shaders, dans des conteneurs de shaders Xbox compressés. Sur un échantillon de 4 000, tous se désassemblent ; environ un tiers des shaders de sommets se traduisent.
- 8 765 programmes mesh : ils en ont tiré l'ABI console des mesh shaders (`v5` = identifiant de sommet du groupe, `s3[27:24]` = numéro de wave, `s_sendmsg 9` pour réserver la sortie, exports de position 12, de découpe 13 à 15 et de primitives 20).
- Blocage : aucun de ces programmes mesh n'a de bloc `RTS0` (root signature). Sans la root signature du titre, leurs ressources en SGPR utilisateur (s16 et suivants) ne peuvent pas être liées ; elle se trouve dans l'exécutable manquant.

## Différences de méthode

- Ils travaillent sur des copies déjà déchiffrées de jeux du commerce. AnyXboxX n'utilise que des sources publiques (documentation d'AMD et de Microsoft, tests de LLVM) et des programmes de test libres.
- Leurs vérifications passent par des lancements de titres. AnyXboxX vérifie chaque traduction de shader contre un interpréteur de référence, à l'octet près, et son décodeur contre les tests d'encodage de LLVM : ces deux outils peuvent valider leur traducteur.

## Reprise dans AnyXboxX

1. Ce document et [THIRD_PARTY.md](../../../THIRD_PARTY.md) : citation et licences.
2. Leur dépôt en sous-module Git (`third_party/tombox`), compilé avec `cargo`.
3. Le lanceur `anyxboxx.exe` charge leurs DLL Rust pour les bibliothèques console qu'AnyXboxX ne couvre pas.
4. Leur traducteur `tombox-xshader` vérifié par notre interpréteur de référence sur le GPU.

## Confrontation de leur décodeur

Leur désassembleur RDNA 2 (`tombox-xshader`), appelé depuis la passerelle Rust d'AnyXboxX (`rust/anyxboxx-rdna2`), a été confronté aux 29 522 encodages uniques des tests de LLVM par `tools/compare_tombox.py` : aucune longueur fausse, 24 831 encodages identiques (84 %), 4 586 opcodes absents de leur table et 105 noms différents. Détail et exemples : [tombox-decodeur.md](tombox-decodeur.md). La table d'AnyXboxX, générée depuis LLVM, couvre ces opcodes manquants ; elle peut leur être proposée.

## Son XMA2 : leur décodeur vérifié

Leur crate `xma` (LGPL-2.1 ou ultérieure) est un portage en Rust du décodeur WMA Pro / XMA de FFmpeg (`libavcodec/wmaprodec.c`). Leur test `wmapro_fate_beethoven_matches_reference`, lancé localement avec l'échantillon public `wmapro/Beethovens_9th-1_small.wma` de la suite FATE de FFmpeg et un PCM de référence produit par `ffmpeg`, donne un écart maximal de 1 sur 16 bits et un rapport signal sur bruit de 101,5 dB. AnyXboxX l'utilise dans `rust/anyxboxx-audio` (`anyxboxx_xma <entrée.xma> <sortie.wav>`) pour les fichiers RIFF XMA2 à un flux ; aucun fichier XMA2 public n'a encore servi à vérifier la chaîne complète.

## Table des slots du périphérique d3d12_xs (premier démarrage, W5)

Slots que le titre appelle sur le périphérique console avant le premier `Close` en échec, comparés à la table de
périphérique de TomBox (`crates/compat/d3d12_xs/src/console_slots.rs`, 90 slots) et à ce qu'AnyXboxX sert.
« Assigné » est ce que trouve un grep des affectations `[n] = ...` dans `d3d12_xs/src` de TomBox ; le contexte de
chaque ligne n'a été lu que là où c'est indiqué. Figée par `tests/test_d3d12_device_slots.py`.

| Slot | Rôle mesuré | Table de périphérique TomBox | AnyXboxX |
| --- | --- | --- | --- |
| 8 | CreateCommandQueue (forme PC) | assigné, `device_wrap.rs` | aucun |
| 9 | CreateCommandAllocator | assigné, `device_wrap.rs` | aucun |
| 16 | CreateRootSignature (forme) | assigné à `cmdlist_validate.rs:1218`, contexte non vérifié | aucun |
| 26 | non établi | non assigné au périphérique ; 26 est `fake_barrier` dans la table de liste (`cl_ext.rs:771`) | aucun |
| 36 | CreateFence (forme PC) | non assigné | aucun |
| 41 | CreateCommandSignature | assigné, `device_wrap.rs` | aucun |
| 48 | CreateCommandQueueX (console) | assigné, `device_wrap.rs` | aucun |
| 49 | non établi | non assigné | aucun |
| 57 | DeserializeGraphicsPipelineState | assigné, `device_wrap.rs` | aucun |
| 61 | CreateSampler | assigné, `device_wrap.rs` | aucun |
| 73 | CreateCommandListX (console) | assigné, `device_wrap.rs` | aucun |
| 74 | RegisterPagePool | assigné, `device_wrap.rs` | aucun |
| 79 | ScheduleFrameEvent | assigné, `device_wrap.rs` | aucun |
| 86 | non établi | non assigné | aucun |
| 127 | CreateCommittedResource3 (forme) | hors table : la table de TomBox compte 90 slots | aucun |

AnyXboxX ne sert aucun slot de périphérique : `runtime/d3d12_xs/d3d12_xs.def` exporte `D3D12ConfigureVirtualMemoryX`
et les redirections de signature racine, pas `D3D12XboxCreateDevice`. Le manque est le proxy de périphérique entier,
pas quelques slots : aucune correction simple n'existe. Les slots de liste 9, 131 et 132 appartiennent à la table de
liste de commandes, que TomBox dimensionne à 110 (son tableau `vt` compte 128 entrées) ; 131 et 132 sont hors de celle-ci.
