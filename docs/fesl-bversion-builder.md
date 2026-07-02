---
name: fesl-bversion-builder
description: "How the FESL B-version string \"mercs2-pc_ver_%d\" is built + the content-version int formula"
metadata: 
  node_type: memory
  type: reference
  originSessionId: 2f2e9d64-2da3-4d31-904d-2fb53e40e81d
---

FESL `B-version` content-version string is built in `FUN_008445d0` (live 0x008445d0): `_snprintf(player+0x164, 0x3f, "%s_ver_%d", "mercs2-pc", versionInt)`. Format str @0x00be2da4 = `"%s_ver_%d"`; platform @0x00b9279c = `"mercs2-pc"`. NOTE the Ghidra decomp's `unaff_EDI` arg is wrong — EDI is the dest buffer (player+0x164); the int is a stack param.

**versionInt computed in `FUN_006c8cd0`** (was mis-analyzed as bogus FUN_006c8cbf; repaired 2026-06-30 via scripts/ghidra_scripts/RepairFnBoundaries.java → now clean in all_functions_decomp.txt). Live 0x006c8d2b–0x006c8d5f:
- A = `[0x017C0DF8]`, B = `[0x01175C68]` (content-version override globals).
- Override A is a DORMANT/reserved slot: hard-zeroed at startup by the online-config default-init (writer `0x6b3097 mov [0x017C0DF8],eax` after `xor eax,eax` @0x6b301e; same fn sets the online-enable gate `[0x017C0BC8]=1`). NO other writer in readable .text; zero writers in full decomp; live value 0. So version always falls through A → B → default.
- Override B set only if launched with config/cmdline param name-hash `0xea7dfc85` (parser FUN_004c2c20). Engine takes the arg's VALUE *string* and runs it through its own hash (FUN_0082427f = pandemic_hash_m2, FNV-1a |0x20 +^0x2a finalize) → DAT_01175c68, then XOR'd with 0x6b3c35eb. (i.e. you pass a version STRING, not a number.) Cmdline store tokenized by FUN_008268a0, looked up by name-hash via FUN_00826820. Sibling keys cracked via pandemic_hash_m2: 0x949a9b14="demo", 0x1a7e2e03="code_version" (strtol→DAT_01175c60), 0x263a72e8="data_version" (strtol→DAT_01175c64). 0xea7dfc85 name compiled away (hash-only compare; not in packed exe strings / rainbow table) — to recover, bp the tokenizer at startup. Override A has NO cmdline key/setter (reserved). Live cmdline this session = exe path only → no override → default -320369524.
- If A!=0 use A; elif B!=0 use B; else default `0xECE78C8C`.
- Chosen global is XOR'd with key `0x6B3C35EB` (default 0xECE78C8C path skips the XOR). If `global == 0x6B3C35EB` exactly → versionInt=1.
- The `%s` platform-flag byte read from `[0x017C0DE9]`.

**Signed `%d` on a 32-bit value** → high-bit-set values print negative (not an overflow bug; XOR key makes it look like noise).

Build matrix (PC, game-files/): A237.exe ≡ unpatched.uncracked.exe (sha ada554, PE ts Aug-13-2008); signed.exe (PE ts Sep-12-2008). 53MB patched.cracked.crusedll (Aug-13) + patched.uncracked (Sep-12) are the DE-SECUROM'd counterparts of the ~17MB originals (matched by PE ts). compute fn (0x6c8cd0) is SecuROM-ENCRYPTED on disk in the ~17MB builds (disasm=garbage) but DECRYPTED in the 53MB (bakes 0xECE78C8C x3); builder 0x8445d0 is plaintext in all. So version-compute constant is NOT statically recoverable from retail/SecuROM images — must read live (as done on the 53MB). All local PC builds → -320369524. 1555048492/-487563994 are NOT produced by any local PC build → other platform/patch (Xbox360 default.xex, Jul-11 proto, PS3). crusedll = cruise.dll SecuROM crack, NOT our pmc_bb.dll. Full decomps: output/_ghidra/Mercenaries2.A237.exe_decomp.txt + Mercenaries2-signed.exe_decomp.txt (project proj17). Reusable per-build export: scripts/ghidra_scripts/DecompileAllByName.java.

**CRACKED FORMULA (2026-06-30, from PPC builds):** version = ((seed ^ 0x811C9DC5) * 0x01000193) ^ K, then 0→1. (0x811C9DC5=FNV-1a basis, 0x01000193=FNV prime.) seed = content fingerprint, K = content-version counter. Helper fn: x360 retail 0x82855980 / Jul-proto 0x8284f830 (`xoris r,r,0x811c; ori r,r,0x193; mullw; xor r4; bne; li 1`). PC retail bakes the precomputed result; PPC computes live; PC dev-override uses pandemic_hash_m2(string) ^ 0x6B3C35EB instead.
- ALL base-game builds share seed 0x15F119BE. (seed^basis)*prime = 0xECE78DA1 (constant); only K varies.
- Jul-11-2008 prototype (X360): K=0x12C(300) → 0xECE78C8D = -320369523.
- Retail Xbox360 (JTAG) + retail PC: K=0x12D(301) → 0xECE78C8C = -320369524.
**Functional range / valid-vs-invalid:** field is opaque 32-bit equality token (signed %d display, full int32 range); ONLY 0 is excluded (always remapped to 1). No bounds check anywhere. GATE = Theater matchmaking filter `filter_version` built in FUN_00983d30 (sprintf s__s_filter_version_ @0xb61060) = exact-equality on the client's OWN version → client only sees/joins games whose version == its own. Real EA server filters by it; OUR mercs2_server.py IGNORES it (pnow Start returns ALL games, enter_game joins by GID, no version check) → mismatch does NOT block on our server. B-version in FESL handshake is logged only (game_version), never rejected.
**Modded MP context (why 1555048492/-487563994 exist):** community members joining with DIFFERENT mods compute DIFFERENT version stamps (mods hit PC override path hash(verString)^0x6B3C35EB, or patch baked default) → on a version-filtering path they can't see each other. Fix for interop: identical mod set (same stamp), OR force a common version (override arg 0xea7dfc85 / ASI writes 0x01175C68 / patch baked 0xECE78C8C), OR rely on our non-filtering server (residual risk = gameplay desync, not the version field). 1555048492/-487563994 = two different mod configs' fingerprints; neither invalid, they just don't MATCH.
