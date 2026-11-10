import { init as initGhostty, Terminal } from 'ghostty-web';
import initDemo, { Demo } from './pkg/vt_tui_demo.js';
import './style.css';

const COLS = 100;
const ROWS = 28;
const FONT = '"JetBrains Mono", ui-monospace, Menlo, monospace';

const theme = {
  background: '#171a2c',
  foreground: '#d9dcea',
  cursor: '#d9dcea',
  selectionBackground: '#3b4170',
  black: '#2a2e45',
  red: '#ee6a6a',
  green: '#58c17a',
  yellow: '#e0b44c',
  blue: '#6c8cff',
  magenta: '#b58cff',
  cyan: '#5cc8d6',
  white: '#c9ccda',
  brightBlack: '#6b7090',
  brightRed: '#ff8a8a',
  brightGreen: '#7ee09c',
  brightYellow: '#f2cf72',
  brightBlue: '#94acff',
  brightMagenta: '#cfb0ff',
  brightCyan: '#86e3ee',
  brightWhite: '#f4f5fb',
};

/**
 * The largest font size whose cells fit `COLS` columns in `width` pixels.
 * ghostty-web rounds the cell width up to whole pixels.
 */
function fontSizeFor(width: number): number {
  const context = document.createElement('canvas').getContext('2d')!;
  const cellWidth = (size: number): number => {
    context.font = `${size}px ${FONT}`;
    return Math.ceil(context.measureText('M').width);
  };
  let size = 16;
  while (size > 10 && cellWidth(size) * COLS > width) size -= 0.5;
  return size;
}

/**
 * ghostty-web focuses its container and a hidden textarea when the canvas is
 * clicked. Focusing scrolls the page to bring the element into view, which
 * jumps the page whenever the terminal's top edge is off screen.
 */
function preventFocusScroll(element: HTMLElement): void {
  const focus = element.focus.bind(element);
  element.focus = (options?: FocusOptions) => focus({ ...options, preventScroll: true });
}

class DemoTerminal {
  private readonly term: Terminal;
  private readonly frame: HTMLElement;
  private readonly scenario: string;
  private demo: Demo | null = null;
  /** Milliseconds the current run has been on screen; it pauses off screen. */
  private elapsed = 0;
  private lastTick = performance.now();
  /** While paused, the tasks stop moving but the UI still takes keys. */
  private paused = false;
  private readonly pauseButton: HTMLButtonElement | null;
  private readonly byteCount: HTMLElement | null;
  /** Bytes the current run wrote to the terminal: what an agent would read. */
  private written = 0;
  private visible = false;
  private dragging = false;
  private readonly decoder = new TextDecoder();
  private readonly encoder = new TextEncoder();

  constructor(private readonly root: HTMLElement) {
    this.scenario = root.dataset['scenario'] ?? 'verify';
    this.frame = root.querySelector('.terminal-frame')!;
    const host = root.querySelector<HTMLElement>('.terminal')!;
    const fontSize = fontSizeFor(this.innerWidth());
    this.term = new Terminal({
      cols: COLS,
      rows: ROWS,
      fontSize,
      fontFamily: FONT,
      theme,
      cursorBlink: false,
      scrollback: 5000,
    });
    // ghostty-web focuses the terminal when it opens, now and again a tick
    // later. That would scroll the page to it and take the keyboard on load,
    // so focus is ignored until opening is over.
    const focusHost = host.focus.bind(host);
    host.focus = () => {};
    this.term.open(host);
    if (this.term.textarea) preventFocusScroll(this.term.textarea);
    setTimeout(() => {
      host.focus = (options?: FocusOptions) => focusHost({ ...options, preventScroll: true });
    }, 0);
    this.term.onData((data) => this.demo?.input(this.encoder.encode(data)));
    host.addEventListener('focusin', () => this.frame.classList.add('is-focused'));
    host.addEventListener('focusout', () => this.frame.classList.remove('is-focused'));
    this.forwardMouse(host);
    new ResizeObserver(() => {
      const size = fontSizeFor(this.innerWidth());
      if (size !== this.term.options.fontSize) this.term.options.fontSize = size;
    }).observe(this.frame);

    root.querySelector('.restart')?.addEventListener('click', () => {
      this.start();
      this.focus();
    });
    this.byteCount = root.querySelector<HTMLElement>('.bytes output');
    this.pauseButton = root.querySelector<HTMLButtonElement>('.pause');
    this.pauseButton?.addEventListener('click', () => {
      this.setPaused(!this.paused);
      this.focus();
    });
    for (const input of root.querySelectorAll<HTMLInputElement>('.controls input')) {
      input.addEventListener('change', () => this.start());
    }

    // A timer rather than animation frames, so a run keeps its pace in a
    // tab that isn't painting.
    setInterval(this.tick, 33);
  }

