// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The binding state of one command list, kept in the list's wrapper past the fields the game's inline recording uses
/// (up to +0x564): per bind point whether a root signature is set, its layout (null for one the shim did not see
/// created) and the user SGPRs the root arguments fill (<see cref="UserData"/>). The wrapper starts zeroed and Reset
/// zeroes it again.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct CommandListBindings
{
    /// <summary>Offset in the command-list wrapper (<see cref="XboxInterfaces.CommandListObjectSize"/> bytes).</summary>
    public const int Offset = 0x800;

    /// <summary>User data dwords of a translated program (<c>g_user4[32]</c> in rdna_prelude.hlsli).</summary>
    public const int UserDataDwords = 128;

    public RootLayout* GraphicsLayout;
    public RootLayout* ComputeLayout;
    public int GraphicsRoot;
    public int ComputeRoot;
    public int GraphicsDirty;
    public int ComputeDirty;
    public int HeapsBound;

    /// <summary>1 while the pipeline last set is a stand-in the driver has not built: draws and dispatches are skipped.</summary>
    public int PipelinePending;

    /// <summary>1 once a dispatch or ExecuteIndirect of this recording was skipped: every later one is skipped too.</summary>
    public int ComputeSkipped;

    /// <summary>1 once a compute dispatch of this recording was skipped: with <c>--empty-indirect-after-skip</c> later ExecuteIndirects run nothing.</summary>
    public int ProducerSkipped;

    /// <summary>1 when the recording holds a slot-74 memory wait (<see cref="RetireWaits"/>).</summary>
    public int RetireWrite;
    public fixed uint Graphics[UserDataDwords];
    public fixed uint Compute[UserDataDwords];

    /// <summary>The state inside a command-list wrapper.</summary>
    public static CommandListBindings* Of(XboxObject* list)
    {
        return (CommandListBindings*)((byte*)list + Offset);
    }
}
