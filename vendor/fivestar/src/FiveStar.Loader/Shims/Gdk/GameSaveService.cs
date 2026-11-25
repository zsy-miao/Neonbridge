// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XGameSave.h and XGameSaveFiles.h backed by folders (<see cref="SaveStore"/>): providers, containers, staged updates
/// applied per blob, enumeration, sync and async reads, deletes and the quota. "needsSync" is always false (no cloud).
/// </summary>
internal sealed unsafe class GameSaveService
{
    public const string InitializeProviderApi = "XGameSaveInitializeProviderAsync";
    public const string RemainingQuotaApi = "XGameSaveGetRemainingQuotaAsync";
    public const string DeleteContainerApi = "XGameSaveDeleteContainerAsync";
    public const string ReadBlobDataApi = "XGameSaveReadBlobDataAsync";
    public const string SubmitUpdateApi = "XGameSaveSubmitUpdateAsync";
    public const string SaveFolderApi = "XGameSaveFilesGetFolderWithUiAsync";

    private const string SaveFilesFolder = "savefiles";
    private const string DefaultConfigId = "default";
    private const string XuidFormat = "x16";

    private readonly HandleTable<SaveProvider> _providers = new("XGSPROV");
    private readonly HandleTable<SaveContainer> _containers = new("XGSCONT");
    private readonly HandleTable<SaveUpdate> _updates = new("XGSUPDT");
    private readonly Dictionary<nint, int> _pendingReadCounts = [];
    private readonly GdkConfig _config;
    private readonly UserService _users;
    private readonly AsyncService _async;
    private readonly NativeBoundary _boundary;

    public GameSaveService(GdkConfig config, SaveStore store, UserService users, AsyncService async, NativeBoundary boundary)
    {
        _config = config;
        Store = store;
        _users = users;
        _async = async;
        _boundary = boundary;
    }

    public SaveStore Store { get; }

    public int InitializeProvider(nint user, string? configId, out nint provider)
    {
        _config.Ensure();
        provider = 0;

        if (!_users.IsUser(user))
        {
            return HResult.InvalidArgument;
        }

        string id = configId ?? string.Empty;
        var created = new SaveProvider(Store.RootFor(id), id);
        Directory.CreateDirectory(created.Root);
        provider = _providers.Add(created);
        _boundary.Info($"XGameSave provider \"{created.ConfigId}\" -> {created.Root}");

        return HResult.Ok;
    }

    public int InitializeProviderAsync(nint user, string? configId, nint async)
    {
        var operation = new SaveOperation(this, SaveOperationKind.InitializeProvider, new SaveProviderRequest(user, configId), null, async);

        return _async.BeginManaged(async, InitializeProviderApi, operation);
    }

    public void CloseProvider(nint provider)
    {
        _providers.Remove(provider);
    }

    public int GetRemainingQuota(nint provider, long* remaining)
    {
        var found = _providers.Get(provider);

        if (found is null || remaining == null)
        {
            return HResult.InvalidArgument;
        }

        *remaining = SaveLayout.QuotaBytes - SaveStore.UsedBytes(found);

        return HResult.Ok;
    }

    public int GetRemainingQuotaAsync(nint provider, nint async)
    {
        var found = _providers.Get(provider);

        if (found is null)
        {
            return HResult.InvalidArgument;
        }

        return _async.BeginManaged(async, RemainingQuotaApi, new SaveOperation(this, SaveOperationKind.RemainingQuota, found, null, async));
    }

    public int DeleteContainer(nint provider, byte* name)
    {
        var found = _providers.Get(provider);
        string? text = Utf8Text.Read((nint)name);

        if (found is null || string.IsNullOrEmpty(text))
        {
            return HResult.InvalidArgument;
        }

        return Store.DeleteContainer(found, text);
    }

    public int DeleteContainerAsync(nint provider, byte* name, nint async)
    {
        var found = _providers.Get(provider);
        string? text = Utf8Text.Read((nint)name);

        if (found is null || string.IsNullOrEmpty(text))
        {
            return HResult.InvalidArgument;
        }

        var operation = new SaveOperation(this, SaveOperationKind.DeleteContainer, new SaveContainer(found, text), null, async);

        return _async.BeginManaged(async, DeleteContainerApi, operation);
    }

    /// <summary>A container handle; its folder appears with the first submitted update.</summary>
    public int CreateContainer(nint provider, byte* name, nint* container)
    {
        var found = _providers.Get(provider);
        string? text = Utf8Text.Read((nint)name);

        if (found is null || container == null)
        {
            return HResult.InvalidArgument;
        }

        if (string.IsNullOrEmpty(text) || Encoding.UTF8.GetByteCount(text) > SaveLayout.MaxContainerNameBytes)
        {
            return SaveLayout.InvalidContainerName;
        }

        *container = _containers.Add(new SaveContainer(found, text));

        return HResult.Ok;
    }

    public void CloseContainer(nint container)
    {
        _containers.Remove(container);
    }

