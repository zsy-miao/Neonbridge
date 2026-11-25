"""AnyXboxX — prototype d'émulateur Xbox Series X (interface et ligne de commande)."""
import os
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

# Le lanceur démarre Python en mode isolé (-I), qui n'ajoute pas ce dossier au chemin.
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import pefile  # noqa: E402
from unicorn import UcError  # noqa: E402

from emu import launch  # noqa: E402
from emu.config import manifest_path  # noqa: E402
from emu.machine import Machine  # noqa: E402
from emu.report import analyse  # noqa: E402

WIDTH, HEIGHT, ZOOM = 160, 96, 3
FRAME_MS = 33
DEMO = HERE / "demo.pe"
XBOX_SERIES_X = 7
ERRORS = (ValueError, OSError, ET.ParseError, pefile.PEFormatError, UcError)
TITLE = "AnyXboxX — Xbox Series X emulator (prototype)"


def resolve_target(path):
    """Accepte un dossier de paquet GDK ou directement un exécutable."""
    return launch.resolve(path)


def print_report(path, extra_dirs=()):
    try:
        exe, _ = resolve_target(path)
        print(analyse(exe, extra_dirs).text())
        return 0
    except ERRORS as error:
        print(f"Erreur : {error}", file=sys.stderr)
        return 1


class Window:
    def __init__(self, root, folder):
        import tkinter as tk
        from tkinter import ttk

        self.tk = tk
        self.root = root
        self.folder = folder
        self.timer = None
        self.keys = set()
        self.machine = None
        root.title(TITLE)
        panel = ttk.Frame(root, padding=12)
        panel.pack(fill="both", expand=True)
        self.screen = tk.Canvas(panel, width=WIDTH * ZOOM, height=HEIGHT * ZOOM,
                                bg="#222536", highlightthickness=0)
        self.screen.pack()
        self.picture = self.screen.create_image(0, 0, anchor="nw")
        self.caption = self.screen.create_text(WIDTH * ZOOM / 2, HEIGHT * ZOOM / 2,
                                               text="Emulated Xbox Series X screen", fill="white")
        self.message = tk.StringVar()
        ttk.Label(panel, textvariable=self.message, wraplength=465).pack(fill="x", pady=8)
        buttons = ttk.Frame(panel)
        buttons.pack(anchor="w")
        ttk.Button(buttons, text="Run the demo", command=self.demo).pack(side="left")
        ttk.Button(buttons, text="Open an Xbox folder…", command=self.choose).pack(
            side="left", padx=6)
        self.journal = tk.Text(panel, height=10, width=60, state="disabled",
                               bg="#14161f", fg="#d7dae6", font=("Consolas", 9))
        self.journal.pack(fill="both", expand=True, pady=(8, 0))
        root.bind("<KeyPress>", lambda event: self.keys.add(event.keysym))
        root.bind("<KeyRelease>", lambda event: self.keys.discard(event.keysym))
        root.bind("<FocusOut>", lambda event: self.keys.clear())
        if manifest_path(folder).is_file():
            self.open_package(folder)

    def write(self, text):
        self.journal.configure(state="normal")
        self.journal.insert("end", text + "\n")
        self.journal.see("end")
        self.journal.configure(state="disabled")

    def reset(self):
        if self.timer is not None:
            self.root.after_cancel(self.timer)
            self.timer = None
        self.screen.itemconfigure(self.picture, image="")
        self.screen.itemconfigure(self.caption, state="normal")
        self.keys.clear()
        self.machine = None
        self.journal.configure(state="normal")
        self.journal.delete("1.0", "end")
        self.journal.configure(state="disabled")

    def choose(self):
        from tkinter import filedialog

        folder = filedialog.askdirectory(initialdir=self.folder)
        if folder:
            self.open_package(Path(folder))

    def open_package(self, folder):
        self.reset()
        try:
            exe, title_id = resolve_target(folder)
            self.write(analyse(exe).text())
            machine = Machine(exe, title_id=title_id, log=self.write)
            code = machine.start()
            self.message.set(f"{exe.name} finished. Return value: {code:#x}.")
        except ERRORS as error:
            self.message.set(str(error))

    def demo(self):
        self.reset()
        try:
            self.machine = Machine(DEMO, log=self.write)
            draw = self.machine.image.exports.get("DrawFrame")
            if self.machine.start() != XBOX_SERIES_X or draw is None:
                raise ValueError("The demo does not recognize the emulated Xbox Series X.")
            self.draw = draw
            self.pixels = self.machine.heap.alloc(WIDTH * HEIGHT * 3)
            self.tick = 0
            self.screen.itemconfigure(self.caption, state="hidden")
            self.message.set("Driving demo · Use the left and right arrow keys.")
            self.write("Demo started: XSystemGetDeviceType → XboxScarlettAnaconda (7).")
            self.screen.focus_set()
            self.update_frame()
        except ERRORS as error:
            self.message.set(str(error))

    def update_frame(self):
        self.timer = None
        try:
            direction = int("Right" in self.keys) - int("Left" in self.keys)
            self.machine.call(self.draw, self.pixels, self.tick, direction)
            data = bytes(self.machine.cpu.mem_read(self.pixels, WIDTH * HEIGHT * 3))
            header = f"P6\n{WIDTH} {HEIGHT}\n255\n".encode()
            self.image = self.tk.PhotoImage(data=header + data, format="PPM").zoom(ZOOM)
            self.screen.itemconfigure(self.picture, image=self.image)
            self.tick += 1
            self.timer = self.root.after(FRAME_MS, self.update_frame)
        except ERRORS as error:
            self.message.set(str(error))


