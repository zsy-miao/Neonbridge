"""Résumé du journal de VIctory : lignes d'images écrites au format de host.rs (commits épinglés 7b0f736 et fd021bf, même format)."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
import resume_journal_victory as rj  # noqa: E402

FIN = ("; heap buffers 0.0 passes 0.0 ms;                  texture slots written so far: 12 placed, 3 of objects; "
       "textures picked by index: 0 uploaded so far (0 MB), 0 waiting, 0 passed over unmapped")


def ligne_image(fps, ms, gpu, back, draws, dropped=300.0, couleur=False):
    """Une ligne d'images comme VIctory l'écrit (format de host.rs), précédée de l'en-tête de tracing."""
    niveau = "\x1b[32m INFO\x1b[0m" if couleur else " INFO"
    return (f"2026-11-24T20:35:40.123456Z{niveau} victory_gpu::host: {fps:.1f} fps, {ms:.1f} ms/frame "
            f"(92.1 preparing: copies 49.5 ms 126.8 MB, textures 7.4 ms 28.5 MB; {gpu:.1f} on the GPU; "
            f"{back:.1f} copying back 62.3 MB), {draws:.0f} draws/frame, dropped {dropped:.0f} without a host "
            f"pipeline (2 being made) and 12 indirect{FIN}")


JOURNAL = "\n".join([
    ligne_image(30.0, 33.3, 4.0, 1.0, 40),                       # menu : moins de 100 draws
    ligne_image(3.9, 255.9, 146.2, 165.2, 5218),
    ligne_image(4.4, 225.0, 102.8, 138.6, 5625, couleur=True),
    ligne_image(2.0, 503.2, 171.7, 196.7, 3653),
    "2026-11-24T20:35:41Z  WARN victory_gpu::pipeline: shader does not translate; its pipelines' draws are dropped",
    "2026-11-24T20:35:42Z  WARN victory_gpu::pipeline: host pipeline refused (E_INVALIDARG)",
    "2026-11-24T20:35:43Z ERROR victory_gpu::host: host GPU device removed after a submission of 3 lists",
    "2026-11-24T20:35:44Z  INFO victory_gpu::host: the pipeline cache does not translate anything here",
])


def test_world_lines_are_kept_and_menu_lines_left_out():
    analyse = rj.analyser(JOURNAL)
    assert analyse["lignes"] == 4
    assert [i["draws"] for i in analyse["images"]] == [5218, 5625, 3653]


def test_every_column_of_the_frame_line_is_read():
    image = rj.analyser(JOURNAL)["images"][0]
    assert image == {"fps": 3.9, "image": 255.9, "preparation": 92.1, "copies": 49.5, "copies_mo": 126.8,
                     "textures": 7.4, "textures_mo": 28.5, "gpu": 146.2, "retour": 165.2, "retour_mo": 62.3,
                     "draws": 5218, "perdus": 300, "en_cours": 2, "indirects": 12}


def test_summary_gives_median_and_range():
    resume = rj.resumer(rj.analyser(JOURNAL)["images"])
    assert resume["image"] == (255.9, 225.0, 503.2)
    assert resume["gpu"] == (146.2, 102.8, 171.7)
    assert rj.resumer([]) == {}


def test_warnings_are_counted_only_at_warn_or_error_level():
    compte = rj.analyser(JOURNAL)["avertissements"]
    assert compte["shader does not translate"] == 1          # la ligne INFO n'est pas comptée
    assert compte["host pipeline refused"] == 1
    assert compte["host GPU device removed"] == 1


def test_draws_threshold_can_be_changed():
    assert len(rj.analyser(JOURNAL, draws_min=5000)["images"]) == 2


def test_powershell_utf16_log_is_read(tmp_path):
    journal = tmp_path / "victory.log"
    journal.write_bytes(b"\xff\xfe" + JOURNAL.encode("utf-16-le"))
    assert len(rj.analyser(rj.lire(journal))["images"]) == 3


def test_comparison_gives_relative_change(tmp_path):
    avant, apres = tmp_path / "a.log", tmp_path / "b.log"
    avant.write_text(ligne_image(4.0, 250.0, 150.0, 160.0, 5000), encoding="utf-8")
    apres.write_text(ligne_image(5.0, 200.0, 150.0, 80.0, 5000), encoding="utf-8")
    sortie = rj.tableau_compare(rj.analyser(rj.lire(avant)), rj.analyser(rj.lire(apres)), markdown=True)
    assert "| ms/frame | 250 | 200 | -20 % |" in sortie
    assert "| copying back (ms) | 160 | 80.0 | -50 % |" in sortie


def test_main_exits_with_one_when_no_world_line(tmp_path, capsys):
    menus = tmp_path / "menus.log"
    menus.write_text(ligne_image(30.0, 33.3, 4.0, 1.0, 40), encoding="utf-8")
    assert rj.main([str(menus)]) == 1
    assert "rien à résumer" in capsys.readouterr().out
    complet = tmp_path / "monde.log"
    complet.write_text(JOURNAL, encoding="utf-8")
    assert rj.main([str(complet), "--markdown"]) == 0
    assert "| on the GPU (ms) | 146 | 103-172 |" in capsys.readouterr().out
