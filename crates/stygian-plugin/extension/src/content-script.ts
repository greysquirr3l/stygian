/// <reference types="chrome" />

/**
 * Content script for Stygian Plugin
 * Handles DOM interaction, element selection, and template recording
 */

// Keep content script as a classic script (not ES module) for MV3 compatibility.
type CsExtractionTemplate = any;
type CsRegion = any;
type CsSelector = any;
type PluginRecordingState = any;
type CsElementWithPath = any;

(() => {
  // ─────────────────────────────────────────────────────────────────────────────
  // State Management
  // ─────────────────────────────────────────────────────────────────────────────

  let recordingState: PluginRecordingState = {
    active: false,
    template_name: "",
    regions: [],
  };

  let highlightedElement: Element | null = null;
  let pendingElement: Element | null = null; // element awaiting region name
  let selectedElements: Set<Element> = new Set();
  let regionHistory: CsRegion[] = []; // for undo

  // ─────────────────────────────────────────────────────────────────────────────
  // Message Listener
  // ─────────────────────────────────────────────────────────────────────────────

  chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    console.log("[Content] Received message:", message.type);

    switch (message.type) {
      case "ping":
        sendResponse({ success: true });
        break;

      case "start_recording":
        startRecording(message.template_name);
        sendResponse({ success: true });
        break;

      case "stop_recording":
        stopRecording();
        sendResponse({ success: true, regions: recordingState.regions });
        break;

      case "get_selected_selector":
        if (highlightedElement) {
          const path = getElementPath(highlightedElement);
          sendResponse({
            success: true,
            css_path: path.css,
            xpath_path: path.xpath,
            element_text: highlightedElement.textContent?.slice(0, 100),
          });
        } else {
          sendResponse({ success: false, error: "No element selected" });
        }
        break;

      case "highlight_selector":
        highlightElementBySelector(message.selector);
        sendResponse({ success: true });
        break;

      case "clear_highlights":
        clearAllHighlights();
        sendResponse({ success: true });
        break;

      case "get_page_html":
        sendResponse({
          success: true,
          html: document.documentElement.outerHTML,
        });
        break;

      case "extract_with_template":
        {
          try {
            const extracted = extractWithTemplate(message.template);
            sendResponse({ success: true, ...extracted });
          } catch (error) {
            sendResponse({ success: false, error: String(error) });
          }
        }
        break;

      case "extract_with_template_batch":
        {
          try {
            const extracted = extractWithTemplateBatch(
              message.template,
              message.root_selector,
            );
            sendResponse({ success: true, ...extracted });
          } catch (error) {
            sendResponse({ success: false, error: String(error) });
          }
        }
        break;

      case "inspect_selector_local":
        {
          try {
            const inspected = inspectSelectorLocal(message.selector_css);
            sendResponse({ success: true, ...inspected });
          } catch (error) {
            sendResponse({ success: false, error: String(error) });
          }
        }
        break;

      default:
        sendResponse({ success: false, error: "Unknown message type" });
    }

    return true; // Keep the message channel open for async responses
  });

  // ─────────────────────────────────────────────────────────────────────────────
  // Recording Functions
  // ─────────────────────────────────────────────────────────────────────────────

  function startRecording(templateName: string) {
    recordingState.active = true;
    recordingState.template_name = templateName;
    recordingState.regions = [];
    regionHistory = [];

    console.log("[Content] Recording started:", templateName);

    showRecordingOverlay();

    document.addEventListener("mouseover", onElementHover, true);
    document.addEventListener("mousedown", onElementClick, true);
    document.addEventListener("keydown", onRecordingKey, true);
  }

  function stopRecording() {
    recordingState.active = false;

    console.log("[Content] Recording stopped");

    removeRecordingOverlay();
    removeNameInputCard();
    removeHoverTooltip();

    document.removeEventListener("mouseover", onElementHover, true);
    document.removeEventListener("mousedown", onElementClick, true);
    document.removeEventListener("keydown", onRecordingKey, true);

    clearAllHighlights();
    pendingElement = null;
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // DOM Interaction
  // ─────────────────────────────────────────────────────────────────────────────

  function onElementHover(event: Event) {
    if (!recordingState.active || pendingElement) return;

    const target = event.target as Element;
    // Skip our own injected UI elements
    if ((target as HTMLElement).closest("[data-stygian]")) return;
    if (target === highlightedElement) return;

    if (highlightedElement) {
      unhighlightElement(highlightedElement);
    }

    highlightedElement = target;
    highlightElement(target, "hover");
    showHoverTooltip(target);
  }

  function onElementClick(event: MouseEvent) {
    if (!recordingState.active) return;
    if ((event.target as HTMLElement).closest("[data-stygian]")) return;
    if (!highlightedElement) return;

    event.preventDefault();
    event.stopPropagation();

    // Lock element and show name input card
    pendingElement = highlightedElement;
    unhighlightElement(highlightedElement);
    highlightElement(pendingElement, "locked");
    removeHoverTooltip();
    showNameInputCard(pendingElement);
  }

  function onRecordingKey(event: KeyboardEvent) {
    if (!recordingState.active) return;

    // Escape: cancel pending pick or stop recording
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (pendingElement) {
        unhighlightElement(pendingElement);
        pendingElement = null;
        removeNameInputCard();
        return;
      }
      stopRecording();
      chrome.runtime.sendMessage({ type: "recording_stopped" });
      return;
    }

    // U: undo last region (only when no card is open)
    if ((event.key === "u" || event.key === "U") && !pendingElement) {
      const nameCard = document.getElementById("stygian-name-card");
      if (nameCard) return; // card is open, let natural key handling proceed
      event.preventDefault();
      event.stopPropagation();
      undoLastRegion();
    }
  }

  function undoLastRegion() {
    if (recordingState.regions.length === 0) return;
    const removed = recordingState.regions.pop();
    console.log("[Content] Undo region:", removed?.name);
    updateOverlayCount();
    chrome.runtime.sendMessage({
      type: "region_undo",
      regions: recordingState.regions,
    });
  }

  function extractWithTemplate(template: CsExtractionTemplate): {
    data: Record<string, unknown>;
    metadata: {
      regions_successful: number;
      total_regions: number;
      elapsed_ms: number;
      source: string;
    };
  } {
    const startedAt = performance.now();
    const regions = Array.isArray(template?.regions) ? template.regions : [];
    const data: Record<string, unknown> = {};
    let regionsSuccessful = 0;

    for (const region of regions) {
      const value = extractRegionValue(region);
      if (value !== null && value !== undefined && value !== "") {
        regionsSuccessful++;
      }
      data[region.name] = value;
    }

    return {
      data,
      metadata: {
        regions_successful: regionsSuccessful,
        total_regions: regions.length,
        elapsed_ms: Math.round(performance.now() - startedAt),
        source: "local_fallback",
      },
    };
  }

  function extractWithTemplateBatch(
    template: CsExtractionTemplate,
    rootSelector: string,
  ): {
    root_selector: string;
    results: Array<Record<string, unknown>>;
    total_matched: number;
    successful: number;
    debug: { evaluation_scope: string; first_root_html: string | null };
  } {
    if (typeof rootSelector !== "string" || rootSelector.trim().length === 0) {
      throw new Error("Missing root selector");
    }

    const roots = Array.from(document.querySelectorAll(rootSelector));
    if (roots.length === 0) {
      throw new Error(`root_selector matched no elements: ${rootSelector}`);
    }

    const results = roots.map((root) => {
      const regions = Array.isArray(template?.regions) ? template.regions : [];
      const data: Record<string, unknown> = {};
      let successfulRegions = 0;

      for (const region of regions) {
        const value = extractRegionValue(region, root);
        if (value !== null && value !== undefined && value !== "") {
          successfulRegions += 1;
        }
        data[region.name] = value;
      }

      return {
        data,
        successful_regions: successfulRegions,
      };
    });

    return {
      root_selector: rootSelector,
      results,
      total_matched: roots.length,
      successful: results.filter((entry) => typeof entry.data === "object")
        .length,
      debug: {
        evaluation_scope: "root_fragment",
        first_root_html: roots[0]?.outerHTML?.slice(0, 2000) ?? null,
      },
    };
  }

  function extractRegionValue(
    region: CsRegion,
    root: ParentNode = document,
  ): unknown {
    const selector = region?.selector;
    const cssSelector = selector?.css;
    const xpathSelector = selector?.xpath;

    let node: Element | null = null;
    if (typeof cssSelector === "string" && cssSelector.length > 0) {
      try {
        if (root instanceof Document) {
          node = root.querySelector(cssSelector);
        } else if (
          root instanceof Element ||
          root instanceof DocumentFragment
        ) {
          node = root.querySelector(cssSelector);
        }
      } catch {
        // Ignore invalid selector; fallback to XPath.
      }
    }

    if (
      !node &&
      typeof xpathSelector === "string" &&
      xpathSelector.length > 0
    ) {
      try {
        const result = document.evaluate(
          xpathSelector,
          root instanceof Node ? root : document,
          null,
          XPathResult.FIRST_ORDERED_NODE_TYPE,
          null,
        );
        node = result.singleNodeValue as Element | null;
      } catch {
        // Ignore invalid XPath.
      }
    }

    const rawValue = node?.textContent?.trim() ?? "";
    return applyTransformations(rawValue, region?.transformations);
  }

  function inspectSelectorLocal(selectorCss: string): {
    selector: string;
    selector_type: string;
    valid: boolean;
    match_count: number;
    preview: string;
  } {
    if (typeof selectorCss !== "string" || selectorCss.trim().length === 0) {
      throw new Error("Selector cannot be empty");
    }

    const matches = Array.from(document.querySelectorAll(selectorCss));
    const preview =
      matches[0]?.textContent?.trim().slice(0, 120) ?? "No elements matched";

    return {
      selector: selectorCss,
      selector_type: "css",
      valid: true,
      match_count: matches.length,
      preview,
    };
  }

  function applyTransformations(
    value: string,
    transformations: any[],
  ): unknown {
    if (!Array.isArray(transformations) || transformations.length === 0) {
      return value;
    }

    let current: unknown = value;

    for (const step of transformations) {
      const type = typeof step === "string" ? step : step?.type;
      if (typeof type !== "string") continue;

      if (type === "Trim" && typeof current === "string") {
        current = current.trim();
      } else if (type === "Lowercase" && typeof current === "string") {
        current = current.toLowerCase();
      } else if (type === "Uppercase" && typeof current === "string") {
        current = current.toUpperCase();
      } else if (type === "RemoveWhitespace" && typeof current === "string") {
        current = current.replace(/\s+/g, "");
      } else if (
        type === "NormalizeWhitespace" &&
        typeof current === "string"
      ) {
        current = current.replace(/\s+/g, " ").trim();
      } else if (type === "StripHtml" && typeof current === "string") {
        current = current.replace(/<[^>]+>/g, "");
      } else if (type === "DecodeHtml" && typeof current === "string") {
        // Behavior matches the prior `textarea.innerHTML` trick:
        // HTML entities are decoded AND any real markup in the input is stripped
        // (e.g. `<b>hi</b>` → `hi`, `&lt;b&gt;hi&lt;/b&gt;` → `<b>hi</b>`).
        // The DOMParser + textContent result is a plain string assigned back to
        // `current` and never written to the DOM.
        // codeql[js/xss-through-dom] - see comment above; the parseFromString
        // call is an entity-decode sink whose textContent result is a string.
        current = new DOMParser()
          // codeql[js/xss-through-dom] - taint-tracking hits this call; result is read-only text.
          .parseFromString(current, "text/html")
          .documentElement.textContent ?? current;
      } else if (type === "ParseJson" && typeof current === "string") {
        try {
          current = JSON.parse(current);
        } catch {
          // Keep original text if parsing fails.
        }
      } else if (type === "Regex" && typeof current === "string") {
        const pattern = typeof step?.pattern === "string" ? step.pattern : "";
        const replacement =
          typeof step?.replacement === "string" ? step.replacement : "";
        try {
          current = current.replace(new RegExp(pattern, "g"), replacement);
        } catch {
          // Ignore invalid regex.
        }
      } else if (type === "RegexExtract" && typeof current === "string") {
        const pattern = typeof step?.pattern === "string" ? step.pattern : "";
        const group =
          typeof step?.group === "number" && Number.isInteger(step.group)
            ? step.group
            : 0;
        try {
          const match = current.match(new RegExp(pattern));
          current = match?.[group] ?? "";
        } catch {
          // Ignore invalid regex.
        }
      } else if (type === "Filter" && typeof current === "string") {
        const pattern = typeof step?.pattern === "string" ? step.pattern : "";
        try {
          current = new RegExp(pattern).test(current) ? current : "";
        } catch {
          // Ignore invalid regex.
        }
      }
    }

    return current;
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Element Highlighting
  // ─────────────────────────────────────────────────────────────────────────────

  // mode: "hover" = blue, "locked" = amber, "confirmed" = green
  function highlightElement(
    element: Element,
    mode: "hover" | "locked" | "confirmed" = "hover",
  ) {
    const rect = element.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) return;

    const highlight = document.createElement("div");
    highlight.className = "stygian-highlight";
    highlight.setAttribute("data-stygian", "highlight");
    highlight.style.cssText = `
      position: fixed;
      top: ${rect.top}px;
      left: ${rect.left}px;
      width: ${rect.width}px;
      height: ${rect.height}px;
      border-radius: 4px;
      z-index: 999998;
      pointer-events: none;
      transition: background-color 0.1s, border-color 0.1s;
    `;

    if (mode === "hover") {
      highlight.style.backgroundColor = "rgba(102, 126, 234, 0.18)";
      highlight.style.border = "2px solid #667eea";
    } else if (mode === "locked") {
      highlight.style.backgroundColor = "rgba(251, 191, 36, 0.25)";
      highlight.style.border = "2px solid #f59e0b";
      highlight.style.boxShadow = "0 0 0 4px rgba(245, 158, 11, 0.2)";
    } else {
      highlight.style.backgroundColor = "rgba(72, 187, 120, 0.2)";
      highlight.style.border = "2px solid #48bb78";
    }

    document.body.appendChild(highlight);
    (element as any).__stygian_highlight = highlight;
  }

  function unhighlightElement(element: Element) {
    const highlight = (element as any).__stygian_highlight;
    if (highlight) {
      highlight.remove();
      delete (element as any).__stygian_highlight;
    }
  }

  function clearAllHighlights() {
    document.querySelectorAll('[data-stygian="highlight"]').forEach((el) => {
      el.remove();
    });
    highlightedElement = null;
    selectedElements.clear();
  }

  function highlightElementBySelector(selector: string) {
    clearAllHighlights();
    try {
      const elements = document.querySelectorAll(selector);
      elements.forEach((el) => {
        highlightElement(el, "confirmed");
        selectedElements.add(el);
      });
    } catch (e) {
      console.error("[Content] Invalid selector:", e);
    }
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Confidence Scoring
  // ─────────────────────────────────────────────────────────────────────────────

  interface SelectorConfidence {
    level: "strong" | "good" | "fragile";
    label: string;
    color: string;
    matchCount: number;
  }

  function getSelectorConfidence(
    element: Element,
    cssSelector: string,
  ): SelectorConfidence {
    let matchCount = 0;
    try {
      matchCount = document.querySelectorAll(cssSelector).length;
    } catch {
      matchCount = 0;
    }

    const hasId = /\#[\w-]+/.test(cssSelector);
    const hasDataAttr = /\[data-/.test(cssSelector);
    const hasNthChild = /nth-child|nth-of-type/.test(cssSelector);
    const hasAriaAttr = /\[aria-/.test(cssSelector);

    let level: "strong" | "good" | "fragile";
    let label: string;
    let color: string;

    if ((hasId || hasDataAttr || hasAriaAttr) && matchCount === 1) {
      level = "strong";
      label = `Strong — 1 unique match`;
      color = "#48bb78";
    } else if (matchCount <= 3 && !hasNthChild) {
      level = "good";
      label = `Good — ${matchCount} match${matchCount !== 1 ? "es" : ""}`;
      color = "#f6ad55";
    } else {
      level = "fragile";
      label = `Fragile — ${matchCount} match${matchCount !== 1 ? "es" : ""}`;
      color = "#fc8181";
    }

    return { level, label, color, matchCount };
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Hover Tooltip
  // ─────────────────────────────────────────────────────────────────────────────

  function showHoverTooltip(element: Element) {
    removeHoverTooltip();

    const path = getElementPath(element);
    const confidence = getSelectorConfidence(element, path.css);
    const rect = element.getBoundingClientRect();

    const tag = element.tagName.toLowerCase();
    const idPart = element.id ? `#${element.id}` : "";
    const classPart =
      element.className && typeof element.className === "string"
        ? `.${element.className.trim().split(/\s+/).slice(0, 2).join(".")}`
        : "";
    const breadcrumb = `${tag}${idPart}${classPart}`;

    const tooltip = document.createElement("div");
    tooltip.id = "stygian-hover-tooltip";
    tooltip.setAttribute("data-stygian", "tooltip");
    tooltip.style.cssText = `
      position: fixed;
      background: rgba(15, 23, 42, 0.95);
      color: white;
      font-family: 'SF Mono', Monaco, Consolas, monospace;
      font-size: 11px;
      padding: 6px 10px;
      border-radius: 6px;
      z-index: 999999;
      pointer-events: none;
      box-shadow: 0 4px 12px rgba(0,0,0,0.3);
      max-width: 320px;
      line-height: 1.5;
    `;
    tooltip.innerHTML = `
      <div style="color:#93c5fd;font-weight:600;margin-bottom:2px">${breadcrumb}</div>
      <div style="color:#cbd5e1;font-size:10px;margin-bottom:3px">${path.css.length > 50 ? path.css.slice(0, 50) + "…" : path.css}</div>
      <div style="color:${confidence.color};font-size:10px">● ${confidence.label} · Click to add region</div>
    `;

    // Position: prefer below element, fall back to above
    const top =
      rect.bottom + 6 < window.innerHeight - 60
        ? rect.bottom + 6
        : rect.top - 56;
    tooltip.style.top = `${Math.max(4, top)}px`;
    tooltip.style.left = `${Math.min(rect.left, window.innerWidth - 330)}px`;

    document.body.appendChild(tooltip);
  }

  function removeHoverTooltip() {
    document.getElementById("stygian-hover-tooltip")?.remove();
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Floating Name Input Card
  // ─────────────────────────────────────────────────────────────────────────────

  function showNameInputCard(element: Element) {
    removeNameInputCard();

    const path = getElementPath(element);
    const confidence = getSelectorConfidence(element, path.css);
    const rect = element.getBoundingClientRect();
    const textPreview = element.textContent?.trim().slice(0, 60) ?? "";

    const card = document.createElement("div");
    card.id = "stygian-name-card";
    card.setAttribute("data-stygian", "name-card");
    card.style.cssText = `
      position: fixed;
      background: white;
      border: 1.5px solid #667eea;
      border-radius: 10px;
      padding: 14px 16px;
      z-index: 999999;
      box-shadow: 0 8px 24px rgba(0,0,0,0.18);
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      font-size: 13px;
      width: 280px;
    `;

    const titleEl = document.createElement("div");
    titleEl.style.cssText =
      "font-weight:600;color:#1e293b;margin-bottom:10px";
    titleEl.textContent = "Add Region";
    card.appendChild(titleEl);

    const nameField = document.createElement("div");
    nameField.style.cssText = "margin-bottom:8px";

    const nameInput = document.createElement("input");
    nameInput.id = "stygian-region-name-input";
    nameInput.type = "text";
    nameInput.placeholder = "e.g. product_title";
    nameInput.style.cssText =
      "width:100%;padding:7px 10px;border:1px solid #d0d7e0;border-radius:6px;font-size:13px;outline:none;box-sizing:border-box";
    nameInput.autocomplete = "off";
    nameInput.spellcheck = false;

    const nameLabel = document.createElement("label");
    nameLabel.htmlFor = nameInput.id;
    nameLabel.style.cssText =
      "display:block;font-size:11px;color:#64748b;margin-bottom:4px;font-weight:500";
    nameLabel.textContent = "REGION NAME";
    nameField.appendChild(nameLabel);
    nameField.appendChild(nameInput);
    card.appendChild(nameField);

    const pathField = document.createElement("div");
    pathField.style.cssText =
      "margin-bottom:10px;padding:6px 8px;background:#f8fafc;border-radius:4px;font-size:10px;color:#64748b;font-family:monospace";
    pathField.textContent =
      path.css.length > 48 ? path.css.slice(0, 48) + "…" : path.css;
    card.appendChild(pathField);

    const confidenceRow = document.createElement("div");
    confidenceRow.style.cssText =
      "display:flex;align-items:center;gap:6px;margin-bottom:10px";

    const dot = document.createElement("span");
    dot.style.cssText =
      "display:inline-block;width:8px;height:8px;border-radius:50%";
    dot.style.background = confidence.color;
    confidenceRow.appendChild(dot);

    const confLabel = document.createElement("span");
    confLabel.style.cssText = "font-size:11px;color:#64748b";
    confLabel.textContent = confidence.label;
    confidenceRow.appendChild(confLabel);

    if (textPreview) {
      const previewSpan = document.createElement("span");
      previewSpan.style.cssText = "font-size:10px;color:#94a3b8";
      previewSpan.textContent = `· "${
        textPreview.length > 20
          ? textPreview.slice(0, 20) + "…"
          : textPreview
      }"`;
      confidenceRow.appendChild(previewSpan);
    }
    card.appendChild(confidenceRow);

    const buttonRow = document.createElement("div");
    buttonRow.style.cssText = "display:flex;gap:8px";

    const cancelBtnEl = document.createElement("button");
    cancelBtnEl.id = "stygian-cancel-btn";
    cancelBtnEl.setAttribute("data-stygian", "name-card");
    cancelBtnEl.style.cssText =
      "flex:1;padding:7px;border:1px solid #e2e8f0;background:white;border-radius:6px;cursor:pointer;font-size:12px;color:#64748b";
    cancelBtnEl.textContent = "Cancel (Esc)";
    buttonRow.appendChild(cancelBtnEl);

    const addBtnEl = document.createElement("button");
    addBtnEl.id = "stygian-add-btn";
    addBtnEl.setAttribute("data-stygian", "name-card");
    addBtnEl.style.cssText =
      "flex:2;padding:7px;border:none;background:#667eea;color:white;border-radius:6px;cursor:pointer;font-size:12px;font-weight:600";
    addBtnEl.textContent = "Add Region (Enter)";
    buttonRow.appendChild(addBtnEl);

    card.appendChild(buttonRow);

    // Position: prefer below the element
    const top =
      rect.bottom + 8 < window.innerHeight - 200
        ? rect.bottom + 8
        : rect.top - 210;
    card.style.top = `${Math.max(8, top)}px`;
    card.style.left = `${Math.min(Math.max(8, rect.left), window.innerWidth - 296)}px`;

    document.body.appendChild(card);

    const input = document.getElementById(
      "stygian-region-name-input",
    ) as HTMLInputElement;
    const addBtn = document.getElementById("stygian-add-btn");
    const cancelBtn = document.getElementById("stygian-cancel-btn");

    input?.focus();

    const confirmRegion = () => {
      const name = input?.value.trim();
      if (!name) {
        input.style.border = "1.5px solid #fc8181";
        input.focus();
        return;
      }
      if (!pendingElement) return;

      const confirmedPath = getElementPath(pendingElement);
      const region: CsRegion = {
        name,
        selector: {
          type: "dual",
          css: confirmedPath.css,
          xpath: confirmedPath.xpath,
        },
        schema: { type: "string" },
        transformations: [],
        _meta: {
          confidence: confidence.level,
          match_count: confidence.matchCount,
        },
      };

      // Flash confirmed
      unhighlightElement(pendingElement);
      highlightElement(pendingElement, "confirmed");
      setTimeout(() => {
        if (pendingElement) unhighlightElement(pendingElement);
        pendingElement = null;
      }, 800);

      recordingState.regions.push(region);
      regionHistory.push(region);
      updateOverlayCount();
      removeNameInputCard();

      console.log("[Content] Region added:", name);
      chrome.runtime.sendMessage({ type: "region_added", region });
    };

    const cancelPick = () => {
      if (pendingElement) {
        unhighlightElement(pendingElement);
        pendingElement = null;
      }
      removeNameInputCard();
    };

    addBtn?.addEventListener("click", confirmRegion);
    cancelBtn?.addEventListener("click", cancelPick);

    // Enter key on input
    input?.addEventListener("keydown", (e: KeyboardEvent) => {
      if (e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        confirmRegion();
      } else if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        cancelPick();
      }
    });
  }

  function removeNameInputCard() {
    document.getElementById("stygian-name-card")?.remove();
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Selector Generation
  // ─────────────────────────────────────────────────────────────────────────────

  function getElementPath(element: Element): { css: string; xpath: string } {
    const helper = (globalThis as any).StygianSelectorUtils;
    if (helper && typeof helper.getElementPath === "function") {
      return helper.getElementPath(element);
    }

    return {
      css: getCSSPath(element),
      xpath: getXPathPath(element),
    };
  }

  function getCSSPath(element: Element): string {
    const path: string[] = [];
    let el: Element | null = element;

    while (el && el !== document.documentElement) {
      let selector = el.tagName.toLowerCase();

      if (el.id) {
        selector += `#${el.id}`;
        path.unshift(selector);
        break;
      }

      if (el.className) {
        const classes = el.className
          .split(/\s+/)
          .filter((c) => c)
          .join(".");
        if (classes) selector += `.${classes}`;
      }

      path.unshift(selector);
      el = el.parentElement;
    }

    return path.join(" > ");
  }

  function getXPathPath(element: Element): string {
    const path: string[] = [];
    let el: Element | null = element;

    while (el && el !== document.documentElement) {
      let index = 1;
      let sibling: Element | null = el.previousElementSibling;

      while (sibling) {
        if (sibling.tagName === el.tagName) {
          index++;
        }
        sibling = sibling.previousElementSibling;
      }

      const tagName = el.tagName.toLowerCase();
      const part = `${tagName}[${index}]`;
      path.unshift(part);

      el = el.parentElement;
    }

    return "/" + path.join("/");
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Recording Overlay UI
  // ─────────────────────────────────────────────────────────────────────────────

  function showRecordingOverlay() {
    removeRecordingOverlay();

    const bar = document.createElement("div");
    bar.id = "stygian-recording-overlay";
    bar.setAttribute("data-stygian", "overlay");
    bar.style.cssText = `
      position: fixed;
      top: 12px;
      left: 50%;
      transform: translateX(-50%);
      background: rgba(15, 23, 42, 0.95);
      color: white;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      font-size: 12px;
      padding: 8px 16px;
      border-radius: 999px;
      z-index: 999999;
      pointer-events: auto;
      box-shadow: 0 4px 16px rgba(0,0,0,0.3);
      display: flex;
      align-items: center;
      gap: 12px;
      white-space: nowrap;
    `;
    bar.innerHTML = `
      <span style="color:#f87171;font-size:10px">●</span>
      <span style="font-weight:600">Recording: ${escapeHtmlContent(recordingState.template_name)}</span>
      <span id="stygian-region-count" style="background:rgba(255,255,255,0.15);padding:2px 8px;border-radius:999px">0 regions</span>
      <span style="color:#94a3b8;font-size:11px">U=undo · Esc=stop</span>
    `;

    document.body.appendChild(bar);
  }

  function updateOverlayCount() {
    const el = document.getElementById("stygian-region-count");
    if (el) {
      const n = recordingState.regions.length;
      el.textContent = `${n} region${n !== 1 ? "s" : ""}`;
    }
  }

  function removeRecordingOverlay() {
    document.getElementById("stygian-recording-overlay")?.remove();
  }

  function escapeHtmlContent(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Initialization
  // ─────────────────────────────────────────────────────────────────────────────

  console.log("[Content] Stygian Plugin content script loaded");

  // Notify service worker that we're ready
  chrome.runtime.sendMessage({
    type: "content_script_ready",
  });
})();
