// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// <c>--d3d12-debug</c>: before the desktop device exists, the D3D12 debug layer (optionally GPU-based validation) and
/// DRED auto-breadcrumbs and page-fault reporting; after, the info-queue messages in the log and a report of the
/// removal reason, the stored messages and the DRED data once the device is removed. Off by default.
/// </summary>
internal sealed unsafe class D3D12Debugging : IDisposable
{
    /// <summary>ID3D12Debug::EnableDebugLayer and ID3D12Debug1::SetEnableGPUBasedValidation (d3d12sdklayers.h).</summary>
    private const int EnableDebugLayerSlot = 3;
    private const int SetGpuValidationSlot = 4;

    /// <summary>ID3D12DeviceRemovedExtendedDataSettings: auto-breadcrumbs and page-fault enablement (d3d12.h).</summary>
    private const int SetBreadcrumbsSlot = 3;
    private const int SetPageFaultsSlot = 4;
    private const int DredForcedOn = 2;

    /// <summary>ID3D12Device::GetDeviceRemovedReason (d3d12.h).</summary>
    private const int RemovedReasonSlot = 37;

    private const string SdkLayers = "d3d12SDKLayers.dll";

    private static readonly Guid DebugIid = new("344488b7-6846-474b-b989-f027448245e0");
    private static readonly Guid Debug1Iid = new("affaa4ca-63fe-4d8e-b8ad-159000af4304");
    private static readonly Guid DredSettingsIid = new("82bc481c-6b9b-4030-aedb-7ee3d1df1e63");

    private readonly D3D12Forwarder _d3d12;
    private readonly LoaderLog _log;
    private readonly RemovalWatch _watch = new();
    private bool _gpuValidation;
    private bool _dredOnly;
    private nint _device;
    private int _reported;

    public D3D12Debugging(D3D12Forwarder d3d12, ImageTable images, LoaderLog log)
    {
        _d3d12 = d3d12;
        _log = log;
        Messages = new InfoQueueLog(images, log);
    }

    public bool Enabled { get; private set; }

    /// <summary>Where the debug layer's messages go.</summary>
    public InfoQueueLog Messages { get; }

    /// <summary>Turns the diagnostics on for the device created next.</summary>
    public void Enable(bool gpuValidation)
    {
        Enabled = true;
        _gpuValidation = gpuValidation;
    }

    /// <summary>DRED and the removal report only (<c>--d3d12-dred</c>): the debug layer slows every call.</summary>
    public void EnableDredOnly()
    {
        Enabled = true;
        _dredOnly = true;
    }

    /// <summary>Before D3D12CreateDevice: the debug layer, when installed, and DRED.</summary>
    public void BeforeDevice()
    {
        if (!Enabled)
        {
            return;
        }

        if (_dredOnly)
        {
            _log.Info("  [d3d12-debug] DRED only, no debug layer");
        }
        else if (File.Exists(Path.Combine(Environment.SystemDirectory, SdkLayers)))
        {
            EnableDebugLayer();
        }
        else
        {
            _log.Warn($"[d3d12-debug] {SdkLayers} is missing: install the Graphics Tools optional feature (Settings > System > Optional features) for the debug layer; DRED stays on");
        }

        EnableDred();
    }

    /// <summary>After D3D12CreateDevice: the message log and the removal watch.</summary>
    public void AttachDevice(nint device)
    {
        if (!Enabled)
        {
            return;
        }

        _device = device;
        Messages.Attach(device);
        int hr = _watch.Start(device, ReportRemoval);
        _log.Info($"  [d3d12-debug] device-removal watch -> {HResult.Format(hr)}");
    }

    /// <summary>A forwarded creation failed: drain the messages and report a removal.</summary>
    public void OnFailure()
    {
        if (!Enabled || _device == 0)
        {
            return;
        }

        Messages.Drain();

        if (HResult.Failed(RemovedReason()))
        {
            ReportRemoval();
        }
    }

    /// <summary>The removal reason, the stored messages and the DRED output, once.</summary>
    public void ReportRemoval()
    {
        if (Interlocked.Exchange(ref _reported, 1) != 0)
        {
            return;
        }

        _log.Error($"[d3d12-debug] device removed: reason {HResult.Format(RemovedReason())}");
        Messages.Drain();
        DredReport.Write(_device, _log);
    }

    public void Dispose()
    {
        Messages.Detach();
        _watch.Dispose();
    }

    private int RemovedReason()
    {
        return ((delegate* unmanaged<nint, int>)ComObject.Slot(_device, RemovedReasonSlot))(_device);
    }

    private void EnableDebugLayer()
    {
        Guid iid = DebugIid;
        nint debug = 0;
        int hr = _d3d12.GetDebugInterface(&iid, &debug);

        if (HResult.Failed(hr))
        {
            _log.Warn($"[d3d12-debug] D3D12GetDebugInterface(ID3D12Debug) failed: {HResult.Format(hr)}; the debug layer stays off");

            return;
        }

        ((delegate* unmanaged<nint, void>)ComObject.Slot(debug, EnableDebugLayerSlot))(debug);
        _log.Info($"  [d3d12-debug] debug layer on, GPU-based validation {_gpuValidation}");

        if (_gpuValidation)
        {
            Guid iid1 = Debug1Iid;
            nint debug1 = 0;

            if (!HResult.Failed(((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(debug, ComObject.QueryInterfaceSlot))(debug, &iid1, &debug1)))
            {
                ((delegate* unmanaged<nint, int, void>)ComObject.Slot(debug1, SetGpuValidationSlot))(debug1, 1);
                ComObject.Release(debug1);
            }
        }

        ComObject.Release(debug);
    }

    private void EnableDred()
    {
        Guid iid = DredSettingsIid;
        nint settings = 0;
        int hr = _d3d12.GetDebugInterface(&iid, &settings);

        if (HResult.Failed(hr))
        {
            _log.Warn($"[d3d12-debug] no DRED settings: {HResult.Format(hr)}");

            return;
        }

        ((delegate* unmanaged<nint, int, void>)ComObject.Slot(settings, SetBreadcrumbsSlot))(settings, DredForcedOn);
        ((delegate* unmanaged<nint, int, void>)ComObject.Slot(settings, SetPageFaultsSlot))(settings, DredForcedOn);
        ComObject.Release(settings);
        _log.Info("  [d3d12-debug] DRED auto-breadcrumbs and page faults on");
    }
}
