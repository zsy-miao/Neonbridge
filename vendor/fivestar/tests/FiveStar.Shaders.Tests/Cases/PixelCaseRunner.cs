// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Shaders.Containers;
using FiveStar.Shaders.Hardware;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using FiveStar.Shaders.Translation;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// Runs a pixel-shader case: appends the export of v20.. (mrt0.., uint4 SV_Target signature so the targets are
/// bit-exact), assembles, translates, compiles (ps_6_7, with dxc or with the DXIL backend), draws on 8x8 and lists every
/// pixel that differs.
/// </summary>
public sealed class PixelCaseRunner
{
    private const int FirstResult = 20;
    private const int WaveSize = 32;

    private readonly WarpFixture _fixture;

    public PixelCaseRunner(WarpFixture fixture)
    {
        _fixture = fixture;
    }

    public IReadOnlyList<string> Run(PixelCase testCase, bool direct = false)
    {
        var pixel = Build(testCase, direct);
        var helper = testCase.HalfCoverage ? "half_vs.hlsl" : "fullscreen_vs.hlsl";
        var vertex = _fixture.Helper(helper, "vs");
        var framebuffer = Framebuffer.Fullscreen(testCase.Targets, PixelCase.Size, PixelCase.Size, testCase.FloatTargets);
        var result = _fixture.Runner.Graphics(new GraphicsJob(vertex, pixel, framebuffer, testCase.Bindings));

        return Compare(testCase, result);
    }

    /// <summary>The export lines: v20.. padded with off to whole mrts, done vm on the last.</summary>
    public static IEnumerable<string> ExportLines(int outputs)
    {
        var registers = Enumerable.Range(FirstResult, outputs).Select(register => $"v{register}").ToList();

        while (registers.Count % 4 != 0)
        {
            registers.Add("off");
        }

        for (var target = 0; target < registers.Count / 4; target++)
        {
            var last = target == (registers.Count / 4) - 1 ? " done vm" : string.Empty;

            yield return $"exp mrt{target} {string.Join(", ", registers.Skip(4 * target).Take(4))}{last}";
        }
    }

    private byte[] Build(PixelCase testCase, bool direct)
    {
        var exports = string.Join("\n", ExportLines(testCase.Outputs));
        var source = testCase.Raw ? testCase.Program.Replace("{EXPORT}", exports, StringComparison.Ordinal) : $"{testCase.Program}\n{exports}\ns_endpgm";
        var assembled = _fixture.Workbench.Assemble(testCase.Name, source, WaveSize);
        var signatures = ShaderSignatures.None;

        if (!testCase.FloatTargets)
        {
            var targets = Enumerable.Range(0, testCase.Targets).Select(index => new SignatureElement("SV_Target", index, "SV_Target", "uint", index, "xyzw", 0xF, 0));
            signatures = new ShaderSignatures([], [.. targets]);
        }

        var config = new ShaderConfig { Stage = HardwareStage.Pixel, WaveSize = WaveSize };
        var program = new ShaderProgram(ShaderStage.Pixel, assembled.Instructions, assembled.Code, signatures, config, null);

        var hlsl = ShaderWorkbench.Translate(program);

        if (direct)
        {
            return _fixture.Workbench.Compile(testCase.Name, hlsl, "ps", direct: true);
        }

        return _fixture.Workbench.Compile(testCase.Name, hlsl, "ps");
    }

    private List<string> Compare(PixelCase testCase, WarpResult result)
    {
        var mismatches = new List<string>();
        var clear = testCase.FloatTargets ? Framebuffer.ClearFloatBits : Framebuffer.ClearUint;

        var pixels = testCase.HalfCoverage ? _fixture.HalfCoverageLanes : _fixture.PixelLanes;

        foreach (var pixel in pixels)
        {
            var expected = testCase.Expect(pixel);
            var actual = Enumerable.Range(0, testCase.Targets).SelectMany(target => result.Pixel(target, pixel.X, pixel.Y)).Take(testCase.Outputs).ToArray();

            if (testCase.HalfCoverage && pixel.X + pixel.Y > 6)
            {
                expected = null;
            }

            if (expected is null)
            {
                if (actual.Any(word => word != clear))
                {
                    mismatches.Add(string.Create(CultureInfo.InvariantCulture, $"({pixel.X},{pixel.Y}) expected discarded, actual {Words(actual)}"));
                }

                continue;
            }

            for (var index = 0; index < expected.Length; index++)
            {
                if (!expected[index].Matches(actual[index]))
                {
                    mismatches.Add(string.Create(
                        CultureInfo.InvariantCulture,
                        $"({pixel.X},{pixel.Y}) v{FirstResult + index} expected {expected[index]} actual {Expected.Hex(actual[index])}"));
                }
            }
        }

        return mismatches;
    }

    private static string Words(uint[] words)
    {
        return string.Join(" ", words.Select(Expected.Hex));
    }
}
