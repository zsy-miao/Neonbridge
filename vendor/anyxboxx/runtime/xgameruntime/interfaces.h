// Objets d'interface du runtime GDK servis par AnyXboxX (voir served_interfaces.inc).
#pragma once

#include <windows.h>

#include <cstdint>

namespace anyxboxx {

// XTaskQueue, XAsync et XThread : interface {073B7DCB-1FCF-4030-94BE-E3C9EB623428}.
void* TaskQueueInterface();

// XUser : profil local hors ligne, fournisseur {01ACD177…} (5 versions d'interface).
void* UserInterface();

// XUserGetGamertag : interface {CEF4FAC0…} du même fournisseur.
void* GamertagInterface();

// Identité Xbox Series X (system_interface.cpp) : XSystem, analytique, Title ID et
// lancement, fonctionnalités du runtime, report d'extinction de l'écran et XLaunchUri.
void* SystemInterface();
void* AnalyticsInterface();
void* GameInterface();
void* FeatureInterface();
void* DisplayInterface();

// XUID du profil local si `user` est un handle XUser valide d'AnyXboxX.
bool LocalUserId(const void* user, uint64_t* xuid);

// Sauvegardes (save_interface.cpp) et stockage local persistant (local_storage_interface.cpp).
void* GameSaveInterface();
void* LocalStorageInterface();

// XPackage : paquet installé en local (package_interface.cpp).
void* PackageInterface();

// Abonnements hors ligne et XError (event_interface.cpp) : invitations (2 versions),
// activations, protocoles ; le rappel ne part jamais sans Xbox Live.
void* InviteInterface();
void* PendingInviteInterface();
void* ActivationInterface();
void* ProtocolInterface();
void* ErrorInterface();

// Jetons d'abonnement partagés (XTaskQueueRegistrationToken) : E_INVALIDARG si rappel ou jeton nul.
HRESULT AddSubscription(const void* callback, void* token);
bool RemoveSubscription(uint64_t token);

// État HDR de l'écran et connectivité réseau du PC (platform_interface.cpp).
void* HdrDisplayInterface();
void* NetworkingInterface();

// XStore : Microsoft Store hors ligne (store_interface.cpp) : licence locale, produits vides.
void* StoreInterface();

// Renvoie l'objet qui sert (fournisseur, interface), ou nullptr.
void* ServedInterface(const GUID& provider, const GUID& iid);

}  // namespace anyxboxx
