// @vitest-environment jsdom
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import {
  BranchIcon,
  CheckIcon,
  ClockIcon,
  CrossIcon,
  GearIcon,
  PrIcon,
  SpinnerIcon,
  WarningIcon,
} from "./icons";

const ICONS = { CheckIcon, CrossIcon, ClockIcon, SpinnerIcon, BranchIcon, PrIcon, GearIcon, WarningIcon };

describe("icons", () => {
  it.each(Object.entries(ICONS))("%s is a hidden inline svg using currentColor", (_name, Icon) => {
    const { container } = render(<Icon />);
    const svg = container.querySelector("svg");
    expect(svg).not.toBeNull();
    expect(svg?.getAttribute("aria-hidden")).toBe("true");
    expect(svg?.getAttribute("focusable")).toBe("false");
    expect(svg?.getAttribute("stroke")).toBe("currentColor");
    expect(svg?.getAttribute("width")).toBe("14");
  });

  it("clamps the size to 12-16", () => {
    const small = render(<CheckIcon size={10} />).container.querySelector("svg");
    expect(small?.getAttribute("width")).toBe("12");
    expect(small?.getAttribute("height")).toBe("12");
    const big = render(<CheckIcon size={20} />).container.querySelector("svg");
    expect(big?.getAttribute("width")).toBe("16");
    const ok = render(<CheckIcon size={13} />).container.querySelector("svg");
    expect(ok?.getAttribute("width")).toBe("13");
  });

  it("marks the spinner for the rotation", () => {
    const svg = render(<SpinnerIcon />).container.querySelector("svg");
    expect(svg?.getAttribute("class")).toContain("icon-spin");
  });
});
