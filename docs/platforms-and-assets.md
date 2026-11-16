# Supported Platforms, Installers, and Third-Party Asset Provenance

This document defines the tested OS support matrix, per-platform runtime dependencies, Windows/Linux release packaging, Dota 2 Game State Integration (GSI) setup, and third-party asset provenance for `truesight-dota`.

## 1. Platform Support Matrix

| Target / Mode | Status | Screen Capture Backend | Persisted Config & Cache | Verification Scope |
|---------------|--------|------------------------|--------------------------|--------------------|
| **Linux (`x86_64`) — X11 / XWayland** | **Supported** | `CAP: X11` (`xprop` + ImageMagick `import -silent`) | `$HOME/.config/truesight-dota/config.toml`, `$HOME/.local/share/truesight-dota/cache` | Tested in Ubuntu dual-feature CI (`--all-features` / `--no-default-features`) + live host X11/XWayland builds |
| **Linux (`x86_64`) — Native Wayland** | **Manual / GSI only** (`#13`) | `CAP: MANUAL` (Wayland Portal backend planned in `#13`) | Same as Linux X11 | Covered by capture-factory degradation tests |
| **Windows (`x86_64`) — Win32/GDI** | **Supported (Release Packaging + CI Smoke Verified; live `dota2.exe` check tracked in `#12`)** | `CAP: WIN32` (`xcap` `PrintWindow`/GDI, Borderless Window or Windowed Dota 2 recommended) | `%APPDATA%\truesight-dota\config.toml`, `%LOCALAPPDATA%\truesight-dota\cache` | Tested in `windows-latest` CI (Clippy, unit tests, `--release` `--demo` smoke run, `.msi`/`.zip` packaging); live desktop `dota2.exe` validation tracked in `#12` |
| **Headless CLI (`--no-default-features`)** | **Supported** | Manual hero commands (`--cli`, `--demo`, `--sync`) + GSI HTTP listener | Same per-OS config/cache paths | Verified cleanly in both Linux and Windows headless CI builds |
| **macOS / Other OS** | **Unsupported** | `CAP: MANUAL` fallback | `$HOME/.config` / `$HOME/.local/share` | Not built or shipped in release artifacts |

### Per-Platform Runtime Dependencies

- **Linux (`x86_64`)**:
  - **Live X11/XWayland capture**: `xprop` (`x11-utils` / `xorg-xprop`) and ImageMagick (`import`) on `PATH`. If missing, TrueSight Dota degrades cleanly to `CAP: MANUAL` while keeping manual hero entry and Dota 2 GSI active.
  - **Wayland & desktop icon integration**: The GUI sets `app_id = "truesight-dota"`, defaults to `1180x700` logical points (`min_inner_size = 640x440` logical points with automatic scrollable fallback below `960x680` logical points so `125%`–`150%` DPI scaling on `720p`/`768p` displays stays usable), and embeds the Gem of True Sight window icon directly from `hero_atlas.bin`. On Wayland/Linux desktop environments, run `install-desktop.sh` (included in the release tarball and [`packaging/linux/`](../packaging/linux/)) to register `truesight-dota.desktop` and the Gem PNG icon (`packaging/icons/truesight-dota.png`).
  - **OpenDota / STRATZ sync**: `curl` on `PATH`.
  - **Building GUI from source**: `pkg-config`, `libxkbcommon-dev`, `libxcb-render0-dev`, `libxcb-shape0-dev`, `libxcb-xfixes0-dev`, `libwayland-dev`, `libgl1-mesa-dev`, `libegl1-mesa-dev`, `libgtk-3-dev` (not needed with `--no-default-features`).
- **Windows (`x86_64`, Windows 10 / Windows 11)**:
  - **Capture backend**: Built-in Win32/GDI (`xcap`), zero extra runtime downloads, no WGC yellow capture border. Run Dota 2 in **Borderless Window** (recommended) or **Windowed** mode; exclusive fullscreen may yield blank/unreadable `PrintWindow` frames and fall back to manual input.
  - **Process & HTTP sync**: Built-in Windows `System32\tasklist.exe` (resolved via `%SystemRoot%` / `%WINDIR%` to verify owning `dota2.exe` PID) and `curl.exe` (`System32\curl.exe` / `PATH`, invoked with `CREATE_NO_WINDOW`).
  - **Steam detection**: Reads `%ProgramFiles(x86)%\Steam\config\loginusers.vdf`, `%ProgramFiles%\Steam\config\loginusers.vdf`, `%LOCALAPPDATA%\Steam\config\loginusers.vdf`, and `HKCU\Software\Valve\Steam\SteamPath`.
  - **Console UX note**: Default Windows release binaries preserve the console subsystem so `--cli`, `--demo`, and `--sync` retain full stdin/stdout/stderr and exit codes. Launching from Explorer/Start Menu opens a companion console window behind the GUI until separate GUI launcher packaging is added.
  - **Windows SmartScreen / Unsigned Binaries**: Release `.exe` and `.msi` files are open-source community builds and are **not EV/Authenticode code-signed**. Windows SmartScreen may show *"Windows protected your PC"* on first run; verify integrity against the release `SHA256SUMS` file before choosing *More info -> Run anyway*.

