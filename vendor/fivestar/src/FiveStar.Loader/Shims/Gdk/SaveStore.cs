// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// The folders behind XGameSave: &lt;save root&gt;\saves\&lt;xuid&gt;\&lt;configId&gt;\&lt;container&gt;\&lt;blob&gt;, names
/// %-escaped, the display name in "$meta". An update writes each blob to a temporary file and moves it over the old one.
/// Updates, reads and deletes run one at a time, a read shares its file for deletion, and a move refused by another
/// process holding the blob (an antivirus scan, an indexer) is tried again for <see cref="MoveRetries"/> times: one
/// failed move failed the update, and the game shows its could-not-autosave dialog for a failed update.
/// </summary>
internal sealed unsafe class SaveStore
{
    private const string MetaFile = "$meta";
    private const string TemporarySuffix = ".~tmp";
    private const string SavesFolder = "saves";
    private const string DefaultConfigId = "default";
    private const string XuidFormat = "x16";

    /// <summary>Tries of a blob's move over the old one, <see cref="MoveRetryDelay"/> apart.</summary>
    public const int MoveRetries = 20;

    private static readonly TimeSpan MoveRetryDelay = TimeSpan.FromMilliseconds(50);
    private static readonly object Files = new();

    private readonly GdkConfig _config;
    private readonly NativeBoundary _boundary;
    private readonly TimeProvider _time;

    public SaveStore(GdkConfig config, NativeBoundary boundary, TimeProvider time)
    {
        _config = config;
        _boundary = boundary;
        _time = time;
    }

    /// <summary>The folder of a provider's containers.</summary>
    public string RootFor(string configId)
    {
        string id = configId.Length == 0 ? DefaultConfigId : configId;

        return Path.Combine(_config.SaveRoot, SavesFolder, UserService.Xuid.ToString(XuidFormat, System.Globalization.CultureInfo.InvariantCulture), NameEscaping.Encode(id));
    }

    public static long UsedBytes(SaveProvider provider)
    {
        if (!Directory.Exists(provider.Root))
        {
            return 0;
        }

        return new DirectoryInfo(provider.Root).EnumerateFiles("*", SearchOption.AllDirectories).Sum(f => f.Length);
    }

    /// <summary>The blobs of a container as (name, file), ordered by name.</summary>
    public static List<(string Name, string File)> Blobs(SaveContainer container)
    {
        if (!Directory.Exists(container.Directory))
        {
            return [];
        }

        return Directory.EnumerateFiles(container.Directory)
            .Select(Path.GetFileName)
            .Where(f => f != MetaFile && !f!.EndsWith(TemporarySuffix, StringComparison.Ordinal))
            .Select(f => (NameEscaping.Decode(f!), Path.Combine(container.Directory, f!)))
            .OrderBy(b => b.Item1, StringComparer.Ordinal)
            .ToList();
    }

    public int DeleteContainer(SaveProvider provider, string name)
    {
        string directory = new SaveContainer(provider, name).Directory;

        lock (Files)
        {
            if (Directory.Exists(directory))
            {
                Directory.Delete(directory, true);
            }
        }

        _boundary.Trace($"XGameSaveDeleteContainer({name})");

        return HResult.Ok;
    }

    /// <summary>Reads the named blobs, or every blob when <paramref name="names"/> is null or empty.</summary>
    public static int ReadBlobs(SaveContainer container, string[]? names, List<SaveBlobData> blobs)
    {
        lock (Files)
        {
            return ReadBlobsLocked(container, names, blobs);
        }
    }

    private static int ReadBlobsLocked(SaveContainer container, string[]? names, List<SaveBlobData> blobs)
    {
        var all = Blobs(container).ToDictionary(b => b.Name, b => b.File, StringComparer.Ordinal);
        var wanted = names is not null && names.Length > 0 ? names : all.Keys.ToArray();

        foreach (string name in wanted)
        {
            if (!all.TryGetValue(name, out var file))
            {
                return SaveLayout.BlobNotFound;
            }

            blobs.Add(new SaveBlobData(name, ReadShared(file)));
        }

        return HResult.Ok;
    }

