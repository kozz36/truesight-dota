# Valve & Dota 2 Compliance Policy (`truesight-dota`)

TrueSight Dota is designed as a **read-only, post-selection draft & build companion** that respects Valve's Dota 2 Terms of Service, Steam Subscriber Agreement, and competitive fair-play rules. Every feature in `truesight-dota` MUST remain inside the Allowed boundaries below and NEVER cross any of the six non-negotiable Red Lines.

## Quick Summary: Allowed vs. Red Lines

| Category | Status | Scope in `truesight-dota` |
|----------|--------|---------------------------|
| **Official Local-Player GSI** | **Allowed** | Listens on `127.0.0.1:4001` for Valve's official `gamestate_integration` JSON for the local player (`player.team_name`, `hero`, `map`, `draft`). |
| **Dota Plus Feature Parity** | **Allowed** | Hero matchup win-rate percentages, synergy/counter breakdowns, and post-draft item build progressions mirror concepts already offered by Valve's Dota Plus. |
| **External Non-Injecting OS Windows** | **Allowed** | Native `eframe`/`egui` desktop window (or future external transparent OS window) that never injects into `dota2.exe` or hooks DirectX/Vulkan/OpenGL. |
| **Public Aggregate Stats & Opt-In Self Comfort** | **Allowed** | Public hero/matchup statistics from OpenDota and STRATZ, plus **opt-in** Personal Comfort v3 for the **local user's own** linked Steam account only. |
| **Visible Screen Capture (Top-Bar Draft)** | **Allowed** | Matches only the 10 publicly visible top-bar hero portraits during draft/live phases using OS screen capture (`xprop`/`import` or Win32 GDI `xcap`). |
| **Enemy Profile / Match History Lookup During Draft** | **RED LINE** | **Forbidden.** Never query enemy players' Steam profiles, match histories, dotabuff/opendota/stratz player pages, or "Overwolf-style" hero-spammer scouting during draft. |
| **Memory Reading, DLL Injection, Graphics Hooks, Packet Sniffing** | **RED LINE** | **Forbidden.** `#![forbid(unsafe_code)]` across all crates; zero process memory reading, zero DLL/SO injection, zero rendering hooks, zero network packet inspection. |
| **Bypassing GSI `allplayers` Spectator Restriction** | **RED LINE** | **Forbidden.** Never attempt to extract hidden enemy player IDs, hidden picks, or spectator-only `allplayers` data in live matchmaking. |
| **Input Automation (Auto-Pick / Auto-Buy / Macros)** | **RED LINE** | **Forbidden.** Never synthesize mouse clicks, keystrokes, or console commands into Dota 2; the player makes every in-game action manually. |
| **OCR of Enemy Inventories, Fog, or Hidden Scoreboard** | **RED LINE** | **Forbidden.** Never OCR enemy item slots, courier contents, Roshan timers, or anything not already shown in the public top-bar hero portrait strip. |
| **Tournament & League Third-Party Software** | **Disclaimer** | Online/LAN tournaments and leagues (e.g. ESL, PGL, DreamLeague, FACEIT) may enforce stricter third-party overlay bans than Valve's public matchmaking policy. **Do not use TrueSight Dota in any tournament or league unless explicitly permitted by the organizer.** |

---

## Mandatory Feature Gate Checklist

Before merging any new feature, PR, or adapter change in `truesight-dota`:

- [ ] **No Enemy Player Scouting**: Does the feature look up only aggregate hero/item meta stats and the local user's own linked Steam account?
- [ ] **No Process / Graphics / Network Intrusion**: Does the workspace remain 100% `#![forbid(unsafe_code)]` external-window-only with zero injection, hooking, or packet sniffing?
- [ ] **No Input Automation**: Does the user retain 100% manual control over picks, bans, and item purchases in Dota 2?
- [ ] **No Hidden State Extraction**: Does the feature rely strictly on official local-player GSI, public APIs, and visible top-bar hero portraits?
