# Installation

## What you need

| Thing | Why |
|---|---|
| A Holux M-241 with a charged battery | the device being repaired |
| Its USB cable | the M-241 uses a CP210x bridge |
| A PC running Windows, Linux or macOS | one binary, no installer |

## Get the binary

Download `m241` (or `m241.exe` on Windows) and `m241-gui` from the latest CI
artifact or release. Put them somewhere on your `PATH`.

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
