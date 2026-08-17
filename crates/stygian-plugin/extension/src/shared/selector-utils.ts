(() => {
  const STABLE_ATTRIBUTE_NAMES = [
    "data-testid",
    "data-test",
    "data-testid",
    "data-qa",
    "data-qa-id",
    "aria-label",
    "name",
    "role",
    "title",
  ];

  function cssEscape(value: string): string {
    const cssApi = (globalThis as any).CSS;
    if (cssApi && typeof cssApi.escape === "function") {
      return cssApi.escape(value);
    }

    return value.replace(/[^a-zA-Z0-9_-]/g, (char) => `\\${char}`);
  }

  function isUnique(selector: string): boolean {
    try {
      return document.querySelectorAll(selector).length === 1;
    } catch {
      return false;
    }
  }

  function isAnchoredSelector(selector: string): boolean {
    return /[#.\[]/.test(selector);
  }

  function isStableClassName(className: string): boolean {
    if (!className || className.length > 40) {
      return false;
    }

    if (/\d{4,}/.test(className)) {
      return false;
    }

    if (/[A-Fa-f0-9]{8,}/.test(className)) {
      return false;
    }

    return /^[a-zA-Z][a-zA-Z0-9_-]*$/.test(className);
  }

  function nthOfTypeSelector(element: Element): string {
    const tagName = element.tagName.toLowerCase();
    let index = 1;
    let sibling = element.previousElementSibling;
    while (sibling) {
      if (sibling.tagName === element.tagName) {
        index += 1;
      }
      sibling = sibling.previousElementSibling;
    }
    return `${tagName}:nth-of-type(${index})`;
  }

  function candidateSelectorsForElement(element: Element): string[] {
    const tagName = element.tagName.toLowerCase();
    const selectors: string[] = [];

    if (element.id) {
      selectors.push(`#${cssEscape(element.id)}`);
      selectors.push(`${tagName}#${cssEscape(element.id)}`);
    }

    for (const attr of STABLE_ATTRIBUTE_NAMES) {
      const value = element.getAttribute(attr);
      if (value && value.trim().length > 0) {
        const escaped = value.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
        selectors.push(`[${attr}="${escaped}"]`);
        selectors.push(`${tagName}[${attr}="${escaped}"]`);
      }
    }

    const stableClasses = Array.from(element.classList).filter(
      isStableClassName,
    );
    if (stableClasses.length > 0) {
      selectors.push(
        `${tagName}.${stableClasses
          .slice(0, 2)
          .map((name) => cssEscape(name))
          .join(".")}`,
      );
    }

    selectors.push(tagName);
    selectors.push(nthOfTypeSelector(element));

    return Array.from(new Set(selectors));
  }

  function uniqueSelectorForElement(element: Element): string {
    const localCandidates = candidateSelectorsForElement(element);
    for (const selector of localCandidates) {
      if (isUnique(selector) && isAnchoredSelector(selector)) {
        return selector;
      }
    }

    let current: Element | null = element;
    let suffix = nthOfTypeSelector(element);

    while (current?.parentElement) {
      current = current.parentElement;

      for (const candidate of candidateSelectorsForElement(current)) {
        const combined = `${candidate} > ${suffix}`;
        if (isUnique(combined) && isAnchoredSelector(candidate)) {
          return combined;
        }
      }

      suffix = `${nthOfTypeSelector(current)} > ${suffix}`;
    }

    return suffix;
  }

  function xpathForElement(element: Element): string {
    const parts: string[] = [];
    let current: Element | null = element;

    while (current && current !== document.documentElement) {
      let index = 1;
      let sibling = current.previousElementSibling;
      while (sibling) {
        if (sibling.tagName === current.tagName) {
          index += 1;
        }
        sibling = sibling.previousElementSibling;
      }

      parts.unshift(`${current.tagName.toLowerCase()}[${index}]`);
      current = current.parentElement;
    }

    return `/${parts.join("/")}`;
  }

  function getElementPath(element: Element): { css: string; xpath: string } {
    return {
      css: uniqueSelectorForElement(element),
      xpath: xpathForElement(element),
    };
  }

  (globalThis as any).StygianSelectorUtils = {
    getElementPath,
    uniqueSelectorForElement,
  };
})();
