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
| **Browser profiles** | Chrome, Edge, Brave, Opera, Vivaldi, Firefox: bookmarks, preferences, extensions, history (Firefox: full profile). You pick which profiles. | Files go back to their original profile folders |
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

**Requirements**
- Windows 10/11 x64
- [Rust](https://rustup.rs) stable **1.95 or newer** (MSVC toolchain)
- Visual Studio Build Tools with "Desktop development with C++"
- Optional: [NSIS](https://nsis.sourceforge.io) for the installer

```powershell
git clone https://github.com/<your-username>/vaultpack.git
cd vaultpack
cargo build --release
```

The binary is `target\release\vaultpack.exe`.

To produce the portable exe and installer in `dist\`:

```powershell
.\build.ps1
```

If the window doesn't open on a machine without OpenGL:

```powershell
$env:SLINT_BACKEND = "winit-software"
.\target\release\vaultpack.exe
```

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
