"""Plan d'adressage de la machine émulée et tas simplifié."""
from unicorn import UC_PROT_EXEC, UC_PROT_READ, UC_PROT_WRITE

PAGE = 0x1000
RW = UC_PROT_READ | UC_PROT_WRITE
RX = UC_PROT_READ | UC_PROT_EXEC

# Zones réservées à l'émulateur, loin des bases d'image habituelles (0x140000000).
THUNKS = 0x7000_0000_0000      # une entrée de 16 octets par fonction HLE
THUNK_PAGES = 64
SYSTEM = 0x7000_0010_0000      # TEB puis PEB
STACK = 0x7000_0020_0000
STACK_SIZE = 0x10_0000          # 1 Mio
HEAP = 0x7000_1000_0000
HEAP_LIMIT = 512 * 1024 * 1024  # 512 Mio
HEAP_CHUNK = 0x10_0000


def align(size, unit=PAGE):
    return (size + unit - 1) // unit * unit


class Heap:
    """Allocateur linéaire : suffisant pour un prototype mono-thread.

    Les blocs libérés ne sont pas réutilisés, mais leur taille est conservée
    pour que les fonctions de réallocation copient le bon nombre d'octets.
    """

    def __init__(self, cpu, base=HEAP, limit=HEAP_LIMIT):
        self.cpu = cpu
        self.base = base
        self.limit = limit
        self.cursor = base
        self.mapped = base
        self.sizes = {}

    def alloc(self, size, alignment=16):
        size = max(int(size), 1)
        start = align(self.cursor, alignment)
        end = start + size
        if end - self.base > self.limit:
            return 0
        if end > self.mapped:
            grow = align(end - self.mapped, HEAP_CHUNK)
            self.cpu.mem_map(self.mapped, grow, RW)
            self.mapped += grow
        self.cursor = end
        self.sizes[start] = size
        return start

    def free(self, address):
        return self.sizes.pop(address, None) is not None

    def contains(self, address, size):
        """Vrai si [address, address+size) tient dans un bloc déjà alloué."""
        for start, length in self.sizes.items():
            if start <= address and address + size <= start + length:
                return True
        return False

    def size_of(self, address):
        return self.sizes.get(address)

    def realloc(self, address, size):
        if not address:
            return self.alloc(size)
        old = self.sizes.get(address)
        if old is None:
            return 0
        new = self.alloc(size)
        if new:
            self.cpu.mem_write(new, bytes(self.cpu.mem_read(address, min(old, size))))
            self.free(address)
        return new

    @property
    def used(self):
        return self.cursor - self.base
