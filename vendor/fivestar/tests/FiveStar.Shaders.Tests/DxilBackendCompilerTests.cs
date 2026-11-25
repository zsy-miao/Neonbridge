// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Compilation;
using FiveStar.Shaders.Tests.Toolchain;
using Xunit;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// The backend as the loader's on-demand compiler: a program it lowers is validated by dxil.dll and written, and one it
/// cannot lower goes to the fallback; both are counted.
/// </summary>
public sealed class DxilBackendCompilerTests
{
    // A gather with computed offsets (texturecompression~CS_CompressBC1Mip0): no advanced-texture-ops flag.
    private const string Lowered = """
        #define VW_WAVE 64
        #define VW_EMULATE 1
        #include "rdna_prelude.hlsli"
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          uint s8 = 8u, s0 = 0u, v21 = id.x;
          Texture2D<float4> _tex10 = ResourceDescriptorHeap[s8];
          SamplerState _smp10 = SamplerDescriptorHeap[s0];
          RWByteAddressBuffer output = ResourceDescriptorHeap[1];
          float4 _img10 = _tex10.GatherAlpha(_smp10, float2(0.5, 0.5), int2((asint(v21 << 26) >> 26), (asint(v21 << 18) >> 26)));
          output.Store(id.x * 4u, asuint(_img10.x));
        }
        """;

    // Function-like macros are outside the dialect the backend lowers.
    private const string NotLowered = """
        #define TWICE(x) ((x) * 2u)
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          uint v0 = TWICE(id.x);
        }
        """;

    // A game-memory load with the loader's window feedback (--evict-windows): gpu_page stores a waiting window's flag.
    private const string Feedback = """
        #define FIVESTAR_WINDOW_FEEDBACK 33554432u
        #define VW_WAVE 64
        #define VW_EMULATE 1
        #include "rdna_prelude.hlsli"
        [numthreads(64, 1, 1)]
        void main(uint3 id : SV_DispatchThreadID) {
          RWByteAddressBuffer output = ResourceDescriptorHeap[1];
          output.Store(id.x * 4u, PtrLoad(id.x * 4u, 0x100u, 0u));
        }
        """;

    [WarpFact]
    public void A_program_the_backend_cannot_lower_goes_to_the_fallback_and_the_rest_is_validated()
    {
        var work = Directory.CreateTempSubdirectory("fivestar-backend-");

        try
        {
            var dxc = ShaderToolchain.Find().Dxc;
            var fallback = new RecordingCompiler(DxcOptions.Defaults(dxc, retry: false));
            var compiler = new DxilBackendCompiler(fallback.Options, fallback, new InProcessBackend(DxilBackendCompiler.ValidatorBeside(dxc)));
            var lowered = new CompileRequest(Path.Combine(work.FullName, "lowered.hlsl"), Lowered, "cs", DisableOptimizations: false);
            var notLowered = new CompileRequest(Path.Combine(work.FullName, "other.hlsl"), NotLowered, "cs", DisableOptimizations: false);

            Assert.True(compiler.Compile(lowered, TimeSpan.Zero).Succeeded);
            Assert.True(File.Exists(lowered.DxilPath));
            Assert.Empty(fallback.Requests);
            Assert.Equal((1, 0, 0), compiler.Counts);

            Assert.True(compiler.Compile(notLowered, TimeSpan.Zero).Succeeded);
            Assert.Equal([notLowered], fallback.Requests);
            Assert.Equal((1, 1, 0), compiler.Counts);

            var feedback = new CompileRequest(Path.Combine(work.FullName, "feedback.hlsl"), Feedback, "cs", DisableOptimizations: false);
            Assert.True(compiler.Compile(feedback, TimeSpan.Zero).Succeeded);
            Assert.Equal((2, 1, 0), compiler.Counts);
        }
        finally
        {
            work.Delete(recursive: true);
        }
    }

    private sealed class RecordingCompiler(DxcOptions options) : IShaderCompiler
    {
        public List<CompileRequest> Requests { get; } = [];

        public DxcOptions Options => options;

        public string Identity => nameof(RecordingCompiler);

        public CompileOutcome Compile(CompileRequest request, TimeSpan timeout)
        {
            Requests.Add(request);

            return CompileOutcome.Success;
        }
    }
}
