import { useCallback, useEffect, useMemo, useState } from "react";

import {
  listPorts,
  readDeviceInfo,
  splitError,
  type DeviceInfo,
  type PortInfo,
} from "../lib/ipc";

/** Current device probe state. */
export type ProbeState =
  | { kind: "idle" }
  | { kind: "probing"; port: string }
  | { kind: "connected"; info: DeviceInfo }
  | { kind: "error"; message: string; hint: string | null };

/** Status tone for the header badge (always paired with a text label). */
export type ConnectionTone = "idle" | "probing" | "connected" | "error";

/**
 * Owns serial port discovery, manual port entry (e.g. `/dev/rfcomm0` or `COM3`),
 * and `device_info` connection verification across all three tabs.
 */
export function useDeviceProbe() {
  const [ports, setPorts] = useState<PortInfo[]>([]);
  const [portsLoading, setPortsLoading] = useState(true);
  const [selectedPort, setSelectedPort] = useState("");
  const [manualMode, setManualMode] = useState(false);
  const [manualPort, setManualPort] = useState("");
  const [state, setState] = useState<ProbeState>({ kind: "idle" });

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
  }, []);

  const toggleManualMode = useCallback(() => {
    setManualMode((prev) => {
      if (!prev && manualPort.trim() === "" && selectedPort !== "") {
        setManualPort(selectedPort);
      }
      return !prev;
    });
  }, [manualPort, selectedPort]);

  const activePort = manualMode ? manualPort.trim() : selectedPort;

  const connect = useCallback(async () => {
    const target = activePort;
    if (target === "") {
      setState({
        kind: "error",
        message: "シリアルポートが選択されていません",
        hint: "一覧からポートを選ぶか、「手動入力」に切り替えてポート名を入力してください。",
      });
      return;
    }
    setState({ kind: "probing", port: target });
    try {
      const info = await readDeviceInfo(target);
      setState({ kind: "connected", info });
    } catch (error) {
      const { message, hint } = splitError(error);
      setState({ kind: "error", message, hint });
    }
  }, [activePort]);

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
    manualMode,
    toggleManualMode,
    manualPort,
    setManualPort,
    activePort,
    state,
    connect,
    tone,
  };
}

export type DeviceProbeController = ReturnType<typeof useDeviceProbe>;
