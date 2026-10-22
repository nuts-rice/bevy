import * as Json from "@moq/json";
import * as Moq from "@moq/net";
import * as Watch from "@moq/watch";


type Button = "up" | "down" | "left" | "right";

const KEY_MAP: Record<string , Button> = {
ArrowUp: "up",
ArrowDown: "down",
ArrowLeft: "left",
ArrowRight: "right"
};

interface CityStatus {
buttons: string[];
}

const GRID_PIXELS = 640 * 360;
const EXPANDED_PIXELS = 1920 * 1080;


interface CityConfig {
seed: string; 
connection: Moq.Connection;
origin: Moq.Origin.Producer;
expanded: Moq.Signals.Signal<string | undefined>;
cityPrefix: string;
viewerPrefix: string;
}

class CityView {
  readonly seed: string;
  readonly root: HTMLElement;

  readonly #signals = new Moq.Signals.Effect();
  readonly #config: CityConfig;
  readonly #info: HTMLElement;

  readonly #canvas = new Moq.Signals.Signal<HTMLCanvasElement | undefined>(undefined);
  readonly #target = new Moq.Signals.Signal<Watch.Video.Target | undefined>(undefined);
  readonly #held = new Set<Button>();
  readonly #buttons = new Moq.Signals.Signal<Button[]>([]);

  readonly #broadcast: Watch.Broadcast;
  readonly #renderer: Watch.Video.Renderer;

  constructor(config: CityConfig) {
    this.seed = config.seed;
    this.#config = config;

    this.root = document.createElement("div");
    this.root.className = "tile";
    const canvas = document.createElement("canvas");
    const label = document.createElement("div");
    label.className = "label";
    label.textContent = `city ${this.seed}`;
    this.#info = document.createElement("div");
    this.#info.className = "info";
    this.root.append(canvas, label, this.#info);
    this.#canvas.set(canvas);

    this.#signals.event(this.root, "click", () => config.expanded.set(this.seed));
    this.#signals.run((effect) => {
      this.root.classList.toggle("expanded", effect.get(config.expanded) === this.seed);
    });

