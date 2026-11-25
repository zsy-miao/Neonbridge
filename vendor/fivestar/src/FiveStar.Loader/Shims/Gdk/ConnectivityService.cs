// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.Network;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XNetworkingGetConnectivityHint and its change event as <c>--network</c> sets them. Offline the level is None and
/// change callbacks never run; sealed online the level is InternetAccess (or <c>--network-level</c>) and each change
/// callback runs once, on the completion port of its queue, right after it registers. The network stack always
/// reports itself initialized, with unrestricted cost.
/// </summary>
internal sealed unsafe class ConnectivityService
{
    private const string RegisterApi = "XNetworkingRegisterConnectivityHintChanged";

    /// <summary>XNetworkingConnectivityCostHint::Unrestricted.</summary>
    private const uint CostUnrestricted = 1;

    private readonly NetworkOptions _options;
    private readonly EventRegistrations _registrations;
    private readonly TaskQueueService _taskQueues;
    private readonly NativeBoundary _boundary;

    public ConnectivityService(NetworkOptions options, EventRegistrations registrations, TaskQueueService taskQueues, NativeBoundary boundary)
    {
        _options = options;
        _registrations = registrations;
        _taskQueues = taskQueues;
        _boundary = boundary;
    }

    public ConnectivityLevel Level => _options.ReportedLevel;

    public int GetHint(byte* hint)
    {
        if (hint == null)
        {
            return HResult.Pointer;
        }

        new Span<byte>(hint, NetworkingService.HintSize).Clear();
        *(uint*)(hint + NetworkingService.HintLevelOffset) = (uint)Level;
        *(uint*)(hint + NetworkingService.HintCostOffset) = CostUnrestricted;
        hint[NetworkingService.HintInitializedOffset] = GdkModule.True;

        return HResult.Ok;
    }

    /// <summary>XNetworkingRegisterConnectivityHintChanged(queue, context, callback, token).</summary>
    public int Register(nint queue, nint context, nint callback, ulong* token)
    {
        int result = _registrations.Register(RegisterApi, callback, token);

        if (HResult.Failed(result) || _options.Mode != NetworkMode.SealedOnline)
        {
            return result;
        }

        var target = _taskQueues.ResolveQueue(queue);

        if (target is null)
        {
            _boundary.Trace($"{RegisterApi}: queue 0x{queue:x} unknown, the change callback does not run");

            return result;
        }

        _ = target.Completion.Submit(new TaskEntry(0, 0, new ConnectivityNotification(this, callback, context)), target, 0);
        _boundary.Trace($"{RegisterApi}: change callback queued once (level {Level})");

        return result;
    }
}
