import { formatBytes } from "../lib/format";
import type { DeviceInfo } from "../lib/ipc";

/**
 * The sample card, shown only when the operator asks for it.
 *
 * The values are the device facts recorded in the toolkit's documents, labelled
 * as a sample in the UI. Without it the success layout could only be inspected
 * with a unit attached.
 */
export const SAMPLE: DeviceInfo = {
  port: "/dev/ttyUSB0",
  release: "B-core_1.20",
  build: "0043",
  model: "01017-00F",
  sdk: "1.0",
  knownRelease: true,
  logUsedBytes: 212_860,
  logEnabled: true,
};

/** What the readout strip shows: a real device, the sample, or nothing yet. */
export function readout(info: DeviceInfo | null): { port: string; firmware: string; log: string } {
  return {
    port: info?.port ?? "—",
    firmware: info?.release ?? "—",
    log: info === null ? "—" : formatBytes(info.logUsedBytes),
  };
}