  /** Focuses the terminal without scrolling the page. */
  private focus(): void {
    this.term.element?.focus({ preventScroll: true });
  }

  /** The frame's width without its padding. */
  private innerWidth(): number {
    const style = getComputedStyle(this.frame);
    return this.frame.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  }

  private option(name: string): boolean {
    return (
      this.root.querySelector<HTMLInputElement>(`.controls input[name="${name}"]`)?.checked ?? false
    );
  }

  private start(): void {
    this.demo?.free();
    this.term.reset();
    this.term.clear();
    this.term.write('\x1bc');
    const seed = Math.floor(Math.random() * 0xffffffff);
    this.demo = new Demo(
      this.scenario,
      COLS,
      ROWS,
      this.option('agent'),
      this.option('fail'),
      seed,
    );
    this.elapsed = 0;
    this.written = 0;
    this.showWritten();
    this.setPaused(false);
  }

  setPaused(paused: boolean): void {
    this.paused = paused;
    if (this.pauseButton) {
      this.pauseButton.textContent = paused ? 'Resume the run' : 'Pause the run';
      this.pauseButton.setAttribute('aria-pressed', String(paused));
    }
  }

  private readonly tick = (): void => {
    const now = performance.now();
    const step = Math.min(now - this.lastTick, 100);
    this.lastTick = now;
    const rect = this.frame.getBoundingClientRect();
    const shown = Math.min(rect.bottom, innerHeight) - Math.max(rect.top, 0);
    this.visible = shown > rect.height * 0.35;
    if (!this.visible) return;
    if (!this.demo) this.start();
    if (!this.paused) this.elapsed += step;
    if (this.pauseButton) this.pauseButton.disabled = !this.demo!.running();
    const bytes = this.demo!.frame(this.elapsed);
    if (bytes.length > 0) {
      this.copySelections(bytes);
      this.term.write(bytes);
      this.written += bytes.length;
      this.showWritten();
    }
  };

  private showWritten(): void {
    if (!this.byteCount) return;
    this.byteCount.textContent =
      this.written < 1024 ? `${this.written} bytes` : `${(this.written / 1024).toFixed(1)} kB`;
  }

  /** The UI copies selected text with OSC 52; hand it to the browser's clipboard. */
  private copySelections(bytes: Uint8Array): void {
    const text = this.decoder.decode(bytes, { stream: true });
    // oxlint-disable-next-line no-control-regex -- OSC 52 starts with ESC and ends with BEL.
    for (const match of text.matchAll(/\x1b\]52;c;([A-Za-z0-9+/=]*)\x07/g)) {
      const binary = atob(match[1] ?? '');
      const decoded = new TextDecoder().decode(Uint8Array.from(binary, (c) => c.charCodeAt(0)));
      void navigator.clipboard?.writeText(decoded).catch(() => {});
    }
  }

  /**
   * ghostty-web doesn't report the mouse to the program, so when the UI turns
   * mouse reporting on, translate presses, drags and the wheel into SGR mouse
   * sequences, as a native terminal would.
   */
  private forwardMouse(host: HTMLElement): void {
    const cell = (event: MouseEvent): [number, number] => {
      const canvas = host.querySelector('canvas');
      const rect = (canvas ?? host).getBoundingClientRect();
      const col = Math.floor(((event.clientX - rect.left) / rect.width) * this.term.cols) + 1;
      const row = Math.floor(((event.clientY - rect.top) / rect.height) * this.term.rows) + 1;
      return [
        Math.max(1, Math.min(this.term.cols, col)),
        Math.max(1, Math.min(this.term.rows, row)),
      ];
    };
    const send = (button: number, event: MouseEvent, release = false): void => {
      const [col, row] = cell(event);
      this.demo?.input(this.encoder.encode(`\x1b[<${button};${col};${row}${release ? 'm' : 'M'}`));
    };
    const tracking = (): boolean => {
      try {
        return this.term.hasMouseTracking();
      } catch {
        return false;
      }
    };

    this.frame.addEventListener(
      'mousedown',
      (event) => {
        if (event.button !== 0 || !tracking()) return;
        event.preventDefault();
        event.stopImmediatePropagation();
        this.focus();
        this.dragging = true;
        send(0, event);
      },
      { capture: true },
    );
    window.addEventListener('mousemove', (event) => {
      if (this.dragging) send(32, event);
    });
    window.addEventListener('mouseup', (event) => {
      if (!this.dragging) return;
      this.dragging = false;
      send(0, event, true);
    });
    this.term.attachCustomWheelEventHandler((event) => {
      if (!tracking()) return false;
      const lines = Math.max(1, Math.min(3, Math.round(Math.abs(event.deltaY) / 40)));
      for (let i = 0; i < lines; i++) send(event.deltaY < 0 ? 64 : 65, event);
      return true;
    });
  }
}

