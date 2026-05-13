"use strict";
/// <reference types="chrome" />
(() => {
    // ─────────────────────────────────────────────────────────────────────────────
    // State
    // ─────────────────────────────────────────────────────────────────────────────
    let currentRecordingTemplate = null;
    let isRecording = false;
    // ─────────────────────────────────────────────────────────────────────────────
    // Tab Navigation
    // ─────────────────────────────────────────────────────────────────────────────
    document.querySelectorAll(".tab-btn").forEach((btn) => {
        btn.addEventListener("click", (e) => {
            const tabName = e.target.getAttribute("data-tab");
            if (!tabName)
                return;
            // Hide all tabs
            document.querySelectorAll(".tab-content").forEach((tab) => {
                tab.classList.remove("active");
            });
            // Deactivate all buttons
            document.querySelectorAll(".tab-btn").forEach((b) => {
                b.classList.remove("active");
            });
            // Show selected tab
            const selectedTab = document.getElementById(tabName);
            if (selectedTab) {
                selectedTab.classList.add("active");
            }
            // Activate button
            e.target.classList.add("active");
            // Load templates when switching to tabs that need them
            if (tabName === "templates") {
                loadTemplates();
            }
            else if (tabName === "apply") {
                loadTemplatesForApply();
            }
        });
    });
    // ─────────────────────────────────────────────────────────────────────────────
    // Templates Tab
    // ─────────────────────────────────────────────────────────────────────────────
    const newTemplateBtn = document.getElementById("new-template-btn");
    newTemplateBtn?.addEventListener("click", () => {
        const name = prompt("Enter template name:");
        if (!name)
            return;
        const description = prompt("Enter template description (optional):");
        chrome.runtime.sendMessage({
            type: "create_template",
            name,
            description: description || undefined,
        }, (response) => {
            if (response.success) {
                loadTemplates();
                showStatus("Template created!", "success");
            }
            else {
                showStatus("Failed to create template: " + response.error, "error");
            }
        });
    });
    // ── Import JSON ──────────────────────────────────────────────────────────────
    const importTemplateBtn = document.getElementById("import-template-btn");
    const importTemplateFile = document.getElementById("import-template-file");
    importTemplateBtn?.addEventListener("click", () => {
        importTemplateFile?.click();
    });
    importTemplateFile?.addEventListener("change", async () => {
        const file = importTemplateFile?.files?.[0];
        if (!file)
            return;
        try {
            const text = await file.text();
            const parsed = JSON.parse(text);
            // Support single template object or array of templates
            const items = Array.isArray(parsed) ? parsed : [parsed];
            let imported = 0;
            let failed = 0;
            for (const item of items) {
                const response = await sendMessage({
                    type: "import_template",
                    json: JSON.stringify(item),
                });
                if (response.success) {
                    imported++;
                }
                else {
                    failed++;
                }
            }
            loadTemplates();
            const msg = failed === 0
                ? `Imported ${imported} template${imported !== 1 ? "s" : ""}!`
                : `Imported ${imported}, failed ${failed}`;
            showStatus(msg, failed === 0 ? "success" : "error");
        }
        catch (_e) {
            showStatus("Invalid JSON file", "error");
        }
        finally {
            // Reset so the same file can be re-imported if needed
            if (importTemplateFile)
                importTemplateFile.value = "";
        }
    });
    // ── Sync from Server ─────────────────────────────────────────────────────────
    const syncTemplatesBtn = document.getElementById("sync-templates-btn");
    syncTemplatesBtn?.addEventListener("click", async () => {
        syncTemplatesBtn.setAttribute("disabled", "true");
        showStatus("Syncing templates from server…", "info");
        try {
            const response = await sendMessage({ type: "sync_from_server" });
            if (response.success) {
                loadTemplates();
                showStatus(`Synced ${response.imported} template${response.imported !== 1 ? "s" : ""} from server!`, "success");
            }
            else {
                showStatus("Sync failed: " + response.error, "error");
            }
        }
        catch (e) {
            showStatus("Sync error: " + String(e), "error");
        }
        finally {
            syncTemplatesBtn.removeAttribute("disabled");
        }
    });
    async function loadTemplates() {
        const response = await sendMessage({ type: "list_templates" });
        const listContainer = document.getElementById("templates-list");
        if (!listContainer)
            return;
        const templates = response.templates;
        if (templates.length === 0) {
            listContainer.innerHTML = '<p class="empty-state">No templates yet.</p>';
            return;
        }
        listContainer.innerHTML = templates
            .map((template) => `
        <div class="template-card">
          <div class="template-info">
            <h3>${template.name ?? "(unnamed)"}</h3>
            ${template.description ? `<p>${template.description}</p>` : ""}
            <div class="template-meta">
              <span>${(template.regions ?? []).length} regions</span>
              <span>${template.metadata?.usage_count ?? 0} uses</span>
            </div>
          </div>
          <div class="template-actions">
            <button class="btn btn-small" onclick="editTemplate('${template.id}')">Edit</button>
            <button class="btn btn-small btn-danger" onclick="deleteTemplate('${template.id}')">Delete</button>
          </div>
        </div>
      `)
            .join("");
    }
    async function editTemplate(templateId) {
        const response = await sendMessage({
            type: "get_template",
            template_id: templateId,
        });
        if (!response.success) {
            showStatus("Failed to load template", "error");
            return;
        }
        const template = response.template;
        const action = prompt(`Template: ${template.name}\n\nOptions:\n1. Edit name\n2. View regions\n3. Export JSON`, "1");
        switch (action) {
            case "1": {
                const newName = prompt("New template name:", template.name);
                if (newName && newName !== template.name) {
                    template.name = newName;
                    await sendMessage({
                        type: "save_template",
                        template,
                    });
                    loadTemplates();
                    showStatus("Template updated!", "success");
                }
                break;
            }
            case "2": {
                const regions = template.regions
                    .map((r) => `- ${r.name}: ${JSON.stringify(r.selector)}`)
                    .join("\n");
                alert(`Regions:\n\n${regions || "No regions"}`);
                break;
            }
            case "3": {
                const exportResponse = await sendMessage({
                    type: "export_template",
                    template_id: templateId,
                });
                if (exportResponse.success) {
                    const dataUrl = `data:text/json,${encodeURIComponent(exportResponse.json)}`;
                    const link = document.createElement("a");
                    link.href = dataUrl;
                    link.download = `${template.name.replace(/\s+/g, "_")}.json`;
                    link.click();
                }
                break;
            }
        }
    }
    async function deleteTemplate(templateId) {
        if (!confirm("Delete this template?"))
            return;
        const response = await sendMessage({
            type: "delete_template",
            template_id: templateId,
        });
        if (response.success) {
            loadTemplates();
            showStatus("Template deleted", "success");
        }
        else {
            showStatus("Failed to delete template", "error");
        }
    }
    // Make functions globally available for onclick handlers
    window.editTemplate = editTemplate;
    window.deleteTemplate = deleteTemplate;
    // ─────────────────────────────────────────────────────────────────────────────
    // Record Tab
    // ─────────────────────────────────────────────────────────────────────────────
    const recordNameInput = document.getElementById("record-name-input");
    const recordDescriptionInput = document.getElementById("record-description-input");
    const startRecordingBtn = document.getElementById("start-recording-btn");
    const finishRecordingBtn = document.getElementById("finish-recording-btn");
    const recordingRegionsList = document.getElementById("recording-regions-list");
    startRecordingBtn?.addEventListener("click", async () => {
        const name = recordNameInput.value.trim();
        if (!name) {
            showStatus("Please enter a template name", "error");
            return;
        }
        // Get active tab
        const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
        if (!tabs[0]) {
            showStatus("No active tab", "error");
            return;
        }
        // Create template
        const createResponse = await sendMessage({
            type: "create_template",
            name,
            description: recordDescriptionInput.value.trim(),
        });
        if (!createResponse.success) {
            showStatus("Failed to create template", "error");
            return;
        }
        currentRecordingTemplate = {
            id: createResponse.template_id,
            name,
            description: recordDescriptionInput.value.trim(),
            regions: [],
            metadata: {
                created_at: new Date().toISOString(),
                updated_at: new Date().toISOString(),
                usage_count: 0,
                version: 1,
                tags: [],
            },
        };
        // Start recording in content script
        try {
            await chrome.tabs.sendMessage(tabs[0].id, {
                type: "start_recording",
                template_name: name,
            });
            isRecording = true;
            startRecordingBtn.disabled = true;
            finishRecordingBtn.disabled = false;
            recordNameInput.disabled = true;
            recordDescriptionInput.disabled = true;
            showStatus("Recording started! Click elements in the page to add regions.", "info");
        }
        catch (error) {
            showStatus("Failed to start recording", "error");
        }
    });
    finishRecordingBtn?.addEventListener("click", async () => {
        const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
        if (!tabs[0] || !currentRecordingTemplate)
            return;
        try {
            const stopResponse = await chrome.tabs.sendMessage(tabs[0].id, {
                type: "stop_recording",
            });
            if (stopResponse.success && stopResponse.regions) {
                currentRecordingTemplate.regions = stopResponse.regions;
                // Save template
                const saveResponse = await sendMessage({
                    type: "save_template",
                    template: currentRecordingTemplate,
                });
                if (saveResponse.success) {
                    showStatus(`Template saved with ${stopResponse.regions.length} regions!`, "success");
                    // Reset form
                    resetRecordingForm();
                    // Switch to templates tab to show the new template
                    document
                        .querySelector('[data-tab="templates"]')
                        ?.dispatchEvent(new MouseEvent("click"));
                }
            }
        }
        catch (error) {
            showStatus("Failed to save template", "error");
        }
    });
    // Listen for region updates from content script
    chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
        if (message.type === "region_added" && currentRecordingTemplate) {
            currentRecordingTemplate.regions.push(message.region);
            updateRecordingRegionsList(currentRecordingTemplate.regions);
        }
        else if (message.type === "recording_stopped") {
            // User pressed Escape to stop recording - reset UI without saving
            resetRecordingForm();
            showStatus("Recording stopped", "info");
        }
        sendResponse({ success: true });
    });
    function resetRecordingForm() {
        recordNameInput.value = "";
        recordDescriptionInput.value = "";
        startRecordingBtn.disabled = false;
        finishRecordingBtn.disabled = true;
        recordNameInput.disabled = false;
        recordDescriptionInput.disabled = false;
        isRecording = false;
        currentRecordingTemplate = null;
        updateRecordingRegionsList([]);
    }
    function updateRecordingRegionsList(regions) {
        if (!recordingRegionsList)
            return;
        if (regions.length === 0) {
            recordingRegionsList.innerHTML =
                '<p class="empty-state">No regions added yet</p>';
            return;
        }
        recordingRegionsList.innerHTML = regions
            .map((region) => `
        <div class="region-item">
          <span class="region-name">${region.name}</span>
          <span class="region-count">${region.transformations.length} transforms</span>
        </div>
      `)
            .join("");
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Apply Tab
    // ─────────────────────────────────────────────────────────────────────────────
    const applyTemplateSelect = document.getElementById("apply-template-select");
    const applyTemplateBtn = document.getElementById("apply-template-btn");
    async function loadTemplatesForApply() {
        const response = await sendMessage({ type: "list_templates" });
        const templates = response.templates;
        applyTemplateSelect.innerHTML =
            '<option value="">-- Choose a template --</option>';
        templates.forEach((template) => {
            const option = document.createElement("option");
            option.value = template.id;
            option.textContent = template.name;
            applyTemplateSelect.appendChild(option);
        });
    }
    applyTemplateBtn?.addEventListener("click", async () => {
        const templateId = applyTemplateSelect.value;
        if (!templateId) {
            showStatus("Please select a template", "error");
            return;
        }
        applyTemplateBtn.disabled = true;
        showStatus("Extracting data...", "info");
        try {
            const response = await sendMessage({
                type: "apply_template",
                template_id: templateId,
            });
            if (response.success) {
                displayResults(response.result);
                showStatus("Extraction complete!", "success");
            }
            else {
                showStatus("Extraction failed: " + response.error, "error");
            }
        }
        catch (error) {
            showStatus("Error during extraction: " + String(error), "error");
        }
        finally {
            applyTemplateBtn.disabled = false;
        }
    });
    function displayResults(result) {
        const resultsContainer = document.getElementById("extraction-results");
        if (!resultsContainer)
            return;
        const data = result.data || {};
        const html = `
    <div class="results-panel">
      <h3>Extraction Results</h3>
      <pre>${JSON.stringify(data, null, 2)}</pre>
      <button class="btn btn-small" onclick="copyResults()">Copy JSON</button>
    </div>
  `;
        resultsContainer.innerHTML = html;
        window.lastResults = JSON.stringify(data, null, 2);
    }
    window.copyResults = function () {
        const results = window.lastResults;
        if (results) {
            navigator.clipboard.writeText(results).then(() => {
                showStatus("Results copied to clipboard!", "success");
            });
        }
    };
    // ─────────────────────────────────────────────────────────────────────────────
    // Utilities
    // ─────────────────────────────────────────────────────────────────────────────
    function sendMessage(message) {
        return new Promise((resolve, reject) => {
            chrome.runtime.sendMessage(message, (response) => {
                if (chrome.runtime.lastError) {
                    reject(new Error(chrome.runtime.lastError.message));
                    return;
                }
                if (typeof response === "undefined") {
                    reject(new Error("No response from extension service worker"));
                    return;
                }
                else {
                    resolve(response);
                }
            });
        });
    }
    function showStatus(message, type = "info") {
        console.log(`[${type.toUpperCase()}]`, message);
        const statusElements = document.querySelectorAll('[id$="-status"]');
        statusElements.forEach((el) => {
            el.textContent = message;
            el.className = `status-message ${type}`;
        });
        // Auto-hide after 3 seconds
        setTimeout(() => {
            statusElements.forEach((el) => {
                if (el.textContent === message) {
                    el.textContent = "";
                }
            });
        }, 3000);
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Settings Tab
    // ─────────────────────────────────────────────────────────────────────────────
    const backendUrlInput = document.getElementById("backend-url-input");
    const saveBackendUrlBtn = document.getElementById("save-backend-url-btn");
    const checkConnectionBtn = document.getElementById("check-connection-btn");
    const connectionStatusEl = document.getElementById("connection-status");
    const connectionStatusText = document.getElementById("connection-status-text");
    const connectionDiagnosticsEl = document.getElementById("connection-diagnostics");
    function setConnectionStatus(connected, label) {
        if (!connectionStatusEl || !connectionStatusText)
            return;
        connectionStatusEl.className = `connection-status ${connected ? "connected" : "disconnected"}`;
        connectionStatusText.textContent = label;
    }
    function setConnectionDiagnostics(label) {
        if (!connectionDiagnosticsEl)
            return;
        connectionDiagnosticsEl.textContent = label;
    }
    function formatCheckTime(date) {
        return date.toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
            second: "2-digit",
        });
    }
    async function loadBackendUrl() {
        try {
            const response = await sendMessage({ type: "get_backend_url" });
            if (response.success && backendUrlInput) {
                backendUrlInput.value = response.url;
            }
        }
        catch (error) {
            showStatus(`Failed to load backend URL: ${String(error)}`, "error");
        }
    }
    async function checkConnection() {
        const startedAt = new Date();
        const t0 = performance.now();
        setConnectionStatus(false, "Checking\u2026");
        try {
            const response = await sendMessage({ type: "check_connection" });
            const elapsedMs = Math.round(performance.now() - t0);
            const checkTime = formatCheckTime(startedAt);
            if (response.success) {
                setConnectionStatus(true, `Connected \u2014 ${response.url}`);
                setConnectionDiagnostics(`Last check: ${checkTime} (${elapsedMs}ms)`);
            }
            else {
                setConnectionStatus(false, `Disconnected: ${response.error}`);
                setConnectionDiagnostics(`Last check: ${checkTime} (${elapsedMs}ms)`);
            }
        }
        catch (error) {
            const elapsedMs = Math.round(performance.now() - t0);
            const checkTime = formatCheckTime(startedAt);
            setConnectionStatus(false, `Disconnected: ${String(error)}`);
            setConnectionDiagnostics(`Last check: ${checkTime} (${elapsedMs}ms)`);
        }
    }
    saveBackendUrlBtn?.addEventListener("click", async () => {
        const url = backendUrlInput?.value.trim();
        if (!url) {
            showStatus("URL cannot be empty", "error");
            return;
        }
        const response = await sendMessage({ type: "set_backend_url", url });
        if (response.success) {
            showStatus("Backend URL saved", "success");
            await checkConnection();
        }
        else {
            showStatus("Failed to save URL: " + response.error, "error");
        }
    });
    checkConnectionBtn?.addEventListener("click", () => {
        checkConnection();
    });
    // Load settings and run initial health check when settings tab is opened
    document
        .querySelector('[data-tab="settings"]')
        ?.addEventListener("click", async () => {
        await loadBackendUrl();
        await checkConnection();
    });
    // ─────────────────────────────────────────────────────────────────────────────
    // Initialization
    // ─────────────────────────────────────────────────────────────────────────────
    console.log("[Popup] Stygian Plugin popup loaded");
    // Load templates on startup
    loadTemplates();
})();
//# sourceMappingURL=popup.js.map