// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Command-list binding against the translated programs' model (docs/d3d12x-shim.md, Binding): every list runs the
/// bindless root signature, the game's root arguments become user data (<see cref="UserData"/>) uploaded per draw, and
/// its descriptor tables become pointers into the <see cref="DescriptorWindow"/>.
/// </summary>
internal sealed unsafe class XboxBindings
{
    /// <summary>Compute and graphics: the root slots come in pairs.</summary>
    private const int BindPoints = 2;

    private readonly LoaderLog _log;

    public XboxBindings(LoaderLog log, OnceLog warnings)
    {
        _log = log;
        Warnings = warnings;
        Window = new DescriptorWindow(log);
    }

    /// <summary>The layouts of the game's root signatures.</summary>
    public RootLayouts Layouts { get; } = new();

    public DescriptorWindow Window { get; }

    public OnceLog Warnings { get; }

    /// <summary>The bindless root signature every list and pipeline uses; 0 before the device or when it failed.</summary>
    public nint Root { get; private set; }

    /// <summary>Upload memory for user data; null before the device or when it failed.</summary>
    public UserDataRing? Ring { get; private set; }

    /// <summary>Creates the root signature and the user-data ring on the new desktop device.</summary>
    public void Attach(nint device)
    {
        nint root = 0;

        if (HResult.Failed(BindlessRoot.Create(device, &root)))
        {
            _log.Warn("[d3d12_xs] no bindless root signature: lists keep the game's root signatures");

            return;
        }

        Root = root;
        Ring = UserDataRing.Create(device);

        if (UserDataConstants.Enabled)
        {
            _log.Info($"[d3d12_xs] user data as {BindlessRootSignature.UserConstants} root constants (--user-data-constants)");
        }

        if (Ring is null)
        {
            _log.Warn("[d3d12_xs] no user-data ring: draws run without user data");
        }
    }

    /// <summary>Binds the device and command-list slots of <see cref="CommandListBindingMethods"/>.</summary>
    public static void Register(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, void*, nuint, Guid*, nint*, int> rootSignature = &CommandListBindingMethods.CreateRootSignature;
        objects.SetHandler(XboxInterfaceKind.Device, CommandListBindingMethods.CreateRootSignatureSlot, (nint)rootSignature);
        RegisterState(objects);
        RegisterArguments(objects);
        RegisterWork(objects);
    }

    private static void RegisterState(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, nint, nint, int> reset = &CommandListBindingMethods.Reset;
        delegate* unmanaged<XboxObject*, nint, nint, uint, int> resetX = &CommandListBindingMethods.ResetX;
        delegate* unmanaged<XboxObject*, uint, nint*, void> heaps = &CommandListBindingMethods.SetDescriptorHeaps;
        delegate* unmanaged<XboxObject*, nint, void> computeRoot = &CommandListBindingMethods.SetComputeRootSignature;
        delegate* unmanaged<XboxObject*, nint, void> graphicsRoot = &CommandListBindingMethods.SetGraphicsRootSignature;

        List(objects, CommandListBindingMethods.ResetSlot, (nint)reset);
        List(objects, CommandListBindingMethods.ResetXSlot, (nint)resetX);
        List(objects, CommandListBindingMethods.SetDescriptorHeapsSlot, (nint)heaps);
        List(objects, CommandListBindingMethods.SetComputeRootSignatureSlot, (nint)computeRoot);
        List(objects, CommandListBindingMethods.SetGraphicsRootSignatureSlot, (nint)graphicsRoot);
    }

    private static void RegisterArguments(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, ulong, void> computeTable = &CommandListBindingMethods.SetComputeRootDescriptorTable;
        delegate* unmanaged<XboxObject*, uint, ulong, void> graphicsTable = &CommandListBindingMethods.SetGraphicsRootDescriptorTable;
        delegate* unmanaged<XboxObject*, uint, uint, uint, void> computeConstant = &CommandListBindingMethods.SetComputeRoot32BitConstant;
        delegate* unmanaged<XboxObject*, uint, uint, uint, void> graphicsConstant = &CommandListBindingMethods.SetGraphicsRoot32BitConstant;

        delegate* unmanaged<XboxObject*, uint, uint, uint*, uint, void> computeConstants = &CommandListBindingMethods.SetComputeRoot32BitConstants;
        delegate* unmanaged<XboxObject*, uint, uint, uint*, uint, void> graphicsConstants = &CommandListBindingMethods.SetGraphicsRoot32BitConstants;
        delegate* unmanaged<XboxObject*, uint, ulong, void> computeAddress = &CommandListBindingMethods.SetComputeRootAddress;
        delegate* unmanaged<XboxObject*, uint, ulong, void> graphicsAddress = &CommandListBindingMethods.SetGraphicsRootAddress;

        List(objects, CommandListBindingMethods.SetComputeRootDescriptorTableSlot, (nint)computeTable);
        List(objects, CommandListBindingMethods.SetComputeRootDescriptorTableSlot + 1, (nint)graphicsTable);
        List(objects, CommandListBindingMethods.SetComputeRoot32BitConstantSlot, (nint)computeConstant);
        List(objects, CommandListBindingMethods.SetComputeRoot32BitConstantSlot + 1, (nint)graphicsConstant);

        List(objects, CommandListBindingMethods.SetComputeRoot32BitConstantsSlot, (nint)computeConstants);
        List(objects, CommandListBindingMethods.SetComputeRoot32BitConstantsSlot + 1, (nint)graphicsConstants);

        // Root CBV, SRV and UAV (slots 41 to 46): compute and graphics alternate.
        for (int slot = CommandListBindingMethods.FirstRootAddressSlot; slot <= CommandListBindingMethods.LastRootAddressSlot; slot += BindPoints)
        {
            List(objects, slot, (nint)computeAddress);
            List(objects, slot + 1, (nint)graphicsAddress);
        }
    }

    private static void RegisterWork(XboxObjects objects)
    {
        delegate* unmanaged<XboxObject*, uint, uint, uint, uint, void> draw = &CommandListBindingMethods.DrawInstanced;
        delegate* unmanaged<XboxObject*, uint, uint, uint, int, uint, void> drawIndexed = &CommandListBindingMethods.DrawIndexedInstanced;
        delegate* unmanaged<XboxObject*, uint, uint, uint, void> dispatch = &CommandListBindingMethods.Dispatch;
        delegate* unmanaged<XboxObject*, nint, uint, nint, ulong, nint, ulong, void> indirect = &CommandListBindingMethods.ExecuteIndirect;

        List(objects, CommandListBindingMethods.DrawInstancedSlot, (nint)draw);
        List(objects, CommandListBindingMethods.DrawIndexedInstancedSlot, (nint)drawIndexed);
        List(objects, CommandListBindingMethods.DispatchSlot, (nint)dispatch);
        List(objects, CommandListBindingMethods.ExecuteIndirectSlot, (nint)indirect);
    }

    private static void List(XboxObjects objects, int slot, nint handler)
    {
        objects.SetHandler(XboxInterfaceKind.CommandList, slot, handler);
    }
}
