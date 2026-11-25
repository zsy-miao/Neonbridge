> English: [leonida-gta6.md](../../research/leonida-gta6.md)

# Leonida et le build Xbox Series 1.0.2.1

État au 23 novembre 2026, commit amont `561b65a832`. Leonida ([ItsNotRexed/Leonida](https://github.com/ItsNotRexed/Leonida), GPL-3.0) recompile statiquement le build Xbox Series 1.0.2.1 pour Windows : le code x86-64 du jeu tourne nativement, le logiciel système de la console est réimplémenté, les shaders RDNA 2 sont traduits en SPIR-V pour Vulkan. Le dépôt ne contient ni le jeu, ni clé, ni exécutable.

Depuis le 23 novembre, AnyXboxX vise d'abord à faire tourner ce build : nous contribuons à Leonida et nous reprenons leurs travaux, en gardant AnyXboxX comme projet générique.

## Ce que nous récupérons

- **Le dépôt complet** : le dépôt amont a été supprimé le 2026-11-24 et son sous-module retiré ; il ne reste que les copies figées de `third_party/leonida/`.
- **Les copies du traducteur** RDNA 2 → SPIR-V dans `third_party/leonida/` : ce sont elles qu'appelle `PrototypeScarlett/emu/rdna2_spirv.py`. Elles ne changent pas.

## Où en est Leonida (d'après leur README)

| Sujet | État chez eux |
| --- | --- |
| Démarrage du jeu | non : il leur manque aussi l'exécutable |
| Shaders RDNA 2 → SPIR-V | 26 503 sur 26 503 traduits, tous valides selon `spirv-val` |
| Ressources | modèles (OBJ), textures (DDS, BCPack partiel), audio (AWC) exportés depuis la copie du joueur |
| Chargeur PE et shims | un exécutable de remplacement tourne de bout en bout (`gta6rt --run`) |
| Direct3D 12 Xbox → Vulkan | en cours (périphérique, files, ressources) ; ray tracing en cours |

Leur chemin critique et les méthodes D3D12 encore ouvertes sont suivis dans leur issue #313. Pour travailler sur un sujet, on le réserve dans l'issue ouverte « Coordinating who works on what ». Une réservation tombe après 12 h sans PR.

## Fonctions GDK : comparaison des deux runtimes

La comparaison se fait par recherche de noms, sans compilation, donc les nombres sont approximatifs. AnyXboxX déclare 396 méthodes (`runtime/xgameruntime/gdk_api_table.inc`) et en implémente environ 170. Leonida nomme environ 310 fonctions dans `runtime/shims/xgameruntime*.cpp` et en implémente environ 190. Aucun import de l'exécutable n'a été mesuré : leur `docs/runtime_imports.md` ne liste que les familles que lient les DLL du paquet (XAsync, XTaskQueue, XUser, XStore, XGameSave, XPackage, XGameUi).

**Implémentées chez nous, absentes ou bouchons chez eux** (pistes de contribution) :

- `XPackage*` : `EnumeratePackages`, `GetPackageKind`, `EstimateDownloadSize`, `CreateInstallationMonitor`, `InstallChunksAsync`, `MountWithUiAsync`, `GetWriteStats`, les fonctions de chunks (`runtime/xgameruntime/package_interface.cpp`).
- `XUserGetGamerPictureAsync` et ses résultats (`user_interface.cpp`) : bouchon `offline_failure<16, E_FAIL>` chez eux.
- `XStoreShow*UIAsync`, `XStoreDownloadAndInstallPackagesAsync`, `XStoreAcquireLicenseForDurablesAsync` : erreur réseau de la boutique hors ligne chez nous, bouchon chez eux.
- `XGameRuntimeIsFeatureAvailable`, `XDisplayAcquireTimeoutDeferral` (`system_interface.cpp`).

**Implémentées chez eux, absentes chez nous** (à reprendre) :

- `XGameStreaming*`, `XGameUiShowMessageDialogAsync` et `XGameUiShowErrorDialogAsync`.
- `XStoreIsLicenseValid`, `XStoreCanAcquireLicenseFor*Async`, `XStoreQueryAddOnLicensesAsync`.
- `XUserFindForDevice`, `XUserRegisterForDeviceAssociationChanged`, `XNetworkingQueryConfigurationSetting`, `XNetworkingVerifyServerCertificate`.

Avant chaque contribution, il faut vérifier sur leur `main` du moment que la fonction manque toujours, puis la réserver.

## Méthode

1. Une branche d'intégration locale (`outils/leonida`) = leur `main` + nos PR en attente. C'est elle qu'on fait tourner, sans attendre leurs fusions.
2. Des PR petites, une par sujet réservé, rebasées juste après chaque fusion : leurs fichiers chauds (`d3d12_vulkan_device.cpp`, `check.py`, `run_gpu_tests.py`) changent dans plus de 100 PR ouvertes.
3. Aucun fichier du jeu dans aucun des deux dépôts.
