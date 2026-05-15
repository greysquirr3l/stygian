// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";

import "../src/shared/selector-utils";

const selectorUtils = (globalThis as any).StygianSelectorUtils as {
  uniqueSelectorForElement: (element: Element) => string;
  getElementPath: (element: Element) => { css: string; xpath: string };
};

describe("selector-utils", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("prefers stable testing attributes when available", () => {
    document.body.innerHTML = `
      <main>
        <button data-testid="save-contact">Save</button>
      </main>
    `;

    const target = document.querySelector("button")!;
    expect(selectorUtils.uniqueSelectorForElement(target)).toBe(
      '[data-testid="save-contact"]',
    );
  });

  it("falls back to a unique ancestor plus nth-of-type when siblings repeat", () => {
    document.body.innerHTML = `
      <section data-testid="users">
        <ul>
          <li>Ada</li>
          <li>Grace</li>
        </ul>
      </section>
      <section data-testid="admins">
        <ul>
          <li>Linus</li>
          <li>Barbara</li>
        </ul>
      </section>
    `;

    const target = document.querySelector(
      '[data-testid="users"] li:nth-of-type(2)',
    )!;
    expect(selectorUtils.uniqueSelectorForElement(target)).toBe(
      '[data-testid="users"] > ul:nth-of-type(1) > li:nth-of-type(2)',
    );
  });

  it("returns both CSS and XPath paths", () => {
    document.body.innerHTML = `
      <div id="contact-list">
        <article>
          <h2>Ada Lovelace</h2>
        </article>
      </div>
    `;

    const target = document.querySelector("h2")!;
    const path = selectorUtils.getElementPath(target);
    expect(path.css).toBe(
      "#contact-list > article:nth-of-type(1) > h2:nth-of-type(1)",
    );
    expect(path.xpath).toBe("/body[1]/div[1]/article[1]/h2[1]");
  });
});
