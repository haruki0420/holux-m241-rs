/**
 * MapLibre's worker, resolved by Vite.
 *
 * MapLibre v6 spawns its worker from a URL it derives from `import.meta.url`
 * (`maplibre-gl-worker.mjs`). In a bundled build that URL points beside the
 * app's own chunk, where the worker file is not emitted, so every source that
 * needs the worker (GeoJSON, vector tiles) silently never parses — the track
 * layer stays empty while the rest of the map looks alive.
 *
 * Importing the file with Vite's `?url` makes Vite copy it into `assets/` and
 * hand back the hashed URL, and `setWorkerUrl` tells MapLibre to use it. Import
 * this module before the map is created.
 */
import { setWorkerUrl } from "maplibre-gl";
import workerUrl from "maplibre-gl/dist/maplibre-gl-worker.mjs?url";

setWorkerUrl(workerUrl);

export {};
