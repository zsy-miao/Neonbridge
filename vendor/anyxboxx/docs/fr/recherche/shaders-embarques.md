> English: [embedded-shaders.md](../../research/embedded-shaders.md)

# Shaders embarqués du paquet : mesure sans llvm-mc

Mesure du 22 novembre 2026, faite localement ; seuls des comptes figurent ici (aucun shader, désassemblage ni
SPIR-V, règle 1 d'AGENTS.md).

`XboxBvhBuilder.dll` (bibliothèque de Microsoft livrée dans le paquet) contient 52 conteneurs DXBC, chacun avec
un programme RDNA 2 natif (`XBCH` → `FSSM`), d'après [la note de Leonida](https://github.com/ItsNotRexed/Leonida/blob/main/docs/embedded_shaders.md).
Ce sont les seuls vrais shaders console disponibles avant l'exécutable du jeu.

| Étape | Résultat |
|---|---|
| Désassemblage par AnyXboxX (sans llvm-mc) | 54 197 instructions ; 1 431 mots sans texte, des données entre les programmes d'un même blob (jusqu'à 5 programmes) |
| Traduction RDNA 2 → SPIR-V (traducteur de Leonida, nourri par notre désassemblage) | **51 programmes sur 52** |
| Échec restant | `buffer_store_short`, règle absente du traducteur |

Deux corrections ont permis ce résultat (avant : 0, puis 41 sur 52) :

1. **Compute shaders** : le traducteur lit l'état de dispatch (`rsrc2`, taille du groupe) dans l'en-tête FSSM.
   `rdna2_spirv.translate` accepte maintenant le blob (`fssm=`) ou cet état (`dispatch=`) ; sans l'un ni
   l'autre, un compute shader est refusé avec un message clair.
2. **Données entre programmes** : avec `code_start`, `disassembly` saute un mot sans texte comme llvm-mc et le
   signale ; le traducteur se recale sur le blob FSSM.

Reproduire (extraction avec les outils GPL-3.0 de Leonida, dans un dossier de travail hors du dépôt) :

```powershell
python -I pe_shader_containers.py <paquet>\XboxBvhBuilder.dll --json <travail>\containers.json --extract <travail>\dxbc
python -I fssm_unpack.py <travail>\dxbc
python tools\fssm_mesure.py <travail>\dxbc
```

Le même outil servira sur les shaders du jeu quand l'exécutable et ses PSO seront disponibles.
