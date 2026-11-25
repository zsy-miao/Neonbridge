> English: [state-of-the-art.md](../../research/state-of-the-art.md)

# Émulateurs et couches de compatibilité Xbox

Recherche faite le 21 novembre 2026 sur le web et GitHub. **Vérifié** : page lue. **À confirmer** : source secondaire ou extrait.

## Constat

Aucun émulateur public sérieux ne lance des jeux Xbox Series X|S. Les projets crédibles sont des **couches de compatibilité** de haut niveau (HLE), sur le modèle de Wine : ils remplacent les bibliothèques de la console par des équivalents PC au lieu de simuler le matériel. Les vidéos et sites qui promettent un « émulateur Xbox Series X » prêt à l'emploi sont des arnaques.

## Projets

| Projet | Cible | Approche | État |
|---|---|---|---|
| [WinDurango](https://github.com/WinDurango/WinDurango) | Xbox One (Durango) | couche de traduction vers Windows : WinRT Xbox, KernelX, D3D11.X ; LGPL-3.0 | Sonic Mania, Minecraft 1.2, LIMBO jouables ; Forza Horizon 2 démarre (vérifié) |
| XWine1 | Xbox One | couche de traduction, non publiée | annoncée en septembre 2024 ([Digital Trends](https://www.digitaltrends.com/gaming/xbox-one-emulation-xwine1/), à confirmer) |
| [Xodus](https://tomshardware.com/software/linux/xbox-pc-and-game-pass-titles-are-coming-to-linux-through-xodus-heroic-launcher-devs-embark-on-new-open-source-reverse-engineering-project) | jeux GDK PC sous Linux | rétro-ingénierie GDK / MSIXVC | début de projet (à confirmer) |
| [Xenia](https://github.com/xenia-project/xenia) | Xbox 360 | recompilation PowerPC, GPU vers D3D12/Vulkan | compatibilité partielle (vérifié) |
| [xemu](https://github.com/xemu-project/xemu) | Xbox d'origine | émulation bas niveau | référence pour l'OG Xbox (à confirmer) |
| [huan](https://github.com/huan-project/huan) | Xbox One | se dit émulateur multiplateforme | non vérifié, prudence |

## Outils utiles

- [xvdtool](https://github.com/emoose/xvdtool) et [XvdTool.Streaming](https://github.com/LukeFZ/XvdTool.Streaming) : lecture des conteneurs XVD/XVC. Le déchiffrement exige des clés absentes de ces dépôts.
- [Qiling](https://github.com/qilingframework/qiling) : émulation de binaires sur Unicorn, avec chargeur PE et API Windows (GPL-2.0).
- [Speakeasy](https://github.com/mandiant/speakeasy) : émulation d'API Windows pour l'analyse de logiciels malveillants (MIT). Modèle direct pour la page HLE d'AnyXboxX.
- [Documentation GDK](https://learn.microsoft.com/gaming/gdk/) : signatures publiques de `XSystemGetDeviceType`, `XGameRuntimeInitialize`, `XTaskQueue*`, etc.
- [Wiki Xbox One Research](https://xboxoneresearch.github.io/wiki/) : formats XVD, système de fichiers, sécurité.

## Ce qu'AnyXboxX en a retenu

| Idée | Source | Mise en œuvre |
|---|---|---|
| Remplacer les API par du code hôte (HLE) | WinDurango, Speakeasy | `emu/hle/` : registre par décorateur, page de thunks |
| Regrouper les DLL qui exposent les mêmes fonctions | WinDurango (KernelX) | `family()` : kernel32, kernelx, api-ms-win-core-* |
| Ne pas bloquer sur une fonction inconnue | Speakeasy | renvoie `E_NOTIMPL` et l'écrit au journal |
| Relocations, TLS, TEB/PEB via GS | Qiling, chargeurs PE | `emu/loader.py`, `emu/machine.py` |
| Rapport de compatibilité | journaux de compatibilité de Xenia | `scarlett.py --rapport` |

## Mise à jour du 21 novembre 2026

- [omonkeyz/xbox-emulator](https://github.com/omonkeyz/xbox-emulator), suite de [TomBox](https://github.com/drycheetah84/TomBox) (Rust, GPL-3.0 ou ultérieure depuis le 22 novembre 2026, MIT ou Apache-2.0 avant) : DLL console de remplacement, Direct3D 12.x console, traducteur de shaders RDNA 2. Fortnite y atteint le salon Battle Royale. Détails et reprise dans AnyXboxX : [tombox.md](tombox.md).
- [xodus-gaming/xgameruntime](https://github.com/xodus-gaming/xgameruntime) (C++, LGPL-2.1) : réimplémentation libre de `xgameruntime.dll` pour Wine, issue de WineGDK (archivé le 13 novembre 2026).
- WinDurango (Xbox One, LGPL-3.0) reste actif ; XWine1 n'a toujours pas de dépôt public.
