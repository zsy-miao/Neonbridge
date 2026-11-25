> Français : [sources-rdna2.md](../fr/recherche/sources-rdna2.md)

# RDNA 2: where to read the primary sources

Rule 2 of [AGENTS.md](../../AGENTS.md) asks RDNA 2 semantics to come from public sources, AMD's ISA documentation first. AMD's site does not serve its PDFs to scripts, so this note lists where each source can be read, and how to tell an RDNA 2 statement from one taken from a neighbouring generation. Never commit these documents: link them.

## 1. AMD, "RDNA 2" Instruction Set Architecture (the reference)

- Edition: "Reference Guide", AMD, **30 November 2020**, 291 pages.
- AMD URLs: `https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf` (the old `https://developer.amd.com/wp-content/resources/RDNA2_Shader_ISA_November2020.pdf` redirects there). Measured on 24 November 2026: `curl` and fetch tools get an HTML page of the "AMD Technical Information Portal" instead of the PDF.
- Archived copies (Wayback Machine), byte-identical, SHA-256 `22aec03f2706dba2e148ffde1f3a25d5cdd42c71a96f95240aeb48d995d44795`:
  - `http://web.archive.org/web/20230214072857id_/https://developer.amd.com/wp-content/resources/RDNA2_Shader_ISA_November2020.pdf`
  - `http://web.archive.org/web/20250719062008id_/https://www.amd.com/content/dam/amd/en/documents/radeon-tech-docs/instruction-set-architectures/rdna2-shader-instruction-set-architecture.pdf`

  The `id_` after the timestamp returns the original file without the archive banner. Check the SHA-256 before citing.
- Searching it: extract the text once, in a virtual environment of its own (not the project's `.venv`), and search the text:

  ```powershell
  py -3.12 -m venv $env:TEMP\pdfenv
  & $env:TEMP\pdfenv\Scripts\python.exe -m pip install pypdf
  & $env:TEMP\pdfenv\Scripts\python.exe -c "import sys; from pypdf import PdfReader; r = PdfReader(sys.argv[1]); open(sys.argv[2], 'w', encoding='utf-8').write(''.join(f'\n=== PAGE {i} ===\n' + (p.extract_text() or '') for i, p in enumerate(r.pages, 1)))" rdna2.pdf rdna2.txt
  ```

  PDF page numbers are 8 higher than the page numbers printed in the document (printed page 19 is PDF page 27). Cite the section and the printed page.
- Already cited in the code: section 3.9 "Vector Compares: VCC and VCCZ" (printed pages 19–20: VCC marks only executing threads, for compares and integer carry-out, and "is fully written; there are no partial mask updates") and section 6.2 (printed page 43: carry-out goes to VCC in VOP2 form, to an arbitrary SGPR pair in VOP3 form).

## 2. AMD machine-readable ISA specifications (GPUOpen)

- `https://gpuopen.com/download/machine-readable-isa/latest/` gives a ZIP with one XML file per architecture, including `amdgpu_isa_rdna2.xml` (RDNA 2) and `amdgpu_isa_rdna3.xml`. Tools to read them: [GPUOpen-Tools/isa_spec_manager](https://github.com/GPUOpen-Tools/isa_spec_manager).
- The RDNA 2 file has the encodings, operand types and sizes, and a one-line description per instruction, **without pseudo-code**. The RDNA 3 file carries a few more notes (for example, on the VOPC encoding: "if EXEC[laneId] is zero, then the comparison for that thread is not performed and the result of VCC[laneId] is zero").

## 3. Neighbouring sources: useful, but say which generation they describe

- **RDNA 3 ISA pseudo-code** (AMD's PDF, also extracted into `rdna3_pcode.py` of tinygrad): often the same opcodes and a precise formula, but it describes RDNA 3. Label any rule taken from it as "RDNA 3" in code and pull requests.
- **LLVM** (`llvm/lib/Target/AMDGPU`, tests in `llvm/test/MC/AMDGPU`): encodings and assembler syntax (the oracle of `tools/rdna2_mnemonics.py`), constant folding of some intrinsics, and code comments. A `TODO` in LLVM is a hint, not a specification.
- **Mesa** (`src/amd/compiler` for ACO, `src/amd/registers` for descriptor fields): how a production compiler uses an instruction, read statically.
- **Hardware measurements published by others**, for example Lyrnc's RDNA 2 (gfx1036) measurements of `v_cvt_rpi/flr_i32_f32` in [VIctory #605](https://github.com/Marvisak/VIctory/pull/605#issuecomment-7123302580): the strongest evidence when the documentation is silent or ambiguous; cite the comment and the chip.

## 4. Order of preference

1. The RDNA 2 PDF (section and printed page).
2. A published RDNA 2 hardware measurement (chip, driver, author).
3. The RDNA 2 machine-readable XML (descriptions, operand types).
4. RDNA 3 pseudo-code, LLVM, Mesa: as a cross-check, named as such.

When none of them settles a case, the code raises `Untranslated` / `Untranslatable` naming the case (rule 3), or says "assumed" next to the choice.
