// Interface XUser du runtime GDK servie par AnyXboxX : un profil local hors ligne.
//
// Le jeu voit un seul utilisateur, toujours connecté, adulte, avec tous les
// privilèges : identifiant fixe, gamertag réglable (ANYXBOXX_GAMERTAG, défaut
// « AnyXboxX »). Aucune connexion à Xbox Live : les jetons et fenêtres de connexion
// échouent proprement (E_NOTIMPL) ou réussissent à vide ; la photo de profil est un PNG généré.
//
// Objet du fournisseur {01ACD177…} : ses versions d'interface prolongent la même
// table (méthodes 3 à 49, gdk_api_table.inc). XUserGetGamertag a sa propre
// interface {CEF4FAC0…}. Signatures : en-tête public XUser.h du GDK.
#include <windows.h>

#include <cstdint>
#include <cstring>
#include <memory>
#include <new>
#include <utility>
#include <vector>

#include "XAsync.h"
#include "XAsyncProvider.h"
#include "XTaskQueue.h"
#include "async_call.h"
#include "gamer_picture.h"
#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

// --- Types publics de XUser.h, au strict nécessaire -------------------------------
using XUserHandle = struct XUser*;
struct XUserLocalId {
  uint64_t value;
};
enum class XUserState : uint32_t { SignedIn = 0, SigningOut = 1, SignedOut = 2 };
enum class XUserAgeGroup : uint32_t { Unknown = 0, Child = 1, Teen = 2, Adult = 3 };
enum class XUserGamertagComponent : uint32_t { Classic = 0, Modern = 1, ModernSuffix = 2, UniqueModern = 3 };
enum class XUserPrivilegeDenyReason : uint32_t { None = 0 };

constexpr HRESULT E_GAMEUSER_USER_NOT_FOUND = static_cast<HRESULT>(0x89245104L);
constexpr HRESULT E_GAMEUSER_DEFERRAL_NOT_AVAILABLE = static_cast<HRESULT>(0x89245103L);
constexpr HRESULT E_NOT_SUFFICIENT_BUFFER_HR = HRESULT_FROM_WIN32(ERROR_INSUFFICIENT_BUFFER);

// --- Profil local ------------------------------------------------------------------
constexpr uint64_t kXuid = 0x0009000000A11B0Bull;  // identifiant fixe, hors des plages Xbox Live connues
constexpr uint64_t kLocalId = 1;
constexpr uint32_t kMaxUsers = 1;
constexpr size_t kMaxGamertag = 15;  // gamertag « classique »
constexpr uint32_t kHandleMagic = 0x31555841;  // « AXU1 »

const char* Gamertag() {
  static char gamertag[kMaxGamertag + 1] = "AnyXboxX";
  static const bool loaded = [] {
    char value[kMaxGamertag + 1] = {};
    const DWORD length = GetEnvironmentVariableA("ANYXBOXX_GAMERTAG", value, sizeof value);
    if (length > 0 && length <= kMaxGamertag) std::memcpy(gamertag, value, length + 1);
    return true;
  }();
  (void)loaded;
  return gamertag;
}

struct UserHandleData {
  uint32_t magic;
};

XUserHandle NewHandle() {
  return reinterpret_cast<XUserHandle>(new (std::nothrow) UserHandleData{kHandleMagic});
}

bool Valid(XUserHandle user) {
  return user != nullptr && reinterpret_cast<UserHandleData*>(user)->magic == kHandleMagic;
}

// --- Opérations asynchrones : terminées au premier tour de la file -----------------
struct AsyncResult {
  HRESULT result;
  bool returns_user;
  XUserHandle user;  // transmis au jeu par GetResult, fermé sinon
};

HRESULT CALLBACK CompletedProvider(XAsyncOp op, const XAsyncProviderData* data) noexcept {
  auto* state = static_cast<AsyncResult*>(data->context);
  switch (op) {
    case XAsyncOp::Begin:
      return XAsyncSchedule(data->async, 0);
    case XAsyncOp::DoWork:
      XAsyncComplete(data->async, state->result, state->returns_user ? sizeof(XUserHandle) : 0);
      return S_OK;
    case XAsyncOp::GetResult:
      std::memcpy(data->buffer, &state->user, sizeof(XUserHandle));
      state->user = nullptr;
      return S_OK;
    case XAsyncOp::Cleanup:
      if (state->user != nullptr) delete reinterpret_cast<UserHandleData*>(state->user);
      delete state;
      return S_OK;
    default:
      return S_OK;
  }
}

