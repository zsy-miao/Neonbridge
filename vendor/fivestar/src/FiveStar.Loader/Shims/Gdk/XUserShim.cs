// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XUser.h entry points; the work is in <see cref="UserService"/>. A token request fails (no Xbox services offline);
/// picture and the other UI functions are in the not-implemented catalog.
/// </summary>
internal static unsafe partial class XUserShim
{
    /// <summary>HRESULT XUserAddAsync(XUserAddOptions options, XAsyncBlock* async).</summary>
    [Shim(GdkModule.Name, "XUserAddAsync")]
    [UnmanagedCallersOnly]
    private static int XUserAddAsync(uint options, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return gdk.Async.BeginManaged(async, UserService.AddApi, new UserAddOperation(gdk.Users, gdk.Boundary, options));
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XUserAddAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserAddAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserAddResult(XAsyncBlock* async, XUserHandle* newUser).</summary>
    [Shim(GdkModule.Name, "XUserAddResult")]
    [UnmanagedCallersOnly]
    private static int XUserAddResult(nint async, nint* newUser)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (newUser == null)
            {
                return HResult.Pointer;
            }

            try
            {
                return gdk.Async.ManagedResult(async, UserService.AddApi, (nuint)sizeof(nint), (nint)newUser);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XUserAddResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserAddResult");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserDuplicateHandle")]
    [UnmanagedCallersOnly]
    private static int XUserDuplicateHandle(nint handle, nint* duplicated)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.Duplicate(handle, duplicated);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserDuplicateHandle");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserCloseHandle")]
    [UnmanagedCallersOnly]
    private static void XUserCloseHandle(nint user)
    {
        try
        {
            ShimContext.Runtime.Gdk.Users.Close(user);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserCloseHandle");
        }
    }

    /// <summary>int32_t XUserCompare(XUserHandle user1, XUserHandle user2): ordered by pointer.</summary>
    [Shim(GdkModule.Name, "XUserCompare")]
    [UnmanagedCallersOnly]
    private static int XUserCompare(nint first, nint second)
    {
        try
        {
            return ((ulong)first).CompareTo((ulong)second);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserCompare");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserGetMaxUsers")]
    [UnmanagedCallersOnly]
    private static int XUserGetMaxUsers(uint* maxUsers)
    {
        if (maxUsers == null)
        {
            return HResult.Pointer;
        }

        *maxUsers = 1;

        return HResult.Ok;
    }

    [Shim(GdkModule.Name, "XUserGetId")]
    [UnmanagedCallersOnly]
    private static int XUserGetId(nint user, ulong* id)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.WriteValue(user, id, UserService.Xuid);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetId");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserFindUserById")]
    [UnmanagedCallersOnly]
    private static int XUserFindUserById(ulong id, nint* handle)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.FindById(id, handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserFindUserById");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserGetLocalId(XUserHandle user, XUserLocalId* userLocalId); XUserLocalId is one uint64_t.</summary>
    [Shim(GdkModule.Name, "XUserGetLocalId")]
    [UnmanagedCallersOnly]
    private static int XUserGetLocalId(nint user, ulong* localId)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.WriteValue(user, localId, UserService.LocalId);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetLocalId");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserFindUserByLocalId(XUserLocalId userLocalId, XUserHandle* handle): the 8-byte struct travels by value.</summary>
    [Shim(GdkModule.Name, "XUserFindUserByLocalId")]
    [UnmanagedCallersOnly]
    private static int XUserFindUserByLocalId(ulong localId, nint* handle)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.FindByLocalId(localId, handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserFindUserByLocalId");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserGetIsGuest")]
    [UnmanagedCallersOnly]
    private static int XUserGetIsGuest(nint user, byte* isGuest)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.GetIsGuest(user, isGuest);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetIsGuest");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserGetState")]
    [UnmanagedCallersOnly]
    private static int XUserGetState(nint user, uint* state)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.GetState(user, state);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetState");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserGetGamertag(XUserHandle, XUserGamertagComponent, size_t gamertagSize, char* gamertag, size_t* gamertagUsed).</summary>
    [Shim(GdkModule.Name, "XUserGetGamertag")]
    [UnmanagedCallersOnly]
    private static int XUserGetGamertag(nint user, uint component, nuint size, byte* gamertag, nuint* used)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.GetGamertag(user, component, size, gamertag, used);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetGamertag");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserGetAgeGroup")]
    [UnmanagedCallersOnly]
    private static int XUserGetAgeGroup(nint user, uint* ageGroup)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.GetAgeGroup(user, ageGroup);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetAgeGroup");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserCheckPrivilege(XUserHandle, XUserPrivilegeOptions, XUserPrivilege, bool* hasPrivilege, XUserPrivilegeDenyReason* reason).</summary>
    [Shim(GdkModule.Name, "XUserCheckPrivilege")]
    [UnmanagedCallersOnly]
    private static int XUserCheckPrivilege(nint user, uint options, uint privilege, byte* hasPrivilege, uint* reason)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            return gdk.Users.CheckPrivilege(user, privilege, hasPrivilege, reason, gdk.Boundary);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserCheckPrivilege");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserRegisterForChangeEvent(XTaskQueueHandle, void* context, XUserChangeEventCallback*, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XUserRegisterForChangeEvent")]
    [UnmanagedCallersOnly]
    private static int XUserRegisterForChangeEvent(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.RegisterForChangeEvent(callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserRegisterForChangeEvent");

            return HResult.Fail;
        }
    }

    /// <summary>bool XUserUnregisterForChangeEvent(XTaskQueueRegistrationToken token, bool wait).</summary>
    [Shim(GdkModule.Name, "XUserUnregisterForChangeEvent")]
    [UnmanagedCallersOnly]
    private static byte XUserUnregisterForChangeEvent(ulong token, byte wait)
    {
        return GdkModule.True;
    }

    /// <summary>HRESULT XUserFindForDevice(const APP_LOCAL_DEVICE_ID* deviceId, XUserHandle* handle).</summary>
    [Shim(GdkModule.Name, "XUserFindForDevice")]
    [UnmanagedCallersOnly]
    private static int XUserFindForDevice(byte* deviceId, nint* handle)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.FindForDevice(deviceId, handle);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserFindForDevice");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserFindControllerForUserWithUiAsync(XUserHandle user, XAsyncBlock* async).</summary>
    [Shim(GdkModule.Name, UserService.FindControllerApi)]
    [UnmanagedCallersOnly]
    private static int XUserFindControllerForUserWithUiAsync(nint user, nint async)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (!gdk.Users.IsUser(user))
            {
                return HResult.InvalidArgument;
            }

            try
            {
                return gdk.Async.BeginManaged(async, UserService.FindControllerApi, new ControllerPairOperation());
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XUserFindControllerForUserWithUiAsync), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserFindControllerForUserWithUiAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserFindControllerForUserWithUiResult(XAsyncBlock* async, APP_LOCAL_DEVICE_ID* deviceId).</summary>
    [Shim(GdkModule.Name, "XUserFindControllerForUserWithUiResult")]
    [UnmanagedCallersOnly]
    private static int XUserFindControllerForUserWithUiResult(nint async, byte* deviceId)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            if (deviceId == null)
            {
                return HResult.Pointer;
            }

            try
            {
                return gdk.Async.ManagedResult(async, UserService.FindControllerApi, UserService.DeviceIdSize, (nint)deviceId);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XUserFindControllerForUserWithUiResult), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserFindControllerForUserWithUiResult");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserGetDefaultAudioEndpointUtf16(XUserLocalId, XUserDefaultAudioEndpointKind, size_t count, wchar_t* id, size_t* used).</summary>
    [Shim(GdkModule.Name, "XUserGetDefaultAudioEndpointUtf16")]
    [UnmanagedCallersOnly]
    private static int XUserGetDefaultAudioEndpointUtf16(ulong localId, uint kind, nuint count, char* endpoint, nuint* used)
    {
        try
        {
            var gdk = ShimContext.Runtime.Gdk;

            try
            {
                return UserService.GetDefaultAudioEndpoint(localId, kind, count, endpoint, used);
            }
            catch (Exception e)
            {
                return gdk.Boundary.Failed(nameof(XUserGetDefaultAudioEndpointUtf16), e);
            }
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetDefaultAudioEndpointUtf16");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserRegisterForDeviceAssociationChanged(XTaskQueueHandle, void* context, callback, XTaskQueueRegistrationToken*).</summary>
    [Shim(GdkModule.Name, "XUserRegisterForDeviceAssociationChanged")]
    [UnmanagedCallersOnly]
    private static int XUserRegisterForDeviceAssociationChanged(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XUserRegisterForDeviceAssociationChanged", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserRegisterForDeviceAssociationChanged");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserRegisterForDefaultAudioEndpointUtf16Changed")]
    [UnmanagedCallersOnly]
    private static int XUserRegisterForDefaultAudioEndpointUtf16Changed(nint queue, nint context, nint callback, ulong* token)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Register("XUserRegisterForDefaultAudioEndpointUtf16Changed", callback, token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserRegisterForDefaultAudioEndpointUtf16Changed");

            return HResult.Fail;
        }
    }

    /// <summary>bool XUserUnregisterFor*(XTaskQueueRegistrationToken token, bool wait).</summary>
    [Shim(GdkModule.Name, "XUserUnregisterForDeviceAssociationChanged")]
    [Shim(GdkModule.Name, "XUserUnregisterForDefaultAudioEndpointUtf16Changed")]
    [UnmanagedCallersOnly]
    private static byte XUserUnregisterForEvent(ulong token, byte wait)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Registrations.Unregister(token);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserUnregisterForEvent");

            return default;
        }
    }

    /// <summary>bool XUserIsSignOutPresent(): the offline user never signs out.</summary>
    [Shim(GdkModule.Name, "XUserIsSignOutPresent")]
    [UnmanagedCallersOnly]
    private static byte XUserIsSignOutPresent()
    {
        return GdkModule.False;
    }

    [Shim(GdkModule.Name, "XUserGetSignOutDeferral")]
    [UnmanagedCallersOnly]
    private static int XUserGetSignOutDeferral(nint* deferral)
    {
        try
        {
            return ShimContext.Runtime.Gdk.Users.GetSignOutDeferral(deferral);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetSignOutDeferral");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserCloseSignOutDeferralHandle")]
    [UnmanagedCallersOnly]
    private static void XUserCloseSignOutDeferralHandle(nint deferral)
    {
    }

    /// <summary>
    /// HRESULT XUserGetTokenAndSignatureAsync(XUserHandle, options, const char* method, const char* url, size_t
    /// headerCount, headers, size_t bodySize, body, XAsyncBlock*): the offline user has no Xbox services token.
    /// </summary>
    [Shim(GdkModule.Name, UserService.TokenApi)]
    [UnmanagedCallersOnly]
    private static int XUserGetTokenAndSignatureAsync(nint user, uint options, byte* method, byte* url, nuint headerCount, nint headers, nuint bodySize, nint body, nint async)
    {
        try
        {
            return BeginUserCall(user, async, UserService.TokenApi, new OfflineOperation(HResult.NetworkUnreachable));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetTokenAndSignatureAsync");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserGetTokenAndSignatureResultSize(XAsyncBlock*, size_t* bufferSize).</summary>
    [Shim(GdkModule.Name, "XUserGetTokenAndSignatureResultSize")]
    [UnmanagedCallersOnly]
    private static int XUserGetTokenAndSignatureResultSize(nint async, nuint* size)
    {
        try
        {
            return AsyncCalls.EmptyCount(async, size);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetTokenAndSignatureResultSize");

            return HResult.Fail;
        }
    }

    /// <summary>HRESULT XUserGetTokenAndSignatureResult(XAsyncBlock*, size_t bufferSize, void* buffer, XUserGetTokenAndSignatureData** ptrToBuffer, size_t* bufferUsed).</summary>
    [Shim(GdkModule.Name, "XUserGetTokenAndSignatureResult")]
    [UnmanagedCallersOnly]
    private static int XUserGetTokenAndSignatureResult(nint async, nuint size, nint buffer, nint* data, nuint* used)
    {
        try
        {
            if (data != null)
            {
                *data = 0;
            }

            if (used != null)
            {
                *used = 0;
            }

            return AsyncCalls.Result(async, UserService.TokenApi, size, buffer);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserGetTokenAndSignatureResult");

            return HResult.Fail;
        }
    }

    /// <summary>
    /// HRESULT XUserResolvePrivilegeWithUiAsync(XUserHandle, XUserPrivilegeOptions, XUserPrivilege, XAsyncBlock*):
    /// XUserCheckPrivilege grants every privilege, so there is nothing to resolve and no UI; S_OK.
    /// </summary>
    [Shim(GdkModule.Name, UserService.ResolvePrivilegeApi)]
    [UnmanagedCallersOnly]
    private static int XUserResolvePrivilegeWithUiAsync(nint user, uint options, uint privilege, nint async)
    {
        try
        {
            return BeginUserCall(user, async, UserService.ResolvePrivilegeApi, new OfflineOperation(HResult.Ok));
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserResolvePrivilegeWithUiAsync");

            return HResult.Fail;
        }
    }

    [Shim(GdkModule.Name, "XUserResolvePrivilegeWithUiResult")]
    [UnmanagedCallersOnly]
    private static int XUserResolvePrivilegeWithUiResult(nint async)
    {
        try
        {
            return AsyncCalls.Result(async, UserService.ResolvePrivilegeApi, 0, 0);
        }
        catch (Exception exception)
        {
            UnmanagedGuard.Caught(exception, "XUserShim.XUserResolvePrivilegeWithUiResult");

            return HResult.Fail;
        }
    }

    private static int BeginUserCall(nint user, nint async, string api, ManagedAsyncOperation operation)
    {
        if (!ShimContext.Runtime.Gdk.Users.IsUser(user))
        {
            return HResult.InvalidArgument;
        }

        return AsyncCalls.Begin(async, api, operation);
    }
}