/**
 * Plays a figure's frames in a loop while it's on screen, holding the last
 * one longer. With reduced motion, it starts paused on the last frame.
 */
function playFrames(player: HTMLElement): void {
  const frames = [...player.querySelectorAll<HTMLElement>('.player-frame')];
  const toggle = player.querySelector<HTMLButtonElement>('.player-toggle');
  const count = player.querySelector<HTMLElement>('.player-count');
  if (frames.length === 0) return;
  const last = frames.length - 1;
  let playing = !matchMedia('(prefers-reduced-motion: reduce)').matches;
  let index = playing ? 0 : last;
  let shownFor = 0;
  const show = (next: number): void => {
    frames.forEach((frame, i) => {
      frame.classList.toggle('is-current', i === next);
      frame.toggleAttribute('aria-hidden', i !== next);
    });
    if (count) count.textContent = `Frame ${next + 1} of ${frames.length}`;
  };
  const setPlaying = (value: boolean): void => {
    playing = value;
    if (toggle) {
      toggle.textContent = playing ? 'Pause' : 'Play';
      toggle.setAttribute('aria-pressed', String(!playing));
    }
  };
  toggle?.addEventListener('click', () => setPlaying(!playing));
  setPlaying(playing);
  show(index);
  const step = 200;
  setInterval(() => {
    const rect = player.getBoundingClientRect();
    if (!playing || rect.bottom < 0 || rect.top > innerHeight) return;
    shownFor += step;
    if (shownFor < (index === last ? 4000 : 1200)) return;
    shownFor = 0;
    index = index === last ? 0 : index + 1;
    show(index);
  }, step);
}

/** Marks the section being read in the contents rail. */
function trackContents(): void {
  const links = new Map<string, HTMLAnchorElement>();
  for (const link of document.querySelectorAll<HTMLAnchorElement>('.rail a')) {
    links.set(link.hash.slice(1), link);
  }
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        for (const link of links.values()) link.removeAttribute('aria-current');
        links.get(entry.target.id)?.setAttribute('aria-current', 'true');
      }
    },
    { rootMargin: '-20% 0px -70% 0px' },
  );
  for (const id of links.keys()) {
    const target = document.getElementById(id);
    if (target) observer.observe(target);
  }
}

async function main(): Promise<void> {
  await Promise.all([
    initGhostty(),
    initDemo(),
    document.fonts.load(`15px ${FONT}`).catch(() => undefined),
    document.fonts.load(`bold 15px ${FONT}`).catch(() => undefined),
  ]);
  const demos = [...document.querySelectorAll<HTMLElement>('.demo')].map(
    (root) => new DemoTerminal(root),
  );
  // For poking at the demos from the console.
  Object.assign(window, { demos });
  trackContents();
  for (const player of document.querySelectorAll<HTMLElement>('.player')) playFrames(player);
}

void main();
