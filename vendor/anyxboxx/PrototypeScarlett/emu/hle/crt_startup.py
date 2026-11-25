"""Démarrage C d'un exécutable (UCRT) : variables globales, argv, environnement.

Ces fonctions renvoient des pointeurs vers des variables du CRT ; chacune est
allouée une seule fois dans le tas de la machine et réutilisée ensuite.
"""
from .core import hle

PROGRAM_NAME = "game.exe"
INT_SIZE = 4
POINTER_SIZE = 8


def _cell(c, key, size=INT_SIZE, data=None):
    """Variable globale du CRT : allouée au premier accès, puis toujours la même."""
    cells = c.machine.crt_cells
    address = cells.get(key)
    if address is None:
        address = c.heap.alloc(size)
        if data:
            c.write(address, data)
        cells[key] = address
    return address


def _text(c, key, value, wide):
    data = value.encode("utf-16-le") + b"\0\0" if wide else value.encode() + b"\0"
    return _cell(c, key, len(data), data)


def _argv(c, wide):
    name = _text(c, f"argv0-{wide}", PROGRAM_NAME, wide)
    table = name.to_bytes(POINTER_SIZE, "little") + bytes(POINTER_SIZE)
    return _cell(c, f"argv-{wide}", 2 * POINTER_SIZE, table)


@hle("crt", "__p__commode", "__p__fmode")
def global_mode(c):
    return _cell(c, "mode")


@hle("crt")
def _errno(c):
    return _cell(c, "errno")


@hle("crt", "__doserrno", "__p__doserrno")
def doserrno(c):
    return _cell(c, "doserrno")


@hle("crt")
def __p___argc(c):
    return _cell(c, "argc", data=(1).to_bytes(INT_SIZE, "little"))


@hle("crt")
def __p___argv(c):
    return _cell(c, "argv-pointer", POINTER_SIZE, _argv(c, False).to_bytes(POINTER_SIZE, "little"))


@hle("crt")
def __p___wargv(c):
    return _cell(c, "wargv-pointer", POINTER_SIZE, _argv(c, True).to_bytes(POINTER_SIZE, "little"))


@hle("crt")
def _get_wide_winmain_command_line(c):
    return _text(c, "cmdline-w", "", wide=True)


@hle("crt")
def _get_narrow_winmain_command_line(c):
    return _text(c, "cmdline-a", "", wide=False)


@hle("crt", "_configure_wide_argv", "_initialize_wide_environment", "_set_new_mode",
     "_callnewh", "_get_startup_new_mode")
def startup_success(c):
    return 0


@hle("crt", "_register_thread_local_exe_atexit_callback", "_set_invalid_parameter_handler",
     "_set_purecall_handler", "_set_abort_behavior")
def startup_ignored(c):
    return 0   # aucun thread secondaire ni gestionnaire d'erreur émulé pour l'instant


def _command_line(c, wide):
    """Ligne de commande Windows : le nom du programme entre guillemets, sans argument."""
    return _text(c, f"getcmdline-{wide}", f'"{c.machine.main.path.name}"', wide)


@hle("kernel")
def GetCommandLineW(c):
    return _command_line(c, wide=True)


@hle("kernel")
def GetCommandLineA(c):
    return _command_line(c, wide=False)


def split_command_line(line):
    """Découpage simplifié de CommandLineToArgvW : espaces hors guillemets, guillemets retirés."""
    words, current, quoted, started = [], [], False, False
    for char in line:
        if char == '"':
            quoted, started = not quoted, True
        elif char in " \t" and not quoted:
            if started:
                words.append("".join(current))
            current, started = [], False
        else:
            current.append(char)
            started = True
    if started:
        words.append("".join(current))
    return words


@hle("shell32")
def CommandLineToArgvW(c):
    """Un seul bloc (table de pointeurs + chaînes), libérable par LocalFree comme sous Windows."""
    words = split_command_line(c.wstr(c.arg(0))) if c.arg(0) else [c.machine.main.path.name]
    strings = [w.encode("utf-16-le") + b"\0\0" for w in words]
    table_size = POINTER_SIZE * (len(words) + 1)
    block = c.heap.alloc(table_size + sum(len(s) for s in strings))
    cursor = block + table_size
    for index, data in enumerate(strings):
        c.write_u64(block + POINTER_SIZE * index, cursor)
        c.write(cursor, data)
        cursor += len(data)
    c.write_u64(block + POINTER_SIZE * len(words), 0)
    if c.arg(1):
        c.write_u32(c.arg(1), len(words))
    return block


@hle("kernel", "GetEnvironmentStringsW", "GetEnvironmentStrings")
def get_environment_strings(c):
    """Bloc d'environnement vide : deux zéros terminaux (quatre octets en UTF-16)."""
    block = c.heap.alloc(4)
    c.write(block, bytes(4))
    return block


@hle("kernel", "FreeEnvironmentStringsW", "FreeEnvironmentStringsA")
def free_environment_strings(c):
    c.heap.free(c.arg(0))
    return 1
