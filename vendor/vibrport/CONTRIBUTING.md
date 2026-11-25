# Como contribuir

Obrigado pelo interesse. Este fork do VIctory foca em placas de 8 GB (NVIDIA RTX 4060 Ti como
máquina de teste) e segue as regras abaixo.

## Antes de tudo

- **Nenhum arquivo do jogo** entra no repositório: nem executáveis, nem RPFs, nem shaders extraídos,
  nem capturas de tela do jogo. O `cargo xtask policy` barra binários e arquivos grandes.
- **Nada de código do xcl** (GPL-2.0-only, incompatível). O projeto é GPL-3.0, como o VIctory.
- Sem dependências baixadas na compilação (SDKs, URLs de git em `Cargo.toml`).

## Compilar e testar

```powershell
cargo build --profile diag -p victory --features diagnostics
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

O CI roda esses mesmos passos num Windows sem placa de vídeo (adaptador WARP); um teste que precisa de
placa de verdade deve pular o adaptador de software.

## Commits e pull requests

- Código, comentários e mensagens de commit em inglês; documentação do fork em português.
- Um commit por mudança, com uma primeira linha que diga o que mudou para quem roda
  (ex.: `GPU: placed textures keep their mips when the card is full`).
- Mudança no tradutor de shaders vem com um teste no estilo dos que já existem em `crates/victory-shader`.
- Se a mudança mexe no que aparece no jogo, diga no PR em que cena testou, a placa e o fps antes e depois.

## Reportar problemas

Use os modelos de issue. Inclua a placa, o driver, as variáveis `VICTORY_*` usadas e o trecho do log
(`victory.log` ou a saída do terminal) em volta do erro.
