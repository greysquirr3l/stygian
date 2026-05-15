"use strict";
/// <reference types="chrome" />
(() => {
    const resultUtils = globalThis.StygianResultUtils;
    // ─────────────────────────────────────────────────────────────────────────────
    // State
    // ─────────────────────────────────────────────────────────────────────────────
    let currentRecordingTemplate = null;
    let isRecording = false;
    let pendingQuickType = null; // quick-action pre-selection
    function openModal(id) {
        const overlay = document.getElementById("modal-overlay");
        const modal = document.getElementById(id);
        if (!overlay || !modal)
            return;
        // Hide all modals first
        overlay
            .querySelectorAll(".modal")
            .forEach((m) => (m.style.display = "none"));
        overlay.style.display = "flex";
        modal.style.display = "block";
        // Focus first input
        const firstInput = modal.querySelector("input, textarea");
        firstInput?.focus();
    }
    function closeModal(id) {
        const overlay = document.getElementById("modal-overlay");
        const modal = document.getElementById(id);
        if (overlay)
            overlay.style.display = "none";
        if (modal)
            modal.style.display = "none";
    }
    // Close on overlay backdrop click
    document.getElementById("modal-overlay")?.addEventListener("click", (e) => {
        if (e.target.id === "modal-overlay") {
            document
                .querySelectorAll(".modal")
                .forEach((m) => (m.style.display = "none"));
            document.getElementById("modal-overlay").style.display =
                "none";
        }
    });
    // Close button / cancel button wiring
    document.querySelectorAll(".modal-close, .modal-cancel").forEach((btn) => {
        btn.addEventListener("click", (e) => {
            const id = e.target.getAttribute("data-modal");
            if (id)
                closeModal(id);
        });
    });
    // Escape key closes the topmost open modal
    document.addEventListener("keydown", (e) => {
        if (e.key !== "Escape")
            return;
        const overlay = document.getElementById("modal-overlay");
        if (overlay?.style.display !== "none" && overlay?.style.display !== "") {
            e.preventDefault();
            overlay.style.display = "none";
            overlay
                ?.querySelectorAll(".modal")
                .forEach((m) => (m.style.display = "none"));
        }
    });
    // ─────────────────────────────────────────────────────────────────────────────
    // Per-Tab Status (replaces fan-out showStatus)
    // ─────────────────────────────────────────────────────────────────────────────
    const TAB_STATUS_IDS = {
        templates: "templates-status",
        record: "recording-status",
        apply: "apply-status",
        settings: "settings-status",
    };
    function showTabStatus(tab, message, type = "info", sticky = false) {
        const elId = TAB_STATUS_IDS[tab] ?? "recording-status";
        const el = document.getElementById(elId);
        if (!el)
            return;
        el.textContent = message;
        el.className = `status-message ${type}${sticky ? " sticky" : ""}`;
        if (!sticky) {
            setTimeout(() => {
                if (el.textContent === message)
                    el.textContent = "";
            }, type === "error" ? 5000 : 3000);
        }
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Draft Autosave / Recovery
    // ─────────────────────────────────────────────────────────────────────────────
    const DRAFT_KEY = "stygian_recording_draft";
    function saveDraft() {
        if (!currentRecordingTemplate)
            return;
        const draft = {
            template: currentRecordingTemplate,
            savedAt: Date.now(),
        };
        chrome.storage.local.set({ [DRAFT_KEY]: draft });
    }
    function clearDraft() {
        chrome.storage.local.remove(DRAFT_KEY);
    }
    async function loadDraftIfExists() {
        return new Promise((resolve) => {
            chrome.storage.local.get(DRAFT_KEY, (result) => {
                const draft = result[DRAFT_KEY];
                if (!draft) {
                    resolve();
                    return;
                }
                // Ignore drafts older than 2 hours
                if (Date.now() - draft.savedAt > 2 * 60 * 60 * 1000) {
                    clearDraft();
                    resolve();
                    return;
                }
                showDraftRecoveryBanner(draft.template, draft.savedAt);
                resolve();
            });
        });
    }
    function showDraftRecoveryBanner(template, savedAt) {
        const banner = document.getElementById("draft-recovery-banner");
        const label = document.getElementById("draft-banner-label");
        if (!banner || !label)
            return;
        const age = Math.round((Date.now() - savedAt) / 60000);
        label.textContent = `"${template.name}" — ${(template.regions ?? []).length} regions — ${age < 1 ? "just now" : `${age}m ago`}`;
        banner.style.display = "flex";
        document.getElementById("draft-resume-btn")?.addEventListener("click", () => {
            resumeDraft(template);
            banner.style.display = "none";
        }, { once: true });
        document.getElementById("draft-discard-btn")?.addEventListener("click", () => {
            clearDraft();
            banner.style.display = "none";
        }, { once: true });
    }
    function resumeDraft(template) {
        // Switch to record tab and restore state
        document
            .querySelector('[data-tab="record"]')
            ?.dispatchEvent(new MouseEvent("click"));
        currentRecordingTemplate = template;
        const nameInput = document.getElementById("record-name-input");
        const descInput = document.getElementById("record-description-input");
        if (nameInput)
            nameInput.value = template.name ?? "";
        if (descInput)
            descInput.value = template.description ?? "";
        updateRecordingRegionsList(template.regions ?? []);
        showTabStatus("record", `Resumed draft with ${(template.regions ?? []).length} regions.`, "info");
    }
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
        const nameInput = document.getElementById("modal-new-name");
        const descInput = document.getElementById("modal-new-description");
        if (nameInput)
            nameInput.value = "";
        if (descInput)
            descInput.value = "";
        openModal("modal-new-template");
    });
    document
        .getElementById("modal-new-confirm")
        ?.addEventListener("click", async () => {
        const name = document.getElementById("modal-new-name")?.value.trim();
        const description = document.getElementById("modal-new-description")?.value.trim();
        if (!name) {
            document.getElementById("modal-new-name").style.borderColor = "#fc8181";
            return;
        }
        closeModal("modal-new-template");
        const response = await sendMessage({
            type: "create_template",
            name,
            description: description || undefined,
        });
        if (response.success) {
            loadTemplates();
            showTabStatus("templates", "Template created!", "success");
        }
        else {
            showTabStatus("templates", "Failed to create template: " + response.error, "error");
        }
    });
    // Enter key inside modal confirms
    document
        .getElementById("modal-new-name")
        ?.addEventListener("keydown", (e) => {
        if (e.key === "Enter")
            document.getElementById("modal-new-confirm")?.click();
    });
    document
        .getElementById("modal-new-description")
        ?.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && e.ctrlKey)
            document.getElementById("modal-new-confirm")?.click();
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
            showTabStatus("templates", msg, failed === 0 ? "success" : "error");
        }
        catch (_e) {
            showTabStatus("templates", "Invalid JSON file", "error");
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
        showTabStatus("templates", "Syncing templates from server\u2026", "info");
        try {
            const response = await sendMessage({ type: "sync_from_server" });
            if (response.success) {
                loadTemplates();
                showTabStatus("templates", `Synced ${response.imported} template${response.imported !== 1 ? "s" : ""} from server!`, "success");
            }
            else {
                showTabStatus("templates", "Sync failed: " + response.error, "error");
            }
        }
        catch (e) {
            showTabStatus("templates", "Sync error: " + String(e), "error");
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
            <button class="btn btn-small" data-action="edit-template" data-template-id="${template.id}">Edit</button>
            <button class="btn btn-small btn-danger" data-action="delete-template" data-template-id="${template.id}">Delete</button>
          </div>
        </div>
      `)
            .join("");
    }
    document
        .getElementById("templates-list")
        ?.addEventListener("click", (event) => {
        const target = event.target;
        if (!target)
            return;
        const button = target.closest("button[data-action][data-template-id]");
        if (!button)
            return;
        const templateId = button.getAttribute("data-template-id");
        if (!templateId)
            return;
        const action = button.getAttribute("data-action");
        if (action === "edit-template") {
            void editTemplate(templateId);
        }
        else if (action === "delete-template") {
            void deleteTemplate(templateId);
        }
    });
    // Tracks which template we're editing
    let editingTemplateId = null;
    async function editTemplate(templateId) {
        const response = await sendMessage({
            type: "get_template",
            template_id: templateId,
        });
        if (!response.success) {
            showTabStatus("templates", "Failed to load template", "error");
            return;
        }
        const template = response.template;
        editingTemplateId = templateId;
        const nameInput = document.getElementById("modal-edit-template-name");
        if (nameInput)
            nameInput.value = template.name ?? "";
        openModal("modal-edit-template");
    }
    document
        .getElementById("modal-edit-template-confirm")
        ?.addEventListener("click", async () => {
        if (!editingTemplateId)
            return;
        const newName = document.getElementById("modal-edit-template-name")?.value.trim();
        if (!newName)
            return;
        closeModal("modal-edit-template");
        const getResp = await sendMessage({
            type: "get_template",
            template_id: editingTemplateId,
        });
        if (!getResp.success)
            return;
        const template = getResp.template;
        template.name = newName;
        const saveResp = await sendMessage({ type: "save_template", template });
        if (saveResp.success) {
            loadTemplates();
            showTabStatus("templates", "Template updated!", "success");
        }
        else {
            showTabStatus("templates", "Failed to update: " + saveResp.error, "error");
        }
        editingTemplateId = null;
    });
    document
        .getElementById("modal-edit-template-name")
        ?.addEventListener("keydown", (e) => {
        if (e.key === "Enter")
            document.getElementById("modal-edit-template-confirm")?.click();
    });
    async function deleteTemplate(templateId) {
        // Use a simple inline confirm approach rather than native confirm()
        const card = document
            .querySelector(`[data-template-id="${templateId}"]`)
            ?.closest(".template-card");
        if (card) {
            card.style.outline = "2px solid #fc8181";
            setTimeout(() => {
                card.style.outline = "";
            }, 2000);
        }
        const response = await sendMessage({
            type: "delete_template",
            template_id: templateId,
        });
        if (response.success) {
            loadTemplates();
            showTabStatus("templates", "Template deleted", "success");
        }
        else {
            showTabStatus("templates", "Failed to delete template", "error");
        }
    }
    // ─────────────────────────────────────────────────────────────────────────────
    // Record Tab
    // ─────────────────────────────────────────────────────────────────────────────
    const recordNameInput = document.getElementById("record-name-input");
    const recordDescriptionInput = document.getElementById("record-description-input");
    const startRecordingBtn = document.getElementById("start-recording-btn");
    const finishRecordingBtn = document.getElementById("finish-recording-btn");
    const recordingRegionsList = document.getElementById("recording-regions-list");
    const recordingActiveBadge = document.getElementById("recording-active-badge");
    const recordingBadgeCount = document.getElementById("recording-badge-count");
    const quickActionsGroup = document.getElementById("quick-actions-group");
    const undoLastRegionBtn = document.getElementById("undo-last-region-btn");
    const testRecordingBtn = document.getElementById("test-recording-btn");
    const recordTestResults = document.getElementById("record-test-results");
    // ── Quick-action type buttons ────────────────────────────────────────────────
    document
        .getElementById("quick-actions-group")
        ?.querySelectorAll("[data-quick-type]")
        .forEach((btn) => {
        btn.addEventListener("click", () => {
            const type = btn.getAttribute("data-quick-type") ?? "";
            if (pendingQuickType === type) {
                pendingQuickType = null;
                btn.classList.remove("active");
            }
            else {
                document
                    .querySelectorAll("[data-quick-type]")
                    .forEach((b) => b.classList.remove("active"));
                pendingQuickType = type;
                btn.classList.add("active");
            }
        });
    });
    // ── Start Recording ──────────────────────────────────────────────────────────
    startRecordingBtn?.addEventListener("click", async () => {
        const name = recordNameInput.value.trim();
        if (!name) {
            showTabStatus("record", "Please enter a template name", "error");
            return;
        }
        const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
        if (!tabs[0]) {
            showTabStatus("record", "No active tab", "error");
            return;
        }
        const createResponse = await sendMessage({
            type: "create_template",
            name,
            description: recordDescriptionInput.value.trim(),
        });
        if (!createResponse.success) {
            showTabStatus("record", "Failed to create template", "error");
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
            // Show new UI elements
            if (recordingActiveBadge)
                recordingActiveBadge.hidden = false;
            if (quickActionsGroup)
                quickActionsGroup.hidden = false;
            if (recordTestResults)
                recordTestResults.innerHTML = "";
            showTabStatus("record", "Recording started! Click elements in the page to add regions.", "info");
        }
        catch (_error) {
            showTabStatus("record", "Failed to start recording", "error");
        }
    });
    // ── Finish Recording ─────────────────────────────────────────────────────────
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
                const saveResponse = await sendMessage({
                    type: "save_template",
                    template: currentRecordingTemplate,
                });
                if (saveResponse.success) {
                    clearDraft();
                    showTabStatus("record", `Template saved with ${stopResponse.regions.length} regions!`, "success");
                    resetRecordingForm();
                    document
                        .querySelector('[data-tab="templates"]')
                        ?.dispatchEvent(new MouseEvent("click"));
                }
            }
        }
        catch (_error) {
            showTabStatus("record", "Failed to save template", "error");
        }
    });
    // ── Undo Last Region ─────────────────────────────────────────────────────────
    undoLastRegionBtn?.addEventListener("click", async () => {
        if (!currentRecordingTemplate)
            return;
        const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
        if (!tabs[0])
            return;
        try {
            await chrome.tabs.sendMessage(tabs[0].id, { type: "region_undo" });
        }
        catch {
            // Fallback if content script messaging fails.
            currentRecordingTemplate.regions = currentRecordingTemplate.regions.slice(0, -1);
            updateRecordingRegionsList(currentRecordingTemplate.regions);
            if (recordingBadgeCount) {
                recordingBadgeCount.textContent = String(currentRecordingTemplate.regions.length);
            }
            if (undoLastRegionBtn) {
                undoLastRegionBtn.disabled =
                    currentRecordingTemplate.regions.length === 0;
            }
            saveDraft();
        }
    });
    // ── Test Extraction ──────────────────────────────────────────────────────────
    testRecordingBtn?.addEventListener("click", async () => {
        if (!currentRecordingTemplate ||
            currentRecordingTemplate.regions.length === 0) {
            if (recordTestResults)
                recordTestResults.innerHTML =
                    '<p class="empty-state">Add at least one region first</p>';
            return;
        }
        const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
        if (!tabs[0])
            return;
        if (testRecordingBtn)
            testRecordingBtn.disabled = true;
        try {
            // Persist latest in-progress regions so apply uses current state.
            const saveResponse = await sendMessage({
                type: "save_template",
                template: currentRecordingTemplate,
            });
            if (!saveResponse.success) {
                if (recordTestResults) {
                    recordTestResults.innerHTML = `<p class="status-message error">Test failed: ${saveResponse.error ?? "failed to save template"}</p>`;
                }
                return;
            }
            const response = await sendMessage({
                type: "apply_template",
                template_id: currentRecordingTemplate.id,
                tab_id: tabs[0].id,
            });
            if (recordTestResults) {
                if (response.success && response.result) {
                    displayResults(response.result, recordTestResults);
                }
                else {
                    recordTestResults.innerHTML = `<p class="status-message error">Test failed: ${response.error ?? "unknown"}</p>`;
                }
            }
        }
        catch (e) {
            if (recordTestResults) {
                recordTestResults.innerHTML = `<p class="status-message error">Error: ${String(e)}</p>`;
            }
        }
        finally {
            if (testRecordingBtn)
                testRecordingBtn.disabled = false;
        }
    });
    // ── Incoming messages from content script ────────────────────────────────────
    chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
        if (message.type === "region_added" && currentRecordingTemplate) {
            currentRecordingTemplate.regions.push(message.region);
            updateRecordingRegionsList(currentRecordingTemplate.regions);
            if (recordingBadgeCount) {
                recordingBadgeCount.textContent = String(currentRecordingTemplate.regions.length);
            }
            if (undoLastRegionBtn)
                undoLastRegionBtn.disabled = false;
            saveDraft();
        }
        else if (message.type === "region_undo" && currentRecordingTemplate) {
            // Content-script-initiated undo (keyboard)
            currentRecordingTemplate.regions =
                currentRecordingTemplate.regions.slice(0, -1);
            updateRecordingRegionsList(currentRecordingTemplate.regions);
            if (recordingBadgeCount) {
                recordingBadgeCount.textContent = String(currentRecordingTemplate.regions.length);
            }
            if (undoLastRegionBtn) {
                undoLastRegionBtn.disabled =
                    currentRecordingTemplate.regions.length === 0;
            }
            saveDraft();
        }
        else if (message.type === "recording_stopped") {
            resetRecordingForm();
            showTabStatus("record", "Recording stopped", "info");
        }
        sendResponse({ success: true });
    });
    // ── Reset form ───────────────────────────────────────────────────────────────
    function resetRecordingForm() {
        recordNameInput.value = "";
        recordDescriptionInput.value = "";
        startRecordingBtn.disabled = false;
        finishRecordingBtn.disabled = true;
        recordNameInput.disabled = false;
        recordDescriptionInput.disabled = false;
        isRecording = false;
        currentRecordingTemplate = null;
        pendingQuickType = null;
        if (recordingActiveBadge)
            recordingActiveBadge.hidden = true;
        if (quickActionsGroup)
            quickActionsGroup.hidden = true;
        if (undoLastRegionBtn)
            undoLastRegionBtn.disabled = true;
        document
            .querySelectorAll("[data-quick-type]")
            .forEach((b) => b.classList.remove("active"));
        updateRecordingRegionsList([]);
        clearDraft();
    }
    // ── Region timeline ──────────────────────────────────────────────────────────
    // Confidence colours shared between content-script and popup
    const CONF_COLORS = {
        strong: "#22c55e",
        good: "#f59e0b",
        fragile: "#f87171",
    };
    function confidenceFromSelector(selector) {
        const isStrong = /#|data-|aria-/.test(selector);
        const level = isStrong
            ? "strong"
            : selector.split(" ").length > 2
                ? "fragile"
                : "good";
        return { level, color: CONF_COLORS[level] ?? "#94a3b8" };
    }
    // Tracks which region we're editing inside the modal
    let editingRegionIndex = null;
    function updateRecordingRegionsList(regions) {
        if (!recordingRegionsList)
            return;
        if (regions.length === 0) {
            recordingRegionsList.innerHTML =
                '<p class="empty-state">No regions added yet</p>';
            return;
        }
        recordingRegionsList.innerHTML = regions
            .map((region, index) => {
            const selectorStr = typeof region.selector === "string"
                ? region.selector
                : JSON.stringify(region.selector);
            const truncated = selectorStr.length > 36
                ? selectorStr.slice(0, 33) + "…"
                : selectorStr;
            const { level, color } = confidenceFromSelector(selectorStr);
            return `
          <div class="region-timeline-item" style="border-left-color:${color}" data-region-index="${index}">
            <span class="region-number" style="color:${color};font-weight:700;min-width:20px">${index + 1}</span>
            <div style="flex:1;min-width:0">
              <div style="font-size:13px;font-weight:600;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${escapeHtml(region.name)}</div>
              <div style="font-size:11px;color:#64748b;font-family:monospace;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">${escapeHtml(truncated)}</div>
            </div>
            <span class="region-confidence-dot" title="${level}" style="background:${color}"></span>
            <div class="region-item-actions">
              <button class="btn-icon" data-region-up="${index}" title="Move up">↑</button>
              <button class="btn-icon" data-region-down="${index}" title="Move down">↓</button>
              <button class="btn-icon" data-region-edit="${index}" title="Edit">✎</button>
              <button class="btn-icon" data-region-delete="${index}" title="Remove">✕</button>
            </div>
          </div>`;
        })
            .join("");
        // Edit button
        recordingRegionsList
            .querySelectorAll("[data-region-edit]")
            .forEach((btn) => {
            btn.addEventListener("click", () => {
                const idx = parseInt(btn.getAttribute("data-region-edit") ?? "-1", 10);
                if (idx < 0 || !currentRecordingTemplate)
                    return;
                const region = currentRecordingTemplate.regions[idx];
                if (!region)
                    return;
                editingRegionIndex = idx;
                const nameIn = document.getElementById("modal-edit-region-name");
                const selIn = document.getElementById("modal-edit-region-selector");
                if (nameIn)
                    nameIn.value = region.name;
                if (selIn) {
                    selIn.value =
                        typeof region.selector === "string"
                            ? region.selector
                            : JSON.stringify(region.selector);
                }
                openModal("modal-edit-region");
            });
        });
        // Delete button
        recordingRegionsList
            .querySelectorAll("[data-region-delete]")
            .forEach((btn) => {
            btn.addEventListener("click", () => {
                const idx = parseInt(btn.getAttribute("data-region-delete") ?? "-1", 10);
                if (idx < 0 || !currentRecordingTemplate)
                    return;
                currentRecordingTemplate.regions.splice(idx, 1);
                updateRecordingRegionsList(currentRecordingTemplate.regions);
                if (recordingBadgeCount) {
                    recordingBadgeCount.textContent = String(currentRecordingTemplate.regions.length);
                }
                if (undoLastRegionBtn) {
                    undoLastRegionBtn.disabled =
                        currentRecordingTemplate.regions.length === 0;
                }
                saveDraft();
            });
        });
        // Reorder up
        recordingRegionsList
            .querySelectorAll("[data-region-up]")
            .forEach((btn) => {
            btn.addEventListener("click", () => {
                const idx = parseInt(btn.getAttribute("data-region-up") ?? "-1", 10);
                if (idx <= 0 || !currentRecordingTemplate)
                    return;
                const regionsRef = currentRecordingTemplate.regions;
                [regionsRef[idx - 1], regionsRef[idx]] = [regionsRef[idx], regionsRef[idx - 1]];
                updateRecordingRegionsList(regionsRef);
                saveDraft();
            });
        });
        // Reorder down
        recordingRegionsList
            .querySelectorAll("[data-region-down]")
            .forEach((btn) => {
            btn.addEventListener("click", () => {
                const idx = parseInt(btn.getAttribute("data-region-down") ?? "-1", 10);
                if (!currentRecordingTemplate)
                    return;
                const regionsRef = currentRecordingTemplate.regions;
                if (idx < 0 || idx >= regionsRef.length - 1)
                    return;
                [regionsRef[idx + 1], regionsRef[idx]] = [regionsRef[idx], regionsRef[idx + 1]];
                updateRecordingRegionsList(regionsRef);
                saveDraft();
            });
        });
    }
    // ── Edit-region modal confirm ─────────────────────────────────────────────────
    document
        .getElementById("modal-edit-region-confirm")
        ?.addEventListener("click", () => {
        if (editingRegionIndex === null || !currentRecordingTemplate)
            return;
        const nameIn = document.getElementById("modal-edit-region-name");
        const selIn = document.getElementById("modal-edit-region-selector");
        const newName = nameIn?.value.trim();
        const newSel = selIn?.value.trim();
        if (!newName)
            return;
        const region = currentRecordingTemplate.regions[editingRegionIndex];
        if (!region)
            return;
        region.name = newName;
        if (newSel)
            region.selector = newSel;
        closeModal("modal-edit-region");
        editingRegionIndex = null;
        updateRecordingRegionsList(currentRecordingTemplate.regions);
        saveDraft();
    });
    // ─────────────────────────────────────────────────────────────────────────────
    // Apply Tab
    // ─────────────────────────────────────────────────────────────────────────────
    const applyTemplateSelect = document.getElementById("apply-template-select");
    const applyTemplateBtn = document.getElementById("apply-template-btn");
    const applyModeSelect = document.getElementById("apply-mode-select");
    const batchRootGroup = document.getElementById("batch-root-group");
    const batchRootSelectorInput = document.getElementById("batch-root-selector-input");
    const validateRootSelectorBtn = document.getElementById("validate-root-selector-btn");
    const applyDebugCheckbox = document.getElementById("apply-debug-checkbox");
    function syncApplyModeUi() {
        if (!applyModeSelect || !batchRootGroup)
            return;
        batchRootGroup.style.display =
            applyModeSelect.value === "batch" ? "block" : "none";
    }
    applyModeSelect?.addEventListener("change", syncApplyModeUi);
    syncApplyModeUi();
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
            showTabStatus("apply", "Please select a template", "error");
            return;
        }
        const mode = applyModeSelect?.value === "batch" ? "batch" : "single";
        const rootSelector = batchRootSelectorInput?.value.trim() ?? "";
        if (mode === "batch" && rootSelector.length === 0) {
            showTabStatus("apply", "Batch mode requires a root selector", "error");
            return;
        }
        applyTemplateBtn.disabled = true;
        showTabStatus("apply", "Extracting data...", "info");
        try {
            const response = await sendMessage({
                type: "apply_template",
                template_id: templateId,
                mode,
                root_selector: rootSelector,
                debug: applyDebugCheckbox?.checked === true,
            });
            if (response.success) {
                const mcpToolError = response.result?.result?.isError === true
                    ? response.result?.result?.content?.[0]?.text
                    : null;
                const backendError = response.result?.error?.message ??
                    response.result?.error ??
                    mcpToolError ??
                    null;
                if (backendError) {
                    showTabStatus("apply", "Extraction failed: " + String(backendError), "error");
                    return;
                }
                displayResults(response.result);
                showTabStatus("apply", "Extraction complete!", "success");
            }
            else {
                showTabStatus("apply", "Extraction failed: " + response.error, "error");
            }
        }
        catch (error) {
            showTabStatus("apply", "Error during extraction: " + String(error), "error");
        }
        finally {
            applyTemplateBtn.disabled = false;
        }
    });
    validateRootSelectorBtn?.addEventListener("click", async () => {
        const selectorCss = batchRootSelectorInput?.value.trim() ?? "";
        if (!selectorCss) {
            showTabStatus("apply", "Enter a root selector to validate", "error");
            return;
        }
        validateRootSelectorBtn.disabled = true;
        showTabStatus("apply", "Validating selector...", "info");
        try {
            const response = await sendMessage({
                type: "inspect_selector",
                selector_css: selectorCss,
            });
            if (!response.success) {
                showTabStatus("apply", "Validation failed: " + response.error, "error");
                return;
            }
            const textPayload = response.result?.result?.content?.[0]?.text;
            const parsed = typeof textPayload === "string"
                ? JSON.parse(textPayload)
                : response.result;
            const count = Number(parsed.match_count ?? 0);
            const preview = typeof parsed.preview === "string"
                ? parsed.preview
                : "Selector validated";
            showTabStatus("apply", `Selector matched ${count} element(s): ${preview}`, "success");
        }
        catch (error) {
            showTabStatus("apply", "Validation error: " + String(error), "error");
        }
        finally {
            validateRootSelectorBtn.disabled = false;
        }
    });
    function displayResults(result, container) {
        const resultsContainer = container ?? document.getElementById("extraction-results");
        if (!resultsContainer)
            return;
        const normalized = resultUtils.normalizeExtractionEnvelope(result);
        const rowPreview = renderResultsTable(normalized.rows.slice(0, 10));
        const jsonPayload = JSON.stringify(normalized.rawPayload, null, 2);
        const csvPayload = resultUtils.recordsToCsv(normalized.rows);
        const summaryChips = [
            `<span class="summary-chip">Mode: ${normalized.mode}</span>`,
            `<span class="summary-chip">Rows: ${normalized.rows.length}</span>`,
        ];
        if (typeof normalized.metadata.regions_successful === "number") {
            summaryChips.push(`<span class="summary-chip">Regions: ${normalized.metadata.regions_successful}/${normalized.metadata.total_regions ?? "?"}</span>`);
        }
        if (typeof normalized.metadata.elapsed_ms === "number") {
            summaryChips.push(`<span class="summary-chip">Elapsed: ${normalized.metadata.elapsed_ms}ms</span>`);
        }
        if (typeof normalized.metadata.root_selector === "string") {
            summaryChips.push(`<span class="summary-chip">Root: ${escapeHtml(normalized.metadata.root_selector)}</span>`);
        }
        const errorsHtml = normalized.errors.length > 0
            ? `<div class="results-errors"><strong>Errors</strong><ul>${normalized.errors
                .map((error) => `<li>${escapeHtml(error)}</li>`)
                .join("")}</ul></div>`
            : "";
        const html = `
    <div class="results-panel">
      <h3>Extraction Results</h3>
      <div class="results-summary">${summaryChips.join("")}</div>
      ${rowPreview}
      ${errorsHtml}
      <div class="results-actions">
        <button class="btn btn-small" data-action="copy-results">Copy JSON</button>
        <button class="btn btn-small btn-secondary" data-action="download-json">Download JSON</button>
        <button class="btn btn-small btn-secondary" data-action="download-csv" ${csvPayload ? "" : "disabled"}>Download CSV</button>
      </div>
      <details class="results-details">
        <summary>Raw Payload</summary>
        <pre>${escapeHtml(jsonPayload)}</pre>
      </details>
      ${normalized.debug ? `<details class="results-details"><summary>Debug</summary><pre>${escapeHtml(JSON.stringify(normalized.debug, null, 2))}</pre></details>` : ""}
    </div>
  `;
        resultsContainer.innerHTML = html;
        window.lastResults = jsonPayload;
        window.lastResultsCsv = csvPayload;
    }
    function renderResultsTable(rows) {
        if (rows.length === 0) {
            return '<p class="empty-state">No rows returned.</p>';
        }
        const headers = Array.from(rows.reduce((acc, row) => {
            Object.keys(row).forEach((key) => acc.add(key));
            return acc;
        }, new Set()));
        const head = headers
            .map((header) => `<th>${escapeHtml(header)}</th>`)
            .join("");
        const body = rows
            .map((row) => {
            const cells = headers
                .map((header) => `<td>${escapeHtml(String(row[header] ?? ""))}</td>`)
                .join("");
            return `<tr>${cells}</tr>`;
        })
            .join("");
        return `
      <div class="results-table-wrap">
        <table class="results-table">
          <thead><tr>${head}</tr></thead>
          <tbody>${body}</tbody>
        </table>
      </div>
    `;
    }
    document
        .getElementById("extraction-results")
        ?.addEventListener("click", (event) => {
        const target = event.target;
        if (!target)
            return;
        const button = target.closest("button[data-action]");
        if (!button)
            return;
        const action = button.getAttribute("data-action");
        if (action === "copy-results") {
            void copyResultsToClipboard();
        }
        else if (action === "download-json") {
            downloadResults("json");
        }
        else if (action === "download-csv") {
            downloadResults("csv");
        }
    });
    async function copyResultsToClipboard() {
        const results = window.lastResults;
        if (!results) {
            showTabStatus("apply", "No results to copy", "error");
            return;
        }
        try {
            await navigator.clipboard.writeText(results);
            showTabStatus("apply", "Results copied to clipboard!", "success");
        }
        catch (_error) {
            showTabStatus("apply", "Failed to copy results", "error");
        }
    }
    function downloadResults(format) {
        const payload = format === "json"
            ? window.lastResults
            : window.lastResultsCsv;
        if (!payload) {
            showTabStatus("apply", `No ${format.toUpperCase()} results available`, "error");
            return;
        }
        const blob = new Blob([payload], {
            type: format === "json" ? "application/json" : "text/csv;charset=utf-8",
        });
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url;
        link.download = `stygian-extraction-results.${format}`;
        link.click();
        URL.revokeObjectURL(url);
        showTabStatus("apply", `${format.toUpperCase()} downloaded`, "success");
    }
    function escapeHtml(value) {
        return value
            .replace(/&/g, "&amp;")
            .replace(/</g, "&lt;")
            .replace(/>/g, "&gt;")
            .replace(/"/g, "&quot;")
            .replace(/'/g, "&#39;");
    }
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
        // Fallback: log + show in all status elements for cases where tab is ambiguous
        console.log(`[${type.toUpperCase()}]`, message);
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
            showTabStatus("settings", `Failed to load backend URL: ${String(error)}`, "error");
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
            showTabStatus("settings", "URL cannot be empty", "error");
            return;
        }
        const response = await sendMessage({ type: "set_backend_url", url });
        if (response.success) {
            showTabStatus("settings", "Backend URL saved", "success");
            await checkConnection();
        }
        else {
            showTabStatus("settings", "Failed to save URL: " + response.error, "error");
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
    // Restore draft if one exists
    loadDraftIfExists();
    // Load templates on startup
    loadTemplates();
    // ── Popup keyboard shortcut: R to start/stop recording ─────────────────────
    document.addEventListener("keydown", (e) => {
        const key = e.key;
        // Only handle when not focused on an input/textarea
        const active = document.activeElement;
        if (active instanceof HTMLInputElement ||
            active instanceof HTMLTextAreaElement)
            return;
        if (key === "r" || key === "R") {
            const recordTab = document.querySelector('[data-tab="record"]');
            const activeTab = document
                .querySelector(".tab-btn.active")
                ?.getAttribute("data-tab");
            if (activeTab !== "record") {
                recordTab?.click();
                return;
            }
            if (isRecording) {
                finishRecordingBtn?.click();
            }
            else if (!startRecordingBtn?.disabled) {
                startRecordingBtn?.click();
            }
        }
    });
})();
//# sourceMappingURL=popup.js.map