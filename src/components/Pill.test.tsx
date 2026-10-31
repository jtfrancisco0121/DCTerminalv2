// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Pill } from "./Pill";

describe("Pill", () => {
  it("shows the words with the tone class, extra class, title and test id", () => {
    render(
      <Pill tone="danger" className="ci-chip" title="why" testId="p">
        CI failed
      </Pill>,
    );
    const pill = screen.getByTestId("p");
    expect(pill.textContent).toBe("CI failed");
    expect(pill.className).toBe("pill tone-danger ci-chip");
    expect(pill.getAttribute("title")).toBe("why");
  });

  it("renders a leading icon before the words", () => {
    render(
      <Pill tone="you" icon={<i data-testid="ic" />} testId="p">
        Ship approval
      </Pill>,
    );
    expect(screen.getByTestId("p").firstElementChild?.getAttribute("data-testid")).toBe("ic");
    expect(screen.getByTestId("p").className).toBe("pill tone-you");
  });
});
