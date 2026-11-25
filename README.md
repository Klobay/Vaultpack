# Vaultpack

Rust workspace: `vaultpack-core` (archive format, crypto, presets, restore) and `vaultpack-app` (Slint UI).

## Build (Windows 10/11 x64)
1. Install Rust stable 1.95 or newer (`rustup update stable`) with the MSVC toolchain and Visual Studio Build Tools.
2. Optional: install NSIS for the installer.
3. Run `.\build.ps1` (or `cargo build --release`).

Output: `dist\Vaultpack-Portable.exe` and `dist\Vaultpack-Setup.exe`.

If the window fails to open on a machine without OpenGL, run with `SLINT_BACKEND=winit-software`.

## Tests
`cargo test -p vaultpack-core`