---

## 2. Game State Integration (`gamestate_integration_truesight.cfg`)

TrueSight Dota listens on loopback `http://127.0.0.1:4001/` (`TRUESIGHT_GSI_ADDR` override, bounded 64 KiB JSON body limit). Every release archive includes [`packaging/gamestate_integration_truesight.cfg`](../packaging/gamestate_integration_truesight.cfg).

- **Windows automatic helper**: Run `install-gsi.ps1` (bundled inside both `truesight-dota-windows-x86_64.msi` and `truesight-dota-windows-x86_64.zip`). It queries `HKCU\Software\Valve\Steam\SteamPath` plus `libraryfolders.vdf` and copies `gamestate_integration_truesight.cfg` into `<SteamLibrary>\steamapps\common\dota 2 beta\game\dota\cfg\gamestate_integration\`.
- **Linux manual placement**: Copy `gamestate_integration_truesight.cfg` to `~/.local/share/Steam/steamapps/common/dota 2 beta/game/dota/cfg/gamestate_integration/gamestate_integration_truesight.cfg` (or `~/.steam/steam/...`).
- **Steam launch option**: In Steam -> Dota 2 -> *Properties* -> *General* -> *Launch Options*, add `-gamestateintegration`.

---

## 3. Release Packaging & Verification Procedure

Tagged `v*` builds in [`.github/workflows/release.yml`](../.github/workflows/release.yml) run full format/Clippy/test gates and release `--demo --no-color` smoke tests before publishing:

- `truesight-dota-linux-x86_64.tar.gz` (`truesight-dota`, `gamestate_integration_truesight.cfg`, `truesight-dota.desktop`, `install-desktop.sh`, `truesight-dota.png`, `LICENSE`, `README.md`)
- `truesight-dota-windows-x86_64.msi` (per-user WiX v4 MSI installer into `%LOCALAPPDATA%\Programs\TrueSight Dota` + Start Menu shortcut with Gem icon, no admin UAC required)
- `truesight-dota-windows-x86_64.zip` (portable archive including `truesight-dota.exe`, `gamestate_integration_truesight.cfg`, `install-gsi.ps1`, `truesight-dota.ico`, `LICENSE`, `README.md`)
- `SHA256SUMS` (unified SHA-256 checksums for all artifacts in the release)

Verify checksums locally after download:
```bash
# Linux
sha256sum -c SHA256SUMS --ignore-missing
```
```powershell
# Windows PowerShell
Get-FileHash .\truesight-dota-windows-x86_64.msi -Algorithm SHA256
```

---

## 4. Third-Party Asset & Data Provenance

The repository's [Apache License 2.0](../LICENSE) covers **only the original source code, documentation, and packaging scripts authored in `truesight-dota`**. It does **not** grant or claim Apache-2.0 ownership or relicensing rights over third-party game artwork, names, schemas, or match datasets listed below:

| Asset / Data Source | Location in Repo | Provenance & Rights Notice | Redistribution / Replacement Considerations |
|---------------------|------------------|----------------------------|---------------------------------------------|
| **Base Hero & Gem Atlas + Derived Desktop Icons** (`127` hero `64x36` RGB portraits + Gem artwork) | [`crates/truesight-adapters/assets/hero_atlas.bin`](../crates/truesight-adapters/assets/hero_atlas.bin), [`packaging/icons/truesight-dota.png`](../packaging/icons/truesight-dota.png), [`packaging/windows/truesight-dota.ico`](../packaging/windows/truesight-dota.ico) | Extracted/derived from official **Dota 2** game assets (**Valve Corporation** IP, not Apache-2.0) | Embedded for offline visual matching and HUD/launcher display; redistribution permission / redistributable replacement tracked in `#20` |
| **Arcana / Persona Atlas** (`49` alternate `64x36` RGB portraits across `30` heroes) | [`crates/truesight-adapters/assets/hero_alt_atlas.bin`](../crates/truesight-adapters/assets/hero_alt_atlas.bin) | Extracted from official **Dota 2** VPK assets (**Valve Corporation** IP, not Apache-2.0) | Enables visual top-bar recognition of equipped Arcana/Persona skins; same Valve IP boundary applies |
| **Hero Identifiers & GSI Schema** (`npc_dota_hero_*`, `DOTA_GAMERULES_STATE_*`) | `truesight-core`, `truesight-adapters::gsi` | **Valve Corporation** Dota 2 identifiers and Game State Integration schema | Protocol interoperability identifiers only |
| **Embedded Statistical Snapshots & Live Sync** | `crates/truesight-adapters/src/snapshot_data.rs`, `snapshot_pairs.rs`, `data/*.json`, runtime sync | Derived from public **OpenDota** API (`/api/heroStats`, `/api/players/{id}/heroes`, `/api/players/{id}/matches`) | Public community API aggregated statistics; refreshed automatically into local cache (`24h` meta / `1h` player TTL) |
| **Optional Synergy Matrix Enrichment** | `crates/truesight-adapters/src/sync.rs` | Optional **STRATZ** GraphQL API (`heroStats.heroVsHeroMatchup`) fetched only when user provides `STRATZ_API_TOKEN` | User-supplied API token; never bundled in repository or releases |
