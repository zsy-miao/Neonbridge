// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.Gdk;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// The GameInput module: <c>HRESULT GameInputCreate(IGameInput** gameInput)</c>, by name and by ordinal 1 (how the
/// console executable imports it). Declared Xbox-only, so the PC GameInput.dll is never loaded. The same entry is registered under xgameruntime.dll, so a thunk ABI table can map
/// a QueryApiImpl slot to it.
/// </summary>
[ShimModule(GameInputLayout.Module, UnresolvedReturn = HResult.NotImplemented, AliasPattern = GameInputLayout.AliasPattern, AliasSymbolPrefix = GameInputLayout.AliasSymbolPrefix)]
internal static unsafe partial class GameInputShim
{
    [Shim(GameInputLayout.Module, GameInputLayout.CreateFunction)]
    [Shim(GameInputLayout.Module, GameInputLayout.CreateOrdinal)]
    [Shim(GdkModule.Name, GameInputLayout.CreateFunction)]
    [UnmanagedCallersOnly]
    private static int GameInputCreate(nint* gameInput)
    {
        try
        {
            if (gameInput == null)
            {
                return HResult.Pointer;
            }

            var runtime = ShimContext.Runtime;

            try
            {
                *gameInput = runtime.Input.Create();

                return HResult.Ok;
            }
            catch (Exception e)
            {
                *gameInput = 0;

                return runtime.Gdk.Boundary.Failed(nameof(GameInputCreate), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "GameInputShim.GameInputCreate");

            return HResult.Fail;
        }
    }
}