NATIVE_WARNING = ("ATTENTION : en mode --natif, le programme s'exécute directement sur ce PC, "
                  "sans isolation. N'utilisez ce mode qu'avec des programmes de confiance.")


def run_native(target, extra_dirs=()):
    """Exécution façon Wine : le code tourne sur le processeur, les imports Xbox sont servis
    par AnyXboxX et le reste par Windows. Le processus se termine avec le jeu."""
    print(NATIVE_WARNING, file=sys.stderr)
    try:
        from emu import native

        exe, _ = resolve_target(target)
        return native.run(exe, (exe.parent, *extra_dirs)) & 0xFFFF_FFFF
    except (ImportError, MemoryError, *ERRORS) as error:
        print(f"Erreur : {error}", file=sys.stderr)
        return 1


def print_translation(package, svg_path=None):
    """Mesure la traduction des DLL console, affiche le tableau et écrit le SVG si demandé."""
    try:
        from emu import bench, translation

        runtime = os.environ.get("ANYXBOXX_RUNTIME") or HERE.parent / "runtime" / "build" / "bin"
        overview = (translation.CPU_AREA, translation.gpu_disassembly_area(), translation.gpu_area(),
                    translation.functions_area(bench.run(Path(package))))
        tombox = os.environ.get("ANYXBOXX_TOMBOX_DIR") or translation.TOMBOX_BUILD
        areas = translation.measure(package, Path(runtime), tombox_dir=tombox,
                                    extra=(*translation.STATIC_AREAS, translation.gdk_runtime_area()))
        areas = (*overview, *areas)
        print(translation.table(areas))
        if svg_path is not None:
            svg_path.write_text(translation.svg(areas, "AnyXboxX: Xbox Series X translation progress"),
                                encoding="utf-8")
        return 0
    except (OSError, ValueError) as error:
        print(f"Erreur : {error}", file=sys.stderr)
        return 1


def print_map(package, svg_path):
    """Écrit la carte SVG de l'avancée (une case par fonction) et affiche les badges."""
    try:
        from emu import status_map, translation

        runtime = os.environ.get("ANYXBOXX_RUNTIME") or HERE.parent / "runtime" / "build" / "bin"
        tombox = os.environ.get("ANYXBOXX_TOMBOX_DIR") or translation.TOMBOX_BUILD
        status = status_map.collect(Path(package), Path(runtime), tombox)
        svg_path.parent.mkdir(parents=True, exist_ok=True)
        svg_path.write_text(status_map.render(status), encoding="utf-8")
        for label, value, _ in status.badges:
            print(f"{label} : {value}")
        return 0
    except (OSError, ValueError) as error:
        print(f"Erreur : {error}", file=sys.stderr)
        return 1


def find_package_folder():
    start = Path(sys.executable if getattr(sys, "frozen", False) else __file__).resolve().parent
    return next((c for c in (start, start.parent, start.parent.parent)
                 if manifest_path(c).is_file()), start)