HRESULT BeginCompleted(XAsyncBlock* async, const void* identity, const char* name, HRESULT result,
                       bool returns_user) {
  auto* state = new (std::nothrow) AsyncResult{result, returns_user, returns_user ? NewHandle() : nullptr};
  if (state == nullptr || (returns_user && state->user == nullptr)) {
    if (state != nullptr) delete state;
    return E_OUTOFMEMORY;
  }
  const HRESULT begun = XAsyncBegin(async, state, identity, name, CompletedProvider);
  if (FAILED(begun)) {
    delete reinterpret_cast<UserHandleData*>(state->user);
    delete state;
  }
  return begun;
}

const char kAddIdentity = 0;
const char kAddByIdIdentity = 0;
const char kEmptyIdentity = 0;

// --- Fonctions de l'interface ----------------------------------------------------------
HRESULT XUserDuplicateHandle(XUserHandle user, XUserHandle* duplicated) noexcept {
  if (!Valid(user) || duplicated == nullptr) return E_INVALIDARG;
  *duplicated = NewHandle();
  return *duplicated != nullptr ? S_OK : E_OUTOFMEMORY;
}

void XUserCloseHandle(XUserHandle user) noexcept {
  if (!Valid(user)) return;
  reinterpret_cast<UserHandleData*>(user)->magic = 0;
  delete reinterpret_cast<UserHandleData*>(user);
}

int32_t XUserCompare(XUserHandle first, XUserHandle second) noexcept {
  // Un seul utilisateur : deux handles valides désignent la même personne.
  if (Valid(first) && Valid(second)) return 0;
  return first < second ? -1 : (first == second ? 0 : 1);
}

HRESULT XUserGetMaxUsers(uint32_t* max_users) noexcept {
  if (max_users == nullptr) return E_INVALIDARG;
  *max_users = kMaxUsers;
  return S_OK;
}

HRESULT XUserAddAsync(uint32_t options, XAsyncBlock* async) noexcept {
  ANYXBOXX_TRACE("[runtime] XUserAddAsync (options 0x%x) -> profil local « %s »", options, Gamertag());
  return BeginCompleted(async, &kAddIdentity, "XUserAddAsync", S_OK, true);
}

HRESULT XUserAddResult(XAsyncBlock* async, XUserHandle* user) noexcept {
  return XAsyncGetResult(async, &kAddIdentity, sizeof(XUserHandle), user, nullptr);
}

HRESULT XUserGetLocalId(XUserHandle user, XUserLocalId* local_id) noexcept {
  if (!Valid(user) || local_id == nullptr) return E_INVALIDARG;
  local_id->value = kLocalId;
  return S_OK;
}

HRESULT XUserFindUserByLocalId(XUserLocalId local_id, XUserHandle* user) noexcept {
  if (user == nullptr) return E_INVALIDARG;
  if (local_id.value != kLocalId) return E_GAMEUSER_USER_NOT_FOUND;
  *user = NewHandle();
  return *user != nullptr ? S_OK : E_OUTOFMEMORY;
}

HRESULT XUserGetId(XUserHandle user, uint64_t* id) noexcept {
  if (!Valid(user) || id == nullptr) return E_INVALIDARG;
  *id = kXuid;
  return S_OK;
}

HRESULT XUserFindUserById(uint64_t id, XUserHandle* user) noexcept {
  if (user == nullptr) return E_INVALIDARG;
  if (id != kXuid) return E_GAMEUSER_USER_NOT_FOUND;
  *user = NewHandle();
  return *user != nullptr ? S_OK : E_OUTOFMEMORY;
}

