# Security Policy

## Architecture & Memory Safety Guarantees

`truesight-dota` enforces strict memory and input safety across all workspace crates (`truesight-core`, `truesight-adapters`, `truesight-app`):

- `#![forbid(unsafe_code)]` across the entire Cargo workspace.
- `unused_must_use = "deny"` across workspace lints.
- Zero `.unwrap()` or `.expect()` calls on external runtime input (GSI HTTP payloads, OpenDota/STRATZ API JSON, Steam `loginusers.vdf`, or local cache files).
- **100% pure core (`truesight-core`)**: zero network access, zero filesystem access, zero async runtime, zero external dependencies.

## Network & Local Input Boundaries

- **Game State Integration (GSI) Listener**: `validate_gsi_bind_addr` strictly enforces IPv4 (`127.0.0.0/8`) and IPv6 (`::1`) loopback addresses on `TRUESIGHT_GSI_ADDR`, rejecting wildcard (`0.0.0.0`, `::`) and non-loopback interfaces before socket bind; rejects payloads exceeding `MAX_GSI_PAYLOAD_BYTES` (64 KiB), buffering events through a bounded queue.
- **External APIs**: Only queries public HTTPS endpoints when syncing (`api.opendota.com`) or optionally reading public GraphQL matchup synergy (`api.stratz.com` when `STRATZ_API_TOKEN` is configured).
- **Steam Account Detection**: Reads local `loginusers.vdf` in read-only mode to infer the `MostRecent` Steam32 ID locally; no Steam credentials or auth tokens are read or transmitted.
- **Per-Account Cache Isolation**: Player caches live exclusively in local user XDG data storage (`~/.local/share/truesight-dota/cache/players/{account_id}/`).

## Reporting a Vulnerability

If you discover a security issue (such as a parser panic, denial-of-service vector on the GSI HTTP listener, or unsafe credential/path handling):

1. **Please do not open a public GitHub issue** with exploit details before coordination.
2. Open a **GitHub Private Security Advisory** via the repository's **Security** tab (`Advisories -> Report a vulnerability`).
3. Include affected crate/file paths, reproduction steps or malformed input payloads, and expected vs actual behavior.

We aim to acknowledge reports within 72 hours and coordinate a tested fix prior to public disclosure.
