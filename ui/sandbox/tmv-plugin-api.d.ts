/** Preview API 2. All operations cross the sandbox bridge and return promises. */
declare namespace TmvPlugin {
  type Permission = "state" | "ui" | "metadata" | "export" | "settings";
  type Region = [number, number, number, number];
  interface ImageInfo {
    path: string; width: number; height: number; sizeZ: number; sizeT: number;
    format: string;
    [key: string]: unknown;
  }
  interface PixelSize { x: number; y?: number; unit: "Å" | "nm" | "µm" | "mm" | "1/µm" | "1/nm" | "1/Å" }
  interface State {
    info: ImageInfo | null; path: string | null; z: number; t: number;
    sizeZ: number; sizeT: number; width: number; height: number;
    selection: Region | null; view: Region | null; pixelSize: PixelSize | null;
  }
  interface Events { open: ImageInfo; slice: { z: number; t: number }; render: undefined }
  type Callback<T = void> = (value: T) => void | Promise<void>;
  type Item =
    | { type: "text"; text: string }
    | { type: "button"; label: string; onClick: Callback }
    | { type: "number"; label: string; value?: number; min?: number; max?: number; step?: number; onChange: Callback<number> }
    | { type: "select"; label: string; options: [string, string][]; value?: string; onChange: Callback<string> }
    | { type: "checkbox"; label: string; value?: boolean; onChange: Callback<boolean> };
  interface App {
    readonly apiVersion: 2;
    readonly appVersion: string;
    state(): Promise<State>;
    on<E extends keyof Events>(event: E, fn: Callback<Events[E]>): Promise<() => Promise<void>>;
    setZ(z: number): Promise<void>;
    setT(t: number): Promise<void>;
    /** PNG bytes; maximum 4096 per dimension and 4 megapixels. */
    renderRegion(region: Region, opts?: { z?: number; t?: number; size?: [number, number] }): Promise<Uint8Array>;
    ui: {
      toast(message: string, ms?: number): Promise<void>;
      showTools(): Promise<void>;
      addToolbarButton(description: { label: string; title?: string; onClick: Callback }): Promise<void>;
      addToolSection(description: { title: string; items: Item[] }): Promise<void>;
    };
    image: {
      addMetadata(path: string, groups: { name: string; items: [string, string][] }[]): Promise<boolean>;
      setPixelSize(path: string, size: PixelSize): Promise<boolean>;
      setFormat(path: string, name: string): Promise<boolean>;
    };
    settings: { get<T>(key: string, fallback: T): Promise<T>; set(key: string, value: unknown): Promise<void> };
    dialog: { save(options: { name: string; filter?: string; exts?: string[] }): Promise<string | null> };
    /** Only the last dialog.save path, once. Cancellation revokes permission. */
    fs: { write(path: string, bytes: Uint8Array): Promise<void> };
  }
}
declare const TifMrcView: {
  readonly apiVersion: 2;
  register(definition: { id: string; activate(app: TmvPlugin.App): void | Promise<void> }): void;
};
