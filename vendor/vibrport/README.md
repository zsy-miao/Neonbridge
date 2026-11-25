# VI-BR-PORT: a versão para placas de 8 GB

Camada de compatibilidade que roda no Windows, de forma nativa, a versão de
**Xbox Series X|S** (plataforma *Scarlett*, jogos feitos com o **GDK**) de um
título, a partir dos arquivos do próprio usuário. O código x86-64 do console
roda sem alteração dentro do nosso processo; por baixo dele ficam as bibliotecas
do sistema do console reescritas (HLE: XGameRuntime, kernel, memória, armazenamento,
áudio, entrada) e um back-end Direct3D 12 que recompila os shaders da GPU do
console (GFX10/RDNA2) para DXIL.

Este fork tem um alvo só: **placa NVIDIA de 8 GB com ondas de 32 lanes**
(testado numa RTX 4060 Ti) ligada a um **monitor 1080p**. Nessa máquina as
árvores de origem enchem a memória da placa, travam conferindo texturas e
esperam minutos por shaders; aqui está o que foi feito para isso.

**Este repositório não contém arquivos, código ou recursos do jogo.** É preciso
ter a sua própria cópia. Nada aqui tem ligação com o desenvolvedor ou a
editora do jogo, nem é aprovado por eles.

Origem: [VIctory](https://github.com/Marvisak/VIctory) → [fxceless/victory-fork](https://github.com/fxceless/victory-fork) → este fork.
A branch de trabalho é a `main`.

## O que só este fork faz

Resultados que não existem nas árvores de origem, cada um com o commit que dá para conferir:

- **Texturas em resolução cheia numa placa de 8 GB, sem rosto preto** (`44823d61`, `da32cb58`,
  `719e3b15`). Os shaders de pixel e de compute marcam quais texturas do heap de views leem de
  verdade; a placa guarda essas e tira primeiro as menos usadas. Antes, as ~116 mil texturas do
  heap iam para a placa por ordem de escrita, ela enchia e ~17 mil eram descartadas e lidas como
  zero: pele, cabelo e roupa saíam pretos. A ideia de marcar os slots vem do PR #1962 do VIctory,
  reescrita para esta árvore.
- **Seções ordenadas de pixel do console (POPS) como ROV** (`913b2f36`, `8107d596`): os 10 shaders
  de recorte de cabelo e roupa dos personagens, recusados pelas outras árvores, traduzem e criam
  pipeline em placas com Rasterizer Ordered Views.
- **Samplers escolhidos por desvio e raios de sombra com endereço lido do heap** (`05e719b3`,
  `4699d89a`): 44 shaders a mais traduzidos.
- **Todos os shaders traduzem: 25.758 de 25.758** numa placa de 32 lanes (contagem do
  `victory translate`; `0615710b`, `f237fecc`, `dc75ed82`, `8a1b59cf`, `96981b74`, `783a54ca`):
  leituras da primeira lane de valores iguais em todas as lanes, recusas refeitas no nível da
  wave, as duas metades de uma wave de 64 do console que se encontram sozinhas em grupos,
  reservas de contador na LDS e máscaras de wave64 copiadas metade por metade. 74 deles ainda
  não passam no validador do DirectX, mas a placa os aceita e eles rodam.
- **Modo de tela do monitor** (`VICTORY_DISPLAY_MODE=1920x1080`): os render targets caem de
  3,4 GB para 2,5 GB numa tela 1080p.


## Estado (25/11/2026)

Máquina de teste: Windows 11, Intel i7-13700K, RTX 4060 Ti 8 GB, 32 GB de RAM,
perfil Series X. As porcentagens são a nossa estimativa de cada etapa, não um
benchmark.

| Etapa | Situação | Progresso | Observações |
|-------|:--------:|----------:|-------------|
| Carrega e inicia | ✅ | 100 % | uns 10 s até o primeiro quadro |
| Vídeo de abertura e menus | ✅ | 100 % | 30 fps estáveis (o limite do próprio título) |
| Cutscene de abertura | 🟡 | 75 % | vai até o fim e chega à cena da prisão; cabelo e roupa com textura e sem flash verde; a pele de parte dos personagens ainda sai preta e falta a luz por ray tracing |
| Entrada no mundo | ✅ | 100 % | as pipelines vêm da biblioteca salva, sem tela preta |
| Jogável (controle, HUD, menus) | 🟡 | 50 % | controle, HUD e menu de pausa funcionam, mas devagar |
| Qualidade da imagem | 🟡 | 55 % | texturas em resolução cheia; sem a luz por ray tracing (sombras pretas) |
| Taxa de quadros | 🟡 | 15 % | 1 a 5 fps na cutscene em 1080p com texturas em resolução cheia |
| Estabilidade | ✅ | 90 % | sessões de uma hora sem travar a placa nem perder o dispositivo |
| **Geral** | | **72 %** | média das linhas acima |

Tradução dos shaders do console para este host e etapas do jogo
(gerado por `cargo xtask progress`):

<!-- progress:start -->
![shaders: 100.0%](https://img.shields.io/badge/shaders-100.0%25-brightgreen)
![shaders Mesh: 100.0%](https://img.shields.io/badge/shaders%20Mesh-100.0%25-brightgreen)
![shaders Vertex/pixel: 100.0%](https://img.shields.io/badge/shaders%20Vertex%2Fpixel-100.0%25-brightgreen)
![shaders Compute: 100.0%](https://img.shields.io/badge/shaders%20Compute-100.0%25-brightgreen)
![medido: 2026-11-25](https://img.shields.io/badge/medido-2026--11--25-blue)

| Parte | Progresso | % | Contagem |
|---|---|---|---|
| **Todos os shaders** | `████████████████████` | **100.0%** | 25758 / 25758 |
| Shaders Mesh | `████████████████████` | 100.0% | 8765 / 8765 |
| Shaders Vertex/pixel | `████████████████████` | 100.0% | 14259 / 14259 |
| Shaders Compute | `████████████████████` | 100.0% | 2734 / 2734 |
| **Etapas do jogo funcionando** | `██████████░░░░░░░░░░` | **50.0%** | 3 / 6 |

| Etapa do jogo | Estado | Medição |
|---|---|---|
| Carrega e inicia | ✅ | uns 10 s até o primeiro quadro |
| Vídeo de abertura e menus | ✅ | 30 fps |
| Cutscene de abertura | ⚠️ | vai até o fim; rostos, cabelo e roupa com textura, mas a cena fica escura sem a luz por ray tracing; 1-5 fps |
| Entrada no mundo | ✅ | alcançada, pipelines vindas da biblioteca salva |
| Jogável | ⚠️ | controle, HUD e pausa funcionam, mas a 4-7 fps em 1080p |
| Luz por ray tracing | ❌ | ainda não roda; tudo na sombra fica preto |

*Medido em 2026-11-25 em NVIDIA GeForce RTX 4060 Ti 8 GB, Intel Core i7-13700K, Windows 11 (`783a54ca`). As contagens de shaders são os shaders de console do dump do Xbox Series 1.0.2.1 que são traduzidos para este host (`victory translate`); as etapas do jogo são o que uma execução alcançou. Regenere com `cargo xtask progress` depois de editar `docs/progress/progress.json`.*
<!-- progress:end -->

## O que este fork acrescenta

Memória (placas de 8 GB):
- O modo de tela informado ao título pode ser o do monitor
  (`VICTORY_DISPLAY_MODE=1920x1080`). No modo 4K padrão, só os render targets
  ocupavam 3,4 GB de um orçamento de 6,6 GB, e as texturas eram descartadas.
- Um nível de mip a menos nas texturas grandes em placas de até 8 GB (piso 2048).
- Render targets ociosos cuja memória o título desmapeou são liberados; os que
  ficam no mesmo endereço com a mesma descrição dividem uma cópia só.
- Texturas lidas por índice (bindless) saem da placa as mais antigas primeiro
  quando ela enche, e voltam quando o título as cita de novo.
- Uma linha do log mostra a memória da placa por tipo.

Velocidade:
- O tradutor de shaders ficou cerca de 2,7 vezes mais rápido a frio, com a mesma saída.
- A conferência de texturas segura só as faixas que lê enquanto o runtime
  remapeia a memória, sem esperar nada (`VICTORY_TEXTURE_NO_WAIT`); um orçamento
  por quadro (`VICTORY_TEXTURE_BUDGET_MS`, 10 por padrão) passa o resto adiante.
- O desfazimento do tiling (swizzle) das texturas do console roda em faixas paralelas.
- A cópia de volta das cópias na placa usa uma fila de cópia própria; a placa
  compara os pedaços com grupos largos.
- Quadros da cutscene que esperavam 1,6 a 7,9 s por envio de texturas agora levam 5 a 67 ms.
- Workers extras de pipeline durante o carregamento.

Correção:
- Programas escritos para ondas de 64 lanes rodam como várias ondas de 32 com
  barreiras de grupo (22 dos 24 programas de cabelo agora são criados).
- Texturas enviadas no prelúdio de uma lista não piscam mais na abertura.
- Um crash nas leituras protegidas do DirectStorage foi corrigido.
- Dispatches de compute que passam do orçamento do quadro são descartados em
  vez de travar o quadro.

Também reúne o trabalho do fxceless e dos forks que ele junta (texturas de pele
e cabelo conferidas de novo depois do envio, limite do heap de views, instruções
f64, liberação de ociosos sob pressão de memória) e mais de cinquenta pull
requests do VIctory (lanes auxiliares no `v_readfirstlane`, índices de mesh com
9 bits, leituras d16, `v_mad_f32` com a soma separada, limites da LDS e outros).

## Como compilar e rodar

Requisitos: Windows 10/11 x64, CPU com AVX2, GPU Direct3D 12 com shader model
6.6 e mesh shaders, Rust (versão fixada em `rust-toolchain.toml`). O título
precisa de vários GiB de memória virtual livre ao iniciar; se o `crashes.log`
mostrar o erro 1455 do Windows, aumente o arquivo de paginação.

```powershell
cargo test -p victory-gpu -p victory-shader          # não precisa de dados do jogo
cargo build --profile diag -p victory --features diagnostics
```

Chaves recomendadas para uma placa de 8 GB:

```powershell
$env:VICTORY_RELAY = '1'                  # 2 mostra defeitos em placas de 8 GB
$env:VICTORY_WIDE_BACK = '1'
$env:VICTORY_WIDE_REFRESH = '1'
$env:VICTORY_DISPLAY_MODE = '1920x1080'   # o tamanho do seu monitor
$env:VICTORY_NO_WATCHDOG = '1'            # os primeiros quadros do mundo são lentos
target\diag\victory.exe run <pasta do jogo>\<executável do título> --execute
```

Antes da primeira execução de um build novo, traduza os shaders que o título usa:

```powershell
target\diag\victory.exe translate <arquivos de shader do título> --write-cache --used-only
```

A primeira execução depois que o tradutor muda refaz a biblioteca de pipelines:
a cutscene de abertura fica quase toda preta por alguns minutos enquanto umas
3.000 pipelines são montadas. As execuções seguintes já as carregam.

Controle: pads XInput. Sem nenhum conectado, o teclado vira um pad enquanto a
janela do título tem o foco (W A S D e I J K L para os analógicos, setas para o
direcional, Enter/Backspace/X/Y para A/B/X/Y, Esc e Tab para Menu e View).

Saves, configurações e o `crashes.log` ficam em `%LOCALAPPDATA%\victory\data`.

## Depuração

| Variável | Efeito |
|----------|--------|
| `VICTORY_LOG=info` | o log com a linha de fps e o tempo de cada parte do quadro |
| `VICTORY_CAPTURE=<pasta>` | salva quadros apresentados como PNG |
| `VICTORY_SKIP_PIPELINES=<hash>,...` | omite desenhos e dispatches cujos shaders tenham esses hashes |
| `VICTORY_D3D12_DRED=1` | numa falha da GPU, a alocação atingida e os comandos em voo |
| `VICTORY_D3D12_WARP=1` | usa o adaptador de software (lento, mas não reinicia o driver) |

Cada chave é explicada num comentário onde é lida no código.

## Problemas conhecidos

- Tudo que está na sombra fica preto: a luz por ray tracing do título ainda não roda.
- Pele e roupa dos personagens saem pretas e a passagem para os vídeos pisca verde.
- Faixas de cor na gradação da cutscene.
- Dois programas de 64 lanes (coleta de triângulos e meshlets) ainda não cabem em
  ondas de 32 e são recusados.
- Uns 24 GB de memória do sistema em uso durante o jogo.

## Créditos

- **[VIctory](https://github.com/Marvisak/VIctory)**, de Marvisak e colaboradores:
  o carregador, os serviços do runtime, o back-end Direct3D 12 e o recompilador
  de shaders. Licença GPL-3.0-or-later, a mesma deste fork.
- **[fxceless/victory-fork](https://github.com/fxceless/victory-fork)** e os forks
  que ele reúne (asdaasdsaddf, triippywhale e outros): a renderização do mundo, o
  relay e o trabalho de memória de onde este fork parte.
- **[XCL](https://github.com/ivall/xcl)** (GPL-2.0-only) serviu só de referência
  de como o título se comporta em outra camada; nenhum código foi copiado.
- As mudanças deste fork foram feitas com o [Claude Code](https://claude.com/claude-code).

## Licença

GPL-3.0-or-later. Veja [LICENSE](LICENSE).
