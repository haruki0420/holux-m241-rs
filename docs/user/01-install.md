# Installation

## What you need

| Thing | Why |
|---|---|
| A Holux M-241 with a charged battery | the device being repaired |
| Its USB cable | the M-241 uses a CP210x bridge |
| A PC running Windows, Linux or macOS | one binary, no installer |

## Get the binary

Releases are built by `.github/workflows/release.yml` on a `v1.0.0`-style tag.
**No release has been published yet** (there is no `vX.Y.Z` tag), so until the
first tag, take the binaries from a CI artifact. A release will carry one CLI
and one GUI binary per platform, plus `LICENSE` and `SHA256SUMS.txt`:

| Platform | CLI | GUI |
|---|---|---|
| Linux (x86_64) | `m241-linux-x86_64` | `m241-gui-linux-x86_64` |
| Windows (x86_64) | `m241-windows-x86_64.exe` | `m241-gui-windows-x86_64.exe` |
| macOS (aarch64) | `m241-macos-aarch64` | `m241-gui-macos-aarch64` |

Verify the download against `SHA256SUMS.txt` (`sha256sum -c SHA256SUMS.txt`;
on Windows, `certutil -hashfile <file> SHA256`). The asset name carries the
platform, so rename it to `m241` (`m241.exe` on Windows) if you like, then put
it somewhere on your `PATH`.

The binaries are **not code-signed**. macOS Gatekeeper may quarantine the
download (`xattr -d com.apple.quarantine m241`, or allow it under System
Settings > Privacy & Security); Windows SmartScreen may block it behind
"More info > Run anyway". The Linux build assumes a glibc distribution with
`libudev.so.1` (it is not a static musl binary).

There is no installer, no daemon, and nothing runs in the background.

## Check it works

```bash
m241 --version
m241 device ports
```

`m241 device ports` lists the serial ports the OS knows about and flags likely
M-241 cables (CP210x, PL2303, FTDI, CH340). If your logger is plugged in and no
port appears:

- **Windows:** the CP210x driver is missing. Install the Silicon Labs driver from
  the `USB Cable Driver(Win10)` package, then replug.
- **Linux:** add yourself to the group that owns the device:
  `sudo usermod -aG dialout $USER`, then log out and back in.
- **macOS:** no driver is needed; the port appears as
  `/dev/tty.usbserial-*`.

## The one file this project does not ship

Writing firmware needs `DA_v3.8.4.bin`, a 14,896-byte program that was bundled
with Holux's own Windows utility. It is proprietary and is **not** included here.

You need it only for `m241 fw flash`. Reading the device, patching the image on
your PC, downloading logs and geotagging photos all work without it.

If you have the old Holux packages, point the tool at the folder:

```bash
m241 device doctor --from-drop <folder> --extract-to vendor/
```

It identifies every file by hash, copies out only the download agent, and tells
you what it found.