    /// <summary>Applies a staged update after checking the quota; a file the system refuses fails it, logged.</summary>
    public int Apply(SaveUpdate update)
    {
        try
        {
            lock (Files)
            {
                return ApplyLocked(update);
            }
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException)
        {
            _boundary.Error($"XGameSave update \"{update.Container.Name}\" failed: {exception.Message} (0x{exception.HResult:x8})");

            return Failure(exception.HResult);
        }
    }

    private int ApplyLocked(SaveUpdate update)
    {
        var container = update.Container;
        long growth = update.Operations.Where(o => o.Data is not null).Sum(o => (long)o.Data!.Length);

        if (growth > SaveLayout.QuotaBytes - UsedBytes(container.Provider))
        {
            return SaveLayout.QuotaExceeded;
        }

        Directory.CreateDirectory(container.Directory);

        foreach (var operation in update.Operations)
        {
            ApplyOperation(container, operation);
        }

        if (update.DisplayName is not null)
        {
            File.WriteAllText(Path.Combine(container.Directory, MetaFile), update.DisplayName);
        }

        Directory.SetLastWriteTimeUtc(container.Directory, _time.GetUtcNow().UtcDateTime);
        _boundary.Info($"XGameSave update \"{container.Name}\": {update.Operations.Count} op(s) -> {container.Directory}");

        return HResult.Ok;
    }

    /// <summary>Calls an XGameSaveContainerInfoCallback for <paramref name="container"/>; returns the callback's result.</summary>
    public static bool InvokeContainerInfo(SaveContainer container, nint context, nint callback)
    {
        var blobs = Blobs(container);
        string meta = Path.Combine(container.Directory, MetaFile);
        string display = File.Exists(meta) ? File.ReadAllText(meta) : string.Empty;
        byte* name = Utf8Text.Allocate(container.Name);
        byte* displayName = Utf8Text.Allocate(display);
        byte* info = stackalloc byte[SaveLayout.ContainerInfoSize];

        new Span<byte>(info, SaveLayout.ContainerInfoSize).Clear();
        *(nint*)(info + SaveLayout.ContainerNameOffset) = (nint)name;
        *(nint*)(info + SaveLayout.ContainerDisplayNameOffset) = (nint)displayName;
        *(uint*)(info + SaveLayout.ContainerBlobCountOffset) = (uint)blobs.Count;
        *(ulong*)(info + SaveLayout.ContainerTotalSizeOffset) = (ulong)blobs.Sum(b => new FileInfo(b.File).Length);
        *(long*)(info + SaveLayout.ContainerLastModifiedOffset) = new DateTimeOffset(Directory.GetLastWriteTimeUtc(container.Directory)).ToUnixTimeSeconds();

        try
        {
            return ((delegate* unmanaged<nint, nint, byte>)callback)((nint)info, context) != 0;
        }
        finally
        {
            NativeMemory.Free(name);
            NativeMemory.Free(displayName);
        }
    }

    private static void ApplyOperation(SaveContainer container, SaveBlobData operation)
    {
        string file = Path.Combine(container.Directory, NameEscaping.Encode(operation.Name));

        if (operation.Data is null)
        {
            if (File.Exists(file))
            {
                File.Delete(file);
            }

            return;
        }

        string temporary = file + TemporarySuffix;
        File.WriteAllBytes(temporary, operation.Data);
        Replace(temporary, file);
    }

    /// <summary>Moves <paramref name="temporary"/> over <paramref name="file"/>, tried again while another process holds it.</summary>
    private static void Replace(string temporary, string file)
    {
        for (int attempt = 1; ; attempt++)
        {
            try
            {
                File.Move(temporary, file, true);

                return;
            }
            catch (IOException) when (attempt < MoveRetries)
            {
                Thread.Sleep(MoveRetryDelay);
            }
            catch (UnauthorizedAccessException) when (attempt < MoveRetries)
            {
                Thread.Sleep(MoveRetryDelay);
            }
        }
    }

    /// <summary>A blob's bytes, its file left open to a move or delete meanwhile.</summary>
    private static byte[] ReadShared(string file)
    {
        using var stream = new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        var bytes = new byte[stream.Length];
        stream.ReadExactly(bytes);

        return bytes;
    }

    /// <summary>The exception's HRESULT when it is a failure, else E_FAIL.</summary>
    private static int Failure(int hr)
    {
        if (HResult.Failed(hr))
        {
            return hr;
        }

        return HResult.Fail;
    }
}
