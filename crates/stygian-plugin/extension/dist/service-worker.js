"use strict";
/// <reference types="chrome" />
(() => {
    // ─────────────────────────────────────────────────────────────────────────────
    // Storage Keys
    // ─────────────────────────────────────────────────────────────────────────────
    const STORAGE_KEY_TEMPLATES = "stygian_plugin_templates";
    const STORAGE_KEY_BACKEND_URL = "stygian_plugin_backend_url";
    const DEFAULT_BACKEND_URL = "http://localhost:3000";
    function normalizeSelector(selector) {
        if (!selector || typeof selector !== "object") {
            return {};
        }
        if (typeof selector.css === "string" ||
            typeof selector.xpath === "string") {
            return {
                ...selector,
                type: selector.type ??
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
    function normalizeTransformation(transformation) {
        if (!transformation || typeof transformation !== "object") {
            return transformation;
        }
        const params = transformation.params && typeof transformation.params === "object"
            ? transformation.params
            : {};
        return {
            ...params,
            ...transformation,
            type: transformation.type,
        };
    }
    function normalizeTemplateShape(template) {
        if (!template || typeof template !== "object") {
            return template;
        }
        const regions = Array.isArray(template.regions)
            ? template.regions.map((region) => ({
                ...region,
                selector: normalizeSelector(region?.selector),
                transformations: Array.isArray(region?.transformations)
                    ? region.transformations.map((step) => normalizeTransformation(step))
                    : [],
            }))
            : [];
        return {
            ...template,
            regions,
            metadata: template.metadata && typeof template.metadata === "object"
                ? template.metadata
                : {},
        };
    }
    /** Resolves the configured backend URL from sync storage, falling back to the default. */
    async function getBackendUrl() {
        const storage = await chrome.storage.sync.get(STORAGE_KEY_BACKEND_URL);
        const url = storage[STORAGE_KEY_BACKEND_URL];
        if (url && url.trim().length > 0) {
            return url.trim().replace(/\/$/, "");
        }
        return DEFAULT_BACKEND_URL;
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Template Management
    // ─────────────────────────────────────────────────────────────────────────────
    async function saveTemplate(template) {
        const normalizedTemplate = normalizeTemplateShape(template);
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        const templates = (storage[STORAGE_KEY_TEMPLATES] ||
            []);
        const index = templates.findIndex((t) => t.id === normalizedTemplate.id);
        if (index >= 0) {
            templates[index] = normalizedTemplate;
        }
        else {
            templates.push(normalizedTemplate);
        }
        await chrome.storage.local.set({
            [STORAGE_KEY_TEMPLATES]: templates,
        });
        console.log("[ServiceWorker] Template saved:", normalizedTemplate.id);
    }
    async function getTemplate(templateId) {
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        const templates = (storage[STORAGE_KEY_TEMPLATES] ||
            []);
        const template = templates.find((t) => t.id === templateId) || null;
        return template ? normalizeTemplateShape(template) : null;
    }
    async function listTemplates() {
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        return (storage[STORAGE_KEY_TEMPLATES] || []).map((template) => normalizeTemplateShape(template));
    }
    async function deleteTemplate(templateId) {
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        const templates = (storage[STORAGE_KEY_TEMPLATES] ||
            []);
        const filtered = templates.filter((t) => t.id !== templateId);
        await chrome.storage.local.set({
            [STORAGE_KEY_TEMPLATES]: filtered,
        });
        console.log("[ServiceWorker] Template deleted:", templateId);
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Backend Communication
    // ─────────────────────────────────────────────────────────────────────────────
    async function callBackendTool(toolName, args) {
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
        }
        catch (error) {
            console.error("[ServiceWorker] Backend communication error:", error);
            throw error;
        }
    }
    async function ensureContentScriptReady(tabId) {
        try {
            await chrome.tabs.sendMessage(tabId, { type: "ping" });
            return;
        }
        catch {
            // Content script may not be injected yet (e.g., tab opened before reload).
        }
        await chrome.scripting.executeScript({
            target: { tabId },
            files: ["dist/content-script.js"],
        });
        // Retry once after explicit injection.
        await chrome.tabs.sendMessage(tabId, { type: "ping" });
    }
    function getToolTextPayload(response) {
        return response?.result?.content?.[0]?.text ?? null;
    }
    function isToolError(response) {
        return Boolean(response?.result?.isError);
    }
    function isTemplateNotFoundToolError(response) {
        if (!isToolError(response))
            return false;
        const text = getToolTextPayload(response);
        return (typeof text === "string" &&
            text.toLowerCase().includes("template not found"));
    }
    function parseToolJsonPayload(response) {
        const text = getToolTextPayload(response);
        if (typeof text !== "string" || text.length === 0) {
            throw new Error("Missing backend payload");
        }
        return JSON.parse(text);
    }
    function mapTransformationsToBackend(transformations) {
        if (!Array.isArray(transformations))
            return [];
        const mapped = [];
        for (const item of transformations) {
            if (typeof item === "string") {
                mapped.push(item);
                continue;
            }
            const type = item?.type;
            if (typeof type !== "string")
                continue;
            if ([
                "Trim",
                "Lowercase",
                "Uppercase",
                "RemoveWhitespace",
                "NormalizeWhitespace",
                "StripHtml",
                "DecodeHtml",
                "ParseJson",
            ].includes(type)) {
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
    async function syncTemplateToBackend(template) {
        const createResponse = await callBackendTool("plugin_create_template", {
            name: template.name,
            description: template.description,
            tags: template.metadata?.tags ?? [],
        });
        if (isToolError(createResponse)) {
            throw new Error(getToolTextPayload(createResponse) ??
                "Failed to create template in backend");
        }
        const created = parseToolJsonPayload(createResponse);
        const backendTemplateId = created.template_id;
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
                throw new Error(getToolTextPayload(addRegionResponse) ??
                    `Failed to sync region '${region.name}'`);
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
    async function applyTemplateLocally(tabId, template) {
        const response = await chrome.tabs.sendMessage(tabId, {
            type: "extract_with_template",
            template,
        });
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
                            const template = {
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
                                await ensureContentScriptReady(tabs[0].id);
                            }
                            catch {
                                sendResponse({
                                    success: false,
                                    error: "Cannot connect to this page. Switch to a normal website tab and reload it, then try again.",
                                });
                                break;
                            }
                            const htmlResponse = await chrome.tabs.sendMessage(tabs[0].id, {
                                type: "get_page_html",
                            });
                            if (!htmlResponse.success) {
                                sendResponse({
                                    success: false,
                                    error: "Failed to get page HTML",
                                });
                                break;
                            }
                            const backendTemplateId = template.metadata?.backend_template_id ?? message.template_id;
                            const applyArgs = {
                                template_id: backendTemplateId,
                                html: htmlResponse.html,
                                url: tabs[0].url || "",
                            };
                            try {
                                // Call backend to execute extraction
                                let backendResponse = await callBackendTool("plugin_apply_template", applyArgs);
                                // If backend does not know this template, sync local template then retry once.
                                if (isTemplateNotFoundToolError(backendResponse)) {
                                    const syncedBackendTemplateId = await syncTemplateToBackend(template);
                                    backendResponse = await callBackendTool("plugin_apply_template", {
                                        ...applyArgs,
                                        template_id: syncedBackendTemplateId,
                                    });
                                }
                                // If backend still returns tool error (e.g., read-only storage), fallback locally.
                                if (isToolError(backendResponse)) {
                                    const localResult = await applyTemplateLocally(tabs[0].id, template);
                                    sendResponse({ success: true, result: localResult });
                                    break;
                                }
                                sendResponse({
                                    success: true,
                                    result: backendResponse,
                                });
                            }
                            catch (_backendError) {
                                // Backend unavailable or write-restricted (e.g., read-only FS): fallback locally.
                                const localResult = await applyTemplateLocally(tabs[0].id, template);
                                sendResponse({ success: true, result: localResult });
                            }
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
                                const template = JSON.parse(message.json);
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
                                    ...(template.metadata ?? {}),
                                };
                                template.regions = template.regions ?? [];
                                await saveTemplate(template);
                                sendResponse({
                                    success: true,
                                    template_id: template.id,
                                });
                            }
                            catch (e) {
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
                                const backendResponse = await callBackendTool("plugin_list_templates", {});
                                // The MCP tool returns a JSON string in result.result.content[0].text
                                const content = backendResponse?.result?.content?.[0]?.text ?? null;
                                if (!content) {
                                    sendResponse({
                                        success: false,
                                        error: "No content in server response",
                                    });
                                    break;
                                }
                                const parsed = JSON.parse(content);
                                const serverTemplates = parsed.templates ?? [];
                                // Upsert by original ID (server-authoritative), preserving ID
                                let imported = 0;
                                for (const template of serverTemplates) {
                                    // Keep the server's ID so future applies route correctly
                                    await saveTemplate(template);
                                    imported++;
                                }
                                sendResponse({ success: true, imported });
                            }
                            catch (e) {
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
                                const data = (await resp.json());
                                sendResponse({
                                    success: true,
                                    status: data["status"] ?? "ok",
                                    url: backendUrl,
                                });
                            }
                            else {
                                sendResponse({
                                    success: false,
                                    error: `HTTP ${resp.status}`,
                                    url: backendUrl,
                                });
                            }
                        }
                        catch (e) {
                            const backendUrl = await getBackendUrl().catch(() => DEFAULT_BACKEND_URL);
                            sendResponse({
                                success: false,
                                error: String(e),
                                url: backendUrl,
                            });
                        }
                        break;
                    }
                    case "set_backend_url": {
                        const newUrl = (message.url || "").trim();
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
            }
            catch (error) {
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
    function generateUUID() {
        return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, function (c) {
            const r = (Math.random() * 16) | 0, v = c === "x" ? r : (r & 0x3) | 0x8;
            return v.toString(16);
        });
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
//# sourceMappingURL=service-worker.js.map