def run_rdna2(data):
    """--rdna2 <shader.bin> --executer : un wave32 dans l'interpréteur de référence."""
    from emu import rdna2_exec

    try:
        wave = rdna2_exec.run(data)
    except (rdna2_exec.Untranslated, RuntimeError) as error:
        print(f"Arrêt : {error}", file=sys.stderr)
        return 1
    used = [n for n in range(106) if wave.s[n]]
    print("SGPR : " + (", ".join(f"s{n}=0x{wave.s[n]:x}" for n in used) or "tous nuls"))
    print(f"VCC=0x{wave.vcc:08x} EXEC=0x{wave.exec:08x} SCC={int(wave.scc)}")
    for n in range(1, len(wave.v)):
        if any(wave.v[n]):
            print(f"v{n} (lignes 0-3) : " + " ".join(f"{x:08x}" for x in wave.v[n][:4]))
    return 0


def main(argv):
    if sys.stdout is not None:
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    # --rapport/--banc <cible> [dossiers de DLL supplémentaires…]
    extra_dirs = tuple(Path(d) for d in argv[2:])
    if len(argv) >= 2 and argv[0] == "--rapport":
        return print_report(argv[1], extra_dirs)
    if len(argv) >= 2 and argv[0] == "--banc":
        from emu import bench

        print(bench.table(bench.run(Path(argv[1]), extra_dirs)))
        return 0
    if len(argv) >= 2 and argv[0] == "--lancer":
        # --lancer <cible> [dossiers de DLL…] [--md rapport.md]
        options = list(argv[2:])
        markdown = None
        if "--md" in options:
            at = options.index("--md")
            if at + 1 >= len(options):
                print("Erreur : --md attend un nom de fichier.", file=sys.stderr)
                return 1
            markdown = Path(options[at + 1])
            del options[at:at + 2]
        return launch.run(Path(argv[1]), tuple(Path(d) for d in options), markdown=markdown)
    if len(argv) >= 2 and argv[0] == "--traduction":
        return print_translation(Path(argv[1]), Path(argv[2]) if len(argv) > 2 else None)
    if len(argv) >= 3 and argv[0] == "--carte":
        return print_map(Path(argv[1]), Path(argv[2]))
    if len(argv) >= 2 and argv[0] == "--natif":
        # --natif <cible> [dossiers de DLL…] [--fps] : --fps affiche les i/s dans le titre de la fenêtre
        options = [arg for arg in argv[2:] if arg != "--fps"]
        if "--fps" in argv[2:]:
            os.environ["ANYXBOXX_FPS"] = "1"
        return run_native(Path(argv[1]), tuple(Path(d) for d in options))
    if len(argv) >= 2 and argv[0] == "--rdna2":
        # --rdna2 <shader.bin> [--liste | --executer | --hlsl] : familles, listing, exécution ou traduction HLSL
        from emu import rdna2

        try:
            data = Path(argv[1]).read_bytes()
        except OSError as error:
            print(f"Erreur : {error}", file=sys.stderr)
            return 1
        if "--liste" in argv[2:]:
            print("\n".join(rdna2.listing(data)))
            return 0
        if "--executer" in argv[2:]:
            return run_rdna2(data)
        if "--hlsl" in argv[2:]:
            from emu import rdna2_hlsl

            try:
                print(rdna2_hlsl.translate(data))
            except rdna2_hlsl.Untranslatable as error:
                print(f"Arrêt : {error}", file=sys.stderr)
                return 1
            return 0
        if "--spirv" in argv[2:]:
            # --spirv [vs|ps|cs|ms|gs] : SPIR-V par le traducteur de Leonida (third_party/leonida)
            from emu import rdna2_spirv

            after = argv[argv.index("--spirv") + 1:]
            stage = after[0] if after and after[0] in rdna2_spirv.STAGES else "cs"
            try:
                print(rdna2_spirv.translate(data, stage))
            except rdna2_spirv.Untranslatable as error:
                print(f"Arrêt : {error}", file=sys.stderr)
                return 1
            return 0
        instructions = rdna2.decode(data)
        for family, count in rdna2.histogram(instructions).items():
            print(f"{family:10} {count}")
        print(f"Total : {len(instructions)} instructions")
        return 0
    if len(argv) >= 2 and argv[0] == "--banc-perf":
        # --banc-perf <exe> [secondes] [graphique.svg] : objectif 60 i/s constants
        from emu import perf

        try:
            seconds = float(argv[2]) if len(argv) > 2 else 10.0
            chart = Path(argv[3]) if len(argv) > 3 else None
            return 0 if perf.run(Path(argv[1]), seconds, chart).constant_60 else 2
        except (OSError, ValueError) as error:
            print(f"Erreur : {error}", file=sys.stderr)
            return 1
    import tkinter as tk

    root = tk.Tk()
    window = Window(root, find_package_folder())
    if argv == ["--demo"]:
        root.after_idle(window.demo)
    root.mainloop()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
