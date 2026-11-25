// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Reference;
using FiveStar.Shaders.Tests.Warp;
using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Differential;

/// <summary>
/// The synthetic inputs of one differential run, the same for both control-flow forms: user SGPRs, a 64 KB GPU-memory
/// window, a buffer and a texture of every redirected type (<see cref="HeapSlots"/>) and the samplers. Words mix small
/// integers, offsets into the window (so pointers read from memory land in it) and floats of moderate size, which keeps
/// data-driven loops short; raw random bits would make many of them run for billions of iterations.
/// </summary>
public sealed class SyntheticData
{
    public const int WindowWords = 16384;
    public const int BufferWords = 4096;
    public const int VertexInputWords = 64 * 16 * 4;

    private const int UserSgprs = 60;
    private const int TextureSize = 8;
    private const int TextureLayers = 4;
    private const int CubeFaces = 6;
    private const int WindowOffsetSteps = 2048;
    private const int SmallIntegers = 32;

    private readonly Random _random;

    public SyntheticData(int seed)
    {
        _random = new Random(seed);
    }

    /// <summary>
    /// The bindings: memory window (slot 0, writable copy at 1), the redirected types, the vertex wrapper's slots and
    /// <paramref name="extra"/> (e.g. the mesh fallback buffers).
    /// </summary>
    public Bindings Bindings(IReadOnlyList<BufferResource> extra)
    {
        var window = Words(WindowWords);
        var buffers = new List<BufferResource> { new(0, window, Writable: false), new(1, [.. window], Writable: true) };
        var textures = new List<TextureResource>();

        foreach (var entry in HeapSlots.Entries)
        {
            for (var slot = entry.Base; slot < entry.Base + HeapSlots.PerType; slot++)
            {
                if (entry.Kind is null)
                {
                    buffers.Add(new BufferResource(slot, Words(BufferWords), entry.Writable));
                }
                else
                {
                    textures.Add(Texture(entry, slot));
                }
            }
        }

        buffers.Add(new BufferResource(HeapSlots.VertexInput, Words(VertexInputWords), Writable: false));
        buffers.Add(new BufferResource(LoopBudget.FlagSlot, new uint[4], Writable: true));
        buffers.AddRange(extra);
        var samplers = Enumerable.Range(0, HeapSlots.SamplerCount)
            .SelectMany(index => new[]
            {
                new SamplerResource(index, Linear: index % 2 == 1),
                new SamplerResource(HeapSlots.SamplerCount + index, Linear: index % 2 == 1) { Comparison = true },
            });

        return new Bindings { UserSgprs = Words(UserSgprs), Buffers = buffers, Textures = textures, Samplers = [.. samplers] };
    }

    /// <summary>A float in [-2, 2] in steps of 1/256, or one of 0, 1, -1, 0.5.</summary>
    public float Real()
    {
        float[] special = [0.0f, 1.0f, -1.0f, 0.5f];

        if (_random.Next(4) == 0)
        {
            return special[_random.Next(special.Length)];
        }

        return (_random.Next(1025) - 512) / 256.0f;
    }

    public uint SmallInteger()
    {
        return (uint)_random.Next(SmallIntegers);
    }

    public uint[] Words(int count)
    {
        var words = new uint[count];

        for (var index = 0; index < count; index++)
        {
            words[index] = Word();
        }

        return words;
    }

    private uint Word()
    {
        var pick = _random.Next(10);

        if (pick < 3)
        {
            return SmallInteger();
        }

        if (pick < 5)
        {
            return (uint)_random.Next(WindowOffsetSteps) * 16;
        }

        return FloatBits.Of(Real());
    }

    private TextureResource Texture(HeapEntry entry, int slot)
    {
        var kind = entry.Kind!.Value;
        var (height, layers) = (TextureSize, 1);

        if (kind is TextureKind.Texture1D or TextureKind.Texture1DArray)
        {
            height = 1;
        }

        if (kind is TextureKind.Texture3D or TextureKind.Texture1DArray or TextureKind.Texture2DArray)
        {
            layers = TextureLayers;
        }

        if (kind == TextureKind.TextureCube)
        {
            layers = CubeFaces;
        }

        var texelWords = 4;

        if (entry.Format != Format.R32G32B32A32_Float)
        {
            texelWords = 1;
        }

        var words = new uint[TextureSize * height * layers * texelWords];

        for (var index = 0; index < words.Length; index++)
        {
            words[index] = TexelWord(entry.Format);
        }

        return new TextureResource(slot, entry.Format, TextureSize, height, words) { Kind = kind, Layers = layers, Writable = entry.Writable };
    }

    private uint TexelWord(Format format)
    {
        if (format == Format.R32G32B32A32_Float)
        {
            return FloatBits.Of(Real());
        }

        return SmallInteger();
    }
}
