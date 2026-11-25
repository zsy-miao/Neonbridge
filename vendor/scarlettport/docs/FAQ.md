# FAQ

Short answers for players. Problems and fixes are in [TROUBLESHOOTING.md](TROUBLESHOOTING.md); the measured state is in [STATUS.md](STATUS.md) and the plan in [ROADMAP.md](ROADMAP.md).

## What should I expect today?

The title boots, gets through the menus, saves and legal screens, loads the open world and starts gameplay. On our test PC (RTX 3060 12 GB, 32 GB RAM, Windows 10, Series S profile) the prologue runs at **about 4 fps**. That is the only measured number; nothing here promises more on your hardware. It is **not yet verified playable**: nobody has played a session end to end.

## Why only about 4 fps?

ScarlettPort is not an emulator. The title's x64 code runs natively, but every GPU program it ships is written for the console's RDNA2 GPU and is translated for your card, and the console's unified memory has to be kept in step with the PC's separate video memory. On our RTX 3060 the frame is now GPU-bound: the per-frame waits on title memory were cut from about 156 ms to about 26 ms, and what is left is the GPU cost of the translated frame plus freezes while world data streams in. Work that is in testing (streaming stutter fixes, CPU recording cuts) has no measured fps gain yet. The intro cutscene is very slow unless you skip it.

## What do I need?

- Windows 10 or 11, x64.
- A D3D12 GPU with Shader Model 6.6 and mesh shaders: NVIDIA RTX 20-series or newer, AMD RX 6000 or newer.
- 32 GB RAM recommended.
- **Your own dump of your own legally purchased copy** of the title.

## Is this legal? Where do I get the game files?

You must dump your own copy. This repository and its releases contain **no game files, code, assets or keys**, never download game files, and do not link to or help obtain them. Requests for game files are closed. The only thing the scripts download is Microsoft's public D3D12 Agility SDK.

## Does it work on AMD? On Windows 10?

NVIDIA (RTX 3060) is the card we test on. AMD RX 6000 and newer meet the requirements and the runtime includes community AMD work, but we have no measured AMD result yet, so a [hardware report](https://github.com/ZigZagBoi1/ScarlettPort/issues/new/choose) helps a lot. Windows 10 is supported: it needs Microsoft's D3D12 Agility SDK runtime (1.614) in a `D3D12` folder beside the runtime executable. The release ZIP already includes it; if you build yourself, `scripts\setup-victory.ps1` fetches it.

## Can I use a controller?

- **Xbox controllers** work best: the runtime reads pads through XInput.
- **PlayStation controllers** (DualShock 4, DualSense) are not read directly. Map them to an Xbox pad with Steam Input or a similar XInput mapper.
- **Keyboard and mouse:** click the window to capture. With no pad connected the keyboard acts as one: Enter = A, Backspace = B, WASD = left stick, IJKL = right stick.

## Why does my antivirus flag it?

See [Antivirus warnings](TROUBLESHOOTING.md#antivirus-warnings). Short version: the release is not code-signed and the runtime does things a game normally does not, so heuristics react. Check the SHA-256 checksum against the release notes.

## Will the first start be slow?

Yes. Shaders have to be translated and pipelines built once. Press **Build shaders** in the launcher before **Play** (with the default World engine, about 3 minutes plus about 8 minutes for pipelines, measured on a 12-thread CPU). Even then the first run can stall while the rest compile. A warm-up that finishes before the first frame (prewarm) is planned and not released yet.

## The game says my save is broken. Did I lose it?

Probably not, and don't delete anything. See [the save prompt](TROUBLESHOOTING.md#the-save-is-broken-prompt).

## Does the launcher go online?

Only to check GitHub Releases for a launcher update (if Updates is on), and it downloads one only when you click it. Details are in the [README FAQ](../README.md#-faq).
