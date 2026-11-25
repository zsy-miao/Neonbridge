// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// The PC's default communications endpoints through the MMDevice API (mmdeviceapi.h), by raw vtable slots: the id
/// string IMMDevice::GetId returns, which XAudio2 and WASAPI accept as a device id.
/// </summary>
internal static unsafe class AudioEndpointProbe
{
    // IMMDeviceEnumerator::GetDefaultAudioEndpoint and IMMDevice::GetId.
    private const int GetDefaultEndpointSlot = 4;
    private const int GetIdSlot = 5;

    /// <summary>ERole eCommunications.</summary>
    private const int CommunicationsRole = 2;

    private static readonly Guid EnumeratorClass = new("bcde0395-e52f-467c-8e3d-c4579291692e");
    private static readonly Guid EnumeratorInterface = new("a95664d2-9614-4f35-a746-de8db63617e6");

    /// <summary>The endpoint id for EDataFlow <paramref name="flow"/> (0 render, 1 capture), or null when there is none.</summary>
    public static string? DefaultCommunications(int flow)
    {
        int init = Ole32.CoInitializeEx(0, Ole32.MultiThreaded);

        try
        {
            return Query(flow);
        }
        finally
        {
            // S_OK and S_FALSE took a reference; RPC_E_CHANGED_MODE (an STA thread) did not.
            if (init >= 0)
            {
                Ole32.CoUninitialize();
            }
        }
    }

    private static string? Query(int flow)
    {
        Guid clsid = EnumeratorClass;
        Guid iid = EnumeratorInterface;
        nint enumerator = 0;
        nint device = 0;

        if (Ole32.CoCreateInstance(&clsid, 0, Ole32.AllContexts, &iid, &enumerator) < 0)
        {
            return null;
        }

        try
        {
            var getDefault = (delegate* unmanaged<nint, int, int, nint*, int>)ComObject.Slot(enumerator, GetDefaultEndpointSlot);

            if (getDefault(enumerator, flow, CommunicationsRole, &device) < 0)
            {
                return null;
            }

            return Id(device);
        }
        finally
        {
            ComObject.Release(device);
            ComObject.Release(enumerator);
        }
    }

    private static string? Id(nint device)
    {
        char* id = null;

        if (((delegate* unmanaged<nint, char**, int>)ComObject.Slot(device, GetIdSlot))(device, &id) < 0)
        {
            return null;
        }

        string? text = Marshal.PtrToStringUni((nint)id);
        Ole32.CoTaskMemFree(id);

        return text;
    }
}