    /// <summary>Calls the callback for the container if it exists; a missing container is S_OK without a call (unconfirmed).</summary>
    public int GetContainerInfo(nint provider, byte* name, nint context, nint callback)
    {
        var found = _providers.Get(provider);
        string? text = Utf8Text.Read((nint)name);

        if (found is null || string.IsNullOrEmpty(text) || callback == 0)
        {
            return HResult.InvalidArgument;
        }

        var container = new SaveContainer(found, text);
        bool exists = Directory.Exists(container.Directory);
        _boundary.Info($"XGameSaveGetContainerInfo(\"{text}\") -> exists {exists}");

        if (exists)
        {
            SaveStore.InvokeContainerInfo(container, context, callback);
        }

        return HResult.Ok;
    }

    public int EnumerateContainers(nint provider, string? prefix, nint context, nint callback)
    {
        var found = _providers.Get(provider);

        if (found is null || callback == 0)
        {
            return HResult.InvalidArgument;
        }

        if (!Directory.Exists(found.Root))
        {
            return HResult.Ok;
        }

        var reported = new List<string>();

        foreach (string directory in Directory.EnumerateDirectories(found.Root).OrderBy(d => d, StringComparer.Ordinal))
        {
            var container = new SaveContainer(found, NameEscaping.Decode(Path.GetFileName(directory)));

            if (prefix is not null && !container.Name.StartsWith(prefix, StringComparison.Ordinal))
            {
                continue;
            }

            reported.Add(container.Name);

            if (!SaveStore.InvokeContainerInfo(container, context, callback))
            {
                break;
            }
        }

        _boundary.Info($"XGameSaveEnumerateContainerInfo(\"{prefix}\") -> [{string.Join(", ", reported)}]");

        return HResult.Ok;
    }

    public int EnumerateBlobs(nint container, string? prefix, nint context, nint callback)
    {
        var found = _containers.Get(container);

        if (found is null || callback == 0)
        {
            return HResult.InvalidArgument;
        }

        byte* info = stackalloc byte[SaveLayout.BlobInfoSize];
        var reported = new List<string>();

        foreach (var (name, file) in SaveStore.Blobs(found))
        {
            if (prefix is not null && !name.StartsWith(prefix, StringComparison.Ordinal))
            {
                continue;
            }

            reported.Add(name);

            if (!InvokeBlobInfo(info, name, new FileInfo(file).Length, context, callback))
            {
                break;
            }
        }

        _boundary.Info($"XGameSaveEnumerateBlobInfo(\"{found.Name}\", \"{prefix}\") -> [{string.Join(", ", reported)}]");

        return HResult.Ok;
    }

    public int ReadBlobData(nint container, byte** blobNames, uint* countOfBlobs, nuint blobsSize, byte* blobData)
    {
        var found = _containers.Get(container);

        if (found is null || countOfBlobs == null)
        {
            return HResult.InvalidArgument;
        }

        var names = ReadNames(blobNames, *countOfBlobs);
        var blobs = new List<SaveBlobData>();
        int hr = SaveStore.ReadBlobs(found, names, blobs);

        if (!HResult.Failed(hr) && (blobData == null || blobsSize < SaveBlobLayout.MinimumSize(blobs)))
        {
            hr = SaveLayout.ProvidedBufferTooSmall;
        }

        _boundary.Info($"XGameSaveReadBlobData(\"{found.Name}\", [{string.Join(", ", names ?? [])}], {blobsSize} bytes) -> 0x{hr:x8}, {blobs.Count} blob(s), {SaveBlobLayout.Size(blobs)} bytes");

        if (HResult.Failed(hr))
        {
            return hr;
        }

        SaveBlobLayout.Write(blobs, blobData, blobsSize);
        *countOfBlobs = (uint)blobs.Count;

        return HResult.Ok;
    }

    /// <summary>The result size (XAsyncGetResultSize) is the full layout size.</summary>
    public int ReadBlobDataAsync(nint container, byte** blobNames, uint countOfBlobs, nint async)
    {
        var found = _containers.Get(container);

        if (found is null)
        {
            return HResult.InvalidArgument;
        }

        var names = ReadNames(blobNames, countOfBlobs);
        _boundary.Info($"XGameSaveReadBlobDataAsync(\"{found.Name}\", [{string.Join(", ", names ?? [])}])");
        var operation = new SaveOperation(this, SaveOperationKind.ReadBlobs, found, names, async);

        return _async.BeginManaged(async, ReadBlobDataApi, operation);
    }

    public int ReadBlobDataResult(nint async, nuint blobsSize, byte* blobData, uint* countOfBlobs)
    {
        int hr = _async.ManagedResult(async, ReadBlobDataApi, blobsSize, (nint)blobData);

        if (hr == HResult.NotSufficientBuffer)
        {
            hr = SaveLayout.ProvidedBufferTooSmall;
        }

        int count;

        lock (_pendingReadCounts)
        {
            _pendingReadCounts.Remove(async, out count);
        }

        if (countOfBlobs != null && !HResult.Failed(hr))
        {
            *countOfBlobs = (uint)count;
        }

        return hr;
    }

