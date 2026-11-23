# Prior art: what was taken from where

Two prior Python implementations exist; neither is a source of truth. Both were audited against the vendor binaries and this project's disassembly (`docs/reference/01-da-protocol.md`).

| Project | Lang / Licence | Established | How this project differs |
|---|---|---|---|
| [`NekobaTechnology/holux-m241-wnro-fix`](https://github.com/NekobaTechnology/holux-m241-wnro-fix) (`7da7d65`) | Python / MIT | `0x402F4` epoch patch (`935_280_000`), DA load `0x00100800`, `$PMTK180*3B` route, "+1024 patch" negative result, dump-twice-and-compare gate | DA protocol derived from disassembly; NMEA tri-state probe before reset lines; profile-based image check; backup diff & post-write read-back |
| `holux-m241-wnro-toolkit` v1 (`6651f8f`, 468 passed / 2 skipped) | Python / MIT | Full CLI, measured echo lags, 2-byte `0xD3` accept reply, no read trailer, log format quirks | Rust; DA driver rebuilt from disassembly; log parser/exporters re-derived from format spec |

## Evidence levels

- **verified**: measured on a retail M-241 (`docs/dev/05-verification.md`).
- **measured by v1**: reported in v1's hardware session (still hypothesis here until reproduced).
- **read**: verified in prior-art source text.
- **hypothesis**: inferred.

## Claims audited against disassembly & hardware

- **`0xD3` reply (`5A 00` vs `A5`)**: Both prior tools read 2 bytes; v1 raised on non-zero status. **Refuted by disassembly** (`01-da-protocol.md` §4.1): status is always `0x00` on accept, and refusal is a **single `0xA5`** (reading 2 bytes after refusal desynchronises).
- **WRITE16 echo lag (`N−1`)**, **no read checksum trailer (`0xA4` separate)**, **10-byte `0xD3` frame**, **DA load `0x00100800`**, **20-byte DA banner (`>BHBLHHHHL`, `C0 03 08 49 … 00C2 22BA`)**: independently corroborated across prior art and hardware/disassembly.
- **BootROM firmware base**: Nekoba names three values (`0x20000000`, `0x0`, `0x200000`); this project **verified `0x00200000`** (`0x20000000` returns unmapped `42 20 02 02`).
- **`0xD2` baud change**: prior tools disagreed (3 B vs 5 B); resolved by disassembly (`01-da-protocol.md` §4.5: 2 args + `0xC0`/`0x5A` handshake).
- **Short final `0xD5` packet**: Nekoba only used 458,752 B (`0x70000`); v1 and this project measured 92-byte final packet for 406,620 B V1.13 (`05-verification.md` §1.5).

## Five consequential differences

1. **BootROM entry**: Nekoba pulses DTR/RTS unconditionally in its dumper (hangs running firmware until battery removal). This project probes NMEA first (`StreamState` tri-state, `AGENTS.md` §2.1) and uses `$PMTK180*3B` (`EntryTiming` from v1, verified 2026-11-22 in `05-verification.md` §1.5).
2. **Pre-/post-write checks**: Nekoba only prints backup SHA-256 and does no read-back; v1 also skips read-back. This project enforces backup-diff (or explicit `--allow-vendor` fixed V1.13 SHA), `verify_device`, and full post-`0xD9` read-back.
3. **Accepted images**: Nekoba requires 458,752 B (`b83204e0…`) and skips mismatched patch bytes with exit 0. This project matches measured V1.12 (406,312 B) / V1.13 (406,620 B) profiles and refuses any unexpected byte.
4. **Version marker**: Nekoba hard-codes `0x59C2C` (V1.13 only). This project uses per-profile offsets (`0x59C28` V1.13, `0x59B14` V1.12); on V1.13 both produce identical patched SHA-256 `901555843895054f77716364a446e8ad41c00b799c781b4ff95144352c141c9a`.
5. **Flash read base**: fixed at verified `0x00200000` (BootROM) vs `0x00000000` (DA).

## Dangers deliberately not copied

Unconditional DTR/RTS pulsing; user-supplied `--start-addr` into `0xD3`/`0xD5`; omitting post-write read-back; continuing past patch/checksum mismatches with exit 0; overwriting per-unit data without an explicit separate gate; porting unverified DA framing.

## Facts taken & why no code was copied

Facts adopted (not code): `0x402F4` → `935_280_000` (`0x402DC` unused; credits catea.tech / Nekoba / v1), DA load `0x00100800`, `$PMTK180*3B` route, "+1024" negative result, dump-twice-and-compare, and Nekoba's 4-byte DA read-back before jump. Both prior projects are MIT, but code was not copied because `CONTRIBUTING.md` requires `01-da-protocol.md` citations for all DA constants and the Rust implementation is re-derived from specs. No proprietary binaries are redistributed.
