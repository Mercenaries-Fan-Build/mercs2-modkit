//! The m2 SDK runtime — `m2-sdk.dll`, the shared layer every SDK-based mod links against.
//!
//! Like `pmc_bb.dll`, it is a single shared DLL that many mods need, so it is Modkit-MANAGED rather
//! than shipped: N mods each carrying their own copy would collide on one filename with no
//! arbitration, and the Shipment format bans a `.dll` in a `place_file` for exactly that reason. So
//! Modkit installs the one canonical copy from the SDK's GitHub release, and shipments declare a
//! dependency on it instead of vendoring it.
//!
//! ⚠ It goes in the GAME ROOT, not `scripts/` beside a plugin. pmc_bb's loader resolves a plugin's
//! imports with a plain `LoadLibraryA`, whose search path includes the exe's directory (the game
//! root) but NOT the loaded plugin's own directory — so an `m2-sdk.dll` sitting in `scripts/` beside
//! a `.asi` is invisible to it, and the plugin fails to load with `0x7E` (ERROR_MOD_NOT_FOUND). The
//! SDK's "ship it beside your `.asi`" guidance only holds for a loader using
//! `LOAD_WITH_ALTERED_SEARCH_PATH` (the Ultimate ASI Loader); pmc_bb does not.

/// The SDK repository whose releases publish `m2-sdk.dll`.
pub const REPO: &str = "Mercenaries-Fan-Build/mercs2-sdk";

/// Ledger key for the installed runtime.
pub const KEY: &str = "m2_sdk";

/// The release asset name, and the on-disk name it must keep — a linked `.asi` imports exactly
/// `m2-sdk.dll`, so it cannot be renamed on the way out.
pub const ASSET: &str = "m2-sdk.dll";
pub const INSTALL_NAME: &str = "m2-sdk.dll";

/// Human label for the component list and progress UI.
pub const LABEL: &str = "m2-sdk.dll (SDK runtime for mods)";

/// A 32-bit DLL is tens of KB; anything under this is an error page served with a 200, not the
/// runtime. Kept well below the smallest real build.
pub const MIN_SIZE: u64 = 8 * 1024;
