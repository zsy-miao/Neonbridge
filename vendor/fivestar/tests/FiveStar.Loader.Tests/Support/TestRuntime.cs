// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Shims.D3D12X.Pipelines;
using FiveStar.Loader.Shims.Gdk;
using FiveStar.Loader.Shims.Network;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// A loader runtime with its own temporary folder for TLS hosts, saves and synthetic images. Disposing unloads every
/// image and deletes the folder, so no test leaves state behind.
/// </summary>
internal sealed class TestRuntime : IDisposable
{
    private TestRuntime(LoaderRuntime runtime, string folder)
    {
        Runtime = runtime;
        Folder = folder;
    }

    public LoaderRuntime Runtime { get; }

    /// <summary>A fresh temporary folder owned by this runtime.</summary>
    public string Folder { get; }

    public string SaveRoot => Path.Combine(Folder, "saves");

    /// <summary>
    /// A runtime whose manual modules are <paramref name="manualModules"/>, whose package root is
    /// <paramref name="packageRoot"/>, whose log lines go to <paramref name="log"/> (nowhere when null) and whose
    /// XNetworking answers follow <paramref name="network"/> (offline when null).
    /// </summary>
    public static TestRuntime Create(IEnumerable<string>? manualModules = null, string? packageRoot = null, TextWriter? log = null, NetworkOptions? network = null)
    {
        string folder = Path.Combine(Path.GetTempPath(), "fivestar-loader-tests", Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(folder);

        var manual = new HashSet<string>(manualModules ?? [], StringComparer.OrdinalIgnoreCase);
        var options = new LoaderOptions([], manual, UnresolvedMode.Stub, true, Path.Combine(folder, "work"), null, Environment.ProcessId);
        var gdk = new GdkOptions(packageRoot, Path.Combine(folder, "saves"), Path.Combine(folder, "data"), folder, SystemService.XboxScarlettAnaconda, Environment.OSVersion.Version, network ?? NetworkOptions.Offline);
        var environment = new LoaderEnvironment(Environment.SystemDirectory, TimeProvider.System, Environment.FailFast);
        var runtime = new LoaderRuntime(options, gdk, new PipelineFolders(null, null, null), environment, new LoaderLog(log ?? TextWriter.Null, null, false));

        return new TestRuntime(runtime, folder);
    }

    /// <summary>The native entry point mapped code would get for module!name.</summary>
    public nint Shim(string module, string name)
    {
        var entry = Runtime.Shims.TryResolve(module, name, 0);

        return entry?.Address ?? throw new InvalidOperationException($"no shim {module}!{name}");
    }

    public nint Gdk(string name)
    {
        return Shim(GdkModule.Name, name);
    }

    public void Dispose()
    {
        Runtime.Loader.UnloadAll();
        Runtime.Dispose();

        try
        {
            Directory.Delete(Folder, true);
        }
        catch (IOException)
        {
            // A TLS host DLL can stay locked until the process exits.
        }
        catch (UnauthorizedAccessException)
        {
            // Same as above.
        }
    }
}
