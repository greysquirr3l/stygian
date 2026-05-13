"use strict";
/// <reference types="chrome" />
(() => {
    // ─────────────────────────────────────────────────────────────────────────────
    // State Management
    // ─────────────────────────────────────────────────────────────────────────────
    let recordingState = {
        active: false,
        template_name: "",
        regions: [],
    };
    let highlightedElement = null;
    let selectedElements = new Set();
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
                }
                else {
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
                    }
                    catch (error) {
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
    function startRecording(templateName) {
        recordingState.active = true;
        recordingState.template_name = templateName;
        recordingState.regions = [];
        console.log("[Content] Recording started:", templateName);
        // Add recording UI
        showRecordingOverlay();
        // Enable element selection on hover
        document.addEventListener("mouseover", onElementHover, true);
        document.addEventListener("mousedown", onElementClick, true);
        // Allow Escape key to stop recording
        document.addEventListener("keydown", onEscapeKey, true);
    }
    function stopRecording() {
        recordingState.active = false;
        console.log("[Content] Recording stopped");
        // Remove recording UI
        removeRecordingOverlay();
        // Disable element selection
        document.removeEventListener("mouseover", onElementHover, true);
        document.removeEventListener("mousedown", onElementClick, true);
        document.removeEventListener("keydown", onEscapeKey, true);
        clearAllHighlights();
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // DOM Interaction
    // ─────────────────────────────────────────────────────────────────────────────
    function onElementHover(event) {
        if (!recordingState.active)
            return;
        const target = event.target;
        if (target === highlightedElement)
            return;
        // Clear previous highlight
        if (highlightedElement) {
            unhighlightElement(highlightedElement);
        }
        // Highlight new element
        highlightedElement = target;
        highlightElement(target);
    }
    function onElementClick(event) {
        if (!recordingState.active)
            return;
        event.preventDefault();
        event.stopPropagation();
        if (!highlightedElement)
            return;
        // In recording mode, add this element as a region
        const name = prompt('Enter region name (e.g., "product_title"):');
        if (!name)
            return;
        const path = getElementPath(highlightedElement);
        const region = {
            name,
            selector: {
                type: "dual",
                css: path.css,
                xpath: path.xpath,
            },
            schema: { type: "string" },
            transformations: [],
        };
        recordingState.regions.push(region);
        console.log("[Content] Region added:", name);
        // Send update to popup
        chrome.runtime.sendMessage({
            type: "region_added",
            region,
        });
    }
    function onEscapeKey(event) {
        if (!recordingState.active)
            return;
        if (event.key === "Escape" || event.code === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            console.log("[Content] Escape pressed - stopping recording");
            stopRecording();
            // Notify popup that recording was stopped
            chrome.runtime.sendMessage({
                type: "recording_stopped",
            });
        }
    }
    function extractWithTemplate(template) {
        const startedAt = performance.now();
        const regions = Array.isArray(template?.regions) ? template.regions : [];
        const data = {};
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
    function extractRegionValue(region) {
        const selector = region?.selector;
        const cssSelector = selector?.css;
        const xpathSelector = selector?.xpath;
        let node = null;
        if (typeof cssSelector === "string" && cssSelector.length > 0) {
            try {
                node = document.querySelector(cssSelector);
            }
            catch {
                // Ignore invalid selector; fallback to XPath.
            }
        }
        if (!node &&
            typeof xpathSelector === "string" &&
            xpathSelector.length > 0) {
            try {
                const result = document.evaluate(xpathSelector, document, null, XPathResult.FIRST_ORDERED_NODE_TYPE, null);
                node = result.singleNodeValue;
            }
            catch {
                // Ignore invalid XPath.
            }
        }
        const rawValue = node?.textContent?.trim() ?? "";
        return applyTransformations(rawValue, region?.transformations);
    }
    function applyTransformations(value, transformations) {
        if (!Array.isArray(transformations) || transformations.length === 0) {
            return value;
        }
        let current = value;
        for (const step of transformations) {
            const type = typeof step === "string" ? step : step?.type;
            if (typeof type !== "string")
                continue;
            if (type === "Trim" && typeof current === "string") {
                current = current.trim();
            }
            else if (type === "Lowercase" && typeof current === "string") {
                current = current.toLowerCase();
            }
            else if (type === "Uppercase" && typeof current === "string") {
                current = current.toUpperCase();
            }
            else if (type === "RemoveWhitespace" && typeof current === "string") {
                current = current.replace(/\s+/g, "");
            }
            else if (type === "NormalizeWhitespace" &&
                typeof current === "string") {
                current = current.replace(/\s+/g, " ").trim();
            }
            else if (type === "StripHtml" && typeof current === "string") {
                current = current.replace(/<[^>]+>/g, "");
            }
            else if (type === "DecodeHtml" && typeof current === "string") {
                const textarea = document.createElement("textarea");
                textarea.innerHTML = current;
                current = textarea.value;
            }
            else if (type === "ParseJson" && typeof current === "string") {
                try {
                    current = JSON.parse(current);
                }
                catch {
                    // Keep original text if parsing fails.
                }
            }
            else if (type === "Regex" && typeof current === "string") {
                const pattern = typeof step?.pattern === "string" ? step.pattern : "";
                const replacement = typeof step?.replacement === "string" ? step.replacement : "";
                try {
                    current = current.replace(new RegExp(pattern, "g"), replacement);
                }
                catch {
                    // Ignore invalid regex.
                }
            }
            else if (type === "RegexExtract" && typeof current === "string") {
                const pattern = typeof step?.pattern === "string" ? step.pattern : "";
                const group = typeof step?.group === "number" && Number.isInteger(step.group)
                    ? step.group
                    : 0;
                try {
                    const match = current.match(new RegExp(pattern));
                    current = match?.[group] ?? "";
                }
                catch {
                    // Ignore invalid regex.
                }
            }
            else if (type === "Filter" && typeof current === "string") {
                const pattern = typeof step?.pattern === "string" ? step.pattern : "";
                try {
                    current = new RegExp(pattern).test(current) ? current : "";
                }
                catch {
                    // Ignore invalid regex.
                }
            }
        }
        return current;
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Element Highlighting
    // ─────────────────────────────────────────────────────────────────────────────
    function highlightElement(element) {
        const rect = element.getBoundingClientRect();
        const highlight = document.createElement("div");
        highlight.className = "stygian-highlight";
        highlight.setAttribute("data-stygian", "highlight");
        highlight.style.position = "fixed";
        highlight.style.top = rect.top + "px";
        highlight.style.left = rect.left + "px";
        highlight.style.width = rect.width + "px";
        highlight.style.height = rect.height + "px";
        highlight.style.backgroundColor = "rgba(52, 152, 219, 0.3)";
        highlight.style.border = "2px solid #3498db";
        highlight.style.borderRadius = "4px";
        highlight.style.zIndex = "999998";
        highlight.style.pointerEvents = "none";
        document.body.appendChild(highlight);
        // Store reference on element
        element.__stygian_highlight = highlight;
    }
    function unhighlightElement(element) {
        const highlight = element.__stygian_highlight;
        if (highlight) {
            highlight.remove();
            delete element.__stygian_highlight;
        }
    }
    function clearAllHighlights() {
        document.querySelectorAll('[data-stygian="highlight"]').forEach((el) => {
            el.remove();
        });
        highlightedElement = null;
        selectedElements.clear();
    }
    function highlightElementBySelector(selector) {
        clearAllHighlights();
        try {
            const elements = document.querySelectorAll(selector);
            elements.forEach((el) => {
                highlightElement(el);
                selectedElements.add(el);
            });
        }
        catch (e) {
            console.error("[Content] Invalid selector:", e);
        }
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Selector Generation
    // ─────────────────────────────────────────────────────────────────────────────
    function getElementPath(element) {
        return {
            css: getCSSPath(element),
            xpath: getXPathPath(element),
        };
    }
    function getCSSPath(element) {
        const path = [];
        let el = element;
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
                if (classes)
                    selector += `.${classes}`;
            }
            path.unshift(selector);
            el = el.parentElement;
        }
        return path.join(" > ");
    }
    function getXPathPath(element) {
        const path = [];
        let el = element;
        while (el && el !== document.documentElement) {
            let index = 1;
            let sibling = el.previousElementSibling;
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
        const overlay = document.createElement("div");
        overlay.id = "stygian-recording-overlay";
        overlay.setAttribute("data-stygian", "overlay");
        overlay.innerHTML = `
    <div style="
      position: fixed;
      top: 0;
      left: 0;
      right: 0;
      bottom: 0;
      background: rgba(0, 0, 0, 0.7);
      display: flex;
      flex-direction: column;
      justify-content: flex-start;
      align-items: center;
      z-index: 999999;
      font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
      pointer-events: none;
    ">
      <div style="
        background: white;
        padding: 12px 20px;
        border-radius: 8px;
        margin-top: 20px;
        box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
        pointer-events: auto;
        display: flex;
        gap: 8px;
        align-items: center;
      ">
        <span style="color: #333; font-size: 14px; font-weight: 500;">
          🔴 Recording: ${recordingState.template_name}
        </span>
        <span style="color: #666; font-size: 12px;">
          Hover to preview, Click to add region
        </span>
      </div>
      <div style="
        color: white;
        font-size: 12px;
        margin-top: 10px;
        background: rgba(0, 0, 0, 0.5);
        padding: 8px 12px;
        border-radius: 4px;
      ">
        Regions added: <strong>${recordingState.regions.length}</strong>
      </div>
    </div>
  `;
        document.body.appendChild(overlay);
        // Overlay is not interactive, so we allow normal interaction through it
        const overlayDiv = overlay.querySelector("div");
        if (overlayDiv) {
            overlayDiv.style.pointerEvents = "auto";
        }
    }
    function removeRecordingOverlay() {
        const overlay = document.getElementById("stygian-recording-overlay");
        if (overlay) {
            overlay.remove();
        }
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
//# sourceMappingURL=content-script.js.map