# Solinear's patches to Apollo

On top of Apollo `v0.4.6` (github.com/ClassicOldSong/Apollo).

| Patch | What it does |
|---|---|
| 0001 | `POST /api/input-remap` changes the key map while running (no restart, a stream keeps going); new `keycombos` config: `[held, pressed, replacement]` triples, e.g. Mac Cmd+Tab -> Alt+Tab |
| 0002 | `--creds` keeps the state file's numbers and booleans (it used to turn them into strings, then fail reading its state) |
| 0003 | The Windows service passes its arguments on to `Sunshine.exe`, so a service under another name can use its own config |
| 0004 | Fix-up for 0001: `keycombos` last in `input_t` (the config is initialized by position) |
| 0005 | Builds with current MSYS2: OpenSSL 3.6 accessors, MinGW's own synthetic pointer declarations |
| 0006 | Host shortcuts (Ctrl+Alt+Shift+F<n> etc.) are detected on the keys the client pressed, not their remap, so they work for a Mac client whose Ctrl is remapped |

Build (MSYS2 UCRT64, packages from Apollo's `docs/building.md`):

```bash
git clone --depth 1 --branch v0.4.6 --recurse-submodules --shallow-submodules https://github.com/ClassicOldSong/Apollo.git apollo-src
cd apollo-src && git am ../solinear/apollo-patches/*.patch
cmake -B build -G Ninja -S . -DBUILD_DOCS=OFF -DBUILD_TESTS=OFF -DCMAKE_BUILD_TYPE=Release
# CMake 4 rejects Boost 1.88's locale source_group: in build/_deps/boost-src/libs/locale/CMakeLists.txt add
#   list(FILTER sources_inc INCLUDE REGEX "^include/boost/locale/")
# after the line `list(FILTER sources_inc EXCLUDE REGEX "^src/")`, then run cmake again
ninja -C build
```

Outputs `build/sunshine.exe` and `build/tools/sunshinesvc.exe` (static apart from Windows DLLs).
