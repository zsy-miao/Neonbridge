// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// Title and package identity and the folders the offline GDK uses. Read on first use from MicrosoftGame.config in the
/// package root (the configured root, else the main image's directory); nothing is copied.
/// </summary>
internal sealed class GdkConfig
{
    private const string TitleIdFormat = "X8";

    private readonly object _gate = new();
    private readonly GdkOptions _options;
    private readonly ImageLoader _loader;
    private readonly LoaderLog _log;
    private bool _loaded;
    private string _packageRoot = string.Empty;
    private PackageIdentity _identity = PackageIdentity.Default;

    public GdkConfig(GdkOptions options, ImageLoader loader, LoaderLog log)
    {
        _options = options;
        _loader = loader;
        _log = log;
    }

    public GdkOptions Options => _options;

    public string PackageRoot
    {
        get
        {
            Ensure();

            return _packageRoot;
        }
    }

    public PackageIdentity Identity
    {
        get
        {
            Ensure();

            return _identity;
        }
    }

    /// <summary>The title id as eight upper-case hex digits.</summary>
    public string TitleIdText => Identity.TitleId.ToString(TitleIdFormat, CultureInfo.InvariantCulture);

    /// <summary>Root of XGameSave containers, XGameSaveFiles folders and persistent local storage.</summary>
    public string SaveRoot => _options.SaveRoot ?? Path.Combine(_options.DefaultDataRoot, TitleIdText);

    public void Ensure()
    {
        lock (_gate)
        {
            if (_loaded)
            {
                return;
            }

            _loaded = true;
            _packageRoot = _options.PackageRoot ?? MainImageDirectory() ?? _options.FallbackPackageRoot;
            Load(Path.Combine(_packageRoot, MicrosoftGameConfig.FileName));
        }
    }

    private string? MainImageDirectory()
    {
        var main = _loader.MainImage;

        if (main is null)
        {
            return null;
        }

        return Path.GetDirectoryName(main.Path);
    }

    private void Load(string path)
    {
        if (!File.Exists(path))
        {
            _log.Warn($"[gdk] no {MicrosoftGameConfig.FileName} in {_packageRoot}; using defaults");

            return;
        }

        try
        {
            _identity = MicrosoftGameConfig.Read(path);
            _log.Info($"  [gdk] package {_identity.Name} {_identity.Version} title 0x{TitleIdText} store {_identity.StoreId} root {_packageRoot}");
        }
        catch (Exception e)
        {
            _log.Warn($"[gdk] could not parse {path}: {e.Message}");
        }
    }
}
