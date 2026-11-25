// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Cache;

/// <summary>A tag 0x41 entry of a shader's metadata: where one root-signature binding reaches the program.</summary>
/// <param name="Kind">How it is passed (u32 at +4, bits 8..15).</param>
/// <param name="Class">Its register class (u32 at +4, bits 0..7).</param>
/// <param name="TableOffset">Table entries: the descriptor's offset in the table, in dwords (u32 at +8); 0 otherwise.</param>
/// <param name="Register">The shader register (u32 at +12).</param>
/// <param name="Stride">The element stride (u32 at +16): the byte stride of a structured buffer, 4 for typed buffers and textures, 1 for raw buffers, 0 for constant buffers.</param>
/// <param name="Size">The descriptor size in dwords (byte 0 of +20): 8 image, 4 buffer or sampler, 2 root descriptor, 4 root constants.</param>
/// <param name="Sgpr">The user SGPR holding the table address, the root descriptor's address (two SGPRs) or the root constants (byte 1 of +20); 0 for static samplers.</param>
/// <param name="Parameter">The root parameter index, or the static sampler's index for static samplers (byte 2 of +20).</param>
public readonly record struct UserDataEntry(
    UserDataKind Kind, BindingClass Class, int TableOffset, int Register, int Stride, int Size, int Sgpr, int Parameter)
{
    /// <summary>Gets the register class as <c>srv</c>, <c>sampler</c>, <c>cbv</c> or <c>uav</c>.</summary>
    public string RegisterClass
    {
        get
        {
            switch (Class)
            {
                case BindingClass.ShaderResource:
                    return "srv";
                case BindingClass.Sampler:
                    return "sampler";
                case BindingClass.ConstantBuffer:
                    return "cbv";
                default:
                    return "uav";
            }
        }
    }
}
