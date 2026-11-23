import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  exportLog,
  loadPhotos,
  logPoints,
  openLog,
  openSample,
  splitError,
  type Bounds,
  type ExportOutcome,
  type DaySummary,
  type LogExportFormat,
  type LogSummary,
  type MediaItemDto,
  type PhotoDto,
  type PointDto,
} from "../lib/ipc";

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

/** How photos are matched to the track: the §6.1 clock and grouping settings. */
export type PhotoMatchOptions = {
  /** Camera clock minus UTC, in minutes (UTC+9 = 540). */
  cameraOffsetMinutes: number;
  /** Whole days added to the track before matching (`--utc-offset-days`). */
  logOffsetDays: number;
  /** Photos closer than this many metres share a group; 0 disables grouping. */
  groupDistanceMeters: number;
};

/** The defaults: no clock correction and no grouping, as ezTour starts. */
const DEFAULT_PHOTO_OPTIONS: PhotoMatchOptions = {
  cameraOffsetMinutes: 0,
  logOffsetDays: 0,
  groupDistanceMeters: 0,
};

/**
 * Owns the Log screen: the document, its points, the selection, the photos and
 * the export.
 *
 * The Rust side keeps the parsed track; this hook keeps only what the screen
 * renders. Every command is called with the `source` the summary reported, so a
 * late reply from a previous document cannot be mistaken for the current one —
 * `request` is bumped on every open and stale replies are dropped.
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
  const [photoOptions, setPhotoOptions] = useState<PhotoMatchOptions>(DEFAULT_PHOTO_OPTIONS);

  // A request from the dock to frame one day. The map owns the camera; this is
  // the one-way message that asks it to move.
  const [fitTarget, setFitTarget] = useState<{ bounds: Bounds; key: number } | null>(null);

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
  }, []);

  const fetchPoints = useCallback(async (source: string, total: number) => {
    const all: PointDto[] = [];
    for (let from = 0; from < total; from += POINT_CHUNK) {
      const to = Math.min(from + POINT_CHUNK, total);
      const slice = await logPoints(source, from, to);
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
        label: source === "sample" ? "見本" : fileName(source),
      });
      try {
        const loaded = source === "sample" ? await openSample() : await openLog(source);
        if (request.current !== ticket) {
          return;
        }
        const loadedPoints = await fetchPoints(loaded.source, loaded.points);
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
          message: "ログのパスが空です",
          hint: "絶対パスを入力するか、ファイルを画面にドロップしてください",
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

  // Playback advances the cursor one point per tick. It is the only timer on the
  // screen and it exists only while the operator is playing.
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
          return 0;
        }
        const next = current + 1;
        if (next >= points.length) {
          setPlaying(false);
          return points[points.length - 1]?.index ?? current;
        }
        return next;
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
        // Jumping to a photo has to move the camera even if follow was off;
        // otherwise the dock would claim a jump the map did not make.
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
    async (directory: string, options?: PhotoMatchOptions): Promise<boolean> => {
      if (summary === null) {
        return false;
      }
      const trimmed = directory.trim();
      if (trimmed === "") {
        setPhotoError("パスが空です\n対処: 写真が入っているフォルダの絶対パスを入力してください。");
        return false;
      }
      const chosen = options ?? photoOptions;
      setPhotoLoading(true);
      setPhotoError(null);
      try {
        const loaded = await loadPhotos(
          summary.source,
          trimmed,
          chosen.cameraOffsetMinutes,
          chosen.logOffsetDays,
          chosen.groupDistanceMeters,
        );
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
    async (format: LogExportFormat, destination: string) => {
      if (summary === null) {
        return;
      }
      if (destination.trim() === "") {
        setExportError("保存先が空です");
        return;
      }
      setExporting(true);
      setExportError(null);
      setExportOutcome(null);
      try {
        const outcome = await exportLog(summary.source, format, destination.trim());
        setExportOutcome(outcome);
      } catch (error) {
        const { message, hint } = splitError(error);
        setExportError(hint === null ? message : `${message}\n${hint}`);
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
    exporting,
    exportOutcome,
    exportError,
    openFile,
    openSample: openSampling,
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

/** The last path segment, for the loading line. */
function fileName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] ?? path;
}