    public void RecordReadCount(nint async, int count)
    {
        lock (_pendingReadCounts)
        {
            _pendingReadCounts[async] = count;
        }
    }

    public int CreateUpdate(nint container, byte* displayName, nint* update)
    {
        var found = _containers.Get(container);

        if (found is null || update == null)
        {
            return HResult.InvalidArgument;
        }

        *update = _updates.Add(new SaveUpdate(found, Utf8Text.Read((nint)displayName)));

        return HResult.Ok;
    }

    public int SubmitBlobWrite(nint update, byte* blobName, byte* data, nuint byteCount)
    {
        var found = _updates.Get(update);
        string? name = Utf8Text.Read((nint)blobName);

        if (found is null || string.IsNullOrEmpty(name) || (data == null && byteCount != 0))
        {
            return HResult.InvalidArgument;
        }

        if (byteCount > SaveLayout.MaxBlobBytes)
        {
            return SaveLayout.UpdateTooBig;
        }

        found.Operations.Add(new SaveBlobData(name, new ReadOnlySpan<byte>(data, (int)byteCount).ToArray()));

        return HResult.Ok;
    }

    public int SubmitBlobDelete(nint update, byte* blobName)
    {
        var found = _updates.Get(update);
        string? name = Utf8Text.Read((nint)blobName);

        if (found is null || string.IsNullOrEmpty(name))
        {
            return HResult.InvalidArgument;
        }

        found.Operations.Add(new SaveBlobData(name, null));

        return HResult.Ok;
    }

    public int SubmitUpdate(nint update)
    {
        var found = _updates.Get(update);

        if (found is null)
        {
            return HResult.InvalidArgument;
        }

        return Store.Apply(found);
    }

    public int SubmitUpdateAsync(nint update, nint async)
    {
        var found = _updates.Get(update);

        if (found is null)
        {
            return HResult.InvalidArgument;
        }

        return _async.BeginManaged(async, SubmitUpdateApi, new SaveOperation(this, SaveOperationKind.SubmitUpdate, found, null, async));
    }

    public void CloseUpdate(nint update)
    {
        _updates.Remove(update);
    }

    /// <summary>XGameSaveFilesGetFolderWithUiAsync: &lt;save root&gt;\savefiles\&lt;xuid&gt;\&lt;configId&gt;.</summary>
    public int GetFolderWithUiAsync(nint user, byte* configId, nint async)
    {
        if (!_users.IsUser(user))
        {
            return HResult.InvalidArgument;
        }

        return _async.BeginManaged(async, SaveFolderApi, new SaveFolderOperation(SaveFilesFolderFor(configId)));
    }

    /// <summary>XGameSaveFilesGetRemainingQuota: the XGameSave quota less what the configuration's save-files folder holds.</summary>
    public int GetSaveFilesRemainingQuota(nint user, byte* configId, long* remaining)
    {
        if (!_users.IsUser(user) || remaining == null)
        {
            return HResult.InvalidArgument;
        }

        var folder = new DirectoryInfo(SaveFilesFolderFor(configId));
        long used = 0;

        if (folder.Exists)
        {
            used = folder.EnumerateFiles("*", SearchOption.AllDirectories).Sum(f => f.Length);
        }

        *remaining = Math.Max(0, SaveLayout.QuotaBytes - used);

        return HResult.Ok;
    }

    /// <summary>&lt;save root&gt;\savefiles\&lt;xuid&gt;\&lt;configId&gt;.</summary>
    private string SaveFilesFolderFor(byte* configId)
    {
        _config.Ensure();
        string id = NameEscaping.Encode(Utf8Text.Read((nint)configId) ?? DefaultConfigId);
        string xuid = UserService.Xuid.ToString(XuidFormat, CultureInfo.InvariantCulture);

        return Path.Combine(_config.SaveRoot, SaveFilesFolder, xuid, id);
    }

    private static string[]? ReadNames(byte** names, uint count)
    {
        if (names == null || count == 0)
        {
            return null;
        }

        var result = new string[count];

        for (int i = 0; i < count; i++)
        {
            result[i] = Utf8Text.Read((nint)names[i]) ?? string.Empty;
        }

        return result;
    }

    private static bool InvokeBlobInfo(byte* info, string name, long size, nint context, nint callback)
    {
        byte* text = Utf8Text.Allocate(name);
        *(nint*)(info + SaveLayout.BlobNameOffset) = (nint)text;
        *(ulong*)(info + SaveLayout.BlobSizeOffset) = (uint)size;

        try
        {
            return ((delegate* unmanaged<nint, nint, byte>)callback)((nint)info, context) != 0;
        }
        finally
        {
            System.Runtime.InteropServices.NativeMemory.Free(text);
        }
    }
}
