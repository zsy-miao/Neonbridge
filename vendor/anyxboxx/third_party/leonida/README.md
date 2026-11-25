# Leonida : traducteur RDNA 2 → SPIR-V (copie)

Fichiers copiés **sans modification** depuis [ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida),
dossier `tools/`, commit `1cc6e3c121` (23 novembre 2026, 15 h). Licence : GNU GPL 3.0 (fichier `LICENSE`,
la même que celle d'AnyXboxX). Tout le mérite de ce code revient aux auteurs de Leonida.

| Fichier | Rôle |
| --- | --- |
| `rdna2spirv.py` | traducteur RDNA 2 (gfx10) → SPIR-V : VS, PS, CS, mesh, GS |
| `shader_ir.py` | lecture du désassemblage LLVM (`<instruction> ; encoding: [...]`) |
| `cfg_structure.py` | contrôle de flux structuré (boucles et gardes au lieu d'un `switch`) |
| `lane_masks.py` | analyse des masques de lanes (EXEC, VCC) |
| `wave64_halves.py` | shaders wave64 graphiques sur des sous-groupes de 32 : le programme est-il invariant par moitié de vague ? |
| `wave64_emulate_opt.py` | analyses exactes du contrôle de flux et des données pour l'émulation wave64 des compute et mesh shaders |
| `fssm_header.py` | table des entrées d'un pixel shader dans le programme RDNA 2 précompilé (FSSM) |
| `shader_meta.py`, `spirv_stubs.py` | métadonnées de pipeline et modules d'appoint |
| `cpu_ref.py`, `ab_control_flow.py` | référence CPU et comparaison A/B du contrôle de flux |
| `amd_swizzle.py` | détuilage d'images selon les motifs addrlib |
| `spirv_opt.py` | optimiseur du SPIR-V produit (arbre des dominateurs : accès aux registres, gardes `%active`, constantes, code mort) |
| `report_paths.py` | deux chemins de rapport désignent-ils le même fichier ? (refus d'écraser une entrée) |
| `gfx10_formats.json` | noms des formats GFX10 |

AnyXboxX l'appelle par `PrototypeScarlett/emu/rdna2_spirv.py`, sur le texte produit par son propre
désassembleur (sans `llvm-mc`) : `scarlett.py --rdna2 <shader.bin> --spirv <vs|ps|cs|ms|gs>`.

Ce qui n'est **pas** copié : tout ce qui est propre à un jeu (formats d'archives et de ressources,
index, hachages, inventaires de shaders, `make_game.py`, dossier `game/`). Pour mettre à jour, recopier
les mêmes fichiers depuis un commit plus récent et changer le numéro de commit ci-dessus.
