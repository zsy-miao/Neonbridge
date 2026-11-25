// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Diagnostics;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;
using Xunit;
using Xunit.Abstractions;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// Opt-in: runs a mesh pipeline the loader made (FIVESTAR_REPRO_MESH and FIVESTAR_REPRO_PIXEL, DXIL files from the
/// program cache) with DispatchMesh(FIVESTAR_REPRO_GROUPS, 1, 1) on zeroed inputs: the window (slots 0 and 1) and the
/// address map (slot 7) zero, user data zero. With FIVESTAR_TEST_ADAPTER=hardware it runs on the GPU and reports how
/// long the dispatch took, to tell a group that never finishes from one that is only slow. Skipped without the files.
/// A captured draw replays with FIVESTAR_REPRO_USER (its user SGPRs, comma-separated hex) and FIVESTAR_REPRO_BUFFERS
/// (`slot=file;slot=file`: raw dumps bound as buffers at those heap slots, the window and the address map included;
/// slot 1, the writable window, is bound writable). The window's last 8 words are printed after the run: an instrumented
/// program can leave counters there (rounds, steps, threads ended).
/// </summary>
public sealed class MeshPipelineRepro : IClassFixture<WarpFixture>
{
    private const int WindowWords = 1 << 16;
    private const int Size = 64;

    /// <summary>Words of the writable window printed after the run.</summary>
    private const int Shown = 8;

    private readonly WarpFixture _fixture;
    private readonly ITestOutputHelper _output;

    public MeshPipelineRepro(WarpFixture fixture, ITestOutputHelper output)
    {
        _fixture = fixture;
        _output = output;
    }

    [WarpFact]
    public void A_cached_mesh_pipeline_runs_to_its_end_on_zeroed_inputs()
    {
        string? mesh = Environment.GetEnvironmentVariable("FIVESTAR_REPRO_MESH");
        string? pixel = Environment.GetEnvironmentVariable("FIVESTAR_REPRO_PIXEL");

        if (mesh is null || pixel is null || !File.Exists(mesh) || !File.Exists(pixel))
        {
            return;
        }

        int groups = int.Parse(Environment.GetEnvironmentVariable("FIVESTAR_REPRO_GROUPS") ?? "4", System.Globalization.CultureInfo.InvariantCulture);
        var bindings = new Bindings { UserSgprs = UserData(), Buffers = Buffers() };

        var clock = Stopwatch.StartNew();
        var result = _fixture.Runner.Mesh(new MeshJob(File.ReadAllBytes(mesh), File.ReadAllBytes(pixel), Framebuffer.Exact(1, Size, Size), bindings, groups));
        _output.WriteLine($"DispatchMesh({groups}, 1, 1) of {Path.GetFileName(mesh)}: {clock.Elapsed.TotalMilliseconds:F1} ms");

        // An instrumented program can leave counters in the last words of the writable window (slot 1).
        if (result.Buffers.TryGetValue(1, out var window))
        {
            _output.WriteLine($"writable window end: {string.Join(' ', window.TakeLast(Shown).Select(word => word.ToString("x8", System.Globalization.CultureInfo.InvariantCulture)))}");
        }
    }

    /// <summary>The user SGPRs of FIVESTAR_REPRO_USER (hex, comma-separated), zero past them; 32 zeros without it.</summary>
    private static uint[] UserData()
    {
        var user = new uint[32];
        string? text = Environment.GetEnvironmentVariable("FIVESTAR_REPRO_USER");

        if (text is null)
        {
            return user;
        }

        var values = text.Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries);

        for (int i = 0; i < Math.Min(values.Length, user.Length); i++)
        {
            user[i] = uint.Parse(values[i].Replace("0x", string.Empty, StringComparison.OrdinalIgnoreCase), System.Globalization.NumberStyles.HexNumber, System.Globalization.CultureInfo.InvariantCulture);
        }

        return user;
    }

    /// <summary>The dumps of FIVESTAR_REPRO_BUFFERS by heap slot; zeroed window (0, 1) and address map (7) where none is given.</summary>
    private static List<BufferResource> Buffers()
    {
        var files = new Dictionary<int, string>();
        string? text = Environment.GetEnvironmentVariable("FIVESTAR_REPRO_BUFFERS");

        foreach (var entry in (text ?? string.Empty).Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
        {
            var parts = entry.Split('=', 2);
            files[int.Parse(parts[0], System.Globalization.CultureInfo.InvariantCulture)] = parts[1];
        }

        foreach (int slot in new[] { 0, 1, 7 })
        {
            files.TryAdd(slot, string.Empty);
        }

        return [.. files.Select(pair => new BufferResource(pair.Key, Words(pair.Value), Writable: pair.Key == 1))];
    }

    /// <summary>A dump's dwords (padded to a whole dword), or a zeroed window-sized buffer without a file.</summary>
    private static uint[] Words(string path)
    {
        if (path.Length == 0)
        {
            return new uint[WindowWords];
        }

        byte[] bytes = File.ReadAllBytes(path);
        var words = new uint[(bytes.Length + 3) / 4];
        Buffer.BlockCopy(bytes, 0, words, 0, bytes.Length);

        return words;
    }
}
