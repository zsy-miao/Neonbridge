> English: [RPF_RESEARCH.md](../../../research/package/RPF_RESEARCH.md)

# Relevé des en-têtes RPF

Les 22 fichiers RPF déclarés dans le layout commencent par les octets ASCII `9FPR`. Leur lecture comme entier 32 bits little-endian donne `0x52504639`. Les valeurs ci-dessous décrivent les octets observés et ne définissent pas encore un lecteur du répertoire interne.

Pour examiner un en-tête externe ou un candidat interne sans lire tout le fichier :

```powershell
python .\inspect_rpf.py .\script.rpf
python .\inspect_rpf.py .\script.rpf --offset 0x1000
python .\inspect_rpf.py .\pack\story000\story000_script.rpf --offset 0x18000
```

Le programme donne les 16 mots little-endian et l'empreinte SHA-256 des 64 octets examinés. L'offset est compté depuis le début du fichier.

| Offset | Observations sur les 22 en-têtes externes |
| --- | --- |
| 0x00 | `0x52504639` |
| 0x04, 0x08, 0x0c | Valeurs variables. Pour les 22 fichiers, `0x04` et `0x08` sont des multiples de 16 et `0x04 > 0x08`. Leur sens précis demande une confirmation. |
| 0x10, 0x14 | Valeurs variables. |
| 0x18, 0x1c | Respectivement `0x00047a00` et `0x00000150`. |
| 0x20, 0x24, 0x28 | Valeurs variables. |
| 0x2c | `0x00090f01` ou `0x00090f02`. |
| 0x30 | `0x00000010`. |
| 0x34 à 0x3c | Trois mots nuls. |

La recherche alignée dans les deux fichiers de scripts a trouvé trois candidats dans `script.rpf`, dont le premier à l'offset zéro, et cinq dans `story000_script.rpf`, dont le premier à zéro. Les huit candidats internes et externes des deux fichiers partagent les valeurs constantes de la table aux offsets 0x18, 0x1c et 0x30. Dans les deux en-têtes externes, le mot à 0x24 vaut respectivement 2 et 4. Ces valeurs coïncident avec le nombre de candidats internes trouvés. Cette corrélation sur deux exemples reste une hypothèse et ne permet pas de conclure que le champ représente le nombre général d'entrées.

Des octets qui reproduisent une signature peuvent survenir dans des données. Les positions détectées constituent donc des candidats à examiner. Les limites exactes des archives internes, les noms des entrées et leur contenu restent à déterminer.
