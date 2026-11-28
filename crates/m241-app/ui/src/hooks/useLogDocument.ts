import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  downloadLog,
  exportLog,
  loadPhotos,
  logPoints,
  openLog,
  openSample,
  splitError,
  type Bounds,
  type DaySummary,
  type DownloadLogOutcome,
  type ExportOutcome,
  type LogExportFormat,
  type LogSummary,
  type MediaItemDto,
  type PhotoDto,
  type PhotoOptionsDto,
  type PointDto,
} from "../lib/ipc";
import { baseName } from "../lib/format";

/** The Log screen's document state. */
export type LogState =
  | { kind: "empty" }
  | { kind: "loading"; label: string }
  | { kind: "ready"; summary: LogSummary }
  | { kind: "error"; message: string; hint: string | null };

/** How many points one `log_points` call asks for. */
const POINT_CHUNK = 50_000;

/** Milliseconds between two points during playback. */
const PLAY_INTERVAL_MS = 80;

/** Default photo matching options. */
export const DEFAULT_PHOTO_OPTIONS: PhotoOptionsDto = {
  cameraOffsetMinutes: 540,
  logOffsetDays: 0,
  groupDistanceMeters: 0,
  maxGapSeconds: 300,
};

/**
 * Owns the active track document, its points, playback state, loaded photo pins,
 * device log download, and export. Shared above the tabs so the Geotag tab can
 * immediately work against the currently loaded track.
 */
