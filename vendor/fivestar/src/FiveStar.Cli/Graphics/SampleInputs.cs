// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Rendering;
using FiveStar.Shaders.Disassembly;

namespace FiveStar.Cli.Graphics;

/// <summary>
/// Everything the unlit sample draws with, taken from the dump: the pipeline from the console's pipeline cache, its two
/// programs translated to DXIL, and one texture.
/// </summary>
public sealed record SampleInputs(SamplePipeline Pipeline, TranslatedShader Vertex, TranslatedShader Pixel, TextureData Texture)
{
    /// <summary>Loads the cache from <c>&lt;dump&gt;/shaders.rpf</c>, finds the unlit pipeline, translates it and reads the texture.</summary>
    public static SampleInputs Load(string dump, string programFolder, string texturePath, int textureIndex, string workFolder, IDisassembler disassembler, string dxc)
    {
        var pass = UnlitPass.Load(dump, programFolder, workFolder, disassembler, dxc);
        var texture = SampleTexture.Load(texturePath, textureIndex);

        return new SampleInputs(pass.Pipeline, pass.Vertex, pass.Pixel, texture);
    }
}
