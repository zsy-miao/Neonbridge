// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Pipelines;
using FiveStar.Loader.Native;
using Vortice.Direct3D12;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>Creates the root signature every translated program runs with (<see cref="BindlessRootSignature"/>) on a raw device.</summary>
internal static unsafe class BindlessRoot
{
    /// <summary>ID3D12Device::CreateRootSignature with the serialized bindless layout.</summary>
    public static int Create(nint device, nint* rootSignature)
    {
        var blob = Serialized();

        if (blob is null)
        {
            return HResult.Fail;
        }

        fixed (byte* bytes = blob)
        {
            return DesktopDevice.CreateRootSignature(device, bytes, (nuint)blob.Length, rootSignature);
        }
    }

    /// <summary>The serialized bindless layout (its user data as root constants with <see cref="Binding.UserDataConstants"/>), or null when the runtime refuses it.</summary>
    public static byte[]? Serialized()
    {
        var description = new VersionedRootSignatureDescription(BindlessRootSignature.Description(Binding.UserDataConstants.Enabled));
        var error = D3D12.D3D12SerializeVersionedRootSignature(description, out var blob);

        if (!string.IsNullOrEmpty(error) || blob is null)
        {
            blob?.Dispose();

            return null;
        }

        using (blob)
        {
            return new ReadOnlySpan<byte>((void*)blob.BufferPointer, (int)(nuint)blob.BufferSize).ToArray();
        }
    }
}
