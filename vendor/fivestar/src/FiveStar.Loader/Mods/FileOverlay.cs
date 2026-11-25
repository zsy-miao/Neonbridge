// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.Gdk;

namespace FiveStar.Loader.Mods;

/// <summary>
/// Mod folders laid over the package root. A path the game opens inside the package root is redirected to the same
/// relative path in the first mod folder that has that file; every other path is left alone. Paths on the console's\n/// package drive (G:) are first mapped to the package root.
/// </summary>
internal sealed unsafe class FileOverlay
{
    /// <summary>The Win32 prefix that turns off path normalization (<c>\\?\C:\...</c>).</summary>
    private const string ExtendedPathPrefix = @"\\?\";

    /// <summary>The drive letter of the console's package mount.</summary>
    private const char PackageDrive = 'G';

    /// <summary>Length of <c>G:\</c>.</summary>
    private const int ConsoleDrivePrefixLength = 3;

    private readonly GdkConfig _config;
    private readonly LoaderLog _log;
    private readonly List<string> _folders = [];

    public FileOverlay(GdkConfig config, LoaderLog log)
    {
        _config = config;
        _log = log;
    }

    /// <summary>The mod folders, highest priority first.</summary>
    public IReadOnlyList<string> Folders => _folders;

    /// <summary>Adds a mod folder below the ones already added; set up before mapped code runs.</summary>
    public void AddFolder(string folder)
    {
        _folders.Add(Path.TrimEndingDirectorySeparator(Path.GetFullPath(folder)));
    }

    /// <summary>The mod file for a UTF-16 path, or null to open the path as given.</summary>
    public string? Redirect(char* path, string function)
    {
        if (path == null)
        {
            return null;
        }

        return Redirect(new string(path), function);
    }

    /// <summary>The mod file for an ANSI path, or null to open the path as given.</summary>
    public string? Redirect(byte* path, string function)
    {
        if (path == null)
        {
            return null;
        }

        return Redirect(Marshal.PtrToStringAnsi((nint)path), function);
    }

    /// <summary>
    /// The file to open instead of <paramref name="path"/>, or null to open the path as given: the mod file, else the
    /// package-root path of a console package path.
    /// </summary>
    public string? Redirect(string? path, string function)
    {
        if (_log.DetailEnabled)
        {
            _log.Detail($"  [file] {function}(\"{path}\")");
        }

        if (string.IsNullOrEmpty(path))
        {
            return null;
        }

        string? package = FromConsoleDrive(path);
        string? mod = FromMods(package ?? path, function);

        return mod ?? package;
    }

    /// <summary>
    /// The console mounts the game package as drive G: (the game opens <c>G:\appdata0.rpf</c>); here that is the package
    /// root.
    /// </summary>
    private string? FromConsoleDrive(string path)
    {
        bool drive = path.Length > ConsoleDrivePrefixLength && char.ToUpperInvariant(path[0]) == PackageDrive && path[1] == Path.VolumeSeparatorChar;

        if (!drive || !IsSeparator(path[ConsoleDrivePrefixLength - 1]))
        {
            return null;
        }

        return Path.Combine(_config.PackageRoot, path[ConsoleDrivePrefixLength..]);
    }

    private static bool IsSeparator(char character)
    {
        return character == Path.DirectorySeparatorChar || character == Path.AltDirectorySeparatorChar;
    }

    private string? FromMods(string path, string function)
    {
        if (_folders.Count == 0)
        {
            return null;
        }

        string? relative = RelativeToPackage(path);

        if (relative is null)
        {
            return null;
        }

        foreach (string folder in _folders)
        {
            string candidate = Path.Combine(folder, relative);

            if (File.Exists(candidate))
            {
                _log.Detail($"  [overlay] {function}(\"{path}\") -> \"{candidate}\"");

                return candidate;
            }
        }

        return null;
    }

    private string? RelativeToPackage(string path)
    {
        string plain = path;

        if (plain.StartsWith(ExtendedPathPrefix, StringComparison.Ordinal))
        {
            plain = plain[ExtendedPathPrefix.Length..];
        }

        string full;

        try
        {
            full = Path.GetFullPath(plain);
        }
        catch (Exception e) when (e is ArgumentException or NotSupportedException or PathTooLongException)
        {
            return null;
        }

        string root = Path.TrimEndingDirectorySeparator(_config.PackageRoot);

        if (full.Length <= root.Length + 1 || !full.StartsWith(root, StringComparison.OrdinalIgnoreCase))
        {
            return null;
        }

        if (full[root.Length] != Path.DirectorySeparatorChar && full[root.Length] != Path.AltDirectorySeparatorChar)
        {
            return null;
        }

        return full[(root.Length + 1)..];
    }
}
