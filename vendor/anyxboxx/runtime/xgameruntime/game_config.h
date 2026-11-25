// Identité du jeu lue dans son MicrosoftGame.config (dossier donné par ANYXBOXX_GAME_DIR,
// que le mode natif d'AnyXboxX renseigne avant de lancer le jeu).
#pragma once

#include <cstdint>
#include <string>

namespace anyxboxx {

// Identité déclarée par MicrosoftGame.config (champs vides s'ils manquent).
struct GameIdentity {
  std::string name;          // <Identity Name="…">
  std::string publisher;     // <Identity Publisher="…">
  std::string version;       // <Identity Version="a.b.c.d">
  std::string display_name;  // <ShellVisuals DefaultDisplayName="…">
  std::string store_id;      // <StoreId>
  std::wstring folder;       // dossier du jeu (ANYXBOXX_GAME_DIR)
};

GameIdentity ReadGameIdentity();

// Title ID déclaré par <TitleId>, ou 0 si le fichier ou l'élément manque.
uint32_t GameTitleId();

// Taille de <PersistentLocalStorage><SizeMB> en octets, ou 0 si non déclarée.
uint64_t GameLocalStorageBytes();

}  // namespace anyxboxx
