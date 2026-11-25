> English: [xbox2windows.md](../../research/xbox2windows.md)

# Xbox2Windows : ce que nous reprenons

[Xbox2Windows](https://github.com/R0Y00/Xbox2Windows) (R0Y00, licence MIT) étudie la recompilation statique de logiciels Xbox Series X|S vers Windows x64 : fonctions CPU traduites en C++ puis en DLL Windows, shaders de pixels gfx1013 traduits en HLSL, textures BC7 récupérées et affichées, état de scripts rejoué. Chaque résultat est vérifié sur GPU matériel et sur WARP, et un banc GPU communautaire couvre plus de 40 cartes. État relevé le 21 novembre 2026 (commit `18d57d9`).

## Repris dans AnyXboxX (en Rust, `rust/anyxboxx-texture`)

| Élément | Leur source | Notre portage | Vérification |
| --- | --- | --- | --- |
| Décodage BC7 (8 modes, partitions, rotation, sélection d'index) | `tools/bc7_blocks.py` | `src/bc7.rs` | leurs blocs calculés à la main, leurs décalages de champs, et leurs deux empreintes SHA-256 du jeu de 512 blocs reproduites à l'identique |
| Agencement GFX10 `SW_4K_S` (tuiles de 4 Kio, 16 × 16 blocs) | `tools/bc7_tiles.py` | `src/tiles.rs` | ordre des bits y0 y1 x0 x1 y2 x2 y3 x3 testé position par position |
| Queue de mips partagée dans une tuile de 4 Kio | `linearize_4k_s_tail` | `src/tiles.rs` | origine des niveaux et bornes |

La ligne de commande `anyxboxx_texture bc7 <blocs.bin> <largeur> <hauteur> <sortie.png> [--4k-s]` écrit un PNG ; un test Python le relit indépendamment (zlib, CRC). Ces briques servent l'issue #14 (`xg_xs`, agencement des textures console).

## Pas repris, et pourquoi

- **BCPack** : leur `tools/bcpack_probe.py` teste une hypothèse de codage rANS sur l'en-tête de 48 octets d'un flux ; ce n'est pas un décodeur, et le modèle des voies variables reste ouvert (leur issue #15). Les blocs « sans zlib » des notes de Codex sur ETPF ([ANALYSE_ETPF.md](paquet/ANALYSE_ETPF.md)) sont peut-être de ce type, sans preuve.
- **Recompilation CPU et FidelityFX** : approche différente de l'exécution native d'AnyXboxX ; leurs fixtures restent une référence utile pour des tests futurs.
