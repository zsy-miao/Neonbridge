# Local Xbox compiler probe

`tools/sonde_compilateur_xbox.py` compiles an original, minimal HLSL pixel shader with locally supplied Xbox graphics tools. It copies the required files to a temporary directory, invokes the Windows SDK `dxc.exe` with the local `dxcompiler_xs.dll`, `xbsc_xs.dll` and `newbe_xs.dll`, and deletes the temporary output afterwards. It does not extract shaders from a game. The DLLs are neither included nor downloaded.

```powershell
python tools/sonde_compilateur_xbox.py --dxc "C:\path\to\dxc.exe" --dll-dir "C:\path\to\local-dlls" --victory "C:\path\to\victory.exe"
```

`dxil.dll` must be beside `dxc.exe`. The optional `--victory` argument checks the output with `victory shader-info` and requires it to identify native pixel shader code. The tool reports missing inputs and compiler failures; success does not establish that any game shader is translated, that the shader runs on a PC GPU, or that graphics are fixed.

Measured on Windows 11 with the user's own DLLs: without `newbe_xs.dll`, full compilation reported that the backend was missing. With it, the synthetic shader compiled and VIctory identified a pixel container with native code. Only this one shader and local DLL versions were tested.