HRESULT XUserGetIsGuest(XUserHandle user, bool* is_guest) noexcept {
  if (!Valid(user) || is_guest == nullptr) return E_INVALIDARG;
  *is_guest = false;
  return S_OK;
}

HRESULT XUserGetState(XUserHandle user, XUserState* state) noexcept {
  if (!Valid(user) || state == nullptr) return E_INVALIDARG;
  *state = XUserState::SignedIn;
  return S_OK;
}

HRESULT XUserGetGamertag(XUserHandle user, XUserGamertagComponent component, size_t size, char* buffer,
                         size_t* used) noexcept {
  if (!Valid(user)) return E_INVALIDARG;
  const char* text = component == XUserGamertagComponent::ModernSuffix ? "" : Gamertag();
  const size_t needed = std::strlen(text) + 1;
  if (used != nullptr) *used = needed;
  if (buffer == nullptr || size < needed) return E_NOT_SUFFICIENT_BUFFER_HR;
  std::memcpy(buffer, text, needed);
  return S_OK;
}

// Photo de profil hors ligne : PNG généré à la taille demandée (gamer_picture.cpp).
const char kPictureIdentity = 0;

HRESULT XUserGetGamerPictureAsync(XUserHandle user, uint32_t size_code, XAsyncBlock* async) noexcept {
  uint32_t side = 0;
  if (!Valid(user) || async == nullptr || !anyxboxx::GamerPictureSide(size_code, &side)) return E_INVALIDARG;
  try {
    auto png = std::make_shared<const std::vector<uint8_t>>(anyxboxx::GamerPicturePng(side));
    anyxboxx::AsyncCall call;
    call.size = png->size();
    call.write = [png](void* buffer) { std::memcpy(buffer, png->data(), png->size()); };
    return anyxboxx::BeginAsyncCall(async, &kPictureIdentity, "XUserGetGamerPictureAsync", std::move(call));
  } catch (const std::bad_alloc&) {
    return E_OUTOFMEMORY;
  }
}

HRESULT XUserGetGamerPictureResultSize(XAsyncBlock* async, size_t* size) noexcept {
  if (size == nullptr) return E_INVALIDARG;
  return XAsyncGetResultSize(async, size);
}

HRESULT XUserGetGamerPictureResult(XAsyncBlock* async, size_t size, void* buffer, size_t* used) noexcept {
  return XAsyncGetResult(async, &kPictureIdentity, size, buffer, used);
}

HRESULT XUserGetAgeGroup(XUserHandle user, XUserAgeGroup* age_group) noexcept {
  if (!Valid(user) || age_group == nullptr) return E_INVALIDARG;
  *age_group = XUserAgeGroup::Adult;
  return S_OK;
}

HRESULT XUserCheckPrivilege(XUserHandle user, uint32_t, uint32_t privilege, bool* has_privilege,
                            XUserPrivilegeDenyReason* reason) noexcept {
  if (!Valid(user) || has_privilege == nullptr) return E_INVALIDARG;
  ANYXBOXX_TRACE("[runtime] XUserCheckPrivilege(%u) -> accordé", privilege);
  *has_privilege = true;
  if (reason != nullptr) *reason = XUserPrivilegeDenyReason::None;
  return S_OK;
}

HRESULT XUserResolvePrivilegeWithUiAsync(XUserHandle, uint32_t, uint32_t, XAsyncBlock* async) noexcept {
  return BeginCompleted(async, &kEmptyIdentity, "XUserResolvePrivilegeWithUiAsync", S_OK, false);
}

HRESULT XUserEmptyResult(XAsyncBlock* async) noexcept { return XAsyncGetStatus(async, false); }

// Jetons Xbox Live : impossibles hors ligne.
HRESULT XUserGetTokenAndSignatureAsync(XUserHandle, uint32_t, const char*, const char*, size_t, const void*,
                                       size_t, const void*, XAsyncBlock*) noexcept {
  return E_NOTIMPL;
}
HRESULT XUserGetTokenAndSignatureUtf16Async(XUserHandle, uint32_t, const wchar_t*, const wchar_t*, size_t,
                                            const void*, size_t, const void*, XAsyncBlock*) noexcept {
  return E_NOTIMPL;
}
HRESULT XUserTokenResultSize(XAsyncBlock*, size_t*) noexcept { return E_NOTIMPL; }
HRESULT XUserTokenResult(XAsyncBlock*, size_t, void*, void**, size_t*) noexcept { return E_NOTIMPL; }

