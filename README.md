# 6ixlaunch

Runs a dumped Xbox Series (Scarlett / GDK) executable on Windows. The game's x64 code runs natively; its console DLLs are replaced by TuxBox Rust providers built from `vendor/tuxbox` (source only, no binaries).

```
6ixlaunch --game <dump dir> [--sandbox] [--debug | --cdb <commands file>] [--log <file>]
6ixlaunch --game <dump dir> --restore
```

Needs Visual Studio Build Tools (x64 C++), cargo, and the x86_64-pc-windows-msvc target. `--sandbox` needs Sandboxie-Plus. `--debug` needs cdb from the Windows SDK. Run `6ixlaunch --help` for every option.
