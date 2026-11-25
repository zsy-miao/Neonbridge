# Running games

Options for `python tuxbox.py run`. `python tuxbox.py run --help` lists all of them.

## Shader preparation

On Linux, the launcher prepares shaders before opening Frosty, from a profile of its game files. ProjectScarlett records the shaders the game requests, but a normal launch doesn't wait for them: it translates shaders as the game asks. After an update that changes the shader translator, run `python tuxbox.py run --prepare-only` first. Preparing the recorded requests together is much faster than translating them during play.

The launcher shows compilation progress and reuses the cache on later runs. After a TuxBox update, shaders that translate to the same source reuse their compiled output. While ProjectScarlett runs, a small panel over the game shows shader and pipeline progress. Pass `--prepare-only` to prepare the cache without opening the game, or `--no-shader-prep` to skip preparation for one launch. Place either option before the game path.

## Project Scarlett graphics trials

On Linux, put `--graphics-menu` before the game path to choose the packaged settings or experimental Series S mode. Use `--graphics-profile series-s` to select it from a script. Series S mode uses Bubblewrap and a temporary copy of the Wine prefix. The copy is discarded at exit.

The Series S trial requested a 1920×1080 render resource and stopped on a texture allocation before presenting. Use the packaged settings for the current playable startup path.

## Game logs

`tuxbox.py run` shows game output in the terminal as it arrives. The first line gives the path to a saved copy. On Linux, logs go in `~/.local/state/project-tuxbox/logs/` (or under `XDG_STATE_HOME`). On Windows, they go in `%LOCALAPPDATA%\TuxBox\logs\`.

To choose a file, pass `--log-file` before the game path. Each run appends to that file.

```sh
python tuxbox.py run --log-file build/lanes.log --app-id Lanes build/lanes
```

On Linux, add `--mangohud` before the game path to see FPS, frame times and hardware load while playing. TuxBox saves a CSV of presented frames and a `summary.json` with average FPS and median, p95 and p99 frame times in a capture folder beside the game log. The Frosty launcher enables this automatically when MangoHud is installed.

On Linux, add `--offline` before the game path to launch without network access. The game runs in a new network namespace, so its crash handler cannot upload reports. ProjectScarlett's handler sends minidumps to an online crash service unless it is blocked. Offline launches need unprivileged user namespaces.

```sh
python tuxbox.py run --offline /path/to/game
```

Run `python tuxbox.py inspect /path/to/game` to see the executable TuxBox will launch and what it found in the package’s `.bin` files. For Frosty, its shipping symbols can also turn addresses from the log into function names. [Symbol lookup example](MATRIX_AWAKENS.md#what-carries-over-to-projectscarlett)