HRESULT XUserResolveIssueWithUiAsync(XUserHandle, const char*, XAsyncBlock* async) noexcept {
  return BeginCompleted(async, &kEmptyIdentity, "XUserResolveIssueWithUiAsync", S_OK, false);
}
HRESULT XUserResolveIssueWithUiUtf16Async(XUserHandle, const wchar_t*, XAsyncBlock* async) noexcept {
  return BeginCompleted(async, &kEmptyIdentity, "XUserResolveIssueWithUiUtf16Async", S_OK, false);
}

// Le profil ne change jamais : l'abonnement est accepté, aucun événement n'arrive.
HRESULT XUserRegisterForChangeEvent(XTaskQueueHandle, void*, void*, XTaskQueueRegistrationToken* token) noexcept {
  static volatile LONG64 next_token = 0;
  if (token == nullptr) return E_INVALIDARG;
  token->token = static_cast<uint64_t>(InterlockedIncrement64(&next_token));
  return S_OK;
}
bool XUserUnregisterForChangeEvent(XTaskQueueRegistrationToken, bool) noexcept { return true; }

HRESULT XUserGetSignOutDeferral(void** deferral) noexcept {
  if (deferral != nullptr) *deferral = nullptr;
  return E_GAMEUSER_DEFERRAL_NOT_AVAILABLE;
}
void XUserCloseSignOutDeferralHandle(void*) noexcept {}

HRESULT XUserAddByIdWithUiAsync(uint64_t id, XAsyncBlock* async) noexcept {
  return BeginCompleted(async, &kAddByIdIdentity, "XUserAddByIdWithUiAsync",
                        id == kXuid ? S_OK : E_GAMEUSER_USER_NOT_FOUND, id == kXuid);
}
HRESULT XUserAddByIdWithUiResult(XAsyncBlock* async, XUserHandle* user) noexcept {
  return XAsyncGetResult(async, &kAddByIdIdentity, sizeof(XUserHandle), user, nullptr);
}

bool XUserIsStoreUser(XUserHandle) noexcept { return false; }

// Invites de connexion à distance de la plateforme : rien à afficher hors ligne.
HRESULT XUserPlatformRemoteConnectSetEventHandlers(XTaskQueueHandle, void*) noexcept { return S_OK; }
HRESULT XUserPlatformRemoteConnectCancelPrompt(void*) noexcept { return S_OK; }
HRESULT XUserPlatformSpopPromptSetEventHandlers(XTaskQueueHandle, void*, void*) noexcept { return S_OK; }
HRESULT XUserPlatformSpopPromptComplete(void*, uint32_t) noexcept { return S_OK; }

bool XUserIsSignOutPresent() noexcept { return false; }
HRESULT XUserSignOutAsync(XUserHandle, XAsyncBlock* async) noexcept {
  return BeginCompleted(async, &kEmptyIdentity, "XUserSignOutAsync", S_OK, false);
}

