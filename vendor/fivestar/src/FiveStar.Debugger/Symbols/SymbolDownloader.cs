// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Debugger.Session;

namespace FiveStar.Debugger.Symbols;

/// <summary>Finds an image's PDB in the symbol stores and downloads it from a store's server into its cache when missing.</summary>
internal sealed class SymbolDownloader : IDisposable
{
    private const string PartialSuffix = ".partial";

    private static readonly TimeSpan RequestTimeout = TimeSpan.FromMinutes(2);

    private readonly IReadOnlyList<SymbolStore> _stores;
    private readonly DebugLog _log;
    private readonly HttpClient _http = new() { Timeout = RequestTimeout };

    public SymbolDownloader(IReadOnlyList<SymbolStore> stores, DebugLog log)
    {
        _stores = stores;
        _log = log;
    }

    /// <summary>The folder that holds the image's PDB, or null when no store has it.</summary>
    public string? Locate(string imagePath)
    {
        if (_stores.Count == 0)
        {
            return null;
        }

        var pdb = PdbIdentity.Read(imagePath);

        if (pdb is null)
        {
            return null;
        }

        foreach (var store in _stores)
        {
            string folder = Path.Combine(store.Cache, pdb.Name, pdb.Key);

            if (File.Exists(Path.Combine(folder, pdb.Name)))
            {
                return folder;
            }

            if (store.Server is not null && Download(store.Server, pdb, folder))
            {
                return folder;
            }
        }

        return null;
    }

    public void Dispose()
    {
        _http.Dispose();
    }

    private bool Download(string server, PdbIdentity pdb, string folder)
    {
        string url = $"{server.TrimEnd('/')}/{pdb.Name}/{pdb.Key}/{pdb.Name}";

        try
        {
            using var response = _http.Send(new HttpRequestMessage(HttpMethod.Get, url));

            if (!response.IsSuccessStatusCode)
            {
                _log.Write($"symbols: {pdb.Name} {pdb.Key}: {(int)response.StatusCode} from {server}");

                return false;
            }

            Directory.CreateDirectory(folder);
            string target = Path.Combine(folder, pdb.Name);
            string partial = target + PartialSuffix;
            Save(response, partial);
            File.Move(partial, target, true);
            _log.Write($"symbols: downloaded {pdb.Name} to {folder}");

            return true;
        }
        catch (Exception e) when (e is HttpRequestException or TaskCanceledException or IOException or UnauthorizedAccessException)
        {
            _log.Write($"symbols: {pdb.Name}: {e.Message}");

            return false;
        }
    }

    private static void Save(HttpResponseMessage response, string path)
    {
        using var source = response.Content.ReadAsStream();
        using var target = File.Create(path);
        source.CopyTo(target);
    }
}
