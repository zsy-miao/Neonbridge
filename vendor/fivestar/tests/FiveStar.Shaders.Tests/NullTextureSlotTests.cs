// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Translation;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// A T# whose heap index is below the first game view (FIVESTAR_FIRST_GAME_VIEW) reads the null texture of its
/// dimension, never a reserved buffer as a texture: the 64-bit image atomic case, its T# naming slot 5 under a first
/// game view of 6, lands in the RWTexture2D null slot, where the run binds its texture.
/// </summary>
public sealed class NullTextureSlotTests : IClassFixture<WarpFixture>
{
    private const int TextureSlot = 5;

    private readonly WarpFixture _fixture;

    public NullTextureSlotTests(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    [Fact]
    public void Each_dimension_has_its_null_slot_after_the_reserved_buffers()
    {
        Assert.Equal((10u, 12u, 15u, 18u, 20u, 21u), (NullTextureSlots.First, NullTextureSlots.For("Texture2D"), NullTextureSlots.For("TextureCube"), NullTextureSlots.For("RWTexture2D"), NullTextureSlots.For("RWTexture3D"), NullTextureSlots.End));
        Assert.Equal("tex_slot(s4, 12u)", NullTextureSlots.Slot("Texture2D", "s4"));
        Assert.Throws<ArgumentException>(() => NullTextureSlots.For("RWTextureCube"));
    }

    [WarpFact]
    public void A_t_sharp_below_the_first_game_view_reads_the_null_texture_of_its_dimension()
    {
        var atomic = ComputeCases.All().Single(testCase => testCase.Name == "cs_image_atomic_64");
        int nullSlot = (int)NullTextureSlots.For("RWTexture2D");
        var redirected = atomic with
        {
            Textures = [atomic.Textures[0] with { HeapIndex = nullSlot }],
            Check = written => atomic.Check!(new Dictionary<int, uint[]> { [TextureSlot] = written[nullSlot] }),
        };

        var mismatches = new ComputeCaseRunner(_fixture).RunDefined(redirected, $"#define FIVESTAR_FIRST_GAME_VIEW {TextureSlot + 1}u\n");

        Assert.True(mismatches.Count == 0, string.Join("\n", mismatches.Take(8)));
    }
}
