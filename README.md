# mercs2-modkit

A desktop mod manager for **Mercenaries 2: World in Flames**. Load mods, detect
conflicts, and assemble validated `vz-patch.wad` files — no command line required.

Built with **Tauri 2** (Rust) + **Vue 3** + **Tailwind CSS** + **Headless UI**.

## What it does

- **Load mods** from a folder containing a `manifest.json` + raw assets.
- **Auto-detect asset types** (peeks the UCFX block header, falls back to file
  extension) with manifest overrides.
- **Detect conflicts** — assets claimed by more than one mod (same
  `pandemic_hash_m2` key), with accessible per-asset resolution.
- **Assemble patch WADs** — SGES-compresses assets and builds `vz-patch.wad`
  via the published [`mercs2_formats`](https://crates.io/crates/mercs2_formats)
  crate; can merge into an existing WAD.
- **Validate** the output with
  [`wad_simulator`](https://crates.io/crates/wad_simulator) before deploying.

## Community incompatibility reports

Before building any Shipments, Modkit fetches mercs.ink's community list of
incompatibilities (`GET /api/v1/incompatibilities`) and checks the Shipments
against it.

- **Matching** is by mercs.ink public id. Only Shipments installed from
  mercs.ink carry one. A folder staged from disk is never matched, even if it
  has the same name. Reports whose other party is a catalog mod can't match a
  Shipment and are ignored.
- **The check runs twice.** It runs before `qm preflight` with the versions
  recorded at install, and again after with the versions qm read from the
  manifests. The refusal says which versions were compared.
- **Only a `confirmed` report blocks the build.** Every confirmed match goes
  into one refusal. If preflight also refused, its findings are in the same
  message.
- **Other statuses only warn.** `reported`, `disputed` and `resolved` reports
  appear in a banner after the build.
- **Caching:** the list is cached in `mercsink-incompatibilities.json` in the
  app data folder, and revalidated with its ETag.
  - If mercs.ink can't be reached, the cached copy is used, and the banner
    says when it was downloaded.
  - With no cached copy, the build goes ahead, and the banner says it wasn't
    checked.
  - Any other error fails the build: a 4xx other than 429, a list that breaks
    the contract (unknown status, reason or source, or a range that isn't
    semver), or a cache file that can't be read. A list Modkit can't use never
    replaces the cached copy.

## Running the game on macOS and Linux

Mercenaries 2 is a 32-bit Windows game, so on macOS and Linux Modkit launches it
through Wine or Proton. **Game Info → Runtime** shows what a launch uses.

- **macOS:** Modkit downloads and manages its own Wine build from
  [Heroic-Games-Launcher/wine-crossover](https://github.com/Heroic-Games-Launcher/wine-crossover).
  Install one or more builds, then pick the one to use. The game folder can be
  anywhere, for example `~/Downloads`; it doesn't need to be inside the prefix.
  On Apple Silicon the Wine build needs Rosetta 2
  (`softwareupdate --install-rosetta --agree-to-license`).
- **Linux:** the game runs through Steam's Proton. Modkit lists every Proton
  that Steam and `compatibilitytools.d` have installed, and you pick one.

Both share one modkit-managed prefix in the app data folder (`wine-prefix` on
macOS, `proton-prefix` on Linux). Your saves are inside it.

### `runtime.json`

The runtime settings live in `runtime.json` in the app data folder. The UI edits
the Wine/Proton selection and the environment variables. Everything else is set
by editing the file by hand:

| Field | Meaning |
| --- | --- |
| `wineTag` | macOS: the installed Wine build to run. |
| `proton` | Linux: the Proton to run (its folder or `proton` script). |
| `steamRoot`, `sniper`, `useContainer` | Linux: Steam root, Steam Linux Runtime entry point, and whether to use its container. |
| `prefix` | Use this prefix instead of the managed one. |
| `env` | `[{ "key": "…", "value": "…" }]` passed to Wine/Proton. |
| `dllOverrides` | `{ "dll": "n" \| "b" \| "n,b" \| "b,n" \| "d" \| "" }`, sent as `WINEDLLOVERRIDES`. |
| `registry` | Values imported into the prefix before every launch (macOS only). Each one is `{ "key": "HKEY_CURRENT_USER\\…", "name": "…", "value": { "type": "string", "data": "…" } }`; `type` can also be `dword` (a number) or `delete` (no `data`). |
| `winedebug` | `WINEDEBUG` channels, e.g. `+seh,+loaddll`. |
| `exeArgs` | Extra arguments passed after the game exe. |

The file is read strictly. An unknown field, an invalid value, or the same
setting given twice (such as `WINEDLLOVERRIDES` in both `env` and
`dllOverrides`) stops the launch with an error that names the file. You can't
set `WINEPREFIX` or `PMC_VERBOSE_LOG` in `env`, because Modkit sets them itself;
on Linux the same applies to Proton's `STEAM_COMPAT_*` variables. Deleting a
`registry` entry from the file doesn't remove the value from the prefix; use
`"type": "delete"` for that.

## Mod manifest

```json
{
  "name": "Vehicle Pack",
  "version": "1.0.0",
  "author": "modder",
  "description": "Adds new vehicles to Maracaibo",
  "requirements": { "game_version": "1.1" },
  "dependencies": ["weapon-rebalance@^1.0"],
  "assets": [
    { "path": "assets/models/vehicle.block", "name": "models/vehicle_01", "type": "auto", "target_patch": "auto" },
    { "path": "assets/scripts/init.lua", "name": "scripts/dlc01/init", "type": "script", "target_patch": "scripts" }
  ]
}
```

`type` and `target_patch` accept `"auto"` or an explicit value.

## Development

Requires Rust (1.94+) and Node.

```bash
npm install
npm run tauri dev      # run the app
npm run build          # typecheck + build the frontend
cargo build --manifest-path src-tauri/Cargo.toml   # build the backend
```

The validator either downloads the `wad_simulator` release binary on first use
or finds one on `PATH` (`cargo install wad_simulator`).

## Releasing & auto-update

Tagged builds (`v*`) publish installers plus a Tauri-updater manifest
(`latest.json`), which installed copies poll so they can update in-place
(Windows NSIS installs and Linux AppImages; the portable exe and
deb/rpm/flatpak link to the release page instead). Update artifacts are
signed; the app rejects any update whose signature doesn't match the public
key in `src-tauri/tauri.conf.json`.

One-time setup for a new signing key:

```bash
npm run tauri signer generate -- -w ~/.tauri/mercs2-modkit.key
```

Paste the printed public key into `plugins.updater.pubkey` in
`src-tauri/tauri.conf.json`, and add the private key file's contents and its
password as the `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` GitHub Actions secrets. Keep the private
key out of the repo and backed up — losing it means shipping a release users
must reinstall manually; leaking it lets anyone sign updates.

## Architecture

- `src-tauri/src/models/` — manifest, project, and conflict types (serde).
- `src-tauri/src/commands/` — `load_mod`, `detect_asset_type`,
  `build_conflict_graph`, `assemble_patch_wad`, `validate_wad`,
  `fetch_wad_simulator`.
- `src/` — Vue frontend: Pinia store (`stores/project.ts`), router, and the
  Project / Mod Detail / Conflicts / Build views.
