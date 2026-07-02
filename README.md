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
