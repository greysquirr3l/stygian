"use strict";
/// <reference types="chrome" />
(() => {
    // ─────────────────────────────────────────────────────────────────────────────
    // Storage Keys
    // ─────────────────────────────────────────────────────────────────────────────
    const STORAGE_KEY_TEMPLATES = "stygian_plugin_templates";
    const STORAGE_KEY_BACKEND_URL = "stygian_plugin_backend_url";
    const DEFAULT_BACKEND_URL = "http://localhost:3000";
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
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        const templates = (storage[STORAGE_KEY_TEMPLATES] ||
            []);
        const index = templates.findIndex((t) => t.id === template.id);
        if (index >= 0) {
            templates[index] = template;
        }
        else {
            templates.push(template);
        }
        await chrome.storage.local.set({
            [STORAGE_KEY_TEMPLATES]: templates,
        });
        console.log("[ServiceWorker] Template saved:", template.id);
    }
    async function getTemplate(templateId) {
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        const templates = (storage[STORAGE_KEY_TEMPLATES] ||
            []);
        return templates.find((t) => t.id === templateId) || null;
    }
    async function listTemplates() {
        const storage = await chrome.storage.local.get(STORAGE_KEY_TEMPLATES);
        return (storage[STORAGE_KEY_TEMPLATES] || []);
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
                            // Call backend to execute extraction
                            const backendResponse = await callBackendTool("plugin_apply_template", {
                                template_id: message.template_id,
                                html: htmlResponse.html,
                                url: tabs[0].url || "",
                            });
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
                                const template = JSON.parse(message.json);
                                // Regenerate ID to avoid conflicts
                                template.id = generateUUID();
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
    // Periodic sync for syncing templates to backend (future feature)
    chrome.alarms.create("sync_templates", { periodInMinutes: 60 });
    chrome.alarms.onAlarm.addListener((alarm) => {
        if (alarm.name === "sync_templates") {
            console.log("[ServiceWorker] Syncing templates to backend...");
            // Future: implement template sync to backend
        }
    });
})();
//# sourceMappingURL=service-worker.js.map