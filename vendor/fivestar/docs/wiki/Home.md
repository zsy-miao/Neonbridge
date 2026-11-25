![Five Star Recomp](https://raw.githubusercontent.com/ThatRecompDev/five-star-recomp/main/docs/assets/banner.svg)

Five Star Recomp works toward running the Xbox Series version of **Grand Theft Auto VI** (1.0.2.1, title ID
`7952987A`) natively on Windows, from a copy you own.

The console's CPU is x86-64, so the game code itself runs on a PC unchanged. The project rebuilds what surrounds it:
GPU shaders, the console runtime, the graphics API and the data formats.

```mermaid
flowchart LR
  subgraph dump["your Xbox Series dump"]
    code["game code, x86-64"]
    shaders["GPU programs, RDNA machine code"]
    runtime["console runtime: GDK, xmem"]
    gfx["graphics API: D3D12.X"]
    data["archives, textures, models, audio"]
  end
  code -->|"runs unchanged, mapped by the loader"| pc["Windows PC"]
  shaders -->|"translated to HLSL, compiled by dxc"| pc
  runtime -->|"loader shims"| pc
  gfx -->|"D3D12, device shim in progress"| pc
  data -->|"format readers"| pc
```

## Pages

| Page | Contents |
|---|---|
| [Status](Status.md) | what works today, with numbers |
| [Roadmap](Roadmap.md) | what is left, grouped by what blocks it |
| [Platforms and performance](Platforms-and-Performance.md) | Windows, Linux, Steam Deck, macOS, and how speed is kept |
| [Archives](Archives.md) | RPF9 archives and the PSO metadata inside them |
| [Textures and BCPack](Textures-and-BCPack.md) | ttpf texture pages and the BCPack codec |
| [Shader translation](Shader-Translation.md) | how native GPU programs become HLSL |
| [Console runtime](Console-Runtime.md) | the loader and the GDK runtime replacement |
| [Related projects](Related-Projects.md) | other work on the same game |

Format specifications that the code implements live in the repository's `docs/` folder. This wiki holds research,
findings and status.
