# VItoriaBr

> 🇧🇷 **Português** | [🇺🇸 **English below**](#english)

Camada nativa de execução em Windows para **um título Xbox Series X|S**, rodando diretamente a partir dos arquivos que o próprio usuário possui.

O código x86-64 original do título executa inalterado dentro do nosso processo; o **VItoriaBr** fornece a plataforma Xbox/GDK por baixo e traduz os dados e shaders do console (D3D12.X / GFX10 → D3D12 / DXIL). Veja [docs/architecture.md](docs/architecture.md). Para uma explicação didática, com diagramas, de por que um jogo de Xbox não roda no PC e de como cada parte do projeto resolve isso, veja [docs/visao-geral.md](docs/visao-geral.md).

> **Base e licença:** Fork/continuação baseada no projeto open-source [Marvisak/VIctory](https://github.com/Marvisak/VIctory), com integração das melhorias GPL-3.0 de renderização do mundo 3D, iluminação, relay/VRAM e otimizador DXIL de [fxceless/victory-fork](https://github.com/fxceless/victory-fork), mantido sob a licença **GPL-3.0-or-later** ([LICENSE](LICENSE)). **Este repositório não contém nem distribui nenhum arquivo de jogo** — todos os caminhos de jogo, cache e dados devem ficar fora do repositório ([docs/asset-policy.md](docs/asset-policy.md)).

---

## Status e Progresso (2026-11-25)

**Em desenvolvimento ativo — mundo aberto 3D já renderizando ao vivo!** O título inicia, roda os menus e vídeos a 30 fps cravados, salva perfil/configurações, entra no mundo 3D sem TDR e chega a **até `6 fps`** na RTX 5060 Ti (`3.500–4.400 draws/frame`), com profundidade/iluminação (`OMSetDepthBounds`), água, veículos, HUD e legendas funcionando.

### Testes reais em hardware (NVIDIA Blackwell)
Testado em: **Windows 11 Pro (build 26300)** · **Ryzen 7 5700X** · **40 GB RAM** · **NVIDIA RTX 5060 Ti 16 GB** (SM 6.6 + Mesh Shaders, driver `621.24`), sobre dump local Xbox Series `1.0.2.1`.

| Estágio | Estado |
|---------|--------|
| Boot, telas iniciais (acessibilidade, idioma PT-BR, configurações, legal) | ✅ Funciona — **30 fps** |
| Vídeo de introdução com áudio | ✅ Funciona (render limpo) |
| Tela inicial, seletor de conta, perfil e saves persistentes | ✅ Funciona |
| Tela de loading entrando no mundo (~50 s) | ✅ Funciona |
| Mundo 3D (cutscene de abertura, sala inicial e mundo aberto) | 🟡 **Renderizando ao vivo na RTX 5060 Ti (até `6 fps`, zero TDR, `3.500–4.400 draws/frame`)** — áreas na sombra ainda escuras (RTGI BVH builder em `50/53`) |

### Cobertura de Shaders & PSOs medida na RTX 5060 Ti (`1.0.2.1`)
Medido com `victory translate --validate --pipelines` (`dxil.dll` Windows SDK `10.0.26100.0` + driver NVIDIA `621.24`):
- **Containers analisados:** `26.599` (`26.148` programas traduzidos para IR e 100% estruturados — `40,6 milhões` de instruções IR).

| Estágio | DXIL emitidos | Validados (`dxil.dll`) | PSOs aceitos (RTX 5060 Ti) | Taxa (PSOs / emitidos) |
|---------|---------------|------------------------|----------------------------|------------------------|
| **Vertex + Pixel** | `14.248 / 14.259` | `14.248` | **`14.248`** | **100,0%** |
| **Mesh** | `8.765 / 8.765` | `8.694` | **`8.765`** | **100,0%** |
| **Compute** | `2.728 / 2.734` | `2.728` | **`2.331`** | **85,4%** |
| **Total** | **`25.741 / 25.758` (99,9%)** | **`25.670`** | **`25.344`** | **98,5%** |

> Os únicos `397` PSOs recusados pela RTX 5060 Ti são compute shaders que exigem **wave64** (GPUs NVIDIA são wave32).

### Melhorias já aplicadas no VItoriaBr
- **Sync `fxceless/victory-fork` (`4f9a19d`, `b401954`) + Estabilidade & PRs da comunidade (`32d7848`, `aa85bb0`, `faca57e`, `d396d11`, `06161b9`):**
  - Depth bounds test (`#82`), renderização completa da primeira sala (`a26859f`), fingerprint barato no sync de descriptors (`9376fd6`) e `VICTORY_RELAY=2` com correção de race condition em `relay_land()` (`aa85bb0`).
  - Guard de tempo/bytes (`PASS_BUDGET`, `faca57e`) em `heap_textures` evitando travamento do watchdog de 30 s no 1º frame 3D.
  - Fixes de estabilidade do upstream (loader lock deadlock, XMA wrap/continuation, dynamic descriptor wraparound, `SHADER_HEAPS` UAF em `mirror()`/`make_waiting()`).
  - PRs da comunidade: **`#6` (`victory warm`)**, **`#3` (Linux portable CI + `docs/visao-geral.md`)**, **`#7` (fix de stride de 16 bytes em `SLOT_VIEWS`)**, **`#10` (RDNA2 LDS bounds checks)**, **`#11` (formatos de textura 189–191)** e **`#12` (packed dot products `dot4AddPacked`)**.
- **Assinaturas e interpolação reconciliadas** (`c0b60a0`, `crates/victory-shader/src/emit/{shader,container}.rs`):
  - `reconcile_row_interp` foi restaurada (o merge grande a havia removido): elementos empacotados na mesma row compartilham um único modo de interpolação; **~250 shaders** deixaram de falhar no `dxil.dll`.
  - A ordem dos elementos de `ISG1`/`OSG1` agora é a do console (sem ordenar por registrador), fechando as **33 falhas de input signature** e **39 de output signature**.
  - Resultado no v7: **`25.588` validados** (eram `25.413`).
- **Descritores de tabela com índice calculado lidos como dado (SLOT_VIEWS)** (`b62bbc0`, `18df163`, `77ad907`, `crates/victory-shader/src/emit/lower.rs` + `crates/victory-gpu`):
  - A nova root SRV `SLOT_VIEWS` (252) expõe, por slot do heap, o endereço/len/stride da view de buffer; o emissor usa essa tabela quando o console lê o **endereço de um descritor** cujo índice é calculado em runtime.
  - `calculateLOD` de texturas array agora deixa a coordenada do slice `undef`, como o DXC exige; isso zerou os `3` `out of bound coord must be undef`.
  - Resultado no v13: **`25.741 / 25.758` DXIL (99,9%)**, **`25.670` validados**, **`25.344` PSOs**; as categorias `descriptor read only as data` (40), `out of bound coord` (3) e `descriptor chosen by control flow` (40) foram **zeradas**.
- **Dois samplers imediatos escolhidos por branch** (`67bb579`, `c248547`): o phi entre os dois samplers constantes virou um handle de **static sampler range** com índice dinâmico. O seletor é um **phi de caminho no join** (true vindo do segundo predecessor), não `reach[pred]` — este último definia o valor em um predecessor e não dominava o join, invalidando o bitcode. Os **40** shaders da categoria (`37 PS_TexturedLit` + `3 MST_DS`) agora emitem, validam e criam PSO.
- **Rastreamento completo de estado de `Shadow` buffers no `HostList`** (`crates/victory-gpu/src/{shadow,host,indirect,draw}.rs`):
  - Antes, `indirect::done()` deixava `title buffer` compartilhados presos em `D3D12_RESOURCE_STATE_UNORDERED_ACCESS`, causando erros de validação em `IASetVertexBuffers` e `id=527` no D3D12 Debug Layer.
  - Agora cada `Shadow` buffer rastreia seu estado via `prelude()` e transições intra-lista (`GENERIC_READ` para VB/IB/CBV/SRV/Indirect `args`+`counter`, `UNORDERED_ACCESS` para UAV/clamp), **zerando os erros de barreira no D3D12 Debug Layer** e estendendo significativamente a execução dentro do mundo 3D na RTX 5060 Ti.
- **Suporte a `MIMG` `a16` e operações LDS de byte/half** (`2471a62`, `crates/victory-shader/src/emit/lower.rs`):
  - `image_sample_b`, `_cl`/`_o`, `image_gather4_l` e leitura/escrita thread-safe de bytes/halves em LDS (`ds_read_{u8,i8,u16,i16}`, `ds_write_{b8,b16}{,_d16_hi}`).
- **Reconciliação de modos de interpolação DXIL por row da assinatura** (`5943771`, `crates/victory-shader/src/emit/shader.rs`):
  - Elementos que compartilham a mesma row na assinatura passam a emitir o mesmo modo de interpolação, **zerando ~48 erros do validador `dxil.dll`**.
- **Reciclagem imediata de slots dinâmicos do descriptor heap** (`a65d054`, `bd33acc`, `crates/victory-gpu/src/{descriptor/host,draw,list,queue}.rs`):
  - Os slots voltam ao heap assim que o `host.execute` síncrono termina, e o `MIN_DYNAMIC` sobe para **`2^18` (262.144)** em heaps limitados (NVIDIA, limite de 1.000.000 views), **zerando os 16.384+ draws/dispatches descartados** no mundo 3D.
- **Cubemap `image_sample_d_cl`** (`77b4b4c`, `crates/victory-shader/src/emit/lower.rs`):
  - `deriv_dwords` ajustado para `grads = 2` em cubemaps (`kind == 5`), destravando +27 shaders.
- **Estabilidade de Mesh Shaders, LDS e barreiras na NVIDIA Blackwell (wave32)** (`bd33acc`, `a13a719`, `1ece3a6`, `911267c`, `808a209`, `crates/victory-shader/src/emit/lower.rs`):
  - `mesh_prologue` limita `setMeshOutputCounts` ao número de threads para não deixar primitivas não-inicializadas; `emitIndices`, `storeVertexOutput` e `storePrimitiveOutput` validam limites contra `max_vertices`/`max_primitives`.
  - Todos os acessos a `LDS` (`ds_read`, `ds_write`, atomics) fazem bounds check fiel ao RDNA2 (OOB lê `0` e descarta escrita, evitando traps de SRAM na NVIDIA).
  - `s_barrier` em blocos que não pós-dominam a entrada, dentro de loops ou após `abort_block` emite apenas memory fence (`TGSM_FENCE`/`UAV_FENCE`), prevenindo deadlocks de `BAR.SYNC` em wave32.
- **Proteção anti-TDR em submissões e `ExecuteIndirect`** (`1ca533d`, `8a75883`, `6f99f75`, `crates/victory-gpu/src/{host,indirect}.rs`):
  - `host::execute()` submete 1 command list por vez com fence wait (evitando acumular >2 s de TDR); `indirect.rs` zera argumentos `DispatchMesh` corrompidos e limita dispatches válidos (`4096` grupos, `BUDGET = 2^22`).
  - `VICTORY_SHADER_LOOP_LIMIT` habilitado também em `release` (com padrão `4096`).

> **Aviso de GPU (TDR):** entrar no mundo 3D em uma GPU real ainda pode disparar reset do driver de vídeo (TDR). Para inspecionar sem risco de travar o driver, use o perfil `diag` com `VICTORY_D3D12_WARP=1` (adaptador em software).

---

## Como compilar e executar

**Requisitos:** Windows 10/11 x64, CPU com AVX2, GPU Direct3D 12 com Shader Model 6.6 + Mesh Shaders, Rust (`rust-toolchain.toml` — toolchain `x86_64-pc-windows-msvc`), VS 2022 MSVC e Windows SDK.

> O título exige vários GiB de memória commit livre no Windows. Se `crashes.log` reportar erro `1455`, feche outros programas ou aumente o arquivo de paginação.

```powershell
# 1. Configurar caminhos (sempre FORA do repositório)
copy victory.example.toml victory.toml
# Edite victory.toml com game_dir, cache_dir e data_dir

# 2. Validar configuração e testes do workspace (não precisa de dados do jogo)
cargo run -p victory -- config
cargo test --workspace

# 3. Compilar em release
cargo build --release -p victory
# (opcional) pré-traduz os shaders para o cache antes da 1ª execução
.\target\release\victory.exe warm <pasta_do_jogo>
.\target\release\victory.exe run <pasta_do_jogo>\<executavel_do_titulo> --execute
```

> Pré-compilação de shaders (`victory warm`), o que ela cobre e como conferir no log: [docs/precompilacao-shaders.md](docs/precompilacao-shaders.md).

### Preset recomendado (testado na RTX 5060 Ti 16 GB — até `6 fps`)
```powershell
# Opcional (1x por atualização): pré-aquece todo o cache de shaders antes de abrir o título
.\target\release\victory.exe warm <pasta_do_jogo>

# Preset de maior FPS medido na RTX 5060 Ti 16 GB (3.5–6.0 fps no mundo aberto, zero TDR)
$env:VICTORY_RELAY = "2"
$env:VICTORY_TEXTURE_NO_WAIT = "1"
$env:VICTORY_CARD_BUFFERS_MB = "6144"
$env:VICTORY_WIDE_REFRESH = "1"
$env:VICTORY_WIDE_MERGE = "1"
$env:VICTORY_JOIN_COPIES = "1"
$env:VICTORY_DISPLAY_MODE = "1920x1080"

.\target\release\victory.exe run --execute <pasta_do_jogo>\<executavel_do_titulo>
```

#### Achados importantes de tuning & diagnóstico
- **`VICTORY_TEXTURE_NO_WAIT=1`**: elimina stalls de lock na leitura de memória do título durante refresh de texturas (`textures` cai de `900–3.700 ms` para `6–25 ms`).
- **`VICTORY_DISPLAY_MODE=1920x1080`**: reduz a resolução interna de renderização mesmo em monitores 4K (a janela apenas faz upscale barato no `Present`).
- **`VICTORY_CARD_FETCH_INFLIGHT` (deixar desligado / `0`)**: ligá-lo derruba `copying back` de `~162 ms` para `~45 ms`, mas sobrecarrega a GPU (`on the GPU` sobe de `~50–65 ms` para `~200–330 ms`), reduzindo o FPS total na RTX 5060 Ti (`1.9–2.6 fps` vs `3.5–6.0 fps`).
- **`VICTORY_BVH_BUILDER=1` (RTGI — experimental)**: atinge **50/53** compute shaders em wave32; os 3 faltantes (`39321a95223001ed`, `77360df21e12ea43`, `9fc111d3be79364f`) falham em `v_readfirstlane_b32` sob divergência. Com `VICTORY_FIRSTLANE_OWN=1` o builder chega a **53/53** e ativa, mas o atalho corrompe offsets de AABB na memória (crash após ~18 s) — manter desligado até a troca completa entre metades da wave64.

### Build de diagnóstico (`diag`)
```powershell
cargo build --profile diag -p victory --features diagnostics
```
Principais variáveis de ambiente no `diag`:
- `VICTORY_D3D12_WARP=1` — força adaptador WARP em software (lento, não causa TDR)
- `VICTORY_D3D12_DRED=1` — grava breadcrumbs DRED em `crashes.log` se ocorrer `DEVICE_HUNG`
- `VICTORY_D3D12_DEBUG=1` / `VICTORY_D3D12_GBV=1` — D3D12 Debug Layer e GPU-Based Validation
- `VICTORY_CAPTURE=<pasta>` / `VICTORY_CAPTURE_FRAMES=<lista>` / `VICTORY_DUMP_TARGETS=1` — salva frames PNG e render targets
- `VICTORY_SHADER_LOOP_LIMIT=<n>` — limita iterações de loops em shaders traduzidos
- `VICTORY_SKIP_PIPELINES=<hash>,...` — pula draws/dispatches de hashes específicos

### Controles (teclado quando não há controle XInput conectado)
| Teclas | Controle |
|--------|----------|
| `W A S D` / `I J K L` | Analógico esquerdo / direito |
| Setas | D-pad |
| `Enter`, `Backspace`, `X`, `Y` | A, B, X, Y |
| `Q` / `E`, `Z` / `C` | LB/RB, LT/RT |
| `F` / `H` | L3 / R3 |
| `Esc`, `Tab` | Menu, View |

---

<a id="english"></a>

## English

### Overview

**VItoriaBr** runs **an Xbox Series X|S title** natively on Windows from files the user supplies.

The title's x86-64 code runs unchanged inside our process; **VItoriaBr** provides the underlying console platform and translates shaders/resources (D3D12.X / GFX10 → host D3D12 / DXIL). See [docs/architecture.md](docs/architecture.md).

> **Upstream & License:** Derived from [Marvisak/VIctory](https://github.com/Marvisak/VIctory) with GPL-3.0 world-rendering, lighting, relay/VRAM, and DXIL optimizer improvements integrated from [fxceless/victory-fork](https://github.com/fxceless/victory-fork), licensed under **GPL-3.0-or-later** ([LICENSE](LICENSE)). **No game files or derived assets are shipped or allowed in this repository** ([docs/asset-policy.md](docs/asset-policy.md)).

### Status & Progress (2026-11-25)

**Active development — 3D open world rendering live!** Boot, menus, intro video with audio, profile/saves, and loading work at 30 fps; inside the 3D open world on RTX 5060 Ti it runs with **zero TDR** at **up to `6 fps`** (`3,500–4,400 draws/frame`), with depth-bounds lighting, ocean/water, vehicles, HUD, and subtitles.

#### Real Hardware Test Results (NVIDIA Blackwell)
Tested on: **Windows 11 Pro (build 26300)** · **Ryzen 7 5700X** · **40 GB RAM** · **NVIDIA RTX 5060 Ti 16 GB** (SM 6.6 + Mesh Shaders, driver `621.24`), Xbox Series build `1.0.2.1`.

| Stage | Status |
|-------|--------|
| Boot & first-boot screens (accessibility, language, settings, legal) | ✅ Works — **30 fps** |
| Intro video with audio | ✅ Works |
| Start screen, account selector, profile & persistent saves | ✅ Works |
| Loading into the world (~50 s) | ✅ Works |
| 3D world (opening cutscene, first room & open world) | 🟡 **Rendering live on RTX 5060 Ti (up to `6 fps`, zero TDR, `3,500–4,400 draws/frame`)** — shadowed areas still dark (RTGI BVH builder at `50/53`) |

#### Measured Shader & PSO Coverage on RTX 5060 Ti (`1.0.2.1`)
Measured via `victory translate --validate --pipelines` (`dxil.dll` Windows SDK `10.0.26100.0` + NVIDIA driver `621.24`):
- **Containers scanned:** `26,599` (`26,148` programs translated & 100% structured — `40.6M` IR instructions).

| Stage | DXIL Emitted | Validated (`dxil.dll`) | Host PSOs (RTX 5060 Ti) | PSO Rate |
|-------|--------------|------------------------|-------------------------|----------|
| **Vertex + Pixel** | `14,248 / 14,259` | `14,248` | **`14,248`** | **100.0%** |
| **Mesh** | `8,765 / 8,765` | `8,694` | **`8,765`** | **100.0%** |
| **Compute** | `2,728 / 2,734` | `2,728` | **`2,331`** | **85.4%** |
| **Total** | **`25,741 / 25,758` (99.9%)** | **`25,670`** | **`25,344`** | **98.5%** |

> The `397` refused Compute PSOs are wave64-only shaders (NVIDIA GPUs run wave32).

#### VItoriaBr Improvements
- **Full `Shadow` buffer D3D12 resource state tracking in `HostList`** (`crates/victory-gpu/src/{shadow,host,indirect,draw}.rs`):
  - Previously, `indirect::done()` left shared title buffers stuck in `D3D12_RESOURCE_STATE_UNORDERED_ACCESS`, causing `IASetVertexBuffers` and barrier mismatch (`id=527`) errors in the D3D12 Debug Layer.
  - Each `Shadow` buffer now tracks its state across lists via `prelude()` and intra-list barriers (`GENERIC_READ` for VB/IB/CBV/SRV/indirect `args`+`counter`, `UNORDERED_ACCESS` for UAV/clamp), **eliminating all buffer state errors in the D3D12 Debug Layer** and significantly extending 3D world execution on the RTX 5060 Ti.
- **`MIMG` `a16` support and LDS byte/half operations** (`2471a62`, `crates/victory-shader/src/emit/lower.rs`):
  - `image_sample_b`, `_cl`/`_o`, `image_gather4_l`, plus thread-safe byte/half reads and writes in LDS (`ds_read_{u8,i8,u16,i16}`, `ds_write_{b8,b16}{,_d16_hi}`).
- **Per-row DXIL interpolation mode reconciliation on the signature** (`5943771`, `crates/victory-shader/src/emit/shader.rs`):
  - Elements sharing the same signature row now emit the same interpolation mode, **clearing ~48 `dxil.dll` validator errors**.
- **Immediate recycling of dynamic descriptor-heap slots** (`a65d054`, `bd33acc`, `crates/victory-gpu/src/{descriptor/host,draw,list,queue}.rs`):
  - Slots return to the heap as soon as the synchronous `host.execute` completes, and `MIN_DYNAMIC` rises to **`2^18` (262,144)** on capped heaps (NVIDIA, 1,000,000-view limit), **eliminating the 16,384+ dropped draws/dispatches** in the 3D world.
- **Cubemap `image_sample_d_cl`** (`77b4b4c`, `crates/victory-shader/src/emit/lower.rs`):
  - Uses `grads = 2` for `deriv_dwords` on cubemaps (`kind == 5`), unblocking +27 shaders.
- **Mesh Shader, LDS, and barrier safety on NVIDIA Blackwell (wave32)** (`bd33acc`, `a13a719`, `1ece3a6`, `911267c`, `808a209`, `crates/victory-shader/src/emit/lower.rs`):
  - `mesh_prologue` caps `setMeshOutputCounts` to thread count so no uninitialized primitives are left; `emitIndices`, `storeVertexOutput`, and `storePrimitiveOutput` enforce `max_vertices`/`max_primitives` bounds.
  - All `LDS` reads, writes, and atomics enforce RDNA2-accurate OOB semantics (read `0`, drop writes) to prevent NVIDIA shared-memory traps.
  - `s_barrier` in non-post-dominating, loop, or post-`abort_block` blocks emits a memory-only fence (`TGSM_FENCE`/`UAV_FENCE`), preventing `BAR.SYNC` deadlocks on wave32.
- **Anti-TDR submission chunking & indirect bounds** (`1ca533d`, `8a75883`, `6f99f75`, `crates/victory-gpu/src/{host,indirect}.rs`):
  - `host::execute()` submits one command list per fence wait; `indirect.rs` zeroes out-of-range `DispatchMesh` arguments and caps valid dispatches (`4096` groups, `BUDGET = 2^22`).
  - `VICTORY_SHADER_LOOP_LIMIT` enabled in `release` builds as well (default `4096`).

> **Warning:** Entering the 3D world on a real GPU can still trigger a display driver reset (TDR). Use a `diag` build with `VICTORY_D3D12_WARP=1` to inspect safely on the software adapter.

### Quick Start

```powershell
copy victory.example.toml victory.toml   # set paths outside the repository
cargo run -p victory -- config
cargo test --workspace
cargo build --release -p victory
.\target\release\victory.exe warm <game_dir>   # optional: pre-translate shaders into the cache first

# Recommended preset tested on RTX 5060 Ti 16 GB (up to 6 fps, zero TDR)
$env:VICTORY_RELAY = "2"
$env:VICTORY_TEXTURE_NO_WAIT = "1"
$env:VICTORY_CARD_BUFFERS_MB = "6144"
$env:VICTORY_WIDE_REFRESH = "1"
$env:VICTORY_WIDE_MERGE = "1"
$env:VICTORY_JOIN_COPIES = "1"
$env:VICTORY_DISPLAY_MODE = "1920x1080"

.\target\release\victory.exe run --execute <game_dir>\<title_exe>
```

> **Tuning notes (RTX 5060 Ti):** Keep `VICTORY_CARD_FETCH_INFLIGHT` off (`0`, default) — enabling it drops `copying back` (`~162 → ~45 ms`) but increases `on the GPU` (`~50–65 → ~200–330 ms`), lowering overall FPS. `VICTORY_BVH_BUILDER=1` reaches `50/53` compute shaders on wave32 (missing 3 `v_readfirstlane_b32` scans: `39321a95223001ed`, `77360df21e12ea43`, `9fc111d3be79364f`); `VICTORY_FIRSTLANE_OWN=1` reaches `53/53` and activates the builder, but corrupts BVH AABB offsets (~18 s crash), so leave it off for now.

Diagnostics build:
```powershell
cargo build --profile diag -p victory --features diagnostics
```

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
