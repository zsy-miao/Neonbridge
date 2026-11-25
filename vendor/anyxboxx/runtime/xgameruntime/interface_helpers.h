// Outils communs aux objets d'interface du runtime GDK servis par AnyXboxX.
//
// Un objet est statique : sa table de méthodes suit l'ordre relevé dans
// gdk_api_table.inc (0 QueryInterface, 1 AddRef, 2 Release, puis les API), et
// chaque méthode reçoit this suivi des arguments de la fonction publique.
#pragma once

#include <windows.h>

namespace anyxboxx {

// Ajoute le paramètre this devant une fonction publique noexcept.
template <typename Signature>
struct Adapter;

template <typename Result, typename... Args>
struct Adapter<Result(Args...) noexcept> {
  template <Result (*Function)(Args...) noexcept>
  static Result WINAPI Call(void*, Args... args) noexcept {
    return Function(args...);
  }
};

struct StaticInterface {
  void* const* methods;
};

// QueryInterface d'un objet statique : lui-même pour IUnknown et pour chaque
// interface que served_interfaces.inc lui attribue.
HRESULT WINAPI StaticQueryInterface(void* self, REFIID iid, void** out) noexcept;
ULONG WINAPI StaticAddRef(void* self) noexcept;
ULONG WINAPI StaticRelease(void* self) noexcept;

// Méthode connue de la table mais pas (encore) écrite : échec propre et trace.
HRESULT WINAPI NotImplementedMethod(void* self) noexcept;

}  // namespace anyxboxx

#define ANYXBOXX_METHOD(function) \
  reinterpret_cast<void*>(&::anyxboxx::Adapter<decltype(function)>::template Call<&function>)

#define ANYXBOXX_IUNKNOWN_METHODS                             \
  reinterpret_cast<void*>(&::anyxboxx::StaticQueryInterface), \
      reinterpret_cast<void*>(&::anyxboxx::StaticAddRef),     \
      reinterpret_cast<void*>(&::anyxboxx::StaticRelease)

#define ANYXBOXX_NOT_IMPLEMENTED reinterpret_cast<void*>(&::anyxboxx::NotImplementedMethod)
