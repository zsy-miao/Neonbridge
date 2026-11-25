"""Fonctions écrites directement en code x86-64 invité.

Certaines fonctions doivent rappeler le programme émulé (par exemple les
constructeurs C++ globaux lancés par _initterm). Un gestionnaire Python ne
peut pas relancer Unicorn depuis un hook ; ces fonctions sont donc fournies
sous forme de petites routines machine copiées dans la page HLE.
"""

# _initterm(begin=RCX, end=RDX) : appelle chaque pointeur non nul de [begin, end).
INITTERM = bytes.fromhex(
    "53 56 4883EC28 4889CB 4889D6"   # push rbx/rsi, réserve la pile, copie les bornes
    "4839F3 7310"                    # boucle : rbx >= rsi → fin
    "488B03 4885C0 7402 FFD0"        # rax = *rbx ; si non nul, call rax
    "4883C308 EBEB"                  # rbx += 8 ; boucle
    "31C0 4883C428 5E 5B C3"         # eax = 0 ; restaure et retourne
)

# _initterm_e : idem, mais s'arrête et renvoie le premier code non nul.
INITTERM_E = bytes.fromhex(
    "53 56 4883EC28 4889CB 4889D6"
    "4839F3 7314"
    "488B03 4885C0 7406 FFD0 85C0 7508"
    "4883C308 EBE7"
    "31C0 4883C428 5E 5B C3"
)

GUEST_CODE = {
    ("crt", "_initterm"): INITTERM,
    ("crt", "_initterm_e"): INITTERM_E,
}
