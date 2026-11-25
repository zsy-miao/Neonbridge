// Opérations asynchrones du runtime : le travail est fait au lancement, le résultat est
// livré par XAsync (file du jeu), puis copié dans le tampon du jeu par GetResult.
#pragma once

#include <windows.h>

#include <functional>
#include <new>
#include <utility>

#include "XAsync.h"
#include "XAsyncProvider.h"

namespace anyxboxx {

struct AsyncCall {
  HRESULT result = S_OK;
  size_t size = 0;                   // taille du résultat (0 : aucun)
  std::function<void(void*)> write;  // remplit le tampon du jeu (size octets)
  std::function<void()> discard;     // libère un résultat jamais lu
};

inline HRESULT CALLBACK AsyncCallProvider(XAsyncOp op, const XAsyncProviderData* data) noexcept {
  auto* call = static_cast<AsyncCall*>(data->context);
  switch (op) {
    case XAsyncOp::Begin:
      return XAsyncSchedule(data->async, 0);
    case XAsyncOp::DoWork:
      XAsyncComplete(data->async, call->result, SUCCEEDED(call->result) ? call->size : 0);
      return S_OK;
    case XAsyncOp::GetResult:
      if (call->write) call->write(data->buffer);
      call->write = nullptr;
      call->discard = nullptr;
      return S_OK;
    case XAsyncOp::Cleanup:
      if (call->discard) call->discard();
      delete call;
      return S_OK;
    default:
      return S_OK;
  }
}

// Lance une opération déjà calculée : `call` décrit son résultat.
inline HRESULT BeginAsyncCall(XAsyncBlock* async, const void* identity, const char* name, AsyncCall call) {
  auto* owned = new (std::nothrow) AsyncCall(std::move(call));
  if (owned == nullptr) return E_OUTOFMEMORY;
  const HRESULT begun = XAsyncBegin(async, owned, identity, name, AsyncCallProvider);
  if (FAILED(begun)) {
    if (owned->discard) owned->discard();
    delete owned;
  }
  return begun;
}

}  // namespace anyxboxx
