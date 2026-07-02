# Mercenaries 2 — Install / Registry / Version Contract (for the modkit)

Everything we mined from the **base installer** (`EASetup.exe` + `installerdata.xml` + `/Support/mnfst.txt`)
and the **v1.1 patch** that the modkit should adopt. The headline: **the multiplayer version is keyed
by the installer-written registry `Region` value**, so normalizing `Region` across all modkit users is
the fix for "can't see each other in matchmaking."

## 1. The version ⇄ region mechanism (why this matters)

```
EASetup.exe -locale {locale}  →  HKLM\…\EA Games\Mercenaries 2 World in Flames\Region = "mercenaries2_<sku>"
game startup                  →  reads Region  →  (transform)  →  override A  (config block 0x017C0BC0 + 0x238 = 0x017C0DF8)
                                  version int   =  override A ^ 0x6B3C35EB        (0 → default 0xECE78C8C = -320369524)
B-version / matchmaking filter =  "mercs2-pc_ver_<version int>"
```
- **No registry key** (loose copy) → `Region`="Invalid Region" → override A = 0 → version `-320369524` (default).
- **Key present** → override A set from `Region` → different region ⇒ different version ⇒ **region-segregated lobbies**.
- Proven live: adding the installer key (`Region=mercenaries2_enru`) flipped override A `0 → 0x378C1DC7`,
  version `-320369524 → 1554903084`.
- Exact `Region→overrideA` transform is still being pinned (it's not a plain hash of the string), **but it
  doesn't matter for interop** — only that every client uses the *same* `Region`.

### Region/SKU strings the game recognizes (from decomp)
| Region value | SKU / locales | v1.1 patch |
|---|---|---|
| `mercenaries2_na` | North America (`en_US`) | "North America" |
| `mercenaries2_enru` | EU English/French/Russian (`en_GB`,`fr_FR`,`ru_RU`) | "EU (UK, France, Russia)" |
| `mercenaries2_esit` | EU Spanish/Italian (`es_ES`,`it_IT`) | "EU (Spain, Italy)" |
| (German, `de_DE`) | German SKU — string unconfirmed in decomp (likely `mercenaries2_de`) | "EU (Germany)" |

## 2. The registry key (the contract)

Path (32-bit app → WOW6432Node on x64): `HKLM\SOFTWARE\WOW6432Node\EA Games\Mercenaries 2 World in Flames`

| Value | Type | Observed | Notes for modkit |
|---|---|---|---|
| `Region` | REG_SZ | `mercenaries2_enru` | **★ The matchmaking key. Set ONE fixed value for all users.** |
| `Install Dir` | REG_SZ | `C:\Games\Mercenaries 2 World in Flames\` | Must be the user's real path (game/patcher use it). |
| `Locale` | REG_SZ | `en_UK` | cosmetic |
| `Language` | REG_SZ | `English (UK)` | cosmetic |
| `Product GUID` | REG_SZ | `{26FDF89A-FA65-4FA2-8522-37CC84DFDCEE}` | constant |
| `Registration` | REG_SZ | `Software\Electronic Arts\EA Games\Mercenaries 2 World in Flames\ergc` | constant |
| `Patch URL` | REG_SZ | `http://www.mercs2.com/patch` | dead URL |
| `DisplayName` / `ProductName` | REG_SZ | `Mercenaries 2: World in Flames(tm)` | cosmetic |
| `Folder` | REG_SZ | Start-menu folder | cosmetic |
| `CD Drive` / `Installed From` | REG_SZ | `N:\` | cosmetic |
| `Suppression Exe` | REG_SZ | (empty) | — |
| subkey `1.0` | — | `DisplayName`, `Language`=16, `LanguageName`=Russian, `ProgramGroup` | cosmetic |

**For multiplayer interop the only values that matter are `Region` (uniform) and `Install Dir` (correct).**

## 3. EASetup.exe invocation (from installerdata.xml)
```
/__Installer/DISK1/EASetup.exe -silent "INSTALLLOCATION=""{installLocation}""" -locale {locale} -autologging
```
`gameVersion="2.0.1.0"`, `manifestVersion="1.1"`. The modkit can either drive EASetup for a clean install, or
(better for loose copies) just write the registry key itself (see §2 + the `.reg` template).

## 4. Locale → language content map (from installerdata.xml `<exclude>` lists)
Each locale ships its own VO/text and **excludes** the others. Language files:
`data/<Lang>.wad` + `data/Audios/vo_stream.<lang>.pws` for Lang ∈ {English, German, Spanish, French, Italian, Russian}.
Locale → keep mapping (everything not listed is excluded):
- `en_US`/`en_GB` → English · `de_DE` → German · `es_ES` → Spanish · `fr_FR` → French · `it_IT` → Italian · `ru_RU` → Russian

The modkit can use this for a clean "pick a language → keep the right `.wad`/`.pws`, drop the rest" feature.

## 5. Install file manifest
`/Support/mnfst.txt` — **1,869 quoted filenames** = the canonical clean-install inventory (exe, all `d3dx9*`/`binkw32`/
audio dlls, `Mercs2.ini`, `GL.ini`, language wads, …). Use it to **verify a clean/complete install** and to tell
stock files from mod-added ones.

## 6. v1.1 patch
- Region-named zips (`Germany` / `Spain,Italy` / `UK,France,Russia`=`enfrru` / `North America`) contain a
  **byte-identical** `mercenaries2_patch1_*.exe` → the patch is **region-agnostic**; it only swaps `Mercenaries2.exe`
  (backing the original up to `BACKUP/`). Patch is dated 12-Sep-2008 (= the `Sep-12` patched-exe build).
- Online-relevant fix in notes: *"can no longer create accounts using spaces in names → online name search works."*
- Modkit: ship/apply the v1.1 patched exe as the MP baseline; mirror the backup-before-swap behavior.

## 7. Modkit action items (priority order)
1. **★ "Normalize region" step** — write the §2 key with a single agreed `Region` (e.g. `mercenaries2_na`) for
   *every* user → uniform `mercs2-pc_ver_…` → cross-install matchmaking works. (Requires writing HKLM → elevation.)
2. **Language manager** — use §4 to install/keep the correct language `.wad`/`.pws`.
3. **Install verifier** — use §5 (`mnfst.txt`) to check completeness / flag tampering.
4. **v1.1 baseline + backup convention** — §6.

See [[fesl-bversion-builder]] for the full version/override RE and the SecuROM-dump method.