void* const kUserMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,                                  // 0-2
    ANYXBOXX_METHOD(XUserDuplicateHandle),                      // 3
    ANYXBOXX_METHOD(XUserCloseHandle),                          // 4
    ANYXBOXX_METHOD(XUserCompare),                              // 5
    ANYXBOXX_METHOD(XUserGetMaxUsers),                          // 6
    ANYXBOXX_METHOD(XUserAddAsync),                             // 7
    ANYXBOXX_METHOD(XUserAddResult),                            // 8
    ANYXBOXX_METHOD(XUserGetLocalId),                           // 9
    ANYXBOXX_METHOD(XUserFindUserByLocalId),                    // 10
    ANYXBOXX_METHOD(XUserGetId),                                // 11
    ANYXBOXX_METHOD(XUserFindUserById),                         // 12
    ANYXBOXX_METHOD(XUserGetIsGuest),                           // 13
    ANYXBOXX_METHOD(XUserGetState),                             // 14
    ANYXBOXX_NOT_IMPLEMENTED,                                   // 15 : aucune fonction publique
    ANYXBOXX_METHOD(XUserGetGamerPictureAsync),                 // 16
    ANYXBOXX_METHOD(XUserGetGamerPictureResultSize),            // 17
    ANYXBOXX_METHOD(XUserGetGamerPictureResult),                // 18
    ANYXBOXX_METHOD(XUserGetAgeGroup),                          // 19
    ANYXBOXX_METHOD(XUserCheckPrivilege),                       // 20
    ANYXBOXX_METHOD(XUserResolvePrivilegeWithUiAsync),          // 21
    ANYXBOXX_METHOD(XUserEmptyResult),                          // 22 XUserResolvePrivilegeWithUiResult
    ANYXBOXX_METHOD(XUserGetTokenAndSignatureAsync),            // 23
    ANYXBOXX_METHOD(XUserTokenResultSize),                      // 24
    ANYXBOXX_METHOD(XUserTokenResult),                          // 25
    ANYXBOXX_METHOD(XUserGetTokenAndSignatureUtf16Async),       // 26
    ANYXBOXX_METHOD(XUserTokenResultSize),                      // 27
    ANYXBOXX_METHOD(XUserTokenResult),                          // 28
    ANYXBOXX_METHOD(XUserResolveIssueWithUiAsync),              // 29
    ANYXBOXX_METHOD(XUserEmptyResult),                          // 30 XUserResolveIssueWithUiResult
    ANYXBOXX_METHOD(XUserResolveIssueWithUiUtf16Async),         // 31
    ANYXBOXX_METHOD(XUserEmptyResult),                          // 32 XUserResolveIssueWithUiUtf16Result
    ANYXBOXX_METHOD(XUserRegisterForChangeEvent),               // 33
    ANYXBOXX_METHOD(XUserUnregisterForChangeEvent),             // 34
    ANYXBOXX_METHOD(XUserGetSignOutDeferral),                   // 35
    ANYXBOXX_METHOD(XUserCloseSignOutDeferralHandle),           // 36
    ANYXBOXX_METHOD(XUserAddByIdWithUiAsync),                   // 37
    ANYXBOXX_METHOD(XUserAddByIdWithUiResult),                  // 38
    ANYXBOXX_NOT_IMPLEMENTED,                                   // 39 : aucune fonction publique
    ANYXBOXX_NOT_IMPLEMENTED,                                   // 40
    ANYXBOXX_NOT_IMPLEMENTED,                                   // 41
    ANYXBOXX_METHOD(XUserIsStoreUser),                          // 42
    ANYXBOXX_METHOD(XUserPlatformRemoteConnectSetEventHandlers),  // 43
    ANYXBOXX_METHOD(XUserPlatformRemoteConnectCancelPrompt),      // 44
    ANYXBOXX_METHOD(XUserPlatformSpopPromptSetEventHandlers),     // 45
    ANYXBOXX_METHOD(XUserPlatformSpopPromptComplete),             // 46
    ANYXBOXX_METHOD(XUserIsSignOutPresent),                     // 47
    ANYXBOXX_METHOD(XUserSignOutAsync),                         // 48
    ANYXBOXX_METHOD(XUserEmptyResult),                          // 49 XUserSignOutResult
};

void* const kGamertagMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,          // 0-2
    ANYXBOXX_METHOD(XUserGetGamertag),  // 3
};

const anyxboxx::StaticInterface kUser{kUserMethods};
const anyxboxx::StaticInterface kGamertag{kGamertagMethods};

}  // namespace

namespace anyxboxx {

void* UserInterface() { return const_cast<StaticInterface*>(&kUser); }

bool LocalUserId(const void* user, uint64_t* xuid) {
  if (!Valid(static_cast<XUserHandle>(const_cast<void*>(user))) || xuid == nullptr) return false;
  *xuid = kXuid;
  return true;
}
void* GamertagInterface() { return const_cast<StaticInterface*>(&kGamertag); }

}  // namespace anyxboxx
