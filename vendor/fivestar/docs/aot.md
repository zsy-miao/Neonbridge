# NativeAOT

`fivestar-loader` builds, trims and links as a NativeAOT executable, but only the JIT build runs the game: the AOT
runtime keeps its thread state in TLS slot 0, and slot 0 is the one the mapped game executable needs (below).

## Publishing

```
dotnet publish src/FiveStar.Loader -c Release -r win-x64 -p:PublishAot=true -o <folder>
```

| Need | Why |
|---|---|
| MSVC build tools (`link.exe`, found through `vswhere.exe`) | the NativeAOT linker step; the Visual Studio Installer folder must be on `PATH` if `vswhere` is not |
| a current .NET 10 SDK | NuGet audit fails the publish (NU1903, warnings are errors) when the SDK's `Microsoft.DotNet.ILCompiler` has a known vulnerability; with an older SDK add `-p:WarningsNotAsErrors=NU1903` |

The project sets `SelfContained` only for the JIT build, so `-p:PublishAot=true` works as is. The output is a single
`fivestar-loader.exe` (about 11 MB) plus symbols.

## CET

The loader builds with `<CETCompat>false</CETCompat>`: with CET shadow stacks, SEH that resumes inside manually mapped
code fail-fasts with 0x30. Both builds honour it:

| Build | How | Check |
|---|---|---|
| JIT | the SDK patches the apphost | `dumpbin /headers`: the `dllchar` debug entry is `00000000 extended DLL characteristics` |
| NativeAOT | the ILCompiler targets pass `/CETCOMPAT:NO` to the linker (`link.rsp` in the intermediate folder) | `dumpbin /headers`: no `dllchar` debug entry, so no `CET compatible` |

## Rules for loader code

The loader, `FiveStar.Formats`, `FiveStar.Graphics` and `FiveStar.Shaders` set `IsAotCompatible`, so every build runs
the trim and AOT analyzers and a warning fails the build.

- No reflection over members, `Assembly.GetTypes`, `Reflection.Emit` or `JsonSerializer` without a source-generated
  context. `RuntimeMethodHandle.GetFunctionPointer` fails under NativeAOT for `[UnmanagedCallersOnly]` methods.
- Native entry points are `[UnmanagedCallersOnly]` methods taken with `&Method`.
- Shims are registered at compile time by `FiveStar.Loader.Generators`: for every type with `[Shim]` methods,
  `[ShimModule]` attributes or `[NotImplementedTable]` fields it writes a `RegisterShims(ShimTable)` that passes each
  entry point as `&Method`, and `ShimTypes.Register` calls them all. A shim type is therefore `partial`
  (`internal static unsafe partial class`); without it the build fails with CS0260. A `[Shim]` method that is not
  `[UnmanagedCallersOnly]` fails to compile in the generated file.

## Why the AOT build cannot run the game

An executable's `thread_local` code reads TLS slot 0 directly (`mov rax,gs:[58h]; mov rax,[rax]`). The loader gives
slot 0 of every thread to the mapped game executable while the thread lives ([loader.md](loader.md), `TlsSlotZero`).
Under the JIT the host's slot 0 holds nothing the runtime needs. A NativeAOT executable holds its runtime's thread
statics there (the current managed thread, its id, lock ownership), so after the swap `lock` throws
`SynchronizationLockException`. The AOT loader refuses such an image with `NotSupportedException` instead.

Running the game under NativeAOT needs the runtime out of slot 0: the loader as a NativeAOT shared library
(`NativeLib=Shared`, its TLS slot assigned dynamically) started by a small native executable that owns slot 0.

## Call cost

`dotnet run -c Release --project benchmarks/FiveStar.Benchmarks -- --filter * --runtimes nativeaot10.0` against the
default (JIT) run, BenchmarkDotNet default job, Ryzen 9 7950X3D, .NET 10.0.7. Every benchmark calls through
`delegate* unmanaged` from managed code, so the managed-to-native transition of the caller is part of each number.

| Path | JIT | NativeAOT |
|---|---:|---:|
| native to native, the baseline | 4.2 ns | 7.9 ns |
| native to shim (`XSystemGetDeviceType`) | 7.9 ns | 9.3 ns |
| D3D12.X forwarding thunk / managed handler | 3.3 / 12.2 ns | 8.2 / 16.2 ns |
| GDK thunk call (`QueryApiImpl`, slot, `Release`) | 60 ns | 110 ns |
| XTaskQueue submit and dispatch | 92 ns | 98 ns |
| XAsync round trip | 466 ns | 526 ns |
| thread-pool queue callback, waited for | 821 ns | 4662 ns |
| `XMemAlloc` and `XMemFree` of 64 bytes | 73 ns | 125 ns |
| BCPack BC1 decode, scalar / AVX2 / AVX-512 | 176 / 70 / 42 us | 174 / 57 / 40 us |

The shim's own cost (native to shim minus the baseline) is 3.7 ns on the JIT and 1.4 ns under NativeAOT; the managed
caller's transition is slower under NativeAOT, which the game, calling from native code, does not pay.

## Start-up

Process start to the game executable being mapped, warm, three runs each: JIT 434 to 485 ms, NativeAOT 398 to 518 ms
(first loader line: JIT 130 to 194 ms, NativeAOT 27 to 145 ms). Most of the time is reading `xgameruntime.lib`.
