// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>The XGameSave *Async operations, each answered by one <see cref="GameSaveService"/> step.</summary>
internal sealed unsafe class SaveOperation : ManagedAsyncOperation
{
    private readonly GameSaveService _service;
    private readonly SaveOperationKind _kind;
    private readonly object _target;
    private readonly string[]? _names;
    private readonly nint _block;
    private nint _handle;
    private long _quota;
    private List<SaveBlobData> _blobs = [];

    public SaveOperation(GameSaveService service, SaveOperationKind kind, object target, string[]? names, nint block)
    {
        _block = block;
        _service = service;
        _kind = kind;
        _target = target;
        _names = names;
    }

    public override AsyncWorkResult Work()
    {
        switch (_kind)
        {
            case SaveOperationKind.InitializeProvider:
                return InitializeProvider();
            case SaveOperationKind.RemainingQuota:
                return RemainingQuota();
            case SaveOperationKind.DeleteContainer:
                return DeleteContainer();
            case SaveOperationKind.ReadBlobs:
                return ReadBlobs();
            default:
                return SubmitUpdate();
        }
    }

    public override int Write(byte* buffer, nuint size)
    {
        switch (_kind)
        {
            case SaveOperationKind.InitializeProvider:
                *(nint*)buffer = _handle;
                break;
            case SaveOperationKind.RemainingQuota:
                *(long*)buffer = _quota;
                break;
            case SaveOperationKind.ReadBlobs:
                SaveBlobLayout.Write(_blobs, buffer, size);
                break;
            default:
                break;
        }

        return HResult.Ok;
    }

    private AsyncWorkResult InitializeProvider()
    {
        var request = (SaveProviderRequest)_target;
        int hr = _service.InitializeProvider(request.User, request.ConfigId, out _handle);

        return new AsyncWorkResult(hr, (nuint)sizeof(nint));
    }

    private AsyncWorkResult RemainingQuota()
    {
        _quota = SaveLayout.QuotaBytes - SaveStore.UsedBytes((SaveProvider)_target);

        return new AsyncWorkResult(HResult.Ok, sizeof(long));
    }

    private AsyncWorkResult DeleteContainer()
    {
        var container = (SaveContainer)_target;

        return new AsyncWorkResult(_service.Store.DeleteContainer(container.Provider, container.Name), 0);
    }

    private AsyncWorkResult ReadBlobs()
    {
        var blobs = new List<SaveBlobData>();
        int hr = SaveStore.ReadBlobs((SaveContainer)_target, _names, blobs);
        _blobs = blobs;
        _service.RecordReadCount(_block, blobs.Count);
        nuint size = HResult.Failed(hr) ? 0 : SaveBlobLayout.Size(blobs);

        return new AsyncWorkResult(hr, size);
    }

    private AsyncWorkResult SubmitUpdate()
    {
        return new AsyncWorkResult(_service.Store.Apply((SaveUpdate)_target), 0);
    }
}
