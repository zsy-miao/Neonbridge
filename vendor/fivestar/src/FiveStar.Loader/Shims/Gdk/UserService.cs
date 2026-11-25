// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.Input;

namespace FiveStar.Loader.Shims.Gdk;

/// <summary>
/// XUser.h: one local, signed-in, offline user. Every add and find returns the same user; its handle is one pointer
/// with a reference count (XUserDuplicateHandle / XUserCloseHandle).
/// </summary>
internal sealed unsafe class UserService
{
    /// <summary>Made-up XUID of the offline user.</summary>
    public const ulong Xuid = 0x0009_0000_0000_0001;

    public const ulong LocalId = 1;

    public const string Gamertag = "Player";

    /// <summary>The name XUserAddAsync / XUserAddResult share as async identity.</summary>
    public const string AddApi = "XUserAddAsync";

    public const string FindControllerApi = "XUserFindControllerForUserWithUiAsync";

    public const string TokenApi = "XUserGetTokenAndSignatureAsync";

    public const string ResolvePrivilegeApi = "XUserResolvePrivilegeWithUiAsync";

    /// <summary>sizeof(APP_LOCAL_DEVICE_ID).</summary>
    public const int DeviceIdSize = 32;

    // XUserDefaultAudioEndpointKind CommunicationRender and CommunicationCapture.
    private const uint EndpointRender = 0;
    private const uint EndpointCapture = 1;

    // Unconfirmed XUser error HRESULTs (facility 0x924, codes 0x5100 and up); check against XUser.h.
    private const int DeferralNotAvailable = unchecked((int)0x89245103);
    private const int UserNotFound = unchecked((int)0x89245104);

    // XUserState, XUserAgeGroup and XUserGamertagComponent values (XUser.h).
    private const uint StateSignedIn = 0;
    private const uint AgeGroupAdult = 3;
    private const uint GamertagModernSuffix = 2;
    private const uint GamertagUniqueModern = 3;

    /// <summary>XUserPrivilegeDenyReason::None.</summary>
    private const uint DenyReasonNone = 0;

    /// <summary>The handle block: two pointer-sized words, zeroed.</summary>
    private const int HandleSize = 2 * sizeof(ulong);

    private readonly nint _user;
    private long _changeToken;
    private int _refs;

    public UserService()
    {
        _user = (nint)NativeMemory.AllocZeroed(HandleSize);
    }

    public int RefCount => Volatile.Read(ref _refs);

    /// <summary>The device the player's controller is: the GameInput id of the first XInput gamepad.</summary>
    public static ReadOnlySpan<byte> ControllerId => DeviceInfoWriter.DeviceId(InputDeviceCatalog.GamepadName(0));

    public bool IsUser(nint handle)
    {
        return handle == _user;
    }

    /// <summary>A new reference to the user handle.</summary>
    public nint Reference()
    {
        Interlocked.Increment(ref _refs);

        return _user;
    }

    public int Duplicate(nint handle, nint* duplicated)
    {
        if (!IsUser(handle) || duplicated == null)
        {
            return HResult.InvalidArgument;
        }

        *duplicated = Reference();

        return HResult.Ok;
    }

    public void Close(nint handle)
    {
        if (IsUser(handle))
        {
            Interlocked.Decrement(ref _refs);
        }
    }

    public int FindById(ulong id, nint* handle)
    {
        if (handle == null)
        {
            return HResult.Pointer;
        }

        if (id != Xuid)
        {
            return UserNotFound;
        }

        *handle = Reference();

        return HResult.Ok;
    }

    public int FindByLocalId(ulong localId, nint* handle)
    {
        if (handle == null)
        {
            return HResult.Pointer;
        }

        if (localId != LocalId)
        {
            return UserNotFound;
        }

        *handle = Reference();

        return HResult.Ok;
    }

