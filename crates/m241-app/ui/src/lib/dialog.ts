import { open, save } from "@tauri-apps/plugin-dialog";

export type DialogFilter = {
  name: string;
  extensions: string[];
};

/** Pick a single existing file via the native OS file dialog. */
export async function pickOpenFile(options: {
  title: string;
  filters?: DialogFilter[];
}): Promise<string | null> {
  const selected = await open({
    title: options.title,
    multiple: false,
    directory: false,
    filters: options.filters,
  });
  return typeof selected === "string" ? selected : null;
}

/** Pick an existing directory via the native OS folder dialog. */
export async function pickDirectory(options: {
  title: string;
}): Promise<string | null> {
  const selected = await open({
    title: options.title,
    multiple: false,
    directory: true,
  });
  return typeof selected === "string" ? selected : null;
}

/** Pick a destination path via the native OS save dialog. */
export async function pickSaveFile(options: {
  title: string;
  defaultPath?: string;
  filters?: DialogFilter[];
}): Promise<string | null> {
  const selected = await save({
    title: options.title,
    defaultPath: options.defaultPath,
    filters: options.filters,
  });
  return typeof selected === "string" ? selected : null;
}
