// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Cases;
using FiveStar.Shaders.Tests.Toolchain;
using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests;

/// <summary>
/// One WARP device, work folder and tool set per test class (classes run one at a time), created on first use so a
/// skipped class creates nothing. The probes record the PC wave layout that lane-dependent expectations need.
/// </summary>
public sealed class WarpFixture : IDisposable
{
    private const int ProbeGroups = 2;
    private const int ProbeGroupSize = 64;
    private const int ProbeHeapSlot = 1;

    private readonly Lazy<string> _folder;
    private readonly Lazy<ShaderWorkbench> _workbench;
    private readonly Lazy<WarpDevice> _device;
    private readonly Lazy<PixelContext[]> _pixelLanes;
    private readonly Lazy<PixelContext[]> _halfCoverageLanes;
    private readonly Lazy<int> _computeWaveSize;
    private readonly Dictionary<string, byte[]> _helpers = [];

    public WarpFixture()
        : this(UserDataBindings.FromEnvironment())
    {
    }

    /// <summary>A fixture whose programs and runs take the user SGPRs as <paramref name="userData"/> says, their sources starting with <paramref name="defines"/>.</summary>
    internal WarpFixture(UserDataBinding userData, string defines = "")
    {
        UserData = userData;

        _folder = new Lazy<string>(() => Directory.CreateTempSubdirectory("fivestar-warp-").FullName);
        _workbench = new Lazy<ShaderWorkbench>(() => new ShaderWorkbench(ShaderToolchain.Find(), _folder.Value) { Header = UserDataBindings.Header(userData) + defines });
        _device = new Lazy<WarpDevice>(() => new WarpDevice());
        _pixelLanes = new Lazy<PixelContext[]>(() => ProbePixels("fullscreen_vs.hlsl"));
        _halfCoverageLanes = new Lazy<PixelContext[]>(() => ProbePixels("half_vs.hlsl"));
        _computeWaveSize = new Lazy<int>(ProbeComputeWave);
    }

    /// <summary>The class's work folder, deleted with the fixture.</summary>
    public string Folder => _folder.Value;

    public ShaderWorkbench Workbench => _workbench.Value;

    public WarpRunner Runner => new(_device.Value, UserData);

    /// <summary>How the programs and runs take the user SGPRs.</summary>
    public UserDataBinding UserData { get; }

    /// <summary>Per pixel of an 8x8 fullscreen draw (row-major): its PC lane, quad slot, wave size and wave.</summary>
    public IReadOnlyList<PixelContext> PixelLanes => _pixelLanes.Value;

    /// <summary>As <see cref="PixelLanes"/> for the half-coverage draw (x + y &lt;= 6); uncovered pixels have no wave.</summary>
    public IReadOnlyList<PixelContext> HalfCoverageLanes => _halfCoverageLanes.Value;

    /// <summary>The PC compute wave size (WARP: 4 lanes).</summary>
    public int ComputeWaveSize => _computeWaveSize.Value;

    /// <summary>A helper shader of the Shaders folder, compiled once.</summary>
    public byte[] Helper(string fileName, string profile)
    {
        if (!_helpers.TryGetValue(fileName, out var shader))
        {
            shader = Workbench.CompileHelper(fileName, profile);
            _helpers[fileName] = shader;
        }

        return shader;
    }

    public void Dispose()
    {
        if (_device.IsValueCreated)
        {
            _device.Value.Dispose();
        }

        if (_folder.IsValueCreated)
        {
            Directory.Delete(_folder.Value, recursive: true);
        }
    }

    private PixelContext[] ProbePixels(string vertexShader)
    {
        var vertex = Helper(vertexShader, "vs");
        var pixel = Helper("probe_ps.hlsl", "ps");
        var size = PixelCase.Size;
        var result = Runner.Graphics(new GraphicsJob(vertex, pixel, Framebuffer.Fullscreen(1, size, size, isFloat: false), new Bindings()));
        var words = Enumerable.Range(0, size * size).Select(index => result.Pixel(0, index % size, index / size)).ToArray();
        var waves = new Dictionary<uint, (int X, int Y)?[]>();

        for (var index = 0; index < words.Length; index++)
        {
            if (words[index][1] == Framebuffer.ClearUint)
            {
                continue;
            }

            if (!waves.TryGetValue(words[index][3], out var wave))
            {
                wave = new (int X, int Y)?[words[index][1]];
                waves[words[index][3]] = wave;
            }

            wave[words[index][0]] = (index % size, index / size);
        }

        return [.. Enumerable.Range(0, words.Length).Select(index => Context(index % size, index / size, words[index], waves))];
    }

    private static PixelContext Context(int x, int y, uint[] words, Dictionary<uint, (int X, int Y)?[]> waves)
    {
        var context = new PixelContext(x, y, (int)words[0], (int)words[2], (int)words[1]);

        if (waves.TryGetValue(words[3], out var wave))
        {
            context = context with { Wave = wave };
        }

        return context;
    }

    private int ProbeComputeWave()
    {
        var shader = Helper("probe_cs.hlsl", "cs");
        var output = new BufferResource(ProbeHeapSlot, new uint[ProbeGroups * ProbeGroupSize * 4], Writable: true);
        var result = Runner.Compute(new ComputeJob(shader, ProbeGroups, 1, 1, new Bindings { Buffers = [output] }));
        var words = result.Buffers[ProbeHeapSlot];
        var wave = (int)words[1];

        for (var thread = 0; thread < ProbeGroups * ProbeGroupSize; thread++)
        {
            var index = thread % ProbeGroupSize;
            var (lane, count, first) = (words[4 * thread], words[(4 * thread) + 1], words[(4 * thread) + 2]);

            if (count != wave || lane != index % wave || first != index - lane)
            {
                throw new InvalidOperationException($"unexpected compute lane layout at thread {thread}: lane {lane}, count {count}, first {first}");
            }
        }

        return wave;
    }
}
