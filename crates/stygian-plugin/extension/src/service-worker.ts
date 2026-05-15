/// <reference types="chrome" />

/**
 * Service Worker for Stygian Plugin
 * Manages template storage, communication with backend MCP server, and template execution
 */

// Keep service worker as a classic script (not ES module) for MV3 compatibility.
type SwExtractionTemplate = any;

(() => {
  // ─────────────────────────────────────────────────────────────────────────────
  // Storage Keys
  // ─────────────────────────────────────────────────────────────────────────────

  const STORAGE_KEY_TEMPLATES = "stygian_plugin_templates";
  const STORAGE_KEY_BACKEND_URL = "stygian_plugin_backend_url";
  const DEFAULT_BACKEND_URL = "http://localhost:3000";

  function normalizeSelector(selector: any): Record<string, any> {
    if (!selector || typeof selector !== "object") {
      return {};
    }

    if (
      typeof selector.css === "string" ||
      typeof selector.xpath === "string"
    ) {
      return {
        ...selector,
        type:
          selector.type ??
          (selector.css && selector.xpath
            ? "dual"
            : selector.css
              ? "css"
              : "xpath"),
      };
    }

    if (typeof selector.Css === "string") {
      return {
        type: "css",
        css: selector.Css,
      };
    }

    if (typeof selector.XPath === "string") {
      return {
        type: "xpath",
        xpath: selector.XPath,
      };
    }

    const dual = selector.Both;
    if (dual && typeof dual === "object") {
      return {
        type: "dual",
        css: dual.css,
        xpath: dual.xpath,
      };
    }

    return { ...selector };
  }

  function normalizeTransformation(transformation: any): any {
    if (!transformation || typeof transformation !== "object") {
      return transformation;
    }

    const params =
      transformation.params && typeof transformation.params === "object"
        ? transformation.params
        : {};

    return {
      ...params,
      ...transformation,
      type: transformation.type,
    };
  }

  function normalizeTemplateShape(
    template: SwExtractionTemplate,
  ): SwExtractionTemplate {
    if (!template || typeof template !== "object") {
      return template;
    }

    const regions = Array.isArray(template.regions)
      ? template.regions.map((region: any) => ({
          ...region,
          selector: normalizeSelector(region?.selector),
          transformations: Array.isArray(region?.transformations)
            ? region.transformations.map((step: any) =>
                normalizeTransformation(step),
              )
            : [],
        }))
      : [];

    return {
      ...template,
      regions,
      metadata:
        template.metadata && typeof template.metadata === "object"
          ? template.metadata
          : {},
    };
  }

  /** Resolves the configured backend URL from sync storage, falling back to the default. */
  async function getBackendUrl(): Promise<string> {
    const storage = await chrome.storage.sync.get(STORAGE_KEY_BACKEND_URL);
    const url = storage[STORAGE_KEY_BACKEND_URL] as string | undefined;
    if (url && url.trim().length > 0) {
      return url.trim().replace(/\/$/, "");
    }
    return DEFAULT_BACKEND_URL;
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Template Management
  // ─────────────────────────────────────────────────────────────────────────────

  async function saveTemplate(template: SwExtractionTemplate): Promise<void> {
    const normalizedTemplate = normalizeTemplateShape(template);
    const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
    const templates = (storage[STORAGE_KEY_TEMPLATES] ||
      []) as SwExtractionTemplate[];

    const index = templates.findIndex((t) => t.id === normalizedTemplate.id);
    if (index >= 0) {
      templates[index] = normalizedTemplate;
    } else {
      templates.push(normalizedTemplate);
    }

    await chrome.storage.local.set({
      [STORAGE_KEY_TEMPLATES]: templates,
    });

    console.log("[ServiceWorker] Template saved:", normalizedTemplate.id);
  }

  async function getTemplate(
    templateId: string,
  ): Promise<SwExtractionTemplate | null> {
    const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
    const templates = (storage[STORAGE_KEY_TEMPLATES] ||
      []) as SwExtractionTemplate[];

    const template = templates.find((t) => t.id === templateId) || null;
    return template ? normalizeTemplateShape(template) : null;
  }

  async function listTemplates(): Promise<SwExtractionTemplate[]> {
    const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
    return (
      (storage[STORAGE_KEY_TEMPLATES] || []) as SwExtractionTemplate[]
    ).map((template) => normalizeTemplateShape(template));
  }

  async function deleteTemplate(templateId: string): Promise<void> {
    const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
    const templates = (storage[STORAGE_KEY_TEMPLATES] ||
      []) as SwExtractionTemplate[];

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
      const backendUrl = await getBackendUrl();
      const response = await fetch(`${backendUrl}/mcp/tools/call`, {
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

  async function ensureContentScriptReady(tabId: number): Promise<void> {
    try {
      await chrome.tabs.sendMessage(tabId, { type: "ping" });
      return;
    } catch {
      // Content script may not be injected yet (e.g., tab opened before reload).
    }

    await chrome.scripting.executeScript({
      target: { tabId },
      files: ["dist/content-script.js"],
    });

    // Retry once after explicit injection.
    await chrome.tabs.sendMessage(tabId, { type: "ping" });
  }

  function getToolTextPayload(response: any): string | null {
    return response?.result?.content?.[0]?.text ?? null;
  }

  function isToolError(response: any): boolean {
    return Boolean(response?.result?.isError);
  }

  function isTemplateNotFoundToolError(response: any): boolean {
    if (!isToolError(response)) return false;
    const text = getToolTextPayload(response);
    return (
      typeof text === "string" &&
      text.toLowerCase().includes("template not found")
    );
  }

  function parseToolJsonPayload(response: any): Record<string, any> {
    const text = getToolTextPayload(response);
    if (typeof text !== "string" || text.length === 0) {
      throw new Error("Missing backend payload");
    }

    return JSON.parse(text) as Record<string, any>;
  }

  function mapTransformationsToBackend(
    transformations: any[] | undefined,
  ): string[] {
    if (!Array.isArray(transformations)) return [];

    const mapped: string[] = [];
    for (const item of transformations) {
      if (typeof item === "string") {
        mapped.push(item);
        continue;
      }

      const type = item?.type;
      if (typeof type !== "string") continue;

      if (
        [
          "Trim",
          "Lowercase",
          "Uppercase",
          "RemoveWhitespace",
          "NormalizeWhitespace",
          "StripHtml",
          "DecodeHtml",
          "ParseJson",
        ].includes(type)
      ) {
        mapped.push(type);
        continue;
      }

      if (type === "Regex") {
        mapped.push(`Regex:${item.pattern ?? ""}/${item.replacement ?? ""}`);
        continue;
      }

      if (type === "RegexExtract") {
        mapped.push(`RegexExtract:${item.pattern ?? ""}/${item.group ?? 0}`);
        continue;
      }

      if (type === "Filter") {
        mapped.push(`Filter:${item.pattern ?? ""}`);
      }
    }

    return mapped;
  }

  async function syncTemplateToBackend(
    template: SwExtractionTemplate,
  ): Promise<string> {
    const createResponse = await callBackendTool("plugin_create_template", {
      name: template.name,
      description: template.description,
      tags: template.metadata?.tags ?? [],
    });

    if (isToolError(createResponse)) {
      throw new Error(
        getToolTextPayload(createResponse) ??
          "Failed to create template in backend",
      );
    }

    const created = parseToolJsonPayload(createResponse);
    const backendTemplateId = created.template_id as string | undefined;
    if (!backendTemplateId) {
      throw new Error("Backend did not return a template_id");
    }

    const regions = Array.isArray(template.regions) ? template.regions : [];
    for (const region of regions) {
      const addRegionResponse = await callBackendTool("plugin_add_region", {
        template_id: backendTemplateId,
        region_name: region.name,
        selector_css: region.selector?.css,
        selector_xpath: region.selector?.xpath,
        transformations: mapTransformationsToBackend(region.transformations),
      });

      if (isToolError(addRegionResponse)) {
        throw new Error(
          getToolTextPayload(addRegionResponse) ??
            `Failed to sync region '${region.name}'`,
        );
      }
    }

    template.metadata = {
      ...(template.metadata ?? {}),
      backend_template_id: backendTemplateId,
      updated_at: new Date().toISOString(),
    };
    await saveTemplate(template);

    return backendTemplateId;
  }

  async function applyTemplateLocally(
    tabId: number,
    template: SwExtractionTemplate,
    options?: { mode?: string; rootSelector?: string },
  ): Promise<any> {
    const response = await chrome.tabs.sendMessage(
      tabId,
      options?.mode === "batch"
        ? {
            type: "extract_with_template_batch",
            template,
            root_selector: options.rootSelector,
          }
        : {
            type: "extract_with_template",
            template,
          },
    );

    if (!response?.success) {
      throw new Error(response?.error ?? "Local extraction failed");
    }

    return {
      data: response.data ?? {},
      metadata: {
        ...(response.metadata ?? {}),
        source: "local_fallback",
      },
    };
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
              const template: SwExtractionTemplate = {
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

              try {
                await ensureContentScriptReady(tabs[0].id!);
              } catch {
                sendResponse({
                  success: false,
                  error:
                    "Cannot connect to this page. Switch to a normal website tab and reload it, then try again.",
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

              const backendTemplateId =
                template.metadata?.backend_template_id ?? message.template_id;
              const mode = message.mode === "batch" ? "batch" : "single";
              const rootSelector =
                typeof message.root_selector === "string"
                  ? message.root_selector.trim()
                  : "";
              const debug = message.debug === true;

              if (mode === "batch" && rootSelector.length === 0) {
                sendResponse({
                  success: false,
                  error: "Batch mode requires a root selector",
                });
                break;
              }

              const applyArgs = {
                template_id: backendTemplateId,
                html: htmlResponse.html,
                url: tabs[0].url || "",
                ...(mode === "batch" ? { root_selector: rootSelector } : {}),
                ...(debug ? { debug: true } : {}),
              };
              const toolName =
                mode === "batch"
                  ? "plugin_extract_batch"
                  : "plugin_apply_template";

              try {
                // Call backend to execute extraction
                let backendResponse = await callBackendTool(
                  toolName,
                  applyArgs,
                );

                // If backend does not know this template, sync local template then retry once.
                if (isTemplateNotFoundToolError(backendResponse)) {
                  const syncedBackendTemplateId =
                    await syncTemplateToBackend(template);
                  backendResponse = await callBackendTool(toolName, {
                    ...applyArgs,
                    template_id: syncedBackendTemplateId,
                  });
                }

                // If backend still returns tool error (e.g., read-only storage), fallback locally.
                if (isToolError(backendResponse)) {
                  const localResult = await applyTemplateLocally(
                    tabs[0].id!,
                    template,
                    { mode, rootSelector },
                  );
                  sendResponse({ success: true, result: localResult });
                  break;
                }

                sendResponse({
                  success: true,
                  result: backendResponse,
                });
              } catch (_backendError) {
                // Backend unavailable or write-restricted (e.g., read-only FS): fallback locally.
                const localResult = await applyTemplateLocally(
                  tabs[0].id!,
                  template,
                  { mode, rootSelector },
                );
                sendResponse({ success: true, result: localResult });
              }
            }
            break;

          case "inspect_selector": {
            const selectorCss =
              typeof message.selector_css === "string"
                ? message.selector_css.trim()
                : "";
            if (selectorCss.length === 0) {
              sendResponse({
                success: false,
                error: "Selector cannot be empty",
              });
              break;
            }

            const tabs = await chrome.tabs.query({
              active: true,
              currentWindow: true,
            });
            if (!tabs[0]) {
              sendResponse({ success: false, error: "No active tab" });
              break;
            }

            try {
              await ensureContentScriptReady(tabs[0].id!);
            } catch {
              sendResponse({
                success: false,
                error:
                  "Cannot connect to this page. Switch to a normal website tab and reload it, then try again.",
              });
              break;
            }

            const htmlResponse = await chrome.tabs.sendMessage(tabs[0].id!, {
              type: "get_page_html",
            });

            try {
              const backendResponse = await callBackendTool(
                "plugin_inspect_selector",
                {
                  html: htmlResponse.html,
                  selector_css: selectorCss,
                },
              );

              if (!isToolError(backendResponse)) {
                sendResponse({ success: true, result: backendResponse });
                break;
              }
            } catch {
              // Fall through to local inspection.
            }

            const localInspection = await chrome.tabs.sendMessage(tabs[0].id!, {
              type: "inspect_selector_local",
              selector_css: selectorCss,
            });

            if (!localInspection?.success) {
              sendResponse({
                success: false,
                error: localInspection?.error ?? "Selector inspection failed",
              });
              break;
            }

            sendResponse({ success: true, result: localInspection });
            break;
          }

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
                const template = JSON.parse(
                  message.json,
                ) as SwExtractionTemplate;
                // Regenerate ID to avoid conflicts
                template.id = generateUUID();
                // Normalise metadata so the popup can always read usage_count etc.
                const now = new Date().toISOString();
                template.metadata = {
                  usage_count: 0,
                  version: 1,
                  tags: [],
                  created_at: now,
                  updated_at: now,
                  ...((template.metadata as Record<string, unknown>) ?? {}),
                };
                template.regions = template.regions ?? [];
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

          case "sync_from_server":
            {
              try {
                const backendResponse = await callBackendTool(
                  "plugin_list_templates",
                  {},
                );
                // The MCP tool returns a JSON string in result.result.content[0].text
                const content =
                  backendResponse?.result?.content?.[0]?.text ?? null;
                if (!content) {
                  sendResponse({
                    success: false,
                    error: "No content in server response",
                  });
                  break;
                }

                const parsed = JSON.parse(content) as {
                  templates?: SwExtractionTemplate[];
                };
                const serverTemplates: SwExtractionTemplate[] =
                  parsed.templates ?? [];

                // Upsert by original ID (server-authoritative), preserving ID
                let imported = 0;
                for (const template of serverTemplates) {
                  // Keep the server's ID so future applies route correctly
                  await saveTemplate(template);
                  imported++;
                }

                sendResponse({ success: true, imported });
              } catch (e) {
                sendResponse({
                  success: false,
                  error: String(e),
                });
              }
            }
            break;

          case "check_connection": {
            try {
              const backendUrl = await getBackendUrl();
              const resp = await fetch(`${backendUrl}/health`, {
                signal: AbortSignal.timeout(3000),
              });
              if (resp.ok) {
                const data = (await resp.json()) as Record<string, unknown>;
                sendResponse({
                  success: true,
                  status: data["status"] ?? "ok",
                  url: backendUrl,
                });
              } else {
                sendResponse({
                  success: false,
                  error: `HTTP ${resp.status}`,
                  url: backendUrl,
                });
              }
            } catch (e) {
              const backendUrl = await getBackendUrl().catch(
                () => DEFAULT_BACKEND_URL,
              );
              sendResponse({
                success: false,
                error: String(e),
                url: backendUrl,
              });
            }
            break;
          }

          case "set_backend_url": {
            const newUrl = ((message.url as string) || "").trim();
            if (!newUrl) {
              sendResponse({ success: false, error: "URL cannot be empty" });
              break;
            }
            await chrome.storage.sync.set({
              [STORAGE_KEY_BACKEND_URL]: newUrl,
            });
            sendResponse({ success: true, url: newUrl });
            break;
          }

          case "get_backend_url": {
            const url = await getBackendUrl();
            sendResponse({ success: true, url });
            break;
          }

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
    return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(
      /[xy]/g,
      function (c) {
        const r = (Math.random() * 16) | 0,
          v = c === "x" ? r : (r & 0x3) | 0x8;
        return v.toString(16);
      },
    );
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Initialization
  // ─────────────────────────────────────────────────────────────────────────────

  console.log("[ServiceWorker] Stygian Plugin service worker loaded");

  // Periodic sync is optional; guard it so worker startup never fails when
  // `alarms` permission is not present in the extension manifest.
  if (chrome.alarms?.create && chrome.alarms?.onAlarm) {
    chrome.alarms.create("sync_templates", { periodInMinutes: 60 });

    chrome.alarms.onAlarm.addListener((alarm) => {
      if (alarm.name === "sync_templates") {
        console.log("[ServiceWorker] Syncing templates to backend...");
        // Future: implement template sync to backend
      }
    });
  }
})();
