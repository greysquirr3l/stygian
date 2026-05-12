/**
 * Content script for Stygian Plugin
 * Handles DOM interaction, element selection, and template recording
 */

import type {
  ExtractionTemplate,
  Region,
  Selector,
  RecordingState,
  ElementWithPath,
} from './types';

// ─────────────────────────────────────────────────────────────────────────────
// State Management
// ─────────────────────────────────────────────────────────────────────────────

let recordingState: RecordingState = {
  active: false,
  template_name: '',
  regions: [],
};

let highlightedElement: Element | null = null;
let selectedElements: Set<Element> = new Set();

// ─────────────────────────────────────────────────────────────────────────────
// Message Listener
// ─────────────────────────────────────────────────────────────────────────────

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  console.log('[Content] Received message:', message.type);

  switch (message.type) {
    case 'start_recording':
      startRecording(message.template_name);
      sendResponse({ success: true });
      break;

    case 'stop_recording':
      stopRecording();
      sendResponse({ success: true, regions: recordingState.regions });
      break;

    case 'get_selected_selector':
      if (highlightedElement) {
        const path = getElementPath(highlightedElement);
        sendResponse({
          success: true,
          css_path: path.css,
          xpath_path: path.xpath,
          element_text: highlightedElement.textContent?.slice(0, 100),
        });
      } else {
        sendResponse({ success: false, error: 'No element selected' });
      }
      break;

    case 'highlight_selector':
      highlightElementBySelector(message.selector);
      sendResponse({ success: true });
      break;

    case 'clear_highlights':
      clearAllHighlights();
      sendResponse({ success: true });
      break;

    case 'get_page_html':
      sendResponse({
        success: true,
        html: document.documentElement.outerHTML,
      });
      break;

    default:
      sendResponse({ success: false, error: 'Unknown message type' });
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

  console.log('[Content] Recording started:', templateName);

  // Add recording UI
  showRecordingOverlay();

  // Enable element selection on hover
  document.addEventListener('mouseover', onElementHover, true);
  document.addEventListener('mousedown', onElementClick, true);
}

function stopRecording() {
  recordingState.active = false;

  console.log('[Content] Recording stopped');

  // Remove recording UI
  removeRecordingOverlay();

  // Disable element selection
  document.removeEventListener('mouseover', onElementHover, true);
  document.removeEventListener('mousedown', onElementClick, true);

  clearAllHighlights();
}

// ─────────────────────────────────────────────────────────────────────────────
// DOM Interaction
// ─────────────────────────────────────────────────────────────────────────────

function onElementHover(event: Event) {
  if (!recordingState.active) return;

  const target = event.target as Element;
  if (target === highlightedElement) return;

  // Clear previous highlight
  if (highlightedElement) {
    unhighlightElement(highlightedElement);
  }

  // Highlight new element
  highlightedElement = target;
  highlightElement(target);
}

function onElementClick(event: MouseEvent) {
  if (!recordingState.active) return;

  event.preventDefault();
  event.stopPropagation();

  if (!highlightedElement) return;

  // In recording mode, add this element as a region
  const name = prompt('Enter region name (e.g., "product_title"):');
  if (!name) return;

  const path = getElementPath(highlightedElement);
  const region: Region = {
    name,
    selector: {
      type: 'dual',
      css: path.css,
      xpath: path.xpath,
    },
    schema: { type: 'string' },
    transformations: [],
  };

  recordingState.regions.push(region);
  console.log('[Content] Region added:', name);

  // Send update to popup
  chrome.runtime.sendMessage({
    type: 'region_added',
    region,
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Element Highlighting
// ─────────────────────────────────────────────────────────────────────────────

function highlightElement(element: Element) {
  const rect = element.getBoundingClientRect();

  const highlight = document.createElement('div');
  highlight.className = 'stygian-highlight';
  highlight.setAttribute('data-stygian', 'highlight');
  highlight.style.position = 'fixed';
  highlight.style.top = rect.top + 'px';
  highlight.style.left = rect.left + 'px';
  highlight.style.width = rect.width + 'px';
  highlight.style.height = rect.height + 'px';
  highlight.style.backgroundColor = 'rgba(52, 152, 219, 0.3)';
  highlight.style.border = '2px solid #3498db';
  highlight.style.borderRadius = '4px';
  highlight.style.zIndex = '999998';
  highlight.style.pointerEvents = 'none';

  document.body.appendChild(highlight);

  // Store reference on element
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
      highlightElement(el);
      selectedElements.add(el);
    });
  } catch (e) {
    console.error('[Content] Invalid selector:', e);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Selector Generation
// ─────────────────────────────────────────────────────────────────────────────

function getElementPath(element: Element): { css: string; xpath: string } {
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
        .join('.');
      if (classes) selector += `.${classes}`;
    }

    path.unshift(selector);
    el = el.parentElement;
  }

  return path.join(' > ');
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

  return '/' + path.join('/');
}

// ─────────────────────────────────────────────────────────────────────────────
// Recording Overlay UI
// ─────────────────────────────────────────────────────────────────────────────

function showRecordingOverlay() {
  const overlay = document.createElement('div');
  overlay.id = 'stygian-recording-overlay';
  overlay.setAttribute('data-stygian', 'overlay');
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
  const overlayDiv = overlay.querySelector('div');
  if (overlayDiv) {
    overlayDiv.style.pointerEvents = 'auto';
  }
}

function removeRecordingOverlay() {
  const overlay = document.getElementById('stygian-recording-overlay');
  if (overlay) {
    overlay.remove();
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

console.log('[Content] Stygian Plugin content script loaded');

// Notify service worker that we're ready
chrome.runtime.sendMessage({
  type: 'content_script_ready',
});