export function useLogDocument() {
  const [state, setState] = useState<LogState>({ kind: "empty" });
  const [points, setPoints] = useState<PointDto[]>([]);
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null);
  const [popupIndex, setPopupIndex] = useState<number | null>(null);
  const [follow, setFollow] = useState(true);
  const [playing, setPlaying] = useState(false);

  const [photos, setPhotos] = useState<PhotoDto[]>([]);
  const [media, setMedia] = useState<MediaItemDto[]>([]);
  const [photoDirectory, setPhotoDirectory] = useState<string | null>(null);
  const [photoSkipped, setPhotoSkipped] = useState<string[]>([]);
  const [photoError, setPhotoError] = useState<string | null>(null);
  const [photoLoading, setPhotoLoading] = useState(false);
  const [photoIndex, setPhotoIndex] = useState<number | null>(null);
  const [photoOptions, setPhotoOptions] = useState<PhotoOptionsDto>(DEFAULT_PHOTO_OPTIONS);

  const [fitTarget, setFitTarget] = useState<{ bounds: Bounds; key: number } | null>(null);

  const [downloading, setDownloading] = useState(false);
  const [downloadOutcome, setDownloadOutcome] = useState<DownloadLogOutcome | null>(null);
  const [downloadError, setDownloadError] = useState<{ message: string; hint: string | null } | null>(
    null,
  );

  const [exporting, setExporting] = useState(false);
  const [exportOutcome, setExportOutcome] = useState<ExportOutcome | null>(null);
  const [exportError, setExportError] = useState<string | null>(null);

  const request = useRef(0);
  const pointByIndex = useMemo(() => {
    const map = new Map<number, PointDto>();
    for (const point of points) {
      map.set(point.index, point);
    }
    return map;
  }, [points]);

  const summary = state.kind === "ready" ? state.summary : null;

  const reset = useCallback(() => {
    setPoints([]);
    setSelectedIndex(null);
    setPopupIndex(null);
    setPlaying(false);
    setPhotos([]);
    setMedia([]);
    setPhotoDirectory(null);
    setPhotoSkipped([]);
    setPhotoError(null);
    setPhotoIndex(null);
    setExportOutcome(null);
    setExportError(null);
    setDownloadOutcome(null);
    setDownloadError(null);
  }, []);

  const fetchPoints = useCallback(async (total: number) => {
    const all: PointDto[] = [];
    for (let from = 0; from < total; from += POINT_CHUNK) {
      const to = Math.min(from + POINT_CHUNK, total);
      const slice = await logPoints(from, to);
      all.push(...slice);
    }
    return all;
  }, []);

  const openSource = useCallback(
    async (source: string) => {
      const ticket = request.current + 1;
      request.current = ticket;
      reset();
      setState({
        kind: "loading",
        label: source === "sample" ? "サンプルデータ" : baseName(source),
      });
      try {
        const loaded = source === "sample" ? await openSample() : await openLog(source);
        if (request.current !== ticket) {
          return;
        }
        const loadedPoints = await fetchPoints(loaded.points);
        if (request.current !== ticket) {
          return;
        }
        setPoints(loadedPoints);
        setSelectedIndex(loadedPoints[0]?.index ?? null);
        setState({ kind: "ready", summary: loaded });
      } catch (error) {
        if (request.current !== ticket) {
          return;
        }
        const { message, hint } = splitError(error);
        setState({ kind: "error", message, hint });
      }
    },
    [fetchPoints, reset],
  );

  const openFile = useCallback(
    (path: string) => {
      const trimmed = path.trim();
      if (trimmed === "") {
        setState({
          kind: "error",
          message: "ログファイルのパスが指定されていません",
          hint: "ファイルダイアログから .bin / .gpx / .kml / .kmz を選択してください。",
        });
        return;
      }
      void openSource(trimmed);
    },
    [openSource],
  );

  const openSampling = useCallback(() => {
    void openSource("sample");
  }, [openSource]);

  const downloadFromDevice = useCallback(
    async (port: string, destination: string): Promise<boolean> => {
      const targetPort = port.trim();
      const dest = destination.trim();
      if (targetPort === "") {
        setDownloadError({
          message: "シリアルポートが選択されていません",
          hint: "上部ヘッダーからシリアルポートを選択または手動入力してください。",
        });
        return false;
      }
      if (dest === "") {
        return false;
      }

      const ticket = request.current + 1;
      request.current = ticket;
      setDownloading(true);
      setDownloadError(null);
      setDownloadOutcome(null);
      setState({ kind: "loading", label: `${targetPort} から取得中…` });
      try {
        const outcome = await downloadLog(targetPort, dest);
        if (request.current !== ticket) {
          return false;
        }
        const loadedPoints = await fetchPoints(outcome.summary.points);
        if (request.current !== ticket) {
          return false;
        }
        reset();
        setPoints(loadedPoints);
        setSelectedIndex(loadedPoints[0]?.index ?? null);
        setDownloadOutcome(outcome);
        setState({ kind: "ready", summary: outcome.summary });
        return true;
      } catch (error) {
        if (request.current !== ticket) {
          return false;
        }
        const split = splitError(error);
        setDownloadError(split);
        setState({ kind: "error", message: split.message, hint: split.hint });
        return false;
      } finally {
        setDownloading(false);
      }
    },
    [fetchPoints, reset],
  );

  const close = useCallback(() => {
    request.current += 1;
    reset();
    setState({ kind: "empty" });
  }, [reset]);

  const selectPoint = useCallback((index: number | null, options?: { popup?: boolean }) => {
    setSelectedIndex(index);
    setPopupIndex(options?.popup === true ? index : null);
  }, []);

  const openPopup = useCallback((index: number) => {
    setSelectedIndex(index);
    setPopupIndex(index);
  }, []);

  const closePopup = useCallback(() => {
    setPopupIndex(null);
  }, []);

  const seek = useCallback((index: number) => {
    setSelectedIndex(index);
    setPopupIndex(null);
  }, []);

  const toggleFollow = useCallback(() => {
    setFollow((current) => !current);
  }, []);

  const togglePlaying = useCallback(() => {
    setPlaying((current) => !current);
  }, []);

  useEffect(() => {
    if (!playing) {
      return;
    }
    if (points.length < 2) {
      setPlaying(false);
      return;
    }
    const timer = window.setInterval(() => {
      setSelectedIndex((current) => {
        if (current === null) {
          return points[0]?.index ?? 0;
        }
        const pos = points.findIndex((pt) => pt.index === current);
        const nextPos = pos < 0 ? 0 : pos + 1;
        if (nextPos >= points.length) {
          setPlaying(false);
          return points[points.length - 1]?.index ?? current;
        }
        return points[nextPos]?.index ?? current;
      });
    }, PLAY_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [playing, points]);

  const selectPhoto = useCallback(
    (index: number | null) => {
      setPhotoIndex(index);
      if (index === null) {
        return;
      }
      const photo = photos[index];
      if (photo?.pointIndex != null) {
        setSelectedIndex(photo.pointIndex);
        setPopupIndex(null);
        setFollow(true);
      }
    },
    [photos],
  );

  const movePhoto = useCallback(
    (step: number) => {
      if (photos.length === 0) {
        return;
      }
      const current = photoIndex ?? 0;
      const next = (current + step + photos.length) % photos.length;
      selectPhoto(next);
    },
    [photoIndex, photos, selectPhoto],
  );

  const fitWhole = useCallback(() => {
    if (summary?.bounds != null) {
      setFitTarget({ bounds: summary.bounds, key: Date.now() });
    }
  }, [summary]);

  const fitDay = useCallback(
    (day: DaySummary) => {
      if (day.firstUtc === null || day.lastUtc === null) {
        return;
      }
      let bounds: Bounds | null = null;
      for (const point of points) {
        if (
          point.utc === null ||
          point.utc < day.firstUtc ||
          point.utc > day.lastUtc ||
          point.lat === null ||
          point.lon === null
        ) {
          continue;
        }
        bounds =
          bounds === null
            ? { west: point.lon, south: point.lat, east: point.lon, north: point.lat }
            : {
                west: Math.min(bounds.west, point.lon),
                south: Math.min(bounds.south, point.lat),
                east: Math.max(bounds.east, point.lon),
                north: Math.max(bounds.north, point.lat),
              };
      }
      if (bounds !== null) {
        setFitTarget({ bounds, key: Date.now() });
      }
    },
    [points],
  );

  const readPhotoFolder = useCallback(
    async (directory: string, options?: PhotoOptionsDto): Promise<boolean> => {
      if (summary === null) {
        return false;
      }
      const trimmed = directory.trim();
      if (trimmed === "") {
        setPhotoError("写真フォルダのパスが空です");
        return false;
      }
      const chosen = options ?? photoOptions;
      setPhotoLoading(true);
      setPhotoError(null);
      try {
        const loaded = await loadPhotos(trimmed, chosen);
        setPhotoOptions(chosen);
        setPhotos(loaded.photos);
        setMedia(loaded.media);
        setPhotoDirectory(loaded.directory);
        setPhotoSkipped(loaded.skipped);
        setPhotoIndex(loaded.photos.length === 0 ? null : 0);
        const first = loaded.photos[0];
        if (first?.pointIndex != null) {
          setSelectedIndex(first.pointIndex);
        }
        return true;
      } catch (error) {
        const { message, hint } = splitError(error);
        setPhotoError(hint === null ? message : `${message}\n${hint}`);
        return false;
      } finally {
        setPhotoLoading(false);
      }
    },
    [summary, photoOptions],
  );

  const writeExport = useCallback(
    async (format: LogExportFormat, destination: string): Promise<ExportOutcome | null> => {
      if (summary === null) {
        return null;
      }
      const dest = destination.trim();
      if (dest === "") {
        setExportError("保存先が空です");
        return null;
      }
      setExporting(true);
      setExportError(null);
      setExportOutcome(null);
      try {
        const outcome = await exportLog(dest, format);
        setExportOutcome(outcome);
        return outcome;
      } catch (error) {
        const { message, hint } = splitError(error);
        setExportError(hint === null ? message : `${message}\n${hint}`);
        return null;
      } finally {
        setExporting(false);
      }
    },
    [summary],
  );

  const selectedPoint =
    selectedIndex === null ? null : (pointByIndex.get(selectedIndex) ?? null);
  const popupPoint =
    popupIndex === null ? null : (pointByIndex.get(popupIndex) ?? null);
  const selectedPhoto = photoIndex === null ? null : (photos[photoIndex] ?? null);
  const selectedPhotoForPoint =
    selectedIndex === null
      ? null
      : (photos.find((photo) => photo.pointIndex === selectedIndex) ?? null);

  return {
    state,
    summary,
    points,
    selectedIndex,
    selectedPoint,
    popupIndex,
    popupPoint,
    follow,
    playing,
    photos,
    media,
    photoDirectory,
    photoSkipped,
    photoError,
    photoLoading,
    photoIndex,
    photoOptions,
    selectedPhoto,
    selectedPhotoForPoint,
    fitTarget,
    fitWhole,
    fitDay,
    downloading,
    downloadOutcome,
    downloadError,
    exporting,
    exportOutcome,
    exportError,
    openFile,
    openSample: openSampling,
    downloadFromDevice,
    close,
    selectPoint,
    openPopup,
    closePopup,
    seek,
    toggleFollow,
    togglePlaying,
    selectPhoto,
    movePhoto,
    readPhotoFolder,
    writeExport,
  };
}

export type LogController = ReturnType<typeof useLogDocument>;
