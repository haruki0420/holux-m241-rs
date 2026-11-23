import { invoke } from "@tauri-apps/api/core";

/** One serial port, mirroring `m241_app::device::PortInfo`. */
export type PortInfo = {
  name: string;
  description: string | null;
  kind: "usb" | "pci" | "bluetooth" | "unknown" | string;
  likely: boolean;
};

/** The identity and log usage, mirroring `m241_app::device::DeviceInfo`. */
export type DeviceInfo = {
  port: string;
  release: string;
  build: string;
  model: string;
  sdk: string | null;
  knownRelease: boolean;
  logUsedBytes: number;
  logEnabled: boolean | null;
};

/** `list_ports` — read-only enumeration; never opens a port. */
export function listPorts(): Promise<PortInfo[]> {
  return invoke<PortInfo[]>("list_ports");
}

/**
 * `device_info` — the one IPC round trip that touches hardware.
 *
 * The Rust side opens the port at the NMEA baud and sends only read requests.
 * Nothing in this module can write, erase or change a setting.
 */
export function readDeviceInfo(port: string): Promise<DeviceInfo> {
  return invoke<DeviceInfo>("device_info", { port });
}

/**
 * Split the two-line error the Rust side returns.
 *
 * Line 1 is the device's or OS's own message; line 2 is `対処: <next action>`.
 * The screen shows them in different places because they answer different
 * questions ("what happened" vs "what do I do").
 */
export function splitError(raw: unknown): { message: string; hint: string | null } {
  const text =
    typeof raw === "string" ? raw : raw instanceof Error ? raw.message : String(raw);
  const lines = text.split("\n");
  const message = lines[0] ?? text;
  const rest = lines.slice(1).join("\n").trim();
  if (rest === "") {
    return { message, hint: null };
  }
  const hint = rest.startsWith("対処: ") ? rest.slice("対処: ".length) : rest;
  return { message, hint };
}