    public int WriteValue(nint user, ulong* destination, ulong value)
    {
        if (!IsUser(user) || destination == null)
        {
            return HResult.InvalidArgument;
        }

        *destination = value;

        return HResult.Ok;
    }

    public int GetIsGuest(nint user, byte* isGuest)
    {
        if (!IsUser(user) || isGuest == null)
        {
            return HResult.InvalidArgument;
        }

        *isGuest = GdkModule.False;

        return HResult.Ok;
    }

    public int GetState(nint user, uint* state)
    {
        return WriteEnum(user, state, StateSignedIn);
    }

    public int GetAgeGroup(nint user, uint* ageGroup)
    {
        return WriteEnum(user, ageGroup, AgeGroupAdult);
    }

    /// <summary>The offline user is unique, so the "#suffix" component is empty.</summary>
    public int GetGamertag(nint user, uint component, nuint size, byte* gamertag, nuint* used)
    {
        if (!IsUser(user) || component > GamertagUniqueModern)
        {
            return HResult.InvalidArgument;
        }

        string text = component == GamertagModernSuffix ? string.Empty : Gamertag;

        return Utf8Text.Write(text, size, gamertag, used);
    }

    /// <summary>Every privilege is granted.</summary>
    public int CheckPrivilege(nint user, uint privilege, byte* hasPrivilege, uint* reason, NativeBoundary boundary)
    {
        if (!IsUser(user) || hasPrivilege == null)
        {
            return HResult.InvalidArgument;
        }

        *hasPrivilege = GdkModule.True;

        if (reason != null)
        {
            *reason = DenyReasonNone;
        }

        boundary.Trace($"XUserCheckPrivilege(privilege {privilege}) -> granted");

        return HResult.Ok;
    }

    /// <summary>Registration succeeds; the event never fires because the user never changes.</summary>
    public int RegisterForChangeEvent(nint callback, ulong* token)
    {
        if (callback == 0 || token == null)
        {
            return HResult.InvalidArgument;
        }

        *token = (ulong)Interlocked.Increment(ref _changeToken);

        return HResult.Ok;
    }

    /// <summary>XUserFindForDevice: every input device belongs to the one player.</summary>
    public int FindForDevice(byte* deviceId, nint* handle)
    {
        if (handle == null)
        {
            return HResult.Pointer;
        }

        if (deviceId == null)
        {
            return HResult.InvalidArgument;
        }

        *handle = Reference();

        return HResult.Ok;
    }

    /// <summary>
    /// XUserGetDefaultAudioEndpointUtf16: the id of the PC's default communications device for that direction, in
    /// the UTF-16 string-out convention; E_NOTFOUND when Windows has none.
    /// </summary>
    public static int GetDefaultAudioEndpoint(ulong localId, uint kind, nuint count, char* buffer, nuint* used)
    {
        if (localId != LocalId)
        {
            return UserNotFound;
        }

        if (kind != EndpointRender && kind != EndpointCapture)
        {
            return HResult.InvalidArgument;
        }

        string? id = AudioEndpointProbe.DefaultCommunications((int)kind);

        if (id is null)
        {
            return HResult.NotFound;
        }

        nuint needed = (nuint)id.Length + 1;

        if (used != null)
        {
            *used = needed;
        }

        if (buffer == null || count < needed)
        {
            return HResult.NotSufficientBuffer;
        }

        id.CopyTo(new Span<char>(buffer, id.Length));
        buffer[id.Length] = '\0';

        return HResult.Ok;
    }

    /// <summary>No sign-out is in progress.</summary>
    public int GetSignOutDeferral(nint* deferral)
    {
        if (deferral != null)
        {
            *deferral = 0;
        }

        return DeferralNotAvailable;
    }

    private int WriteEnum(nint user, uint* destination, uint value)
    {
        if (!IsUser(user) || destination == null)
        {
            return HResult.InvalidArgument;
        }

        *destination = value;

        return HResult.Ok;
    }
}
