// A console that follows the newest line until the user scrolls up, with a button to jump back
// down and follow again. The element is kept between renders; `restoreOutputs` puts the scroll
// position back after each render, because moving it into the new page resets it.
import { h, icon } from "./ui";

const all: LiveOutput[] = [];

export class LiveOutput {
  readonly pre: HTMLPreElement = h("pre", { class: "output" });
  readonly el: HTMLDivElement;
  private readonly jump: HTMLButtonElement;
  private follow = true;
  private top = 0;
  /// When the user last touched the console. Only their scrolling stops following; the resets
  /// from re-rendering don't.
  private touched = 0;
  private dragging = false;
  /// Lines that arrived since the last frame. They're added together, once per frame, because
  /// adding and scrolling per line makes the browser lay out the page thousands of times a second.
  private pending: [string, string][] = [];
  private frame = 0;

  constructor(private readonly maxNodes: number) {
    this.jump = h("button", { class: "btn small output-jump", title: "Jump to the newest output and keep following it", onclick: () => this.toBottom() }, icon("chevronDown", 14), "Latest");
    this.el = h("div", { class: "output-wrap" }, this.pre, this.jump);
    const touch = () => (this.touched = Date.now());
    for (const ev of ["wheel", "touchmove", "keydown"]) this.pre.addEventListener(ev, touch, { passive: true });
    this.pre.addEventListener("pointerdown", () => {
      this.dragging = true;
      touch();
    });
    window.addEventListener("pointerup", () => (this.dragging = false));
    this.pre.addEventListener("scroll", () => this.onScroll());
    this.update();
    all.push(this);
  }

  private atBottom() {
    return this.pre.scrollTop + this.pre.clientHeight >= this.pre.scrollHeight - 4;
  }

  private onScroll() {
    if (!this.pre.isConnected || this.pre.clientHeight === 0) return;
    const byUser = this.dragging || Date.now() - this.touched < 800;
    if (this.atBottom()) this.follow = true;
    else if (byUser) this.follow = false;
    else {
      this.restore();
      return;
    }
    this.top = this.pre.scrollTop;
    this.update();
  }

  private update() {
    this.jump.hidden = this.follow || this.pre.childNodes.length === 0;
  }

  toBottom() {
    this.follow = true;
    this.pre.scrollTop = this.pre.scrollHeight;
    this.top = this.pre.scrollTop;
    this.update();
  }

  /// Puts the scroll position back: the bottom while following, otherwise where the user left it.
  restore() {
    if (!this.pre.isConnected) return;
    if (this.follow) this.pre.scrollTop = this.pre.scrollHeight;
    else this.pre.scrollTop = this.top;
  }

  append(level: string, text: string) {
    this.pending.push([level, text]);
    if (this.pending.length > this.maxNodes) this.pending.splice(0, this.pending.length - this.maxNodes);
    if (!this.frame) this.frame = requestAnimationFrame(() => this.flush());
  }

  private flush() {
    this.frame = 0;
    if (this.pending.length === 0) return;
    const frag = document.createDocumentFragment();
    for (const [level, text] of this.pending) frag.append(level === "info" ? h("span", { class: "info" }, text + "\n") : text + "\n");
    this.pending = [];
    this.pre.append(frag);
    const extra = this.pre.childNodes.length - this.maxNodes;
    if (extra > 0) {
      const range = document.createRange();
      range.setStartBefore(this.pre.firstChild!);
      range.setEndAfter(this.pre.childNodes[extra - 1]);
      range.deleteContents();
    }
    if (this.follow) this.pre.scrollTop = this.pre.scrollHeight;
    this.top = this.pre.scrollTop;
    this.update();
  }

  clear() {
    this.pending = [];
    this.pre.replaceChildren();
    this.follow = true;
    this.top = 0;
    this.update();
  }

  get text() {
    return (this.pre.textContent ?? "") + this.pending.map(([, t]) => t + "\n").join("");
  }

  get empty() {
    return this.pre.childNodes.length === 0 && this.pending.length === 0;
  }
}

export function restoreOutputs() {
  for (const o of all) o.restore();
}
