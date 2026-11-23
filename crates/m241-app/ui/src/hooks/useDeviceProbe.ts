import { useCallback, useEffect, useMemo, useState } from "react";

import {
  listPorts,
  readDeviceInfo,
  splitError,
  type DeviceInfo,
  type PortInfo,
} from "../lib/ipc";

/** What the Connect card is showing right now. */
export type ProbeState =
  | { kind: "idle" }
  | { kind: "probing"; port: string }
  | { kind: "connected"; info: DeviceInfo }
  | { kind: "error"; message: string; hint: string | null };

/** The header chip's four states (`error` still means "not connected"). */
export type ConnectionTone = "idle" | "probing" | "connected" | "error";

/**
 * Owns the Connect screen's state: the port list, the chosen port and the last
 * probe. The state lives above the screen because the header and the status bar
 * both show the connection, and three copies would drift.
 */
export function useDeviceProbe() {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [portsLoading, setPortsLoading] = useState(true);
  const [selectedPort, setSelectedPort] = useState("");
  const [manualPort, setManualPort] = useState("");
  const [state, setState] = useState<ProbeState>({ kind: "idle" });
  const [sample, setSample] = useState(false);

  const refresh = useCallback(async () => {
    setPortsLoading(true);
    try {
      const found = await listPorts();
      setPorts(found);
      setSelectedPort((current) => {
        if (current !== "" && found.some((port) => port.name === current)) {
          return current;
        }
        return found.find((port) => port.likely)?.name ?? found[0]?.name ?? "";
      });
    } catch {
      // `list_ports` returns an empty list when enumeration fails; a rejected
      // invoke means the bridge itself is unavailable. Either way the screen
      // offers the manual field, so an empty list is the honest state.
      setPorts([]);
    } finally {
      setPortsLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const selectPort = useCallback((name: string) => {
    setSelectedPort(name);
    setManualPort("");
  }, []);

  // A typed port wins over the picker: it is how a port the OS does not list
  // (or one that is about to appear) can still be addressed, and it is what
  // makes the failure path testable without a device.
  const activePort = manualPort.trim() !== "" ? manualPort.trim() : selectedPort;

  const connect = useCallback(async () => {
    const target = activePort;
    if (target === "") {
      setState({
        kind: "error",
        message: "ポートが選ばれていません",
        hint: "一覧からポートを選ぶか、ポート名を入力してから「端末情報を読む」を押してください",
      });
      return;
    }
    setSample(false);
    setState({ kind: "probing", port: target });
    try {
      const info = await readDeviceInfo(target);
      setState({ kind: "connected", info });
    } catch (error) {
      const { message, hint } = splitError(error);
      setState({ kind: "error", message, hint });
    }
  }, [activePort]);

  const toggleSample = useCallback(() => {
    setSample((current) => !current);
  }, []);

  const tone: ConnectionTone = useMemo(() => {
    switch (state.kind) {
      case "idle":
        return "idle";
      case "probing":
        return "probing";
      case "connected":
        return "connected";
      case "error":
        return "error";
    }
  }, [state]);

  return {
    ports,
    portsLoading,
    refresh,
    selectedPort,
    selectPort,
    manualPort,
    setManualPort,
    activePort,
    state,
    connect,
    tone,
    sample,
    toggleSample,
  };
}

export type DeviceProbeController = ReturnType<typeof useDeviceProbe>;
