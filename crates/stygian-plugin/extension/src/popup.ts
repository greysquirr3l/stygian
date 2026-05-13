/// <reference types="chrome" />

/**
 * Popup UI logic for Stygian Plugin
 */

// Keep popup as a classic script (not ES module) for MV3 compatibility.
type PopupExtractionTemplate = any;
type PopupRegion = any;

(() => {
  // ─────────────────────────────────────────────────────────────────────────────
  // State
  // ─────────────────────────────────────────────────────────────────────────────

  let currentRecordingTemplate: PopupExtractionTemplate | null = null;
  let isRecording = false;

  // ─────────────────────────────────────────────────────────────────────────────
  // Tab Navigation
  // ─────────────────────────────────────────────────────────────────────────────

  document.querySelectorAll(".tab-btn").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      const tabName = (e.target as HTMLElement).getAttribute("data-tab");
      if (!tabName) return;

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
      (e.target as HTMLElement).classList.add("active");

      // Load templates when switching to tabs that need them
      if (tabName === "templates") {
        loadTemplates();
      } else if (tabName === "apply") {
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
    if (!name) return;

    const description = prompt("Enter template description (optional):");

    chrome.runtime.sendMessage(
      {
        type: "create_template",
        name,
        description: description || undefined,
      },
      (response: any) => {
        if (response.success) {
          loadTemplates();
          showStatus("Template created!", "success");
        } else {
          showStatus("Failed to create template: " + response.error, "error");
        }
      },
    );
  });

  // ── Import JSON ──────────────────────────────────────────────────────────────

  const importTemplateBtn = document.getElementById("import-template-btn");
  const importTemplateFile = document.getElementById(
    "import-template-file",
  ) as HTMLInputElement | null;

  importTemplateBtn?.addEventListener("click", () => {
    importTemplateFile?.click();
  });

  importTemplateFile?.addEventListener("change", async () => {
    const file = importTemplateFile?.files?.[0];
    if (!file) return;

    try {
      const text = await file.text();
      const parsed = JSON.parse(text);
      // Support single template object or array of templates
      const items: any[] = Array.isArray(parsed) ? parsed : [parsed];

      let imported = 0;
      let failed = 0;
      for (const item of items) {
        const response = await sendMessage({
          type: "import_template",
          json: JSON.stringify(item),
        });
        if (response.success) {
          imported++;
        } else {
          failed++;
        }
      }

      loadTemplates();
      const msg =
        failed === 0
          ? `Imported ${imported} template${imported !== 1 ? "s" : ""}!`
          : `Imported ${imported}, failed ${failed}`;
      showStatus(msg, failed === 0 ? "success" : "error");
    } catch (_e) {
      showStatus("Invalid JSON file", "error");
    } finally {
      // Reset so the same file can be re-imported if needed
      if (importTemplateFile) importTemplateFile.value = "";
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
        showStatus(
          `Synced ${response.imported} template${response.imported !== 1 ? "s" : ""} from server!`,
          "success",
        );
      } else {
        showStatus("Sync failed: " + response.error, "error");
      }
    } catch (e) {
      showStatus("Sync error: " + String(e), "error");
    } finally {
      syncTemplatesBtn.removeAttribute("disabled");
    }
  });

  async function loadTemplates() {
    const response = await sendMessage({ type: "list_templates" });

    const listContainer = document.getElementById("templates-list");
    if (!listContainer) return;

    const templates = response.templates as PopupExtractionTemplate[];

    if (templates.length === 0) {
      listContainer.innerHTML = '<p class="empty-state">No templates yet.</p>';
      return;
    }

    listContainer.innerHTML = templates
      .map(
        (template) => `
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
      `,
      )
      .join("");
  }

  document
    .getElementById("templates-list")
    ?.addEventListener("click", (event) => {
      const target = event.target as HTMLElement | null;
      if (!target) return;

      const button = target.closest(
        "button[data-action][data-template-id]",
      ) as HTMLButtonElement | null;
      if (!button) return;

      const templateId = button.getAttribute("data-template-id");
      if (!templateId) return;

      const action = button.getAttribute("data-action");
      if (action === "edit-template") {
        void editTemplate(templateId);
      } else if (action === "delete-template") {
        void deleteTemplate(templateId);
      }
    });

  async function editTemplate(templateId: string) {
    const response = await sendMessage({
      type: "get_template",
      template_id: templateId,
    });

    if (!response.success) {
      showStatus("Failed to load template", "error");
      return;
    }

    const template = response.template as PopupExtractionTemplate;

    const action = prompt(
      `Template: ${template.name}\n\nOptions:\n1. Edit name\n2. View regions\n3. Export JSON`,
      "1",
    );

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
          .map((r: PopupRegion) => `- ${r.name}: ${JSON.stringify(r.selector)}`)
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

  async function deleteTemplate(templateId: string) {
    if (!confirm("Delete this template?")) return;

    const response = await sendMessage({
      type: "delete_template",
      template_id: templateId,
    });

    if (response.success) {
      loadTemplates();
      showStatus("Template deleted", "success");
    } else {
      showStatus("Failed to delete template", "error");
    }
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Record Tab
  // ─────────────────────────────────────────────────────────────────────────────

  const recordNameInput = document.getElementById(
    "record-name-input",
  ) as HTMLInputElement;
  const recordDescriptionInput = document.getElementById(
    "record-description-input",
  ) as HTMLTextAreaElement;
  const startRecordingBtn = document.getElementById(
    "start-recording-btn",
  ) as HTMLButtonElement | null;
  const finishRecordingBtn = document.getElementById(
    "finish-recording-btn",
  ) as HTMLButtonElement | null;
  const recordingRegionsList = document.getElementById(
    "recording-regions-list",
  );

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
      await chrome.tabs.sendMessage(tabs[0].id!, {
        type: "start_recording",
        template_name: name,
      });

      isRecording = true;
      startRecordingBtn!.disabled = true;
      finishRecordingBtn!.disabled = false;
      recordNameInput.disabled = true;
      recordDescriptionInput.disabled = true;

      showStatus(
        "Recording started! Click elements in the page to add regions.",
        "info",
      );
    } catch (error) {
      showStatus("Failed to start recording", "error");
    }
  });

  finishRecordingBtn?.addEventListener("click", async () => {
    const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
    if (!tabs[0] || !currentRecordingTemplate) return;

    try {
      const stopResponse = await chrome.tabs.sendMessage(tabs[0].id!, {
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
          showStatus(
            `Template saved with ${stopResponse.regions.length} regions!`,
            "success",
          );

          // Reset form
          resetRecordingForm();

          // Switch to templates tab to show the new template
          document
            .querySelector('[data-tab="templates"]')
            ?.dispatchEvent(new MouseEvent("click"));
        }
      }
    } catch (error) {
      showStatus("Failed to save template", "error");
    }
  });

  // Listen for region updates from content script
  chrome.runtime.onMessage.addListener(
    (message: any, _sender: any, sendResponse: any) => {
      if (message.type === "region_added" && currentRecordingTemplate) {
        currentRecordingTemplate.regions.push(message.region);
        updateRecordingRegionsList(currentRecordingTemplate.regions);
      } else if (message.type === "recording_stopped") {
        // User pressed Escape to stop recording - reset UI without saving
        resetRecordingForm();
        showStatus("Recording stopped", "info");
      }
      sendResponse({ success: true });
    },
  );

  function resetRecordingForm() {
    recordNameInput.value = "";
    recordDescriptionInput.value = "";
    startRecordingBtn!.disabled = false;
    finishRecordingBtn!.disabled = true;
    recordNameInput.disabled = false;
    recordDescriptionInput.disabled = false;
    isRecording = false;
    currentRecordingTemplate = null;
    updateRecordingRegionsList([]);
  }

  function updateRecordingRegionsList(regions: PopupRegion[]) {
    if (!recordingRegionsList) return;

    if (regions.length === 0) {
      recordingRegionsList.innerHTML =
        '<p class="empty-state">No regions added yet</p>';
      return;
    }

    recordingRegionsList.innerHTML = regions
      .map(
        (region) => `
        <div class="region-item">
          <span class="region-name">${region.name}</span>
          <span class="region-count">${region.transformations.length} transforms</span>
        </div>
      `,
      )
      .join("");
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Apply Tab
  // ─────────────────────────────────────────────────────────────────────────────

  const applyTemplateSelect = document.getElementById(
    "apply-template-select",
  ) as HTMLSelectElement;
  const applyTemplateBtn = document.getElementById(
    "apply-template-btn",
  ) as HTMLButtonElement | null;

  async function loadTemplatesForApply() {
    const response = await sendMessage({ type: "list_templates" });
    const templates = response.templates as PopupExtractionTemplate[];

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

    applyTemplateBtn!.disabled = true;
    showStatus("Extracting data...", "info");

    try {
      const response = await sendMessage({
        type: "apply_template",
        template_id: templateId,
      });

      if (response.success) {
        const mcpToolError =
          response.result?.result?.isError === true
            ? response.result?.result?.content?.[0]?.text
            : null;
        const backendError =
          response.result?.error?.message ??
          response.result?.error ??
          mcpToolError ??
          null;
        if (backendError) {
          showStatus("Extraction failed: " + String(backendError), "error");
          return;
        }

        displayResults(response.result);
        showStatus("Extraction complete!", "success");
      } else {
        showStatus("Extraction failed: " + response.error, "error");
      }
    } catch (error) {
      showStatus("Error during extraction: " + String(error), "error");
    } finally {
      applyTemplateBtn!.disabled = false;
    }
  });

  function displayResults(result: any) {
    const resultsContainer = document.getElementById("extraction-results");
    if (!resultsContainer) return;

    const data = normaliseExtractionResult(result);
    const html = `
    <div class="results-panel">
      <h3>Extraction Results</h3>
      <pre>${JSON.stringify(data, null, 2)}</pre>
      <button class="btn btn-small" data-action="copy-results">Copy JSON</button>
    </div>
  `;

    resultsContainer.innerHTML = html;
    (window as any).lastResults = JSON.stringify(data, null, 2);
  }

  document
    .getElementById("extraction-results")
    ?.addEventListener("click", (event) => {
      const target = event.target as HTMLElement | null;
      if (!target) return;

      const button = target.closest(
        "button[data-action='copy-results']",
      ) as HTMLButtonElement | null;
      if (!button) return;

      void copyResultsToClipboard();
    });

  function normaliseExtractionResult(result: any): Record<string, any> {
    if (result?.data && typeof result.data === "object") {
      return result.data as Record<string, any>;
    }

    const textPayload = result?.result?.content?.[0]?.text;
    if (typeof textPayload === "string") {
      try {
        const parsed = JSON.parse(textPayload) as Record<string, any>;
        if (parsed?.data && typeof parsed.data === "object") {
          return parsed.data as Record<string, any>;
        }
        return parsed;
      } catch {
        return { raw: textPayload };
      }
    }

    return {};
  }

  async function copyResultsToClipboard(): Promise<void> {
    const results = (window as any).lastResults;
    if (!results) {
      showStatus("No results to copy", "error");
      return;
    }

    try {
      await navigator.clipboard.writeText(results);
      showStatus("Results copied to clipboard!", "success");
    } catch (error) {
      showStatus("Failed to copy results", "error");
    }
  }

  // ─────────────────────────────────────────────────────────────────────────────
  // Utilities
  // ─────────────────────────────────────────────────────────────────────────────

  function sendMessage(message: any): Promise<any> {
    return new Promise((resolve, reject) => {
      chrome.runtime.sendMessage(message, (response: any) => {
        if (chrome.runtime.lastError) {
          reject(new Error(chrome.runtime.lastError.message));
          return;
        }

        if (typeof response === "undefined") {
          reject(new Error("No response from extension service worker"));
          return;
        } else {
          resolve(response);
        }
      });
    });
  }

  function showStatus(
    message: string,
    type: "success" | "error" | "info" = "info",
  ) {
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

  const backendUrlInput = document.getElementById(
    "backend-url-input",
  ) as HTMLInputElement | null;
  const saveBackendUrlBtn = document.getElementById(
    "save-backend-url-btn",
  ) as HTMLButtonElement | null;
  const checkConnectionBtn = document.getElementById(
    "check-connection-btn",
  ) as HTMLButtonElement | null;
  const connectionStatusEl = document.getElementById("connection-status");
  const connectionStatusText = document.getElementById(
    "connection-status-text",
  );
  const connectionDiagnosticsEl = document.getElementById(
    "connection-diagnostics",
  );

  function setConnectionStatus(connected: boolean, label: string) {
    if (!connectionStatusEl || !connectionStatusText) return;
    connectionStatusEl.className = `connection-status ${connected ? "connected" : "disconnected"}`;
    connectionStatusText.textContent = label;
  }

  function setConnectionDiagnostics(label: string) {
    if (!connectionDiagnosticsEl) return;
    connectionDiagnosticsEl.textContent = label;
  }

  function formatCheckTime(date: Date): string {
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
    } catch (error) {
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
      } else {
        setConnectionStatus(false, `Disconnected: ${response.error}`);
        setConnectionDiagnostics(`Last check: ${checkTime} (${elapsedMs}ms)`);
      }
    } catch (error) {
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
    } else {
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
