> English: [rdna2-sources.md](../../research/rdna2-sources.md)

# RDNA 2 : où lire les sources primaires

La règle 2 d'[AGENTS.md](../../../AGENTS.md) demande que la sémantique RDNA 2 vienne de sources publiques, d'abord la documentation ISA d'AMD. Le site d'AMD ne sert pas ses PDF aux scripts : cette note dit où lire chaque source, et comment distinguer ce qui vaut pour RDNA 2 de ce qui vient d'une génération voisine. Ne jamais ajouter ces documents au dépôt : les lier.

## 1. AMD, « "RDNA 2" Instruction Set Architecture » (la référence)

- Édition : « Reference Guide », AMD, **30 novembre 2020**, 291 pages.
- URL d'AMD : `https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf` (l'ancienne `https://developer.amd.com/wp-content/resources/RDNA2_Shader_ISA_November2020.pdf` y redirige). Mesuré le 24 novembre 2026 : `curl` et les outils de récupération reçoivent une page HTML du « AMD Technical Information Portal » au lieu du PDF.
- Copies archivées (Wayback Machine), identiques octet par octet, SHA-256 `22aec03f2706dba2e148ffde1f3a25d5cdd42c71a96f95240aeb48d995d44795` :
  - `http://web.archive.org/web/20230214072857id_/https://developer.amd.com/wp-content/resources/RDNA2_Shader_ISA_November2020.pdf`
  - `http://web.archive.org/web/20250719062008id_/https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf`

  Le `id_` après la date renvoie le fichier d'origine, sans le bandeau de l'archive. Vérifier le SHA-256 avant de citer.
- Pour chercher dedans : extraire le texte une fois, dans un environnement virtuel à part (pas le `.venv` du projet), puis chercher dans le texte :

  ```powershell
  py -3.12 -m venv $env:TEMP\pdfenv
  & $env:TEMP\pdfenv\Scripts\python.exe -m pip install pypdf
  & $env:TEMP\pdfenv\Scripts\python.exe -c "import sys; from pypdf import PdfReader; r = PdfReader(sys.argv[1]); open(sys.argv[2], 'w', encoding='utf-8').write(''.join(f'\n=== PAGE {i} ===\n' + (p.extract_text() or '') for i, p in enumerate(r.pages, 1)))" rdna2.pdf rdna2.txt
  ```

  Les numéros de page du PDF dépassent de 8 ceux imprimés dans le document (la page imprimée 19 est la page 27 du PDF). Citer la section et la page imprimée.
- Déjà cité dans le code : section 3.9 « Vector Compares: VCC and VCCZ » (pages imprimées 19-20 : VCC ne marque que les threads qui s'exécutent, pour les comparaisons et la retenue des additions et soustractions entières, et « is fully written; there are no partial mask updates ») et section 6.2 (page imprimée 43 : la retenue va dans VCC en forme VOP2, dans une paire de SGPR quelconque en forme VOP3).

## 2. Spécifications ISA lisibles par machine d'AMD (GPUOpen)

- `https://gpuopen.com/download/machine-readable-isa/latest/` donne un ZIP avec un fichier XML par architecture, dont `amdgpu_isa_rdna2.xml` (RDNA 2) et `amdgpu_isa_rdna3.xml`. Outils pour les lire : [GPUOpen-Tools/isa_spec_manager](https://github.com/GPUOpen-Tools/isa_spec_manager).
- Le fichier RDNA 2 a les encodages, les types et tailles d'opérandes et une description d'une ligne par instruction, **sans pseudo-code**. Le fichier RDNA 3 porte quelques notes de plus (par exemple, pour l'encodage VOPC : « if EXEC[laneId] is zero, then the comparison for that thread is not performed and the result of VCC[laneId] is zero »).

## 3. Sources voisines : utiles, mais dire quelle génération elles décrivent

- **Pseudo-code de l'ISA RDNA 3** (PDF d'AMD, aussi extrait dans `rdna3_pcode.py` de tinygrad) : souvent les mêmes opcodes et une formule précise, mais il décrit RDNA 3. Étiqueter « RDNA 3 » toute règle qui en vient, dans le code et les PR.
- **LLVM** (`llvm/lib/Target/AMDGPU`, tests dans `llvm/test/MC/AMDGPU`) : encodages et syntaxe de l'assembleur (l'oracle de `tools/rdna2_mnemonics.py`), repli constant de certains intrinsèques, commentaires du code. Un `TODO` de LLVM est un indice, pas une spécification.
- **Mesa** (`src/amd/compiler` pour ACO, `src/amd/registers` pour les champs des descripteurs) : comment un compilateur de production emploie une instruction, lu statiquement.
- **Mesures matérielles publiées par d'autres**, par exemple celles de Lyrnc sur RDNA 2 (gfx1036) pour `v_cvt_rpi/flr_i32_f32` dans [VIctory #605](https://github.com/Marvisak/VIctory/pull/605#issuecomment-7123302580) : la preuve la plus forte quand la documentation se tait ou est ambiguë ; citer le commentaire et la puce.

## 4. Ordre de préférence

1. Le PDF RDNA 2 (section et page imprimée).
2. Une mesure matérielle RDNA 2 publiée (puce, pilote, auteur).
3. Le XML RDNA 2 lisible par machine (descriptions, types d'opérandes).
4. Pseudo-code RDNA 3, LLVM, Mesa : en recoupement, nommés comme tels.

Quand aucune ne tranche, le code lève `Untranslated` / `Untranslatable` en nommant le cas (règle 3), ou écrit « supposé » à côté du choix.
