// @vitest-environment jsdom
// The in-page click finder of plugin/scripts/qa-browser.mjs (pickTarget), run in jsdom. jsdom has no layout, so each test stubs
// getBoundingClientRect and document.elementFromPoint: the finder depends on those two APIs only.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { pickTarget } from "../plugin/scripts/qa-browser.mjs";

const box = (el, x, y, w = 80, h = 20) => {
  el.getBoundingClientRect = () => ({ left: x, top: y, width: w, height: h, right: x + w, bottom: y + h, x, y });
  return el;
};
const add = (parent, tag, attrs = {}, text = "") => {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  el.textContent = text;
  parent.appendChild(el);
  return el;
};
/** elementFromPoint answers from a table of point -> element (the topmost thing there). */
const hits = (table) => {
  document.elementFromPoint = (x, y) => table.get(`${x},${y}`) ?? null;
};
const centre = (x, y, w = 80, h = 20) => `${x + w / 2},${y + h / 2}`;

beforeEach(() => {
  document.body.innerHTML = "";
});
afterEach(() => {
  delete document.elementFromPoint;
});

describe("pickTarget", () => {
  it("a: prefers the Resume button inside the open dialog over the one behind it", () => {
    const behind = box(add(document.body, "button", {}, "Resume"), 0, 0);
    const dialog = box(add(document.body, "div", { role: "dialog" }), 100, 100, 300, 200);
    const inside = box(add(dialog, "button", {}, "Resume"), 120, 120);
    hits(
      new Map([
        [centre(0, 0), dialog],
        [centre(120, 120), inside],
      ]),
    );
    expect(pickTarget(document, { text: "resume" })).toEqual({ x: 160, y: 130 });
    expect(behind).toBeTruthy();
  });

  it("ranks an aria-modal dialog before a plain popover that comes later", () => {
    const modal = box(add(document.body, "div", { role: "dialog", "aria-modal": "true" }), 0, 0, 300, 300);
    const inModal = box(add(modal, "button", {}, "Save"), 10, 10);
    const pop = box(add(document.body, "div", { role: "dialog" }), 400, 0, 100, 100);
    const inPop = box(add(pop, "button", {}, "Save"), 410, 10);
    hits(
      new Map([
        [centre(10, 10), inModal],
        [centre(410, 10), inPop],
      ]),
    );
    expect(pickTarget(document, { text: "save" })).toEqual({ x: 50, y: 20 });
  });

  it("b: names the overlay when the only match is hidden behind it", () => {
    const button = box(add(document.body, "button", {}, "Resume"), 0, 0);
    const overlay = box(add(document.body, "div", { class: "overlay" }, "Runs and more"), 0, 0, 500, 500);
    hits(new Map([[centre(0, 0), overlay]]));
    expect(button).toBeTruthy();
    expect(pickTarget(document, { text: "resume" })).toEqual({ covered: 'div.overlay "Runs and more"' });
  });

  it("c: within limits the search, and a missing within is an error", () => {
    const main = box(add(document.body, "main"), 0, 0, 400, 400);
    const side = box(add(document.body, "aside"), 500, 0, 100, 100);
    const a = box(add(main, "button", {}, "Go"), 10, 10);
    const b = box(add(side, "button", {}, "Go"), 510, 10);
    hits(
      new Map([
        [centre(10, 10), a],
        [centre(510, 10), b],
      ]),
    );
    expect(pickTarget(document, { text: "go", within: "aside" })).toEqual({ x: 550, y: 20 });
    expect(pickTarget(document, { text: "go", within: "main" })).toEqual({ x: 50, y: 20 });
    expect(pickTarget(document, { text: "go", within: "#nope" })).toEqual({ error: "nothing matches within #nope" });
    expect(pickTarget(document, { text: "nothing here", within: "main" })).toBeNull();
  });

  it("d: a selector click on a covered element is covered, and one on a free element is picked", () => {
    const covered = box(add(document.body, "button", { id: "a" }, "A"), 0, 0);
    const free = box(add(document.body, "button", { id: "b" }, "B"), 0, 100);
    const cover = box(add(document.body, "div", { class: "scrim" }), 0, 0, 300, 50);
    hits(
      new Map([
        [centre(0, 0), cover],
        [centre(0, 100), free],
      ]),
    );
    expect(covered).toBeTruthy();
    expect(pickTarget(document, { selector: "#a" })).toEqual({ covered: 'div.scrim ""' });
    expect(pickTarget(document, { selector: "#b" })).toEqual({ x: 40, y: 110 });
  });

  it("skips a covered match for a later reachable one", () => {
    const one = box(add(document.body, "button", {}, "Next"), 0, 0);
    const two = box(add(document.body, "button", {}, "Next"), 0, 100);
    const cover = box(add(document.body, "div"), 0, 0, 300, 50);
    hits(
      new Map([
        [centre(0, 0), cover],
        [centre(0, 100), two],
      ]),
    );
    expect(one).toBeTruthy();
    expect(pickTarget(document, { text: "next" })).toEqual({ x: 40, y: 110 });
  });

  it("reports off screen and not visible instead of throwing", () => {
    box(add(document.body, "button", {}, "Far"), 5000, 5000);
    hits(new Map());
    expect(pickTarget(document, { text: "far" })).toEqual({ unreachable: "off screen" });
    document.body.innerHTML = "";
    box(add(document.body, "button", { style: "display:none" }, "Gone"), 0, 0, 0, 0);
    expect(pickTarget(document, { text: "gone" })).toEqual({ unreachable: "not visible" });
  });

  it("works when rebuilt from its source, as the driver injects it, in a page without innerText", () => {
    const rebuilt = new Function(`return (${pickTarget.toString()})`)();
    const button = box(add(document.body, "button", {}, "Resume"), 0, 0);
    hits(new Map([[centre(0, 0), button]]));
    expect("innerText" in button).toBe(false);
    expect(rebuilt(document, { text: "RESUME" })).toEqual({ x: 40, y: 10 });
  });
});
