# Installation

Needs: a Holux M-241 with a charged battery, its USB cable (CP210x), and Windows / Linux / macOS.

## Get the binary

Releases are built by `.github/workflows/release.yml` on a `vX.Y.Z` tag (**no release published yet** — use a CI artifact until the first tag). Each release ships `LICENSE`, `SHA256SUMS.txt`, and:

| Platform | CLI | GUI |
|---|---|---|
| Linux (x86_64, glibc + `libudev.so.1`) | `m241-linux-x86_64` | `m241-gui-linux-x86_64` |
| Windows (x86_64) | `m241-windows-x86_64.exe` | `m241-gui-windows-x86_64.exe` |
| macOS (aarch64) | `m241-macos-aarch64` | `m241-gui-macos-aarch64` |

Verify against `SHA256SUMS.txt` (`sha256sum -c SHA256SUMS.txt` / Windows `certutil -hashfile <file> SHA256`), optionally rename to `m241` (`m241.exe`), and place on your `PATH`. Binaries are **unsigned**: macOS may require `xattr -d com.apple.quarantine m241` (or Privacy & Security); Windows SmartScreen requires "More info > Run anyway".

## Verify port access

```bash
m241 --version
m241 device ports
```

If no port appears:
- **Windows**: install the Silicon Labs CP210x VCP driver and replug.
- **Linux**: `sudo usermod -aG dialout $USER`, then log out and back in.
- **macOS**: appears as `/dev/tty.usbserial-*` (no driver needed).

## Proprietary file not shipped (`DA_v3.8.4.bin`)

Only `m241 fw flash` needs the 14,896-byte proprietary `DA_v3.8.4.bin` from Holux's update package (reading, patching, log download, and geotagging work without it). Extract it from your local vendor folder:

```bash
m241 device doctor --from-drop <folder> --extract-to vendor/
```
