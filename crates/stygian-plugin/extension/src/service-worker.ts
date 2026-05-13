/**
 * Service Worker for Stygian Plugin
 * Manages template storage, communication with backend MCP server, and template execution
 */

import type {
    ExtractionTemplate
} from "./types";

// ─────────────────────────────────────────────────────────────────────────────
// Storage Keys
// ─────────────────────────────────────────────────────────────────────────────

const STORAGE_KEY_TEMPLATES = "stygian_plugin_templates";
const BACKEND_ENDPOINT = "http://localhost:3000"; // Change to your backend URL

// ─────────────────────────────────────────────────────────────────────────────
// Template Management
// ─────────────────────────────────────────────────────────────────────────────

async function saveTemplate(template: ExtractionTemplate): Promise<void> {
  const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
  const templates = (storage[STORAGE_KEY_TEMPLATES] ||
    []) as ExtractionTemplate[];

  const index = templates.findIndex((t) => t.id === template.id);
  if (index >= 0) {
    templates[index] = template;
  } else {
    templates.push(template);
  }

  await chrome.storage.local.set({
    [STORAGE_KEY_TEMPLATES]: templates,
  });

  console.log("[ServiceWorker] Template saved:", template.id);
}

async function getTemplate(
  templateId: string,
): Promise<ExtractionTemplate | null> {
  const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
  const templates = (storage[STORAGE_KEY_TEMPLATES] ||
    []) as ExtractionTemplate[];

  return templates.find((t) => t.id === templateId) || null;
}

async function listTemplates(): Promise<ExtractionTemplate[]> {
  const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
  return (storage[STORAGE_KEY_TEMPLATES] || []) as ExtractionTemplate[];
}

async function deleteTemplate(templateId: string): Promise<void> {
  const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
  const templates = (storage[STORAGE_KEY_TEMPLATES] ||
    []) as ExtractionTemplate[];

  const filtered = templates.filter((t) => t.id !== templateId);
  await chrome.storage.local.set({
    [STORAGE_KEY_TEMPLATES]: filtered,
  });

  console.log("[ServiceWorker] Template deleted:", templateId);
}

// ─────────────────────────────────────────────────────────────────────────────
// Backend Communication
// ─────────────────────────────────────────────────────────────────────────────

async function callBackendTool(
  toolName: string,
  args: Record<string, any>,
): Promise<any> {
  console.log("[ServiceWorker] Calling backend tool:", toolName, args);

  try {
    const response = await fetch(`${BACKEND_ENDPOINT}/mcp/tools/call`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: Date.now(),
        method: "tools/call",
        params: {
          name: toolName,
          arguments: args,
        },
      }),
    });

    if (!response.ok) {
      throw new Error(`Backend error: ${response.statusText}`);
    }

    const result = await response.json();
    console.log("[ServiceWorker] Backend response:", result);

    return result;
  } catch (error) {
    console.error("[ServiceWorker] Backend communication error:", error);
    throw error;
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Message Listener
// ─────────────────────────────────────────────────────────────────────────────

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  console.log("[ServiceWorker] Received message:", message.type);

  // Handle async responses
  (async () => {
    try {
      switch (message.type) {
        case "create_template":
          {
            const template: ExtractionTemplate = {
              id: generateUUID(),
              name: message.name,
              description: message.description,
              regions: [],
              metadata: {
                created_at: new Date().toISOString(),
                updated_at: new Date().toISOString(),
                usage_count: 0,
                version: 1,
                tags: message.tags || [],
              },
            };

            await saveTemplate(template);
            sendResponse({
              success: true,
              template_id: template.id,
            });
          }
          break;

        case "save_template":
          {
            await saveTemplate(message.template);
            sendResponse({ success: true });
          }
          break;

        case "get_template":
          {
            const template = await getTemplate(message.template_id);
            sendResponse({
              success: !!template,
              template,
            });
          }
          break;

        case "list_templates":
          {
            const templates = await listTemplates();
            sendResponse({
              success: true,
              templates,
            });
          }
          break;

        case "delete_template":
          {
            await deleteTemplate(message.template_id);
            sendResponse({ success: true });
          }
          break;

        case "apply_template":
          {
            const template = await getTemplate(message.template_id);
            if (!template) {
              sendResponse({
                success: false,
                error: "Template not found",
              });
              break;
            }

            // Get page HTML from content script
            const tabs = await chrome.tabs.query({
              active: true,
              currentWindow: true,
            });
            if (!tabs[0]) {
              sendResponse({
                success: false,
                error: "No active tab",
              });
              break;
            }

            const htmlResponse = await chrome.tabs.sendMessage(tabs[0].id!, {
              type: "get_page_html",
            });

            if (!htmlResponse.success) {
              sendResponse({
                success: false,
                error: "Failed to get page HTML",
              });
              break;
            }

            // Call backend to execute extraction
            const backendResponse = await callBackendTool(
              "plugin_apply_template",
              {
                template_id: message.template_id,
                html: htmlResponse.html,
                url: tabs[0].url || "",
              },
            );

            sendResponse({
              success: true,
              result: backendResponse,
            });
          }
          break;

        case "export_template":
          {
            const template = await getTemplate(message.template_id);
            if (!template) {
              sendResponse({
                success: false,
                error: "Template not found",
              });
              break;
            }

            const json = JSON.stringify(template, null, 2);
            sendResponse({
              success: true,
              json,
            });
          }
          break;

        case "import_template":
          {
            try {
              const template = JSON.parse(message.json) as ExtractionTemplate;
              // Regenerate ID to avoid conflicts
              template.id = generateUUID();
              await saveTemplate(template);
              sendResponse({
                success: true,
                template_id: template.id,
              });
            } catch (e) {
              sendResponse({
                success: false,
                error: "Invalid JSON",
              });
            }
          }
          break;

        default:
          sendResponse({
            success: false,
            error: "Unknown message type",
          });
      }
    } catch (error) {
      console.error("[ServiceWorker] Error handling message:", error);
      sendResponse({
        success: false,
        error: String(error),
      });
    }
  })();

  // Return true to indicate we'll send response asynchronously
  return true;
});

// ─────────────────────────────────────────────────────────────────────────────
// Utilities
// ─────────────────────────────────────────────────────────────────────────────

function generateUUID(): string {
  return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function (c) {
    const r = (Math.random() * 16) | 0,
      v = c === "x" ? r : (r & 0x3) | 0x8;
    return v.toString(16);
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Initialization
// ─────────────────────────────────────────────────────────────────────────────

console.log("[ServiceWorker] Stygian Plugin service worker loaded");

// Periodic sync for syncing templates to backend (future feature)
chrome.alarms.create("sync_templates", { periodInMinutes: 60 });

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === "sync_templates") {
    console.log("[ServiceWorker] Syncing templates to backend...");
    // Future: implement template sync to backend
  }
});