    // Video pipeline.
    this.#broadcast = new Watch.Broadcast({
      origin: config.origin,
      name: Moq.Path.from(`${config.cityPrefix}/${this.seed}`),
      enabled: true,
    });
    this.#signals.cleanup(() => this.#broadcast.close());

    const source = new Watch.Video.Source({
      broadcast: this.#broadcast,
      target: this.#target,
      supported: Watch.Video.Decoder.supported,
      probe: config.connection.probe,
    });
    this.#signals.cleanup(() => source.close());

    const sync = new Watch.Sync({ delay: "auto", probe: config.connection.probe });
    this.#signals.cleanup(() => sync.close());

    const videoEnabled = new Moq.Signals.Signal(false);
    const decoder = new Watch.Video.Decoder({ source, sync, enabled: videoEnabled });
    this.#signals.cleanup(() => decoder.close());

    this.#renderer = new Watch.Video.Renderer({ decoder, canvas: this.#canvas });
    this.#signals.cleanup(() => this.#renderer.close());

    this.#signals.run(this.#runPixelBudget.bind(this));
    this.#signals.run(this.#runVideoEnabled.bind(this, videoEnabled));
    this.#signals.run(this.#runStatus.bind(this));
    this.#signals.run(this.#runInput.bind(this));
    this.#signals.run(this.#runCommands.bind(this));
  }

  close() {
    this.#signals.close();
    this.root.remove();
  }

  #runPixelBudget(effect: Moq.Signals.Effect) {
    const expanded = effect.get(this.#config.expanded) === this.seed;
    this.#target.set({ pixels: expanded ? EXPANDED_PIXELS : GRID_PIXELS });
  }

  #runVideoEnabled(videoEnabled: Moq.Signals.Signal<boolean>, effect: Moq.Signals.Effect) {
    const expanded = effect.get(this.#config.expanded);
    const visible = effect.get(this.#renderer.out.visible);
    // Subscribing is what makes bevy_city render this city, so only download what is on screen.
    videoEnabled.set((expanded === undefined || expanded === this.seed) && visible);
  }

  #runStatus(effect: Moq.Signals.Effect) {
    const active = effect.get(this.#broadcast.out.active);
    if (!active) return;

    const track = active.track("status").subscribe({ priority: 10 });
    effect.cleanup(() => track.close());
    const consumer = new Json.Snapshot.Consumer<CityStatus>({ track });

    effect.spawn(async () => {
      for (;;) {
        let status: CityStatus | undefined;
        try {
          status = await consumer.next();
        } catch (err) {
          console.warn("invalid status JSON:", err);
          continue;
        }
        if (!status) break;
        this.#info.textContent = status.buttons.length ? `pressed: ${status.buttons.join(" ")}` : "";
      }
    });
  }

  // Keyboard controls, only for the expanded city.
  #runInput(effect: Moq.Signals.Effect) {
    if (effect.get(this.#config.expanded) !== this.seed) return;

    effect.event(window, "keydown", (e) => {
      const button = KEY_MAP[e.key.length === 1 ? e.key.toLowerCase() : e.key];
      if (!button) return;
      e.preventDefault();
      if (this.#held.has(button)) return;
      this.#held.add(button);
      this.#buttons.set([...this.#held]);
    });
    effect.event(window, "keyup", (e) => {
      const button = KEY_MAP[e.key.length === 1 ? e.key.toLowerCase() : e.key];
      if (!button || !this.#held.delete(button)) return;
      this.#buttons.set([...this.#held]);
    });
    // A key released while the tab is unfocused never sends keyup.
    const release = () => {
      this.#held.clear();
      this.#buttons.set([]);
    };
    effect.event(window, "blur", release);
    effect.cleanup(release);
  }

  // Publish `viewer/{seed}/{id}` with a "command" track while this city is expanded.
  #runCommands(effect: Moq.Signals.Effect) {
    if (effect.get(this.#config.connection.status) !== "connected") return;
    if (effect.get(this.#config.expanded) !== this.seed) return;

    const viewerId = Math.random().toString(36).slice(2, 8);
    const broadcast = this.#config.origin.createBroadcast(
      Moq.Path.from(`${this.#config.viewerPrefix}/${this.seed}/${viewerId}`),
    );
    broadcast.announce();
    effect.cleanup(() => broadcast.close());

    const track = broadcast.createTrack("command");
    const producer = new Json.Snapshot.Producer<Record<string, unknown>>({ track });
    effect.cleanup(() => producer.finish());

    effect.run((inner) => {
      if (inner.get(track.closed) !== undefined) return;
      producer.update({ type: "buttons", buttons: inner.get(this.#buttons) });
    });
  }
}

const STYLE = `
:host { display: block; }
.bar { display: none; align-items: center; gap: 1rem; padding: 0.5rem 1rem; font: 0.75rem monospace; color: #888; }
:host([data-expanded]) .bar { display: flex; }
.bar button { font: inherit; color: #e0e0e0; background: #222; border: 1px solid #444; border-radius: 4px; padding: 0.25rem 0.75rem; cu
rsor: pointer; }
.grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 1rem; padding: 1rem; }
:host([data-expanded]) .grid { grid-template-columns: 1fr; }
:host([data-expanded]) .tile:not(.expanded) { display: none; }
.empty { padding: 2rem; text-align: center; font: 0.8rem monospace; color: #555; }
.grid:not(:empty) + .empty { display: none; }
.tile { position: relative; aspect-ratio: 16 / 9; background: #111; border: 1px solid #333; border-radius: 6px; overflow: hidden; curso
r: pointer; }
.tile:hover { border-color: #8bac0f; }
.tile.expanded { cursor: default; border-color: #333; }
canvas { display: block; width: 100%; height: 100%; }
.label, .info { position: absolute; left: 0.5rem; padding: 2px 6px; border-radius: 3px; font: 0.75rem monospace; color: #ddd; backgroun
d: rgba(0, 0, 0, 0.6); }
.label { top: 0.5rem; }
.info { bottom: 0.5rem; }
.info:empty { display: none; }
`;

const OBSERVED = ["url", "prefix-city", "prefix-viewer"] as const;
type Observed = (typeof OBSERVED)[number];

export default class MoqCity extends HTMLElement {
  static observedAttributes = OBSERVED;

  readonly connection: Moq.Connection;
  /** The origin viewer broadcasts are published into, served across reconnects. */
  readonly origin = new Moq.Origin.Producer();
  /** The seed of the city shown full size, if any. */
  readonly expanded = new Moq.Signals.Signal<string | undefined>(undefined);

  readonly #signals = new Moq.Signals.Effect();
  readonly #enabled = new Moq.Signals.Signal(false);
  readonly #cityPrefix = new Moq.Signals.Signal("city");
  readonly #viewerPrefix = new Moq.Signals.Signal("viewer");
  readonly #cities = new Map<string, CityView>();
  readonly #grid: HTMLElement;

  constructor() {
    super();

    this.connection = new Moq.Connection({
      enabled: this.#enabled,
      publish: this.origin.consume(),
      consume: this.origin,
      share: false,
    });
    this.#signals.cleanup(() => this.connection.close());
    this.#signals.cleanup(() => this.origin.close());

    const shadow = this.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = STYLE;
    const bar = document.createElement("div");
    bar.className = "bar";
    const back = document.createElement("button");
    back.textContent = "← back";
    const hint = document.createElement("span");
    hint.textContent = "arrows / WASD: left/right orbit, up/down zoom · Esc to go back";
    bar.append(back, hint);
    this.#grid = document.createElement("div");
    this.#grid.className = "grid";
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "Waiting for cities…";
    shadow.append(style, bar, this.#grid, empty);

    this.#signals.event(back, "click", () => this.expanded.set(undefined));
    this.#signals.event(window, "keydown", (e) => {
      if (e.key === "Escape") this.expanded.set(undefined);
    });
    this.#signals.run((effect) => {
      this.toggleAttribute("data-expanded", effect.get(this.expanded) !== undefined);
    });

    this.#signals.run(this.#runDiscovery.bind(this));
  }

  connectedCallback() {
    this.#enabled.set(true);
  }

  disconnectedCallback() {
    this.#enabled.set(false);
  }

  attributeChangedCallback(name: Observed, _oldValue: string | null, newValue: string | null) {
    switch (name) {
      case "url":
        this.connection.url.set(newValue ? new URL(newValue) : undefined);
        break;
      case "prefix-city":
        this.#cityPrefix.set(newValue ?? "city");
        break;
      case "prefix-viewer":
        this.#viewerPrefix.set(newValue ?? "viewer");
        break;
    }
  }

  get url(): URL | undefined {
    return this.connection.url.peek();
  }

  set url(value: string | URL | undefined) {
    this.connection.url.set(value ? new URL(value) : undefined);
  }

  #runDiscovery(effect: Moq.Signals.Effect) {
    const cityPrefix = effect.get(this.#cityPrefix);
    const viewerPrefix = effect.get(this.#viewerPrefix);

    const announced = this.origin
      .consume()
      .announced(Moq.Path.Pattern.parse("*").rooted(Moq.Path.from(cityPrefix)));
    effect.cleanup(() => announced.close());
    effect.cleanup(() => {
      for (const city of this.#cities.values()) city.close();
      this.#cities.clear();
    });

    effect.spawn(async () => {
      for (;;) {
        const entry = await effect.race(announced.next());
        if (!entry) break;

        const capture = entry.captures?.[0];
        if (!capture?.isLiteral) continue;
        const seed = capture.text;

        if (Moq.Announce.isActive(entry.kind) && !this.#cities.has(seed)) {
          const city = new CityView({
            seed,
            connection: this.connection,
            origin: this.origin,
            expanded: this.expanded,
            cityPrefix,
            viewerPrefix,
          });
          this.#cities.set(seed, city);
          this.#grid.append(city.root);
        } else if (entry.kind === "retracted") {
          this.#cities.get(seed)?.close();
          this.#cities.delete(seed);
          if (this.expanded.peek() === seed) this.expanded.set(undefined);
        }
      }
    });
  }
}

customElements.define("moq-city", MoqCity);

declare global {
  interface HTMLElementTagNameMap {
    "moq-city": MoqCity;
  }
}
