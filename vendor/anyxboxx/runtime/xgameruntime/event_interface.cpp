// Abonnements hors ligne (invitations, activations, protocoles) et rapport d'erreurs XError.
//
// Méthodes et interfaces : gdk_api_table.inc. Signatures : en-têtes publics XGameInvite.h,
// XGameActivation.h, XGameProtocol.h et XError.h du GDK (Microsoft Learn).
// Sans Xbox Live, aucune invitation ni activation n'arrive : l'abonnement réussit avec un
// jeton unique et le rappel ne part jamais. Accepter une invitation reste non implémenté.
#include <windows.h>

#include <cstdint>
#include <mutex>
#include <unordered_set>

#include "interface_helpers.h"
#include "interfaces.h"
#include "trace.h"

namespace {

// XTaskQueueRegistrationToken : structure d'un seul uint64_t, passée par valeur.
struct RegistrationToken {
  uint64_t token;
};

class Subscriptions {
 public:
  HRESULT Add(const void* callback, RegistrationToken* token) {
    if (callback == nullptr || token == nullptr) return E_INVALIDARG;
    std::lock_guard<std::mutex> lock(mutex_);
    token->token = ++last_;
    live_.insert(last_);
    return S_OK;
  }
  bool Remove(RegistrationToken token) {
    std::lock_guard<std::mutex> lock(mutex_);
    return live_.erase(token.token) == 1;
  }

 private:
  std::mutex mutex_;
  uint64_t last_ = 0;
  std::unordered_set<uint64_t> live_;
};

Subscriptions& Registry() {
  static Subscriptions registry;
  return registry;
}

// --- Abonnements : (file, contexte, rappel, jeton) puis (jeton, attendre) ---------------
HRESULT Subscribe(void*, void*, void* callback, RegistrationToken* token) noexcept {
  return Registry().Add(callback, token);
}
bool Unsubscribe(RegistrationToken token, bool) noexcept { return Registry().Remove(token); }

// --- XError : rappel du jeu et options (aucun arrêt forcé sous AnyXboxX) ----------------
using ErrorCallback = bool(CALLBACK*)(HRESULT, const char*, void*);

struct ErrorState {
  std::mutex mutex;
  ErrorCallback callback = nullptr;
  void* context = nullptr;
};

ErrorState& Errors() {
  static ErrorState state;
  return state;
}

// XErrorReport n'est pas déclaré dans XError.h (seulement cité). Signature d'après xgameruntime-docs
// (GPL-3.0, faits seulement) : HRESULT XErrorReport(HRESULT, const char*) ; S_OK une fois traité.
HRESULT XErrorReport(HRESULT hr, const char* message) noexcept {
  ErrorCallback callback = nullptr;
  void* context = nullptr;
  {
    std::lock_guard<std::mutex> lock(Errors().mutex);
    callback = Errors().callback;
    context = Errors().context;
  }
  anyxboxx::Log("[runtime] XErrorReport 0x%08lX : %s", static_cast<unsigned long>(hr),
                message != nullptr ? message : "");
  if (callback != nullptr) callback(hr, message, context);
  return S_OK;
}

void XErrorSetCallback(ErrorCallback callback, void* context) noexcept {
  std::lock_guard<std::mutex> lock(Errors().mutex);
  Errors().callback = callback;
  Errors().context = context;
}

// Options (OutputDebugString, DebugBreak, FailFast) : notées, jamais appliquées, pour que
// l'émulateur ne s'arrête pas sur une erreur que le jeu sait traiter.
void XErrorSetOptions(uint32_t when_debugger, uint32_t without_debugger) noexcept {
  ANYXBOXX_TRACE("[runtime] XErrorSetOptions(%u, %u) noté", when_debugger, without_debugger);
}

void* const kInviteMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,      // 0-2
    ANYXBOXX_METHOD(Subscribe),     // 3 XGameInviteRegisterForEvent
    ANYXBOXX_METHOD(Unsubscribe),   // 4 XGameInviteUnregisterForEvent
};
void* const kPendingInviteMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,      // 0-2
    ANYXBOXX_NOT_IMPLEMENTED,       // 3 (servie par l'interface précédente)
    ANYXBOXX_NOT_IMPLEMENTED,       // 4
    ANYXBOXX_METHOD(Subscribe),     // 5 XGameInviteRegisterForPendingEvent
    ANYXBOXX_METHOD(Unsubscribe),   // 6 XGameInviteUnregisterForPendingEvent
    ANYXBOXX_NOT_IMPLEMENTED,       // 7 XGameInviteAcceptPendingInvite
};
void* const kActivationMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,      // 0-2
    ANYXBOXX_METHOD(Subscribe),     // 3 XGameActivationRegisterForEvent
    ANYXBOXX_METHOD(Unsubscribe),   // 4 XGameActivationUnregisterForEvent
    ANYXBOXX_NOT_IMPLEMENTED,       // 5 XGameActivationAcceptPendingInvite
};
void* const kProtocolMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,      // 0-2
    ANYXBOXX_METHOD(Subscribe),     // 3 XGameProtocolRegisterForActivation
    ANYXBOXX_METHOD(Unsubscribe),   // 4 XGameProtocolUnregisterForActivation
};
void* const kErrorMethods[] = {
    ANYXBOXX_IUNKNOWN_METHODS,             // 0-2
    ANYXBOXX_METHOD(XErrorReport),         // 3
    ANYXBOXX_METHOD(XErrorSetCallback),    // 4
    ANYXBOXX_METHOD(XErrorSetOptions),     // 5
};

const anyxboxx::StaticInterface kInvite{kInviteMethods};
const anyxboxx::StaticInterface kPendingInvite{kPendingInviteMethods};
const anyxboxx::StaticInterface kActivation{kActivationMethods};
const anyxboxx::StaticInterface kProtocol{kProtocolMethods};
const anyxboxx::StaticInterface kError{kErrorMethods};

}  // namespace

namespace anyxboxx {

void* InviteInterface() { return const_cast<StaticInterface*>(&kInvite); }
void* PendingInviteInterface() { return const_cast<StaticInterface*>(&kPendingInvite); }
void* ActivationInterface() { return const_cast<StaticInterface*>(&kActivation); }
void* ProtocolInterface() { return const_cast<StaticInterface*>(&kProtocol); }
void* ErrorInterface() { return const_cast<StaticInterface*>(&kError); }

HRESULT AddSubscription(const void* callback, void* token) {
  return Registry().Add(callback, static_cast<RegistrationToken*>(token));
}
bool RemoveSubscription(uint64_t token) { return Registry().Remove(RegistrationToken{token}); }

}  // namespace anyxboxx
