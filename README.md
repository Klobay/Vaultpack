<div align="center">

# 🛡️ Vaultpack

**Back up your files and system extras into one secure archive, and restore them to their original paths on a fresh Windows install.**

![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11%20x64-0078D4)
![Language](https://img.shields.io/badge/language-Rust-DEA584)
![UI](https://img.shields.io/badge/UI-Slint-2F7DF6)
![License](https://img.shields.io/badge/license-MIT-green)

</div>

---

## Overview

Vaultpack is a native Windows desktop app written 100% in Rust. It packs the files and folders you choose, plus things like browser profiles, saved Wi-Fi networks and your installed-programs list, into a single `.vpk` archive. On a new PC you open that archive and restore everything to where it came from, even if your username or drive letters changed.

## Features

### Backup
- Drag & drop, click-to-browse, or native file/folder pickers
- Exclude rules (`*.tmp`, `node_modules`, cache folders) with one-click presets
- Reusable backup profiles (save and load your selection)
- Pre-scan with total size, file count, free-space check, unreadable-path warnings, and a warning when the destination is on the same drive as the source
- Locked files are read through Volume Shadow Copy when running as administrator, otherwise skipped and reported
- Automatic verification pass after the archive is written
- Writes to a temporary file and renames on success, so a failed backup never leaves a broken archive

### Special presets
| Preset | What it saves | Restore |
|---|---|---|
| **Browser profiles** | Chrome, Edge, Brave, Opera, Vivaldi, Firefox, Zen, LibreWolf, Waterfox: bookmarks, preferences, extensions, history (Firefox-based browsers like Zen: full profile). You pick which profiles. | Files go back to their original profile folders |
| **Wi-Fi networks** | All saved profiles with keys via `netsh wlan export` | Re-added with `netsh wlan add profile` |
| **Installed programs** | Registry uninstall entries plus `winget` matches (name, version, publisher, winget ID) | Checklist; matched programs install via `winget`, the rest become an exportable manual-reinstall list (CSV/TXT) |

> Chromium saved passwords and cookies only decrypt on the original Windows account. Wi-Fi keys are stored in clear text, so set a password when using that preset.

### Restore
- Open a `.vpk` by dialog, drag & drop, or double-click
- Lazy tree with tri-state checkboxes, per-node sizes, search, Select All / Deselect All / Invert / Expand all / Collapse all
- Path tokens (`%USERPROFILE%`, `%APPDATA%`, `%LOCALAPPDATA%`, `%PROGRAMDATA%`) and drive letters are resolved for the current machine, with optional drive remapping or restoring to a different root
- Conflict policies: overwrite, skip, keep both, overwrite only if older
- Timestamps, attributes and empty folders are restored
- Protected locations (Program Files, Windows, other users) are detected, with an option to relaunch as administrator
- Every restored file is hash-checked; final summary of restored, skipped and failed items with reasons, and an exportable log

### Interface
- Frameless Fluent-style window with Mica/acrylic backdrop (solid colour fallback)
- Dark and light themes that follow the system
- English and Arabic, with RTL layout
- Progress overlay with overall and per-file bars, speed, ETA, pause and cancel; all work runs on background threads so the UI never blocks

## The `.vpk` format

| Part | Details |
|---|---|
| Layout | Magic bytes + version, header, chunked data, manifest, footer index |
| Chunking | Files streamed in 4 MiB chunks, so huge backups never load into RAM |
| Compression | zstd (Fast / Recommended / Maximum) |
| Integrity | BLAKE3 hash per file |
| Encryption (optional) | Argon2id key derivation + XChaCha20-Poly1305 per chunk. The manifest is encrypted too, so file names are hidden. No password means no encryption. |
| Manifest | Original path, type (file / dir / symlink / junction), size, timestamps, attributes, hash, chunk offsets |

## Download

Grab the latest build from the [Releases](../../releases) page:
- `Vaultpack-Setup.exe`: installer (registers the `.vpk` extension and icon)
- `Vaultpack-Portable.exe`: single exe, no install

## Build from source

Follow these steps to build your own copy of Vaultpack on Windows.

### 1. Install the requirements

| Tool | Why | Where to get it |
|---|---|---|
| **Git** | to clone the repo | https://git-scm.com/download/win |
| **Rust** (stable **1.95 or newer**, MSVC toolchain) | to compile the app | https://rustup.rs (download and run `rustup-init.exe`, choose the default install) |
| **Visual Studio Build Tools** with the **"Desktop development with C++"** workload | the linker and Windows SDK Rust needs | https://visualstudio.microsoft.com/visual-cpp-build-tools/ (rustup will offer to install it) |
| **NSIS** (optional) | only for building the installer | https://nsis.sourceforge.io |

After installing Rust, close and reopen your terminal, then check it:

```cmd
rustc --version
cargo --version
```

If `rustc` shows a version older than 1.95, update it:

```cmd
rustup update stable
```

### 2. Clone the repo

```cmd
git clone https://github.com/<your-username>/vaultpack.git
```

### 3. Open the project folder in a terminal

Go into the folder that contains `Cargo.toml`:

```cmd
cd vaultpack
```

Tip: you can also open the folder in File Explorer, click the address bar, type `cmd` and press Enter.

### 4. Build

```cmd
cargo build --release
```

The first build takes several minutes because it downloads and compiles all dependencies. Later builds are much faster.

When it finishes, your own version of the app is here:

```
target\release\vaultpack.exe
```

Run it directly or copy it anywhere. It is a single, self-contained exe.

### 5. (Optional) Build the portable exe and installer

With NSIS installed, run this from the project folder:

```powershell
powershell -ExecutionPolicy Bypass -File .\build.ps1
```

Output in the `dist\` folder:
- `Vaultpack-Portable.exe`: single exe, no install
- `Vaultpack-Setup.exe`: installer (needs NSIS)

If NSIS is not installed, the script still builds the portable exe and skips the installer.

### Troubleshooting

- **`link.exe not found` or similar linker errors:** install Visual Studio Build Tools with the "Desktop development with C++" workload, then reopen the terminal.
- **Rust version errors:** run `rustup update stable`.
- **The window doesn't open or is black (no OpenGL):**
  ```powershell
  $env:SLINT_BACKEND = "winit-software"
  .\target\release\vaultpack.exe
  ```
- **PowerShell blocks `build.ps1`:** use the `-ExecutionPolicy Bypass` command shown above.

## Tests

```powershell
cargo test -p vaultpack-core
```

Covers round-trips (encrypted and unencrypted), wrong password, corrupted archives, conflict policies, path tokenising and drive remapping, protected-location detection, long paths, unicode names, symlinks, and mocked Wi-Fi / program / browser presets.

## Project structure

```
vaultpack/
├─ crates/
│  ├─ vaultpack-core/   archive format, crypto, chunk pipeline, presets, restore
│  └─ vaultpack-app/    Slint UI and app wiring
├─ installer/           NSIS script
└─ build.ps1            release + portable + installer build
```

## Known limitations

- Wi-Fi export and Volume Shadow Copy need administrator rights.
- Junctions and directory symlinks are told apart by their target, which is a heuristic.
- Chromium passwords and cookies cannot be decrypted on a different Windows account.

## License

MIT. See `LICENSE`.
